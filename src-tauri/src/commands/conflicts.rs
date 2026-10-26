//! Conflict center, sync issues and recovery history.

use devbridge_core::time::now_ms;
use devbridge_core::{RelPath, WorkspaceId};
use devbridge_store::{ConflictRecord, RecoveryItem, SyncIssue};
use devbridge_sync::apply::Resolution;
use serde::{Deserialize, Serialize};
use tauri::State;

use super::blocking;
use crate::error::{CmdError, CmdResult};
use crate::state::{AppState, lock};

/// Largest file shown as text in the comparison view.
const TEXT_LIMIT: u64 = 2 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictView {
    #[serde(flatten)]
    pub conflict: ConflictRecord,
    pub workspace_name: String,
    pub remote_device_name: String,
}

fn names(
    s: &devbridge_store::Store,
) -> CmdResult<(std::collections::HashMap<String, String>, std::collections::HashMap<String, String>)> {
    let ws = s.list_workspaces()?.into_iter().map(|w| (w.id.to_string(), w.name)).collect();
    let dev = s.list_devices()?.into_iter().map(|d| (d.id.to_string(), d.name)).collect();
    Ok((ws, dev))
}

#[tauri::command]
pub fn list_conflicts(
    state: State<'_, AppState>,
    workspace_id: Option<WorkspaceId>,
    include_resolved: bool,
) -> CmdResult<Vec<ConflictView>> {
    let s = state.store()?;
    let (ws, dev) = names(&s)?;
    Ok(s.conflicts_list(workspace_id, !include_resolved)?
        .into_iter()
        .map(|c| ConflictView {
            workspace_name: ws.get(&c.workspace_id.to_string()).cloned().unwrap_or_default(),
            remote_device_name: dev
                .get(&c.remote_device_id.to_string())
                .cloned()
                .unwrap_or_else(|| "another device".into()),
            conflict: c,
        })
        .collect())
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Side {
    Text { text: String },
    Binary { size: u64 },
    Missing,
}

fn side(bytes: Option<Vec<u8>>, size: Option<u64>, deleted: bool) -> Side {
    match bytes {
        Some(b) => match String::from_utf8(b) {
            Ok(text) if !text.contains('\0') => Side::Text { text },
            Ok(t) => Side::Binary { size: t.len() as u64 },
            Err(e) => Side::Binary { size: e.as_bytes().len() as u64 },
        },
        None if deleted => Side::Missing,
        None => Side::Binary { size: size.unwrap_or(0) },
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictDetail {
    #[serde(flatten)]
    pub view: ConflictView,
    pub local: Side,
    pub remote: Side,
    pub recovery: Vec<RecoveryItem>,
}

#[tauri::command]
pub async fn conflict_detail(state: State<'_, AppState>, id: String) -> CmdResult<ConflictDetail> {
    let (c, view_names, recovery) = {
        let s = state.store()?;
        let c = s.conflict_get(&id)?;
        let (ws, dev) = names(&s)?;
        let recovery = s.recovery_for_conflict(&id)?;
        (c, (ws, dev), recovery)
    };
    let (local, remote) = state.engine.conflict_contents(c.workspace_id, &id, TEXT_LIMIT).await?;
    let local_side = side(local, c.local_size, c.kind == "local_deleted");
    let remote_side = side(remote, c.remote_size, c.kind == "remote_deleted");
    Ok(ConflictDetail {
        view: ConflictView {
            workspace_name: view_names.0.get(&c.workspace_id.to_string()).cloned().unwrap_or_default(),
            remote_device_name: view_names
                .1
                .get(&c.remote_device_id.to_string())
                .cloned()
                .unwrap_or_else(|| "another device".into()),
            conflict: c,
        },
        local: local_side,
        remote: remote_side,
        recovery,
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionKind {
    KeepLocal,
    KeepRemote,
    SaveBoth,
    Merged,
}

#[tauri::command]
pub async fn resolve_conflict(
    state: State<'_, AppState>,
    id: String,
    resolution: ResolutionKind,
    merged_text: Option<String>,
) -> CmdResult<()> {
    let (ws, device_name) = {
        let s = state.store()?;
        let c = s.conflict_get(&id)?;
        let name = s.device_get(c.remote_device_id).map(|d| d.name).unwrap_or_else(|_| "other device".into());
        (c.workspace_id, name)
    };
    let res = match resolution {
        ResolutionKind::KeepLocal => Resolution::KeepLocal,
        ResolutionKind::KeepRemote => Resolution::KeepRemote,
        ResolutionKind::SaveBoth => {
            let date = chrono_like_date(now_ms());
            let safe: String = device_name.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect();
            Resolution::SaveBoth { suffix: format!("(conflict from {} {date})", safe.trim()) }
        }
        ResolutionKind::Merged => Resolution::Merged(
            merged_text.ok_or_else(|| CmdError::new("invalid", "The merged text is missing."))?.into_bytes(),
        ),
    };
    Ok(state.engine.resolve_conflict(ws, &id, res).await?)
}

/// `YYYY-MM-DD` (UTC) without a date-time dependency.
fn chrono_like_date(ms: i64) -> String {
    let days = ms.div_euclid(86_400_000);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

#[tauri::command]
pub fn list_issues(state: State<'_, AppState>, workspace_id: Option<WorkspaceId>) -> CmdResult<Vec<SyncIssue>> {
    Ok(state.store()?.issues_list(workspace_id)?)
}

#[tauri::command]
pub fn dismiss_issue(
    state: State<'_, AppState>,
    workspace_id: WorkspaceId,
    path: String,
    kind: String,
) -> CmdResult<()> {
    Ok(state.store()?.issue_dismiss(workspace_id, &path, &kind)?)
}

#[tauri::command]
pub fn list_recovery(state: State<'_, AppState>, workspace_id: Option<WorkspaceId>) -> CmdResult<Vec<RecoveryItem>> {
    Ok(state.store()?.recovery_list(workspace_id)?)
}

/// Writes a recovery copy back into its project as a *new* file next to the
/// original (never overwriting anything). Returns the new relative path.
#[tauri::command]
pub async fn restore_recovery(state: State<'_, AppState>, id: i64) -> CmdResult<String> {
    let item = state.store()?.recovery_get(id)?;
    let ws: WorkspaceId =
        item.workspace_id.parse().map_err(|_| CmdError::new("not_found", "That project no longer exists."))?;
    let root = state.workspace_root(ws)?;
    let blob = state
        .engine
        .recovery()
        .blob_path(&item.content_hash)
        .filter(|p| p.is_file())
        .ok_or_else(|| CmdError::new("not_found", "The recovery copy is missing."))?;
    let staging = root.join(".devbridge").join("staging");
    let original = RelPath::parse(&item.path).map_err(|e| CmdError::new("invalid_path", e.to_string()))?;
    let store = state.store.clone();
    let rel = blocking(move || {
        let date = chrono_like_date(now_ms());
        let mut n = 1;
        loop {
            let suffix = if n == 1 { format!("(recovered {date})") } else { format!("(recovered {date} {n})") };
            let target = original.with_suffix(&suffix).map_err(|e| CmdError::new("invalid_path", e.to_string()))?;
            if std::fs::symlink_metadata(target.to_fs(&root)).is_err() {
                devbridge_fs::apply::commit_copy(
                    &root,
                    &target,
                    &blob,
                    &staging,
                    &item.content_hash,
                    &devbridge_fs::apply::Expect::Absent,
                    None,
                )
                .map_err(|e| CmdError::new("restore_failed", e.to_string()))?;
                let _ = std::fs::remove_dir(&staging);
                let _ = std::fs::remove_dir(root.join(".devbridge"));
                lock(&store)?.audit(
                    "recovery.restored",
                    Some(&ws.to_string()),
                    None,
                    serde_json::json!({ "path": target.as_str() }),
                )?;
                return Ok(target.to_string());
            }
            n += 1;
        }
    })
    .await?;
    state.engine.rescan(ws);
    Ok(rel)
}

#[tauri::command]
pub fn remove_recovery(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    let s = state.store()?;
    if let Some(hash) = s.recovery_remove(id)? {
        state.engine.recovery().remove(&hash).map_err(CmdError::internal)?;
    }
    s.audit("recovery.removed", None, None, serde_json::json!({ "id": id }))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::chrono_like_date(0), "1970-01-01");
        assert_eq!(super::chrono_like_date(1_761_436_800_000), "2025-10-26");
        assert_eq!(super::chrono_like_date(951_782_400_000), "2000-02-29");
    }
}
