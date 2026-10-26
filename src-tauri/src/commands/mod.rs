//! Tauri commands, grouped by area. Each validates input and delegates to a
//! crate; anything that may block runs on a blocking worker.

pub mod conflicts;
pub mod devices;
pub mod diagnostics;
pub mod git;
pub mod remote;
pub mod sync;
pub mod workspaces;

use crate::error::{CmdError, CmdResult};

pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(CmdError::internal)?
}
