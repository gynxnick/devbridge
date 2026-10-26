//! Opening a workspace in external apps. These only *start* the app; the
//! launched process is independent of DevBridge.

use std::path::Path;
use std::process::{Child, Stdio};

use thiserror::Error;

use crate::tools::command_for;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("VS Code wasn't found. Install it, or set the path to its `code` command in Settings → Tools.")]
    VsCodeMissing,
    #[error("Couldn't start {app}: {source}")]
    Spawn { app: &'static str, source: std::io::Error },
    #[error("Opening a terminal isn't supported on this platform yet.")]
    Unsupported,
}

/// Reaps the child in the background so it never lingers as a zombie.
fn detach(mut child: Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

/// Opens `root` in VS Code using its CLI (`code` / `code.cmd`).
pub fn open_in_vscode(code_cli: Option<&Path>, root: &Path) -> Result<(), LaunchError> {
    let code = code_cli.ok_or(LaunchError::VsCodeMissing)?;
    let child = command_for(code)
        .arg(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| LaunchError::Spawn { app: "VS Code", source: e })?;
    detach(child);
    Ok(())
}

/// Opens the platform's default terminal app in `root`.
pub fn open_terminal(root: &Path) -> Result<(), LaunchError> {
    let spawn = |exe: &str, args: &[&std::ffi::OsStr]| {
        command_for(Path::new(exe)).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    };
    let r = root.as_os_str();
    let child = if cfg!(target_os = "macos") {
        spawn("open", &["-a".as_ref(), "Terminal".as_ref(), r])
    } else if cfg!(windows) {
        // Prefer Windows Terminal; fall back to a classic console window.
        spawn("wt.exe", &["-d".as_ref(), r]).or_else(|_| new_console(root))
    } else {
        return Err(LaunchError::Unsupported);
    };
    detach(child.map_err(|e| LaunchError::Spawn { app: "Terminal", source: e })?);
    Ok(())
}

/// Starts `cmd.exe` in its own console window with `root` as working directory.
/// Avoids `cmd /C start …`, whose quoting rules differ from normal arguments.
#[cfg(windows)]
fn new_console(root: &Path) -> std::io::Result<Child> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    std::process::Command::new("cmd.exe").current_dir(root).creation_flags(CREATE_NEW_CONSOLE).spawn()
}

#[cfg(not(windows))]
fn new_console(_root: &Path) -> std::io::Result<Child> {
    Err(std::io::ErrorKind::Unsupported.into())
}
