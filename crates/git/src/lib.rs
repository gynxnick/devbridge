//! Git adapter. Shells out to the Git CLI so the user's own configuration,
//! hooks and credential helpers behave exactly as they do in a terminal.
//!
//! Safety rules (enforced here, not in the UI):
//! - commands run without a shell and with `GIT_TERMINAL_PROMPT=0`, so they can
//!   never block on an interactive prompt; network commands have a timeout;
//! - nothing here resets, force-pushes, cleans or discards changes;
//! - operations that could touch uncommitted work (pull, switch) are refused
//!   with [`GitError::NeedsConfirmation`] unless the caller confirms;
//! - all Git output is passed through credential redaction.

mod status;

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use devbridge_core::fsutil::{canonicalize, path_key};
use devbridge_core::redact::redact;
use serde::Serialize;
use thiserror::Error;

pub use status::{EntryKind, Status, StatusEntry};

#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git was not found. Install Git or set its path in Settings → Tools.")]
    NotFound,
    #[error("Git at {path} could not be run: {source}")]
    Spawn { path: String, source: std::io::Error },
    #[error("git {args} failed: {stderr}")]
    Failed { args: String, stderr: String },
    #[error("git {args} timed out after {secs} s. Check your network connection and credentials.")]
    Timeout { args: String, secs: u64 },
    #[error("This may affect uncommitted work: {}", .warnings.join(" "))]
    NeedsConfirmation { warnings: Vec<String> },
    #[error("{0}")]
    Refused(String),
}

pub type Result<T> = std::result::Result<T, GitError>;

const LOCAL_TIMEOUT: Duration = Duration::from_secs(30);
const NETWORK_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone)]
pub struct Git {
    exe: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoInfo {
    pub toplevel: PathBuf,
    pub is_toplevel: bool,
    pub branch: Option<String>,
    pub head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    pub short: String,
    pub author: String,
    pub time: i64,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Branch {
    pub name: String,
    pub upstream: Option<String>,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remote {
    pub name: String,
    /// Fetch URL with any embedded credentials redacted.
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatchUp {
    pub from: String,
    pub to: String,
    pub files: usize,
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

impl Git {
    pub fn locate(custom: Option<&Path>) -> Result<Self> {
        if let Some(p) = custom {
            return if p.is_file() { Ok(Self { exe: p.to_path_buf() }) } else { Err(GitError::NotFound) };
        }
        if let Ok(p) = which::which("git") {
            return Ok(Self { exe: p });
        }
        default_locations().into_iter().find(|p| p.is_file()).map(|exe| Self { exe }).ok_or(GitError::NotFound)
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    fn command(&self, cwd: &Path) -> Command {
        let mut c = Command::new(&self.exe);
        c.current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .env("GIT_EDITOR", "true")
            .stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            c.creation_flags(CREATE_NO_WINDOW);
        }
        c
    }

    /// Runs git with a timeout, capturing output on threads (no pipe deadlock).
    fn exec(&self, cwd: &Path, args: &[&str], stdin: Option<&str>, timeout: Duration) -> Result<Out> {
        let mut cmd = self.command(cwd);
        cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if stdin.is_some() {
            cmd.stdin(Stdio::piped());
        }
        let mut child = cmd.spawn().map_err(|e| GitError::Spawn { path: self.exe.display().to_string(), source: e })?;
        if let (Some(input), Some(mut w)) = (stdin, child.stdin.take()) {
            let input = input.to_owned();
            std::thread::spawn(move || {
                let _ = w.write_all(input.as_bytes());
            });
        }
        let mut so = child.stdout.take().expect("piped");
        let mut se = child.stderr.take().expect("piped");
        let t_out = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = so.read_to_end(&mut b);
            b
        });
        let t_err = std::thread::spawn(move || {
            let mut b = Vec::new();
            let _ = se.read_to_end(&mut b);
            b
        });
        let start = Instant::now();
        let status = loop {
            match child.try_wait() {
                Ok(Some(s)) => break s,
                Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(GitError::Timeout { args: args.join(" "), secs: timeout.as_secs() });
                }
            }
        };
        let stdout = String::from_utf8_lossy(&t_out.join().unwrap_or_default()).into_owned();
        let stderr = redact(String::from_utf8_lossy(&t_err.join().unwrap_or_default()).trim());
        Ok(Out { ok: status.success(), stdout, stderr })
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        self.run_t(cwd, args, LOCAL_TIMEOUT)
    }

    fn run_t(&self, cwd: &Path, args: &[&str], timeout: Duration) -> Result<String> {
        let out = self.exec(cwd, args, None, timeout)?;
        if !out.ok {
            return Err(GitError::Failed { args: args.join(" "), stderr: out.stderr });
        }
        Ok(out.stdout)
    }

    pub fn version(&self) -> Result<String> {
        let out = self.run(Path::new("."), &["--version"])?;
        let out = out.trim();
        Ok(parse_version(out).unwrap_or_else(|| out.to_owned()))
    }

    pub fn discover(&self, dir: &Path) -> Result<Option<RepoInfo>> {
        let out = self.exec(dir, &["rev-parse", "--show-toplevel"], None, LOCAL_TIMEOUT)?;
        if !out.ok {
            return Ok(None);
        }
        let toplevel = PathBuf::from(out.stdout.trim());
        let is_toplevel = same_path(&toplevel, dir);
        let trimmed = |r: Result<String>| r.ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        let branch = trimmed(self.run(dir, &["symbolic-ref", "--short", "-q", "HEAD"]));
        let head = trimmed(self.run(dir, &["rev-parse", "--verify", "-q", "HEAD"]));
        Ok(Some(RepoInfo { toplevel, is_toplevel, branch, head }))
    }

    pub fn init(&self, dir: &Path) -> Result<RepoInfo> {
        if let Some(existing) = self.discover(dir)? {
            return Err(GitError::Refused(format!("Already inside a repository at {}", existing.toplevel.display())));
        }
        self.run(dir, &["init"])?;
        self.discover(dir)?.ok_or_else(|| GitError::Refused("Repository not found after init".into()))
    }

    pub fn status(&self, dir: &Path) -> Result<Status> {
        let out = self.run(dir, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
        Ok(status::parse(&out))
    }

    pub fn log(&self, dir: &Path, n: u32) -> Result<Vec<Commit>> {
        let fmt = "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s%x1e";
        let out = self.exec(dir, &["log", &format!("-n{n}"), fmt], None, LOCAL_TIMEOUT)?;
        if !out.ok {
            // A repository without commits has no log; that's not an error.
            return if out.stderr.contains("does not have any commits") {
                Ok(vec![])
            } else {
                Err(GitError::Failed { args: "log".into(), stderr: out.stderr })
            };
        }
        Ok(out
            .stdout
            .split('\x1e')
            .filter_map(|rec| {
                let f: Vec<&str> = rec.trim_start_matches('\n').split('\x1f').collect();
                (f.len() == 5).then(|| Commit {
                    hash: f[0].into(),
                    short: f[1].into(),
                    author: f[2].into(),
                    time: f[3].parse().unwrap_or(0),
                    subject: f[4].into(),
                })
            })
            .collect())
    }

    pub fn branches(&self, dir: &Path) -> Result<Vec<Branch>> {
        let out =
            self.run(dir, &["for-each-ref", "--format=%(HEAD)%1f%(refname:short)%1f%(upstream:short)", "refs/heads"])?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let f: Vec<&str> = l.split('\x1f').collect();
                (f.len() == 3).then(|| Branch {
                    current: f[0] == "*",
                    name: f[1].into(),
                    upstream: Some(f[2].to_owned()).filter(|s| !s.is_empty()),
                })
            })
            .collect())
    }

    pub fn remotes(&self, dir: &Path) -> Result<Vec<Remote>> {
        let out = self.run(dir, &["remote", "-v"])?;
        let mut v: Vec<Remote> = Vec::new();
        for l in out.lines().filter(|l| l.ends_with("(fetch)")) {
            let mut it = l.split_whitespace();
            if let (Some(name), Some(url)) = (it.next(), it.next()) {
                v.push(Remote { name: name.into(), url: redact(url) });
            }
        }
        Ok(v)
    }

    pub fn remote_add(&self, dir: &Path, name: &str, url: &str) -> Result<()> {
        validate_ref_like(name)?;
        validate_url(url)?;
        self.run(dir, &["remote", "add", "--", name, url]).map(|_| ())
    }

    pub fn remote_set_url(&self, dir: &Path, name: &str, url: &str) -> Result<()> {
        validate_ref_like(name)?;
        validate_url(url)?;
        self.run(dir, &["remote", "set-url", "--", name, url]).map(|_| ())
    }

    /// Removing a remote deletes its remote-tracking branches (not your
    /// commits); the UI asks for confirmation first.
    pub fn remote_remove(&self, dir: &Path, name: &str) -> Result<()> {
        validate_ref_like(name)?;
        self.run(dir, &["remote", "remove", "--", name]).map(|_| ())
    }

    pub fn fetch(&self, dir: &Path) -> Result<()> {
        self.run_t(dir, &["fetch", "--all", "--prune", "--no-recurse-submodules"], NETWORK_TIMEOUT).map(|_| ())
    }

    /// Fast-forward-only pull. Refuses (needs confirmation) on a dirty tree;
    /// Git itself also refuses to overwrite local changes.
    pub fn pull(&self, dir: &Path, confirmed: bool) -> Result<()> {
        let st = self.status(dir)?;
        if st.upstream.is_none() {
            return Err(GitError::Refused(
                "This branch has no upstream. Push it first or set one up with your Git tools.".into(),
            ));
        }
        if !confirmed && !st.is_clean() {
            return Err(GitError::NeedsConfirmation {
                warnings: vec![format!(
                    "{} file(s) have uncommitted changes. Git will stop without changing them if the pull would overwrite them.",
                    st.entries.len()
                )],
            });
        }
        self.run_t(dir, &["pull", "--ff-only", "--no-rebase"], NETWORK_TIMEOUT).map(|_| ())
    }

    /// Pushes the current branch. Never forces. Sets the upstream on first push.
    pub fn push(&self, dir: &Path, remote: Option<&str>) -> Result<()> {
        let st = self.status(dir)?;
        let branch = st.branch.clone().ok_or_else(|| GitError::Refused("Can't push from a detached HEAD.".into()))?;
        if st.upstream.is_some() {
            return self.run_t(dir, &["push"], NETWORK_TIMEOUT).map(|_| ());
        }
        let remote =
            match remote {
                Some(r) => r.to_owned(),
                None => self.remotes(dir)?.first().map(|r| r.name.clone()).ok_or_else(|| {
                    GitError::Refused("No remote is configured. Add one in the Git panel first.".into())
                })?,
            };
        validate_ref_like(&remote)?;
        self.run_t(dir, &["push", "--set-upstream", "--", &remote, &branch], NETWORK_TIMEOUT).map(|_| ())
    }

    pub fn stage(&self, dir: &Path, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut args = vec!["add", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.run(dir, &args).map(|_| ())
    }

    /// Removes paths from the index only; the working tree is untouched.
    pub fn unstage(&self, dir: &Path, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let has_head = self.run(dir, &["rev-parse", "--verify", "-q", "HEAD"]).is_ok();
        let mut args: Vec<&str> =
            if has_head { vec!["restore", "--staged", "--"] } else { vec!["rm", "--cached", "-q", "-r", "--"] };
        args.extend(paths.iter().map(String::as_str));
        self.run(dir, &args).map(|_| ())
    }

    pub fn commit(&self, dir: &Path, message: &str) -> Result<String> {
        if message.trim().is_empty() {
            return Err(GitError::Refused("Write a commit message first.".into()));
        }
        let out = self.exec(dir, &["commit", "-F", "-"], Some(message), LOCAL_TIMEOUT)?;
        if !out.ok {
            let mut stderr = out.stderr;
            if stderr.contains("Please tell me who you are") || stderr.contains("user.email") {
                stderr = "Git doesn't know your name and email yet. Run `git config --global user.name \"Your Name\"` and `git config --global user.email you@example.com`.".into();
            } else if stderr.is_empty() {
                stderr = out.stdout.lines().last().unwrap_or("nothing to commit").to_owned();
            }
            return Err(GitError::Failed { args: "commit".into(), stderr });
        }
        Ok(self.run(dir, &["rev-parse", "HEAD"])?.trim().to_owned())
    }

    /// Switches (optionally creating) a branch. Refuses on a dirty tree unless
    /// confirmed; Git itself still refuses if the switch would overwrite work.
    pub fn switch(&self, dir: &Path, branch: &str, create: bool, confirmed: bool) -> Result<()> {
        validate_ref_like(branch)?;
        let st = self.status(dir)?;
        if !confirmed && !st.is_clean() {
            return Err(GitError::NeedsConfirmation {
                warnings: vec![format!(
                    "{} file(s) have uncommitted changes. They stay in your working tree; Git stops if switching would overwrite any of them.",
                    st.entries.len()
                )],
            });
        }
        let args: Vec<&str> = if create { vec!["switch", "-c", branch] } else { vec!["switch", branch] };
        self.run(dir, &args).map(|_| ())
    }

    /// After DevBridge synced another device's work into this checkout, Git
    /// sees those files as "modified" while the commit that contains them is
    /// on the remote. This fast-forwards HEAD to the upstream **only if** every
    /// file that differs between HEAD and upstream already matches upstream
    /// exactly, and then moves only the branch and the index: no file in the
    /// working tree is ever written (a merge would rewrite files, e.g. changing
    /// line endings under `core.autocrlf`, which DevBridge would sync back).
    pub fn catch_up(&self, dir: &Path) -> Result<CatchUp> {
        let st = self.status(dir)?;
        if st.upstream.is_none() {
            return Err(GitError::Refused("This branch has no upstream.".into()));
        }
        let head = st.head.clone().ok_or_else(|| GitError::Refused("This repository has no commits yet.".into()))?;
        let target = self.run(dir, &["rev-parse", "--verify", "@{u}"])?.trim().to_owned();
        if target == head {
            return Err(GitError::Refused("Already up to date with the upstream.".into()));
        }
        if !self.exec(dir, &["merge-base", "--is-ancestor", "HEAD", "@{u}"], None, LOCAL_TIMEOUT)?.ok {
            return Err(GitError::Refused(
                "Your branch and its upstream have diverged. Resolve this with your usual Git tools (DevBridge never rewrites history).".into(),
            ));
        }
        let changed: Vec<String> = self
            .run(dir, &["diff", "--name-only", "-z", "HEAD", &target])?
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        // Every dirty path must be one the upstream commit changes, and every
        // changed path must already be in its upstream state.
        let dirty: Vec<&StatusEntry> = st.entries.iter().filter(|e| e.kind != EntryKind::Ignored).collect();
        if let Some(e) = dirty.iter().find(|e| !changed.contains(&e.path)) {
            return Err(GitError::Refused(format!(
                "{} has changes that aren't part of the upstream commit. Commit or stash it first.",
                e.path
            )));
        }
        for p in &changed {
            if !self.worktree_matches(dir, &target, p)? {
                return Err(GitError::Refused(format!(
                    "{p} doesn't match the upstream version yet (is sync finished?)."
                )));
            }
        }
        // Fast-forward the branch (only if it still points at `head`), then
        // make the index match the new commit. The working tree already holds
        // the upstream content, so it's left exactly as it is.
        let msg = format!("devbridge: catch up to {}", &target[..target.len().min(12)]);
        self.run(dir, &["update-ref", "-m", &msg, "HEAD", &target, &head])?;
        if let Err(e) = self.run(dir, &["read-tree", &target]) {
            let _ = self.run(dir, &["update-ref", "-m", "devbridge: undo catch up", "HEAD", &head, &target]);
            return Err(e);
        }
        // Refresh cached file stats so the next `git status` is fast and clean.
        let _ = self.run(dir, &["update-index", "-q", "--refresh"]);
        Ok(CatchUp { from: head, to: target, files: changed.len() })
    }

    /// Whether the working-tree file at `path` equals `rev:path` (or both are absent).
    fn worktree_matches(&self, dir: &Path, rev: &str, path: &str) -> Result<bool> {
        let on_disk = dir.join(path);
        let in_rev = self.exec(dir, &["rev-parse", "--verify", "-q", &format!("{rev}:{path}")], None, LOCAL_TIMEOUT)?;
        match (on_disk.is_file(), in_rev.ok) {
            (false, false) => Ok(true),
            (true, true) => Ok(self.run(dir, &["hash-object", "--", path])?.trim() == in_rev.stdout.trim()),
            _ => Ok(false),
        }
    }

    pub fn config_get(&self, dir: &Path, key: &str) -> Option<String> {
        self.run(dir, &["config", "--get", key]).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
    }
}

fn validate_ref_like(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && !s.starts_with('-')
        && !s.contains("..")
        && !s.ends_with(".lock")
        && s.chars().all(|c| !c.is_control() && !matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'));
    if ok { Ok(()) } else { Err(GitError::Refused(format!("“{s}” isn't a valid name."))) }
}

fn validate_url(s: &str) -> Result<()> {
    if s.trim().is_empty() || s.starts_with('-') || s.chars().any(char::is_control) {
        return Err(GitError::Refused("That remote URL isn't valid.".into()));
    }
    // `ext::` transports run arbitrary commands; never allow them.
    if s.to_ascii_lowercase().starts_with("ext::") {
        return Err(GitError::Refused("ext:: remote URLs are not allowed.".into()));
    }
    Ok(())
}

fn parse_version(s: &str) -> Option<String> {
    s.strip_prefix("git version ")?.split_whitespace().next().map(str::to_owned)
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (canonicalize(a), canonicalize(b)) {
        (Ok(a), Ok(b)) => path_key(&a) == path_key(&b),
        _ => path_key(a) == path_key(b),
    }
}

fn default_locations() -> Vec<PathBuf> {
    if cfg!(windows) {
        let mut v = Vec::new();
        for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(var) {
                let base = PathBuf::from(base);
                v.push(base.join("Git").join("cmd").join("git.exe"));
                v.push(base.join("Programs").join("Git").join("cmd").join("git.exe"));
            }
        }
        v
    } else {
        ["/usr/bin/git", "/opt/homebrew/bin/git", "/usr/local/bin/git"].iter().map(PathBuf::from).collect()
    }
}

#[cfg(test)]
mod tests;
