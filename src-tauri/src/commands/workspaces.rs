//! Workspace registration, tools, settings and activity.

use std::path::{Path, PathBuf};

use devbridge_core::{
    AppSettings, DEVICE_NAME_MAX_CHARS, Device, WORKSPACE_NAME_MAX_CHARS, Workspace, WorkspaceColor, WorkspaceIcon,
    WorkspaceId, validate_display_name,
};
use devbridge_git::RepoInfo;
use devbridge_store::{AuditEvent, GitRepoRecord, WorkspaceOffer};
use devbridge_workspace::tools::{self, ToolReport};
use devbridge_workspace::{ImportRequest, PathInspection, launch};
use serde::Serialize;
use tauri::State;

use super::blocking;
use crate::error::{CmdError, CmdResult};
use crate::state::{AppState, listen_from, lock, tuning_from};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSummary {
    #[serde(flatten)]
    pub workspace: Workspace,
    pub open_conflicts: i64,
    pub pending_changes: i64,
    pub open_issues: i64,
    pub git: Option<GitRepoRecord>,
    /// False if the root folder is currently missing (unmounted disk, moved, …).
    pub root_available: bool,
    pub paused: bool,
    /// Devices this workspace actively syncs with, and how many are online.
    pub synced_devices: usize,
    pub online_devices: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub device: Device,
    pub workspaces: Vec<WorkspaceSummary>,
    pub paired_device_count: usize,
    pub online_device_count: usize,
    pub offers: Vec<WorkspaceOffer>,
}

#[tauri::command]
pub async fn get_overview(state: State<'_, AppState>) -> CmdResult<Overview> {
    let store = state.store.clone();
    let local = state.local_device;
    let engine = state.engine.clone();
    blocking(move || {
        let s = lock(&store)?;
        let devices = s.list_devices()?;
        let device = devices
            .iter()
            .find(|d| d.id == local)
            .cloned()
            .ok_or_else(|| CmdError::internal("local device missing"))?;
        let workspaces = s
            .list_workspaces()?
            .into_iter()
            .map(|w| {
                let shares: Vec<_> = s.share_states(w.id)?.into_iter().filter(|p| p.shared && p.joined).collect();
                Ok(WorkspaceSummary {
                    open_conflicts: s.count_open_conflicts(w.id)?,
                    pending_changes: s.count_queued_for(w.id)? + s.transfers_active_count(w.id)?,
                    open_issues: s.count_issues(w.id)?,
                    git: s.get_git_repo(w.id)?,
                    root_available: Path::new(&w.root_path).is_dir(),
                    paused: engine.is_paused(w.id),
                    online_devices: shares.iter().filter(|p| engine.is_online(p.device_id)).count(),
                    synced_devices: shares.len(),
                    workspace: w,
                })
            })
            .collect::<CmdResult<Vec<_>>>()?;
        let paired: Vec<_> = devices.iter().filter(|d| d.status == devbridge_core::DeviceStatus::Paired).collect();
        Ok(Overview {
            device,
            workspaces,
            paired_device_count: paired.len(),
            online_device_count: paired.iter().filter(|d| engine.is_online(d.id)).count(),
            offers: s.offers_list()?,
        })
    })
    .await
}

#[tauri::command]
pub async fn inspect_path(state: State<'_, AppState>, path: String) -> CmdResult<PathInspection> {
    let git = state.git()?;
    let store = state.store.clone();
    let protected = state.protected.clone();
    blocking(move || {
        let s = lock(&store)?;
        Ok(devbridge_workspace::inspect_path(&s, git.as_ref(), Path::new(&path), &protected))
    })
    .await
}

#[tauri::command]
pub async fn import_workspace(state: State<'_, AppState>, request: ImportRequest) -> CmdResult<Workspace> {
    let git = state.git()?;
    let store = state.store.clone();
    let protected = state.protected.clone();
    let device = state.local_device;
    let ws = blocking(move || {
        let s = lock(&store)?;
        let ws = devbridge_workspace::import_workspace(&s, git.as_ref(), device, &request, &protected)?;
        tracing::info!(workspace = %ws.id, "workspace imported");
        Ok(ws)
    })
    .await?;
    state.engine.workspace_added(ws.id);
    Ok(ws)
}

#[tauri::command]
pub fn update_workspace(
    state: State<'_, AppState>,
    id: WorkspaceId,
    name: String,
    color: WorkspaceColor,
    icon: WorkspaceIcon,
) -> CmdResult<Workspace> {
    let name = validate_display_name(&name, WORKSPACE_NAME_MAX_CHARS)?;
    Ok(state.store()?.update_workspace_appearance(id, &name, color, icon)?)
}

/// Unregisters a workspace. The project folder and its files are left
/// untouched; devices it was shared with are told to stop syncing it.
#[tauri::command]
pub async fn remove_workspace(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<()> {
    let peers: Vec<_> = state.store()?.share_states(id)?.into_iter().map(|p| p.device_id).collect();
    for p in peers {
        let _ = state.engine.unshare(id, p).await;
    }
    state.engine.workspace_removed(id);
    state.store()?.delete_workspace(id)?;
    tracing::info!(workspace = %id, "workspace removed from DevBridge (files untouched)");
    Ok(())
}

#[tauri::command]
pub async fn refresh_git(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<Option<RepoInfo>> {
    let git = state.git()?;
    let store = state.store.clone();
    blocking(move || {
        let root = PathBuf::from(lock(&store)?.get_workspace(id)?.root_path);
        let Some(git) = git else { return Ok(None) };
        let info = git.discover(&root)?;
        if let Some(i) = &info {
            lock(&store)?.upsert_git_repo(
                id,
                &GitRepoRecord {
                    toplevel: i.toplevel.display().to_string(),
                    last_branch: i.branch.clone(),
                    last_head: i.head.clone(),
                    last_checked_at: Some(devbridge_core::time::now_ms()),
                },
            )?;
        }
        Ok(info)
    })
    .await
}

#[tauri::command]
pub async fn open_in_vscode(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<()> {
    let root = state.workspace_root(id)?;
    let custom = state.store()?.settings()?.vscode_path.map(PathBuf::from);
    blocking(move || {
        let status = tools::detect_vscode(custom.as_deref());
        launch::open_in_vscode(status.path.as_deref().map(Path::new), &root)?;
        Ok(())
    })
    .await
}

#[tauri::command]
pub fn open_terminal(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<()> {
    let root = state.workspace_root(id)?;
    Ok(launch::open_terminal(&root)?)
}

#[tauri::command]
pub fn rename_local_device(state: State<'_, AppState>, name: String) -> CmdResult<Device> {
    let name = validate_display_name(&name, DEVICE_NAME_MAX_CHARS)?;
    let s = state.store()?;
    s.rename_device(state.local_device, &name)?;
    state.engine.set_device_name(&name);
    s.local_device()?.ok_or_else(|| CmdError::internal("local device missing"))
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CmdResult<AppSettings> {
    Ok(state.store()?.settings()?)
}

#[tauri::command]
pub async fn save_settings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> CmdResult<AppSettings> {
    let mut settings = settings;
    // Normalise blank custom paths to "auto-detect".
    for p in [&mut settings.vscode_path, &mut settings.git_path] {
        if p.as_deref().is_some_and(|s| s.trim().is_empty()) {
            *p = None;
        }
    }
    settings.exclude_patterns.retain(|p| !p.trim().is_empty());
    settings.validate()?;
    let saved = {
        let s = state.store()?;
        s.save_settings(&settings)?;
        s.settings()?
    };
    state.engine.set_tuning(tuning_from(&saved));
    state.engine.set_listen(listen_from(&saved)).await;
    {
        use tauri_plugin_autostart::ManagerExt;
        let al = app.autolaunch();
        let r = if saved.launch_at_login { al.enable() } else { al.disable() };
        if let Err(e) = r {
            tracing::warn!(error = %e, "couldn't update launch-at-login");
        }
    }
    Ok(saved)
}

#[tauri::command]
pub async fn detect_tools(state: State<'_, AppState>) -> CmdResult<ToolReport> {
    let settings = state.store()?.settings()?;
    blocking(move || {
        Ok(tools::detect_all(
            settings.vscode_path.as_deref().map(Path::new),
            settings.git_path.as_deref().map(Path::new),
        ))
    })
    .await
}

#[tauri::command]
pub fn list_activity(state: State<'_, AppState>, limit: Option<u32>) -> CmdResult<Vec<AuditEvent>> {
    Ok(state.store()?.recent_audit(limit.unwrap_or(50).min(500))?)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDefaults {
    pub always_excluded: &'static [&'static str],
    pub default_excludes: &'static [&'static str],
    pub secret_patterns: &'static [&'static str],
    pub secret_exceptions: &'static [&'static str],
}

#[tauri::command]
pub fn sync_defaults() -> SyncDefaults {
    use devbridge_core::rules::*;
    SyncDefaults {
        always_excluded: ALWAYS_EXCLUDED,
        default_excludes: DEFAULT_EXCLUDES,
        secret_patterns: DEFAULT_SECRET_PATTERNS,
        secret_exceptions: SECRET_EXCEPTIONS,
    }
}
