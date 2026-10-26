//! Remote Workspace Mode commands (setup crate: `devbridge-remote`).
//!
//! Host side: choose which projects are reachable remotely, approve or revoke
//! client keys. Client side: request access, then open a host project in VS
//! Code through Remote-SSH. All exchanges travel over the paired, mutually
//! authenticated DevBridge connection.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;

use devbridge_core::protocol::{Message, RemoteWorkspace};
use devbridge_core::{DeviceId, OsKind, WorkspaceId};
use devbridge_remote as remote;
use devbridge_store::RemoteHost;
use serde::Serialize;
use tauri::{Emitter, Manager, State};

use super::blocking;
use crate::error::{CmdError, CmdResult};
use crate::state::{AppState, lock};

/// Pending access requests on the host: device → public key.
#[derive(Default)]
pub struct Pending(pub Mutex<HashMap<DeviceId, String>>);

fn home() -> CmdResult<PathBuf> {
    dirs_home().ok_or_else(|| CmdError::new("no_home", "Couldn't find your home folder."))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantView {
    pub device_id: DeviceId,
    pub device_name: String,
    pub granted_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostView {
    #[serde(flatten)]
    pub host: RemoteHost,
    pub device_name: String,
    pub online: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub hosting: Vec<WorkspaceId>,
    pub ssh_server_running: bool,
    pub ssh_server_guidance: &'static str,
    pub grants: Vec<GrantView>,
    pub pending: Vec<GrantView>,
    pub hosts: Vec<HostView>,
    pub include_configured: bool,
    pub remote_ssh_installed: bool,
}

#[tauri::command]
pub async fn remote_status(state: State<'_, AppState>, pending: State<'_, Pending>) -> CmdResult<RemoteStatus> {
    let home = home()?;
    let pending: Vec<DeviceId> = pending.0.lock().unwrap_or_else(|p| p.into_inner()).keys().copied().collect();
    let store = state.store.clone();
    let engine = state.engine.clone();
    blocking(move || {
        let s = lock(&store)?;
        let name = |d: DeviceId| s.device_get(d).map(|d| d.name).unwrap_or_else(|_| "Unknown device".into());
        Ok(RemoteStatus {
            hosting: s.remote_hosting()?,
            ssh_server_running: remote::local_ssh_server_running(),
            ssh_server_guidance: remote::ssh_server_guidance(),
            grants: s
                .remote_grants()?
                .into_iter()
                .map(|g| GrantView { device_name: name(g.device_id), device_id: g.device_id, granted_at: g.granted_at })
                .collect(),
            pending: pending
                .into_iter()
                .map(|d| GrantView { device_name: name(d), device_id: d, granted_at: 0 })
                .collect(),
            hosts: s
                .remote_hosts()?
                .into_iter()
                .map(|h| HostView { device_name: name(h.device_id), online: engine.is_online(h.device_id), host: h })
                .collect(),
            include_configured: remote::include_present(&home),
            remote_ssh_installed: remote::remote_ssh_installed(&home),
        })
    })
    .await
}

/// Host: make a project available (or not) to approved clients.
#[tauri::command]
pub async fn remote_set_hosting(state: State<'_, AppState>, workspace_id: WorkspaceId, enabled: bool) -> CmdResult<()> {
    state.store()?.set_remote_hosting(workspace_id, enabled)?;
    // Tell approved, connected clients about the new list.
    let grants = state.store()?.remote_grants()?;
    for g in grants {
        if state.engine.is_online(g.device_id) {
            let msg = granted_message(&state)?;
            state.engine.send_remote(g.device_id, msg).await;
        }
    }
    Ok(())
}

/// Client: ask a paired host for remote access.
#[tauri::command]
pub async fn remote_request_access(state: State<'_, AppState>, device_id: DeviceId) -> CmdResult<()> {
    if !state.engine.is_online(device_id) {
        return Err(CmdError::new("offline", "That device isn't connected. Make sure DevBridge is running on it."));
    }
    let home = home()?;
    let me = state.local_device.to_string();
    let host = device_id.to_string();
    let key = blocking(move || {
        remote::ensure_keypair(&home, &host, &me).map_err(|e| CmdError::new("ssh_key", e.to_string()))
    })
    .await?;
    state.engine.send_remote(device_id, Message::RemoteAccessRequest { ssh_public_key: key }).await;
    state.store()?.audit("remote.access_requested", None, Some(&device_id.to_string()), serde_json::json!({}))?;
    Ok(())
}

fn granted_message(state: &AppState) -> CmdResult<Message> {
    let s = state.store()?;
    let workspaces = s
        .remote_hosting()?
        .into_iter()
        .filter_map(|id| s.get_workspace(id).ok())
        .map(|w| RemoteWorkspace { ws: w.id, name: w.name, path: w.root_path })
        .collect();
    Ok(Message::RemoteAccessGranted {
        ssh_user: remote::current_user(),
        ssh_port: 22,
        host_os: OsKind::current(),
        workspaces,
        host_keys: remote::host_public_keys(),
    })
}

/// Host: approve or deny a pending request.
#[tauri::command]
pub async fn remote_decide(
    state: State<'_, AppState>,
    pending: State<'_, Pending>,
    device_id: DeviceId,
    allow: bool,
) -> CmdResult<()> {
    let key = pending
        .0
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(&device_id)
        .ok_or_else(|| CmdError::new("expired", "That request is no longer pending."))?;
    if !allow {
        state
            .engine
            .send_remote(device_id, Message::RemoteAccessDenied { reason: "The host declined the request.".into() })
            .await;
        return Ok(());
    }
    let home = home()?;
    let id = device_id.to_string();
    let k = key.clone();
    blocking(move || {
        remote::authorize_key(&home, &k, &id).map_err(|e| CmdError::new("authorize_failed", e.to_string()))?;
        Ok(())
    })
    .await?;
    state.store()?.remote_grant(device_id, &key)?;
    let msg = granted_message(&state)?;
    state.engine.send_remote(device_id, msg).await;
    Ok(())
}

/// Revokes remote access in either direction for `device_id`.
#[tauri::command]
pub async fn remote_revoke(state: State<'_, AppState>, device_id: DeviceId) -> CmdResult<()> {
    revoke_keys_for(&state, device_id).await;
    state.engine.send_remote(device_id, Message::RemoteAccessRevoked {}).await;
    Ok(())
}

/// Removes host grants and client host entries for a device (also used when a
/// device is unpaired).
pub async fn revoke_keys_for(state: &AppState, device_id: DeviceId) {
    let Ok(home) = home() else { return };
    let id = device_id.to_string();
    let h = home.clone();
    let _ = blocking(move || remote::remove_keys(&h, &id).map_err(CmdError::internal)).await;
    if let Ok(s) = state.store() {
        let _ = s.remote_ungrant(device_id);
        if let Ok(Some(host)) = s.remote_host_delete(device_id) {
            let _ = remote::remove_host_entry(&home, &host.alias);
        }
    }
}

/// Client: open a host project in VS Code via Remote-SSH. `allow_include`
/// confirms adding the `Include` line to `~/.ssh/config`.
#[tauri::command]
pub async fn remote_open(
    state: State<'_, AppState>,
    device_id: DeviceId,
    workspace_id: WorkspaceId,
    allow_include: bool,
) -> CmdResult<()> {
    let home = home()?;
    let host = state
        .store()?
        .remote_host_get(device_id)?
        .ok_or_else(|| CmdError::new("no_access", "Request remote access to this device first."))?;
    let ws = host
        .workspaces
        .iter()
        .find(|w| w.ws == workspace_id)
        .cloned()
        .ok_or_else(|| CmdError::new("not_hosted", "The host no longer shares that project remotely."))?;
    if !remote::remote_ssh_installed(&home) {
        return Err(CmdError::new(
            "remote_ssh_missing",
            "Install the “Remote - SSH” extension (publisher: Microsoft) in VS Code, then try again.",
        ));
    }
    if !remote::include_present(&home) {
        if !allow_include {
            return Err(CmdError::new("include_needed", "DevBridge needs to add one Include line to ~/.ssh/config."));
        }
        remote::ensure_include(&home).map_err(|e| CmdError::new("ssh_config", e.to_string()))?;
        state.store()?.audit("remote.ssh_config_include_added", None, None, serde_json::json!({}))?;
    }
    let custom = state.store()?.settings()?.vscode_path.map(PathBuf::from);
    let code = devbridge_workspace::tools::detect_vscode(custom.as_deref())
        .path
        .ok_or_else(|| CmdError::new("launch", "VS Code wasn't found. Set its path in Settings → Tools."))?;
    let uri = remote::folder_uri(&host.alias, host.host_os == OsKind::Windows, &ws.path);
    std::process::Command::new(code)
        .args(["--folder-uri", &uri])
        .spawn()
        .map_err(|e| CmdError::new("launch", format!("Couldn't start VS Code: {e}")))?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCheck {
    pub devbridge_online: bool,
    pub ssh_reachable: bool,
}

/// Client: connection health for a host.
#[tauri::command]
pub async fn remote_check(state: State<'_, AppState>, device_id: DeviceId) -> CmdResult<RemoteCheck> {
    let host = state.store()?.remote_host_get(device_id)?;
    let online = state.engine.is_online(device_id);
    let addr = host.and_then(|h| format!("{}:{}", h.ssh_host, h.ssh_port).parse::<SocketAddr>().ok());
    let reachable = match addr {
        Some(a) => blocking(move || Ok(remote::port_reachable(a))).await?,
        None => false,
    };
    Ok(RemoteCheck { devbridge_online: online, ssh_reachable: reachable })
}

/// Handles remote-workspace messages arriving from a paired device.
pub async fn on_message(app: &tauri::AppHandle, from: DeviceId, raw: serde_json::Value) {
    let Ok(msg) = serde_json::from_value::<Message>(raw) else { return };
    let Some(state) = app.try_state::<AppState>() else { return };
    match msg {
        Message::RemoteAccessRequest { ssh_public_key } => {
            if let Ok(key) = remote::validate_public_key(&ssh_public_key) {
                if let Some(p) = app.try_state::<Pending>() {
                    p.0.lock().unwrap_or_else(|p| p.into_inner()).insert(from, key);
                }
                let _ = app.emit("remote", serde_json::json!({ "kind": "request", "deviceId": from }));
            }
        }
        Message::RemoteAccessGranted { ssh_user, ssh_port, host_os, workspaces, host_keys } => {
            let Ok(home) = home() else { return };
            let Ok(s) = state.store() else { return };
            let Ok(device) = s.device_get(from) else { return };
            let ip = s
                .device_address(from)
                .ok()
                .flatten()
                .and_then(|a| a.parse::<SocketAddr>().ok())
                .map(|a| a.ip().to_string());
            let Some(ip) = ip else { return };
            let alias = remote::alias_for(&device.name, &from.to_string());
            let key_path = remote::key_path(&home, &from.to_string());
            let entry = remote::HostEntry {
                alias: &alias,
                host: &ip,
                port: ssh_port,
                user: &ssh_user,
                key_path: &key_path,
                host_keys: &host_keys,
            };
            if let Err(e) = remote::write_host_entry(&home, &entry) {
                tracing::warn!(error = %e, "couldn't write SSH host entry");
                return;
            }
            let _ = s.remote_host_save(&RemoteHost {
                device_id: from,
                alias,
                ssh_user,
                ssh_host: ip,
                ssh_port,
                host_os,
                key_path: key_path.display().to_string(),
                workspaces,
                granted_at: devbridge_core::time::now_ms(),
            });
            drop(s);
            let _ = app.emit("remote", serde_json::json!({ "kind": "granted", "deviceId": from }));
        }
        Message::RemoteAccessDenied { reason } => {
            let _ = app.emit("remote", serde_json::json!({ "kind": "denied", "deviceId": from, "reason": reason }));
        }
        Message::RemoteAccessRevoked {} => {
            if let (Ok(home), Ok(s)) = (home(), state.store())
                && let Ok(Some(h)) = s.remote_host_delete(from)
            {
                let _ = remote::remove_host_entry(&home, &h.alias);
            }
            let _ = app.emit("remote", serde_json::json!({ "kind": "revoked", "deviceId": from }));
        }
        _ => {}
    }
}
