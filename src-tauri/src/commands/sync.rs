//! Sync controls, sharing and offers.

use std::path::Path;

use devbridge_core::time::now_ms;
use devbridge_core::{DeviceId, SyncState, Workspace, WorkspaceId, WorkspaceMode};
use devbridge_store::{ShareState, TransferJob};
use serde::Serialize;
use tauri::State;

use super::blocking;
use crate::error::{CmdError, CmdResult};
use crate::state::{AppState, lock};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    #[serde(flatten)]
    pub share: ShareState,
    pub online: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSync {
    pub paused: bool,
    pub shares: Vec<ShareView>,
    pub transfers: Vec<TransferJob>,
    pub failed_transfers: Vec<TransferJob>,
    pub queued_changes: i64,
    pub open_issues: i64,
}

#[tauri::command]
pub async fn workspace_sync(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<WorkspaceSync> {
    let store = state.store.clone();
    let engine = state.engine.clone();
    blocking(move || {
        let s = lock(&store)?;
        Ok(WorkspaceSync {
            paused: engine.is_paused(id),
            shares: s
                .share_states(id)?
                .into_iter()
                .map(|share| ShareView { online: engine.is_online(share.device_id), share })
                .collect(),
            transfers: s.transfers_list(id)?,
            failed_transfers: s.transfers_failed(id)?,
            queued_changes: s.count_queued_for(id)?,
            open_issues: s.count_issues(id)?,
        })
    })
    .await
}

#[tauri::command]
pub fn set_paused(state: State<'_, AppState>, id: WorkspaceId, paused: bool) -> CmdResult<()> {
    state.engine.set_paused(id, paused);
    state.store()?.audit(
        if paused { "sync.paused" } else { "sync.resumed" },
        Some(&id.to_string()),
        None,
        serde_json::json!({}),
    )?;
    Ok(())
}

#[tauri::command]
pub fn rescan(state: State<'_, AppState>, id: WorkspaceId) {
    state.engine.rescan(id);
}

#[tauri::command]
pub fn retry_transfers(state: State<'_, AppState>, id: WorkspaceId) {
    state.engine.retry(id);
}

#[tauri::command]
pub async fn share_workspace(state: State<'_, AppState>, id: WorkspaceId, device_id: DeviceId) -> CmdResult<()> {
    Ok(state.engine.share(id, device_id).await?)
}

#[tauri::command]
pub async fn unshare_workspace(state: State<'_, AppState>, id: WorkspaceId, device_id: DeviceId) -> CmdResult<()> {
    Ok(state.engine.unshare(id, device_id).await?)
}

#[tauri::command]
pub fn list_offers(state: State<'_, AppState>) -> CmdResult<Vec<devbridge_store::WorkspaceOffer>> {
    Ok(state.store()?.offers_list()?)
}

/// Joins a project another device shared. If the project isn't registered
/// here yet, `path` is the local folder to sync it into (an empty folder is
/// recommended; existing files that differ become conflicts, never overwrites).
#[tauri::command]
pub async fn join_offer(
    state: State<'_, AppState>,
    workspace_id: WorkspaceId,
    device_id: DeviceId,
    path: Option<String>,
) -> CmdResult<()> {
    let exists = state.store()?.get_workspace(workspace_id).is_ok();
    if !exists {
        let path = path.ok_or_else(|| CmdError::new("path_required", "Choose a folder for this project first."))?;
        let offer = state
            .store()?
            .offer_get(workspace_id, device_id)?
            .ok_or_else(|| CmdError::new("not_found", "That offer is no longer available."))?;
        let store = state.store.clone();
        let protected = state.protected.clone();
        let local = state.local_device;
        blocking(move || {
            let s = lock(&store)?;
            let existing: Vec<(String, std::path::PathBuf)> =
                s.list_workspaces()?.into_iter().map(|w| (w.name, w.root_path.into())).collect();
            let regs: Vec<devbridge_workspace::roots::Registered<'_>> =
                existing.iter().map(|(n, r)| devbridge_workspace::roots::Registered { name: n, root: r }).collect();
            let root = devbridge_workspace::validate_root(Path::new(&path), &regs, &protected)?;
            let now = now_ms();
            s.insert_workspace(&Workspace {
                id: workspace_id,
                name: offer.name,
                root_path: root.display().to_string(),
                color: offer.color,
                icon: offer.icon,
                mode: WorkspaceMode::Synced,
                sync_state: SyncState::Offline,
                active_device_id: Some(local),
                last_synced_at: None,
                created_at: now,
                updated_at: now,
            })?;
            Ok(())
        })
        .await?;
    }
    Ok(state.engine.join(workspace_id, device_id).await?)
}

#[tauri::command]
pub fn decline_offer(state: State<'_, AppState>, workspace_id: WorkspaceId, device_id: DeviceId) -> CmdResult<()> {
    Ok(state.store()?.offer_delete(workspace_id, device_id)?)
}
