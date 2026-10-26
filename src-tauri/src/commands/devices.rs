//! Devices, pairing and discovery.

use std::net::SocketAddr;

use devbridge_core::{Device, DeviceId};
use devbridge_net::DEFAULT_PORT;
use devbridge_sync::{EngineStatus, PairingWindow};
use serde::Serialize;
use tauri::State;

use crate::error::{CmdError, CmdResult};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceView {
    #[serde(flatten)]
    pub device: Device,
    pub online: bool,
    pub short_fingerprint: Option<String>,
}

#[tauri::command]
pub fn list_devices(state: State<'_, AppState>) -> CmdResult<Vec<DeviceView>> {
    Ok(state
        .store()?
        .list_devices()?
        .into_iter()
        .map(|d| DeviceView {
            online: d.is_local || state.engine.is_online(d.id),
            short_fingerprint: d.cert_fingerprint.as_deref().map(devbridge_net::identity::short_fingerprint),
            device: d,
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    #[serde(flatten)]
    pub status: EngineStatus,
    pub short_fingerprint: String,
}

#[tauri::command]
pub fn engine_status(state: State<'_, AppState>) -> StatusView {
    let status = state.engine.status();
    StatusView { short_fingerprint: devbridge_net::identity::short_fingerprint(&status.fingerprint), status }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredView {
    pub device_id: String,
    pub name: String,
    pub address: String,
}

/// Devices on the network that currently have a pairing window open.
#[tauri::command]
pub fn list_discovered(state: State<'_, AppState>) -> Vec<DiscoveredView> {
    state
        .engine
        .discovered()
        .into_iter()
        .filter_map(|d| {
            Some(DiscoveredView {
                name: d.pairing_name?,
                address: d.addrs.first()?.to_string(),
                device_id: d.device_id,
            })
        })
        .collect()
}

#[tauri::command]
pub fn pairing_open(state: State<'_, AppState>) -> CmdResult<PairingWindow> {
    let w = state.engine.pairing_open()?;
    state.store()?.audit("pairing.opened", None, None, serde_json::json!({}))?;
    Ok(w)
}

#[tauri::command]
pub fn pairing_close(state: State<'_, AppState>) {
    state.engine.pairing_close();
}

#[tauri::command]
pub fn pairing_current(state: State<'_, AppState>) -> Option<PairingWindow> {
    state.engine.pairing_current()
}

#[tauri::command]
pub fn pairing_decide(state: State<'_, AppState>, request_id: String, approve: bool) -> CmdResult<()> {
    if state.engine.pairing_decide(&request_id, approve) {
        Ok(())
    } else {
        Err(CmdError::new("expired", "This pairing request has expired."))
    }
}

/// `address` is `host:port` or just `host` (default port).
#[tauri::command]
pub async fn pair_join(state: State<'_, AppState>, address: String, code: String) -> CmdResult<Device> {
    let address = address.trim();
    let addr: SocketAddr = address.parse().or_else(|_| format!("{address}:{DEFAULT_PORT}").parse()).map_err(|_| {
        CmdError::new("invalid_address", "Enter the address shown on the other computer, like 192.168.1.20:47821.")
    })?;
    Ok(state.engine.pair_join(addr, &code).await?)
}

#[tauri::command]
pub async fn revoke_device(state: State<'_, AppState>, id: DeviceId) -> CmdResult<()> {
    // Remote-workspace keys for this device are removed as well.
    crate::commands::remote::revoke_keys_for(&state, id).await;
    Ok(state.engine.revoke(id).await?)
}
