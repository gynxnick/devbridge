//! DevBridge desktop shell. Tauri commands are thin adapters over the
//! `devbridge-*` crates; business logic does not live here.

mod commands;
mod error;
mod logging;
mod state;

use std::path::PathBuf;

use devbridge_sync::EngineEvent;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(|app| {
            // DEVBRIDGE_DATA_DIR lets a second instance run on the same computer
            // (with its own identity, database and port) for testing.
            let data_dir = match std::env::var_os("DEVBRIDGE_DATA_DIR") {
                Some(d) => PathBuf::from(d),
                None => app.path().app_data_dir()?,
            };
            let logs_dir = data_dir.join("logs");
            let guard = logging::init(&logs_dir);
            logging::install_panic_hook(&logs_dir);
            app.manage(LogGuard(guard));
            let state = state::AppState::init(data_dir, logs_dir)?;
            forward_events(app.handle().clone(), &state);
            let minimized = std::env::args().any(|a| a == "--minimized")
                || state.store().map(|s| s.settings().map(|s| s.start_minimized).unwrap_or(false)).unwrap_or(false);
            if minimized && let Some(w) = app.get_webview_window("main") {
                let _ = w.minimize();
            }
            app.manage(state);
            app.manage(commands::remote::Pending::default());
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event
                && let Some(s) = window.app_handle().try_state::<state::AppState>()
            {
                s.engine.shutdown();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::workspaces::get_overview,
            commands::workspaces::inspect_path,
            commands::workspaces::import_workspace,
            commands::workspaces::update_workspace,
            commands::workspaces::remove_workspace,
            commands::workspaces::refresh_git,
            commands::workspaces::open_in_vscode,
            commands::workspaces::open_terminal,
            commands::workspaces::rename_local_device,
            commands::workspaces::get_settings,
            commands::workspaces::save_settings,
            commands::workspaces::detect_tools,
            commands::workspaces::list_activity,
            commands::workspaces::sync_defaults,
            commands::git::git_overview,
            commands::git::git_stage,
            commands::git::git_unstage,
            commands::git::git_commit,
            commands::git::git_fetch,
            commands::git::git_pull,
            commands::git::git_push,
            commands::git::git_switch,
            commands::git::git_remote_add,
            commands::git::git_remote_set_url,
            commands::git::git_remote_remove,
            commands::git::git_catch_up,
            commands::git::git_init,
            commands::sync::workspace_sync,
            commands::sync::set_paused,
            commands::sync::rescan,
            commands::sync::retry_transfers,
            commands::sync::share_workspace,
            commands::sync::unshare_workspace,
            commands::sync::list_offers,
            commands::sync::join_offer,
            commands::sync::decline_offer,
            commands::devices::list_devices,
            commands::devices::engine_status,
            commands::devices::list_discovered,
            commands::devices::pairing_open,
            commands::devices::pairing_close,
            commands::devices::pairing_current,
            commands::devices::pairing_decide,
            commands::devices::pair_join,
            commands::devices::revoke_device,
            commands::conflicts::list_conflicts,
            commands::conflicts::conflict_detail,
            commands::conflicts::resolve_conflict,
            commands::conflicts::list_issues,
            commands::conflicts::dismiss_issue,
            commands::conflicts::list_recovery,
            commands::conflicts::restore_recovery,
            commands::conflicts::remove_recovery,
            commands::diagnostics::diagnostics,
            commands::diagnostics::export_diagnostics,
            commands::remote::remote_status,
            commands::remote::remote_set_hosting,
            commands::remote::remote_request_access,
            commands::remote::remote_decide,
            commands::remote::remote_revoke,
            commands::remote::remote_open,
            commands::remote::remote_check,
        ])
        .run(tauri::generate_context!())
        .expect("error while running DevBridge");
}

struct LogGuard(#[allow(dead_code)] Option<tracing_appender::non_blocking::WorkerGuard>);

/// Forwards engine events to the UI and, when enabled, to system notifications.
fn forward_events(app: tauri::AppHandle, state: &state::AppState) {
    let mut rx = state.engine.subscribe();
    let store = state.store.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let ev = match rx.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            };
            if let EngineEvent::Remote { device_id, message } = &ev {
                commands::remote::on_message(&app, *device_id, message.clone()).await;
            }
            let _ = app.emit("engine", &ev);
            let notify = state::lock(&store).ok().and_then(|s| s.settings().ok()).is_some_and(|s| s.notifications);
            let focused = app.get_webview_window("main").and_then(|w| w.is_focused().ok()).unwrap_or(false);
            if notify && !focused {
                let body = match &ev {
                    EngineEvent::ConflictDetected { path, .. } => {
                        Some(("Conflict needs your decision", format!("{path} changed on two devices.")))
                    }
                    EngineEvent::PairingRequest { device_name, .. } => {
                        Some(("Pairing request", format!("{device_name} wants to pair.")))
                    }
                    EngineEvent::OfferReceived { name, .. } => {
                        Some(("Project shared with you", format!("“{name}” is available to join.")))
                    }
                    EngineEvent::Error { message, .. } => Some(("Sync problem", message.clone())),
                    _ => None,
                };
                if let Some((title, body)) = body {
                    let _ = app.notification().builder().title(title).body(body).show();
                }
            }
        }
    });
}
