//! Detection of the developer tools DevBridge works alongside.
//!
//! Detection only *locates* tools and asks for their version. It never reads
//! Claude Code configuration, credentials or session data.
//!
//! GUI apps on macOS don't inherit the login shell's PATH, so besides PATH we
//! check each tool's standard install locations.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSource {
    Custom,
    Path,
    KnownLocation,
    NotFound,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub name: &'static str,
    pub found: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub source: ToolSource,
    /// Actionable guidance when something is missing or broken.
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolReport {
    pub vscode: ToolStatus,
    pub git: ToolStatus,
    pub claude_cli: ToolStatus,
    pub claude_extension: ToolStatus,
}

const VERSION_TIMEOUT: Duration = Duration::from_secs(10);

pub fn detect_all(vscode_custom: Option<&Path>, git_custom: Option<&Path>) -> ToolReport {
    ToolReport {
        vscode: detect_vscode(vscode_custom),
        git: detect_git(git_custom),
        claude_cli: detect_claude_cli(),
        claude_extension: detect_claude_extension(dirs::home_dir().as_deref()),
    }
}

/// Locates an executable: custom path, then PATH names, then known locations.
fn locate(custom: Option<&Path>, path_names: &[&str], known: &[PathBuf]) -> Option<(PathBuf, ToolSource)> {
    if let Some(c) = custom {
        return c.is_file().then(|| (c.to_path_buf(), ToolSource::Custom));
    }
    for n in path_names {
        if let Ok(p) = which::which(n) {
            return Some((p, ToolSource::Path));
        }
    }
    known.iter().find(|p| p.is_file()).map(|p| (p.clone(), ToolSource::KnownLocation))
}

fn status(
    name: &'static str,
    found: Option<(PathBuf, ToolSource)>,
    version_args: &[&str],
    parse: fn(&str) -> Option<String>,
    missing_hint: &str,
    custom_given: bool,
) -> ToolStatus {
    match found {
        None => ToolStatus {
            name,
            found: false,
            path: None,
            version: None,
            source: ToolSource::NotFound,
            hint: Some(if custom_given {
                "The custom path in Settings doesn't point to a file. Fix it or clear it to auto-detect.".into()
            } else {
                missing_hint.into()
            }),
        },
        Some((path, source)) => {
            let out = run_with_timeout(&path, version_args, VERSION_TIMEOUT);
            let version = out.as_deref().and_then(parse);
            let hint = version.is_none().then(|| {
                format!("Found {} but it didn't report a version; it may be damaged or blocked.", path.display())
            });
            ToolStatus { name, found: true, path: Some(path.display().to_string()), version, source, hint }
        }
    }
}

pub fn detect_vscode(custom: Option<&Path>) -> ToolStatus {
    let found = locate(custom, &["code"], &vscode_locations());
    status(
        "Visual Studio Code",
        found,
        &["--version"],
        first_line,
        if cfg!(target_os = "macos") {
            "Install VS Code from code.visualstudio.com and move it to Applications, or set the path to its `code` command in Settings → Tools."
        } else {
            "Install VS Code from code.visualstudio.com, or set the path to `code.cmd` in Settings → Tools."
        },
        custom.is_some(),
    )
}

pub fn detect_git(custom: Option<&Path>) -> ToolStatus {
    let found = devbridge_git::Git::locate(custom).ok().map(|g| {
        let src = if custom.is_some() {
            ToolSource::Custom
        } else if which::which("git").is_ok() {
            ToolSource::Path
        } else {
            ToolSource::KnownLocation
        };
        (g.exe().to_path_buf(), src)
    });
    status(
        "Git",
        found,
        &["--version"],
        |s| s.strip_prefix("git version ").map(|v| v.split_whitespace().next().unwrap_or(v).to_owned()),
        if cfg!(target_os = "macos") {
            "Install Git with `xcode-select --install` or from git-scm.com."
        } else {
            "Install Git for Windows from git-scm.com."
        },
        custom.is_some(),
    )
}

pub fn detect_claude_cli() -> ToolStatus {
    let found = locate(None, &["claude"], &claude_locations());
    status(
        "Claude Code CLI",
        found,
        &["--version"],
        |s| first_line(s).map(|l| l.split_whitespace().next().unwrap_or(&l).to_owned()),
        "Optional. The VS Code extension works on its own; to use Claude Code in a terminal, install it from docs.anthropic.com/claude-code.",
        false,
    )
}

/// Finds the newest installed Claude Code VS Code extension.
pub fn detect_claude_extension(home: Option<&Path>) -> ToolStatus {
    let mut best: Option<(Vec<u64>, String, PathBuf)> = None;
    if let Some(home) = home {
        for dir in [".vscode/extensions", ".vscode-insiders/extensions"] {
            let Ok(entries) = std::fs::read_dir(home.join(dir)) else { continue };
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Some(ver) = parse_extension_version(&name) {
                    let key: Vec<u64> = ver.split('.').map(|p| p.parse().unwrap_or(0)).collect();
                    if best.as_ref().is_none_or(|(k, ..)| key > *k) {
                        best = Some((key, ver, e.path()));
                    }
                }
            }
        }
    }
    match best {
        Some((_, version, path)) => ToolStatus {
            name: "Claude Code extension",
            found: true,
            path: Some(path.display().to_string()),
            version: Some(version),
            source: ToolSource::KnownLocation,
            hint: None,
        },
        None => ToolStatus {
            name: "Claude Code extension",
            found: false,
            path: None,
            version: None,
            source: ToolSource::NotFound,
            hint: Some("Install \"Claude Code\" (publisher: Anthropic) from the VS Code Extensions view. Sign in from VS Code; DevBridge never handles your Claude credentials.".into()),
        },
    }
}

/// `anthropic.claude-code-2.1.310-darwin-arm64` → `2.1.310`.
fn parse_extension_version(dir_name: &str) -> Option<String> {
    let rest = dir_name.strip_prefix("anthropic.claude-code-")?;
    let ver = rest.split('-').next()?;
    (!ver.is_empty() && ver.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
        .then(|| ver.to_owned())
}

fn first_line(s: &str) -> Option<String> {
    s.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_owned)
}

/// Runs `exe args…`, returning stdout if it exits successfully within `timeout`.
pub(crate) fn run_with_timeout(exe: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut cmd = command_for(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                if !st.success() {
                    return None;
                }
                let mut out = String::new();
                std::io::Read::read_to_string(child.stdout.as_mut()?, &mut out).ok()?;
                return Some(out);
            }
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// A `Command` for `exe` that never flashes a console window on Windows.
pub(crate) fn command_for(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

fn vscode_locations() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        let bundles = ["Visual Studio Code.app", "Visual Studio Code - Insiders.app"];
        let mut bases = vec![PathBuf::from("/Applications")];
        if let Some(h) = dirs::home_dir() {
            bases.push(h.join("Applications"));
        }
        for base in &bases {
            for (b, bin) in bundles.iter().zip(["code", "code-insiders"]) {
                v.push(base.join(b).join("Contents/Resources/app/bin").join(bin));
            }
        }
        v.push("/usr/local/bin/code".into());
        v.push("/opt/homebrew/bin/code".into());
    } else if cfg!(windows) {
        for (var, sub) in [
            ("LOCALAPPDATA", r"Programs\Microsoft VS Code\bin\code.cmd"),
            ("ProgramFiles", r"Microsoft VS Code\bin\code.cmd"),
            ("LOCALAPPDATA", r"Programs\Microsoft VS Code Insiders\bin\code-insiders.cmd"),
        ] {
            if let Some(base) = std::env::var_os(var) {
                v.push(PathBuf::from(base).join(sub));
            }
        }
    } else {
        v.extend(["/usr/bin/code", "/snap/bin/code"].map(PathBuf::from));
    }
    v
}

fn claude_locations() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = dirs::home_dir() {
        if cfg!(windows) {
            v.push(h.join(r".local\bin\claude.exe"));
            if let Some(appdata) = std::env::var_os("APPDATA") {
                v.push(PathBuf::from(appdata).join(r"npm\claude.cmd"));
            }
        } else {
            v.push(h.join(".local/bin/claude"));
            v.push(h.join(".claude/local/claude"));
            v.push(h.join(".npm-global/bin/claude"));
        }
    }
    if !cfg!(windows) {
        v.extend(["/opt/homebrew/bin/claude", "/usr/local/bin/claude"].map(PathBuf::from));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_extension_dir_names() {
        assert_eq!(parse_extension_version("anthropic.claude-code-2.1.310-darwin-arm64").as_deref(), Some("2.1.310"));
        assert_eq!(parse_extension_version("anthropic.claude-code-1.0.0").as_deref(), Some("1.0.0"));
        assert_eq!(parse_extension_version("anthropic.claude-code-latest"), None);
        assert_eq!(parse_extension_version("ms-python.python-2024.1.0"), None);
    }

    #[test]
    fn picks_newest_extension_numerically() {
        let home = tempfile::tempdir().unwrap();
        let ext = home.path().join(".vscode/extensions");
        for d in
            ["anthropic.claude-code-2.1.9-darwin-arm64", "anthropic.claude-code-2.1.10-darwin-arm64", "other.ext-9.9.9"]
        {
            std::fs::create_dir_all(ext.join(d)).unwrap();
        }
        let s = detect_claude_extension(Some(home.path()));
        assert!(s.found);
        assert_eq!(s.version.as_deref(), Some("2.1.10"));
    }

    #[test]
    fn missing_extension_has_hint() {
        let home = tempfile::tempdir().unwrap();
        let s = detect_claude_extension(Some(home.path()));
        assert!(!s.found && s.hint.is_some());
    }

    #[test]
    fn bad_custom_path_reports_custom_hint() {
        let s = detect_vscode(Some(Path::new("/no/such/code")));
        assert!(!s.found);
        assert!(s.hint.unwrap().contains("custom path"));
    }

    #[test]
    fn detects_git_with_version() {
        let s = detect_git(None);
        assert!(s.found, "git is required for the test suite");
        assert!(s.version.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn version_probe_times_out() {
        let t = Instant::now();
        assert_eq!(run_with_timeout(Path::new("/bin/sleep"), &["5"], Duration::from_millis(200)), None);
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
