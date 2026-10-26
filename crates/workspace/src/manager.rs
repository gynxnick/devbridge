//! Import and inspection of workspaces. Orchestrates root validation, Git
//! detection and persistence; holds no UI or Tauri dependencies.

use std::path::{Path, PathBuf};

use devbridge_core::time::now_ms;
use devbridge_core::{
    DeviceId, NameError, SyncState, WORKSPACE_NAME_MAX_CHARS, Workspace, WorkspaceColor, WorkspaceIcon, WorkspaceId,
    WorkspaceMode, validate_display_name,
};
use devbridge_git::{Git, GitError, RepoInfo};
use devbridge_store::{GitRepoRecord, Store, StoreError};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::roots::{Protected, Registered, RootError, validate_root, warnings_for};

#[derive(Debug, Error)]
pub enum ManagerError {
    #[error(transparent)]
    Root(#[from] RootError),
    #[error(transparent)]
    Name(#[from] NameError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(
        "Git isn't available, so a repository can't be initialized. Install Git or set its path in Settings → Tools."
    )]
    GitUnavailable,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRequest {
    pub path: String,
    pub name: Option<String>,
    pub color: Option<WorkspaceColor>,
    pub icon: Option<WorkspaceIcon>,
    /// Run `git init` if the folder isn't inside a repository.
    #[serde(default)]
    pub init_git: bool,
}

/// Result of checking a folder before import; shown in the import dialog.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathInspection {
    pub canonical_path: Option<String>,
    pub suggested_name: String,
    pub suggested_color: WorkspaceColor,
    pub git: Option<RepoInfo>,
    pub git_available: bool,
    pub warnings: Vec<String>,
    /// Set when the folder can't be imported; the message is user-facing.
    pub error: Option<String>,
}

fn registered(store: &Store) -> Result<Vec<(String, PathBuf)>, StoreError> {
    Ok(store.list_workspaces()?.into_iter().map(|w| (w.name, PathBuf::from(w.root_path))).collect())
}

fn check_root(store: &Store, raw: &Path, protected: &Protected) -> Result<PathBuf, ManagerError> {
    let existing = registered(store)?;
    let regs: Vec<Registered<'_>> =
        existing.iter().map(|(name, root)| Registered { name, root: root.as_path() }).collect();
    Ok(validate_root(raw, &regs, protected)?)
}

fn folder_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Project".into())
}

/// Deterministic default color so a project looks the same on every device.
pub fn color_for(name: &str) -> WorkspaceColor {
    let h = name.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    WorkspaceColor::ALL[h as usize % WorkspaceColor::ALL.len()]
}

fn git_warnings(git: Option<&RepoInfo>, git_available: bool) -> Vec<String> {
    let mut w = Vec::new();
    match git {
        Some(info) if !info.is_toplevel => w.push(format!(
            "This folder is inside the Git repository at {}. Git actions will apply to that whole repository.",
            info.toplevel.display()
        )),
        None if git_available => w.push(
            "This folder isn't a Git repository. DevBridge syncs files between devices; Git remotes are still the recommended way to keep history.".into(),
        ),
        None => w.push("Git wasn't found, so Git status won't be shown for this project.".into()),
        _ => {}
    }
    w
}

pub fn inspect_path(store: &Store, git: Option<&Git>, raw: &Path, protected: &Protected) -> PathInspection {
    let name = folder_name(raw);
    let mut out = PathInspection {
        canonical_path: None,
        suggested_color: color_for(&name),
        suggested_name: name,
        git: None,
        git_available: git.is_some(),
        warnings: Vec::new(),
        error: None,
    };
    let root = match check_root(store, raw, protected) {
        Ok(r) => r,
        Err(e) => {
            out.error = Some(e.to_string());
            return out;
        }
    };
    out.git = git.and_then(|g| g.discover(&root).ok().flatten());
    out.warnings = warnings_for(&root);
    out.warnings.extend(git_warnings(out.git.as_ref(), out.git_available));
    out.canonical_path = Some(root.display().to_string());
    out
}

/// Validates and registers a workspace. Never modifies the folder, except for
/// `git init` when explicitly requested.
pub fn import_workspace(
    store: &Store,
    git: Option<&Git>,
    local_device: DeviceId,
    req: &ImportRequest,
    protected: &Protected,
) -> Result<Workspace, ManagerError> {
    let root = check_root(store, Path::new(&req.path), protected)?;
    let name = validate_display_name(req.name.as_deref().unwrap_or(&folder_name(&root)), WORKSPACE_NAME_MAX_CHARS)?;

    let mut repo = git.map(|g| g.discover(&root)).transpose()?.flatten();
    if repo.is_none() && req.init_git {
        repo = Some(git.ok_or(ManagerError::GitUnavailable)?.init(&root)?);
    }

    let now = now_ms();
    let ws = Workspace {
        id: WorkspaceId::new(),
        color: req.color.unwrap_or_else(|| color_for(&name)),
        icon: req.icon.unwrap_or(if repo.is_some() { WorkspaceIcon::Code } else { WorkspaceIcon::Folder }),
        name,
        root_path: root.display().to_string(),
        mode: WorkspaceMode::Synced,
        sync_state: SyncState::LocalOnly,
        active_device_id: Some(local_device),
        last_synced_at: None,
        created_at: now,
        updated_at: now,
    };
    store.insert_workspace(&ws)?;
    if let Some(r) = &repo {
        store.upsert_git_repo(
            ws.id,
            &GitRepoRecord {
                toplevel: r.toplevel.display().to_string(),
                last_branch: r.branch.clone(),
                last_head: r.head.clone(),
                last_checked_at: Some(now),
            },
        )?;
    }
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;
    use devbridge_core::OsKind;

    struct Fx {
        _t: tempfile::TempDir,
        dir: PathBuf,
        store: Store,
        dev: DeviceId,
        git: Git,
    }

    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("My Project");
        std::fs::create_dir(&dir).unwrap();
        let store = Store::open_in_memory().unwrap();
        let dev = store.ensure_local_device("Mac", OsKind::MacOs, None).unwrap().id;
        Fx { _t: t, dir, store, dev, git: Git::locate(None).unwrap() }
    }

    fn req(path: &Path) -> ImportRequest {
        ImportRequest { path: path.display().to_string(), name: None, color: None, icon: None, init_git: false }
    }

    #[test]
    fn imports_plain_folder_without_touching_it() {
        let f = fx();
        let ws = import_workspace(&f.store, Some(&f.git), f.dev, &req(&f.dir), &Protected::default()).unwrap();
        assert_eq!(ws.name, "My Project");
        assert_eq!(ws.icon, WorkspaceIcon::Folder);
        assert_eq!(ws.sync_state, SyncState::LocalOnly);
        assert_eq!(std::fs::read_dir(&f.dir).unwrap().count(), 0, "import must not create files");
        assert!(f.store.get_git_repo(ws.id).unwrap().is_none());
    }

    #[test]
    fn init_git_on_request_and_records_repo() {
        let f = fx();
        let r = ImportRequest { init_git: true, name: Some("  Demo ".into()), ..req(&f.dir) };
        let ws = import_workspace(&f.store, Some(&f.git), f.dev, &r, &Protected::default()).unwrap();
        assert_eq!(ws.name, "Demo");
        assert!(f.dir.join(".git").is_dir());
        assert!(f.store.get_git_repo(ws.id).unwrap().is_some());
    }

    #[test]
    fn init_git_without_git_is_actionable_error() {
        let f = fx();
        let r = ImportRequest { init_git: true, ..req(&f.dir) };
        assert!(matches!(
            import_workspace(&f.store, None, f.dev, &r, &Protected::default()),
            Err(ManagerError::GitUnavailable)
        ));
        assert!(f.store.list_workspaces().unwrap().is_empty(), "nothing registered on failure");
    }

    #[test]
    fn inspect_reports_errors_and_warnings() {
        let f = fx();
        let i = inspect_path(&f.store, Some(&f.git), &f.dir, &Protected::default());
        assert!(i.error.is_none());
        assert_eq!(i.suggested_name, "My Project");
        assert!(i.warnings.iter().any(|w| w.contains("isn't a Git repository")));

        import_workspace(&f.store, Some(&f.git), f.dev, &req(&f.dir), &Protected::default()).unwrap();
        let again = inspect_path(&f.store, Some(&f.git), &f.dir, &Protected::default());
        assert!(again.error.unwrap().contains("already registered"));
    }

    #[test]
    fn rejects_invalid_name() {
        let f = fx();
        let r = ImportRequest { name: Some("   ".into()), ..req(&f.dir) };
        assert!(matches!(
            import_workspace(&f.store, Some(&f.git), f.dev, &r, &Protected::default()),
            Err(ManagerError::Name(NameError::Empty))
        ));
    }

    #[test]
    fn color_is_deterministic() {
        assert_eq!(color_for("devbridge"), color_for("devbridge"));
    }
}
