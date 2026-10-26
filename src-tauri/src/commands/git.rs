//! Git panel commands. Git runs in the repository's top-level folder (status
//! paths are relative to it). Safety checks live in `devbridge-git`.

use std::path::PathBuf;

use devbridge_core::WorkspaceId;
use devbridge_git::{Branch, CatchUp, Commit, Git, Remote, Status};
use serde::Serialize;
use tauri::State;

use super::blocking;
use crate::error::{CmdError, CmdResult};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitOverview {
    pub is_repo: bool,
    pub toplevel: Option<String>,
    pub status: Option<Status>,
    pub log: Vec<Commit>,
    pub branches: Vec<Branch>,
    pub remotes: Vec<Remote>,
    /// `core.autocrlf`, shown because differing values across devices cause
    /// whole-file line-ending changes.
    pub autocrlf: Option<String>,
}

fn git_and_root(state: &AppState, id: WorkspaceId) -> CmdResult<(Git, PathBuf)> {
    let root = state.workspace_root(id)?;
    let git = state
        .git()?
        .ok_or_else(|| CmdError::new("git_unavailable", "Git isn't installed (or its path in Settings is wrong)."))?;
    Ok((git, root))
}

/// Runs `f` in the repository top level for workspace `id`.
async fn in_repo<T: Send + 'static>(
    state: &AppState,
    id: WorkspaceId,
    f: impl FnOnce(&Git, &std::path::Path) -> Result<T, devbridge_git::GitError> + Send + 'static,
) -> CmdResult<T> {
    let (git, root) = git_and_root(state, id)?;
    blocking(move || {
        let info =
            git.discover(&root)?.ok_or_else(|| CmdError::new("not_a_repo", "This project isn't a Git repository."))?;
        Ok(f(&git, &info.toplevel)?)
    })
    .await
}

#[tauri::command]
pub async fn git_overview(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<GitOverview> {
    let (git, root) = git_and_root(&state, id)?;
    blocking(move || {
        let Some(info) = git.discover(&root)? else {
            return Ok(GitOverview {
                is_repo: false,
                toplevel: None,
                status: None,
                log: vec![],
                branches: vec![],
                remotes: vec![],
                autocrlf: None,
            });
        };
        let dir = info.toplevel;
        Ok(GitOverview {
            is_repo: true,
            toplevel: Some(dir.display().to_string()),
            status: Some(git.status(&dir)?),
            log: git.log(&dir, 20)?,
            branches: git.branches(&dir)?,
            remotes: git.remotes(&dir)?,
            autocrlf: git.config_get(&dir, "core.autocrlf"),
        })
    })
    .await
}

fn check_paths(paths: &[String]) -> CmdResult<()> {
    for p in paths {
        if p.is_empty()
            || p.starts_with('-')
            || p.split(['/', '\\']).any(|s| s == "..")
            || std::path::Path::new(p).is_absolute()
        {
            return Err(CmdError::new("invalid_path", format!("“{p}” isn't a path inside the repository.")));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn git_stage(state: State<'_, AppState>, id: WorkspaceId, paths: Vec<String>) -> CmdResult<()> {
    check_paths(&paths)?;
    in_repo(&state, id, move |g, d| g.stage(d, &paths)).await
}

#[tauri::command]
pub async fn git_unstage(state: State<'_, AppState>, id: WorkspaceId, paths: Vec<String>) -> CmdResult<()> {
    check_paths(&paths)?;
    in_repo(&state, id, move |g, d| g.unstage(d, &paths)).await
}

#[tauri::command]
pub async fn git_commit(state: State<'_, AppState>, id: WorkspaceId, message: String) -> CmdResult<String> {
    in_repo(&state, id, move |g, d| g.commit(d, &message)).await
}

#[tauri::command]
pub async fn git_fetch(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<()> {
    in_repo(&state, id, |g, d| g.fetch(d)).await
}

#[tauri::command]
pub async fn git_pull(state: State<'_, AppState>, id: WorkspaceId, confirmed: bool) -> CmdResult<()> {
    in_repo(&state, id, move |g, d| g.pull(d, confirmed)).await
}

#[tauri::command]
pub async fn git_push(state: State<'_, AppState>, id: WorkspaceId, remote: Option<String>) -> CmdResult<()> {
    in_repo(&state, id, move |g, d| g.push(d, remote.as_deref())).await
}

#[tauri::command]
pub async fn git_switch(
    state: State<'_, AppState>,
    id: WorkspaceId,
    branch: String,
    create: bool,
    confirmed: bool,
) -> CmdResult<()> {
    in_repo(&state, id, move |g, d| g.switch(d, &branch, create, confirmed)).await
}

#[tauri::command]
pub async fn git_remote_add(state: State<'_, AppState>, id: WorkspaceId, name: String, url: String) -> CmdResult<()> {
    in_repo(&state, id, move |g, d| g.remote_add(d, &name, &url)).await
}

#[tauri::command]
pub async fn git_remote_set_url(
    state: State<'_, AppState>,
    id: WorkspaceId,
    name: String,
    url: String,
) -> CmdResult<()> {
    in_repo(&state, id, move |g, d| g.remote_set_url(d, &name, &url)).await
}

#[tauri::command]
pub async fn git_remote_remove(state: State<'_, AppState>, id: WorkspaceId, name: String) -> CmdResult<()> {
    in_repo(&state, id, move |g, d| g.remote_remove(d, &name)).await
}

#[tauri::command]
pub async fn git_catch_up(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<CatchUp> {
    in_repo(&state, id, |g, d| g.catch_up(d)).await
}

#[tauri::command]
pub async fn git_init(state: State<'_, AppState>, id: WorkspaceId) -> CmdResult<()> {
    let (git, root) = git_and_root(&state, id)?;
    blocking(move || {
        git.init(&root)?;
        Ok(())
    })
    .await
}
