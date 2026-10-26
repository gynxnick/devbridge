//! Diagnostics view and exportable report. The report contains versions,
//! settings, counts, recent activity and (redacted) log lines — never file
//! contents, keys or tokens. Project paths are included (they help support);
//! the export dialog says so.

use serde::Serialize;
use tauri::State;

use crate::error::{CmdError, CmdResult};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub app_version: &'static str,
    pub os: String,
    pub arch: &'static str,
    pub schema_version: i64,
    pub data_dir: String,
    pub logs_dir: String,
    pub git_found: bool,
    pub identity_storage: String,
    pub recovery_bytes: u64,
    pub listen_address: Option<String>,
}

#[tauri::command]
pub fn diagnostics(state: State<'_, AppState>) -> CmdResult<Diagnostics> {
    Ok(Diagnostics {
        app_version: env!("CARGO_PKG_VERSION"),
        os: os_info::get().to_string(),
        arch: std::env::consts::ARCH,
        schema_version: state.store()?.schema_version()?,
        data_dir: state.data_dir.display().to_string(),
        logs_dir: state.logs_dir.display().to_string(),
        git_found: state.git()?.is_some(),
        identity_storage: state.identity_storage.clone(),
        recovery_bytes: state.engine.recovery().usage_bytes(),
        listen_address: state.engine.status().listen_address,
    })
}

/// Writes a JSON diagnostic report to `path` (chosen by the user).
#[tauri::command]
pub fn export_diagnostics(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    let s = state.store()?;
    let workspaces: Vec<_> = s
        .list_workspaces()?
        .into_iter()
        .map(|w| {
            serde_json::json!({
                "id": w.id, "name": w.name, "root": w.root_path, "mode": w.mode, "syncState": w.sync_state,
                "lastSyncedAt": w.last_synced_at,
                "openConflicts": s.count_open_conflicts(w.id).unwrap_or(0),
                "issues": s.count_issues(w.id).unwrap_or(0),
                "queued": s.count_queued_for(w.id).unwrap_or(0),
                "transfers": s.transfers_active_count(w.id).unwrap_or(0),
            })
        })
        .collect();
    let devices: Vec<_> = s
        .list_devices()?
        .into_iter()
        .map(|d| serde_json::json!({ "id": d.id, "name": d.name, "os": d.os, "status": d.status, "lastSeenAt": d.last_seen_at }))
        .collect();
    let mut settings = serde_json::to_value(s.settings()?).map_err(CmdError::internal)?;
    // Custom tool paths can contain the user name; keep only whether they're set.
    for k in ["vscodePath", "gitPath"] {
        if let Some(v) = settings.get_mut(k) {
            *v = serde_json::Value::Bool(!v.is_null());
        }
    }
    let report = serde_json::json!({
        "generatedAt": devbridge_core::time::now_ms(),
        "app": { "version": env!("CARGO_PKG_VERSION"), "os": os_info::get().to_string(), "arch": std::env::consts::ARCH },
        "schemaVersion": s.schema_version()?,
        "identityStorage": state.identity_storage,
        "engine": state.engine.status(),
        "workspaces": workspaces,
        "devices": devices,
        "settings": settings,
        "recentActivity": s.recent_audit(200)?,
        "log": crate::logging::tail(&state.logs_dir, 500),
        "crashes": crate::logging::crash_tail(&state.logs_dir, 100),
    });
    let text = serde_json::to_string_pretty(&report).map_err(CmdError::internal)?;
    std::fs::write(&path, devbridge_core::redact::redact(&text))
        .map_err(|e| CmdError::new("io", format!("Couldn't save the report: {e}")))?;
    s.audit("diagnostics.exported", None, None, serde_json::json!({}))?;
    Ok(())
}
