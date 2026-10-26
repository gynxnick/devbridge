//! Remote Workspace Mode: one computer (the host) keeps the project and runs
//! VS Code Server, Claude Code and builds; the other (the client) connects
//! with VS Code Remote-SSH. DevBridge only automates the setup around the
//! supported, maintained Remote-SSH workflow:
//!
//! - the client generates a dedicated SSH key (`~/.ssh/devbridge_<host>`);
//! - its public key travels to the host over the paired, mutually
//!   authenticated DevBridge connection, and the host's user approves it;
//! - the host appends it to `authorized_keys`, tagged `devbridge:<device-id>`
//!   so it can be revoked precisely;
//! - the host's SSH host keys travel back over the same authenticated channel
//!   and are pinned in a DevBridge-owned known-hosts file (no
//!   trust-on-first-use prompt);
//! - the client gets a `Host` entry in `~/.ssh/devbridge_config`, included
//!   from `~/.ssh/config` only after the user agrees.
//!
//! DevBridge never enables system services and never touches other keys or
//! other `Host` entries.

use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum RemoteError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("ssh-keygen isn't available. Install the OpenSSH client (built into macOS and Windows 10+).")]
    NoKeygen,
    #[error("ssh-keygen failed: {0}")]
    Keygen(String),
    #[error("invalid {0}")]
    Invalid(&'static str),
}

pub type Result<T> = std::result::Result<T, RemoteError>;

pub const MARKER_PREFIX: &str = "devbridge:";

pub fn ssh_dir(home: &Path) -> PathBuf {
    home.join(".ssh")
}

fn ensure_ssh_dir(home: &Path) -> Result<PathBuf> {
    let d = ssh_dir(home);
    if !d.exists() {
        std::fs::create_dir_all(&d)?;
        set_private(&d, true);
    }
    Ok(d)
}

#[cfg(unix)]
fn set_private(p: &Path, dir: bool) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(if dir { 0o700 } else { 0o600 }));
}

#[cfg(not(unix))]
fn set_private(_p: &Path, _dir: bool) {}

/// Client key file for a given host device: `~/.ssh/devbridge_<id8>`.
pub fn key_path(home: &Path, host_device_id: &str) -> PathBuf {
    let short: String = host_device_id.chars().take(8).collect::<String>().to_lowercase();
    ssh_dir(home).join(format!("devbridge_{short}"))
}

/// Creates the client key pair if needed and returns the public key line.
pub fn ensure_keypair(home: &Path, host_device_id: &str, my_device_id: &str) -> Result<String> {
    ensure_ssh_dir(home)?;
    let key = key_path(home, host_device_id);
    let pubkey = key.with_extension("pub");
    if !key.exists() || !pubkey.exists() {
        let keygen = which_keygen().ok_or(RemoteError::NoKeygen)?;
        let comment = format!("{MARKER_PREFIX}{my_device_id}");
        let out =
            Command::new(keygen).args(["-q", "-t", "ed25519", "-N", "", "-C", &comment, "-f"]).arg(&key).output()?;
        if !out.status.success() {
            return Err(RemoteError::Keygen(String::from_utf8_lossy(&out.stderr).trim().to_owned()));
        }
        set_private(&key, false);
    }
    let line = std::fs::read_to_string(pubkey)?;
    validate_public_key(line.trim())
}

fn which_keygen() -> Option<PathBuf> {
    if let Ok(p) = which_in_path("ssh-keygen") {
        return Some(p);
    }
    let candidates: &[&str] =
        if cfg!(windows) { &[r"C:\Windows\System32\OpenSSH\ssh-keygen.exe"] } else { &["/usr/bin/ssh-keygen"] };
    candidates.iter().map(PathBuf::from).find(|p| p.is_file())
}

fn which_in_path(name: &str) -> io::Result<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(&exe)).find(|p| p.is_file()))
        .ok_or_else(|| io::ErrorKind::NotFound.into())
}

/// Accepts exactly one `ssh-ed25519 <base64> [comment]` key. Rejects options
/// (e.g. `command=`), other key types, and anything multi-line, so a peer can
/// never inject extra `authorized_keys` directives.
pub fn validate_public_key(line: &str) -> Result<String> {
    if line.contains('\n') || line.contains('\r') {
        return Err(RemoteError::Invalid("public key"));
    }
    let mut parts = line.split_whitespace();
    let (Some(kind), Some(b64)) = (parts.next(), parts.next()) else { return Err(RemoteError::Invalid("public key")) };
    if kind != "ssh-ed25519"
        || b64.len() < 60
        || b64.len() > 100
        || !b64.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    {
        return Err(RemoteError::Invalid("public key"));
    }
    Ok(format!("{kind} {b64}"))
}

pub fn authorized_keys_path(home: &Path) -> PathBuf {
    ssh_dir(home).join("authorized_keys")
}

/// Appends `key` for `device_id` unless already present. Returns true if added.
pub fn authorize_key(home: &Path, key: &str, device_id: &str) -> Result<bool> {
    let key = validate_public_key(key)?;
    if !device_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(RemoteError::Invalid("device id"));
    }
    ensure_ssh_dir(home)?;
    let path = authorized_keys_path(home);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.split_whitespace().take(2).collect::<Vec<_>>().join(" ") == key) {
        return Ok(false);
    }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    writeln!(f, "{key} {MARKER_PREFIX}{device_id}")?;
    set_private(&path, false);
    Ok(true)
}

/// Removes only the lines DevBridge added for `device_id`.
pub fn remove_keys(home: &Path, device_id: &str) -> Result<usize> {
    let path = authorized_keys_path(home);
    let Ok(existing) = std::fs::read_to_string(&path) else { return Ok(0) };
    let marker = format!("{MARKER_PREFIX}{device_id}");
    let (keep, removed): (Vec<&str>, Vec<&str>) =
        existing.lines().partition(|l| l.split_whitespace().last() != Some(marker.as_str()));
    if removed.is_empty() {
        return Ok(0);
    }
    let mut out = keep.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    write_atomic(&path, out.as_bytes())?;
    set_private(&path, false);
    Ok(removed.len())
}

fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("devbridge-tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// The host's SSH server public keys (`type base64`), if readable.
pub fn host_public_keys() -> Vec<String> {
    let dir = if cfg!(windows) { PathBuf::from(r"C:\ProgramData\ssh") } else { PathBuf::from("/etc/ssh") };
    let mut keys = Vec::new();
    for name in ["ssh_host_ed25519_key.pub", "ssh_host_ecdsa_key.pub", "ssh_host_rsa_key.pub"] {
        if let Ok(s) = std::fs::read_to_string(dir.join(name)) {
            let mut p = s.split_whitespace();
            if let (Some(t), Some(k)) = (p.next(), p.next())
                && (t.starts_with("ssh-") || t.starts_with("ecdsa-"))
            {
                keys.push(format!("{t} {k}"));
            }
        }
    }
    keys
}

pub fn valid_host_key(k: &str) -> bool {
    let mut p = k.split_whitespace();
    matches!((p.next(), p.next(), p.next()), (Some(t), Some(b), None)
        if (t.starts_with("ssh-") || t.starts_with("ecdsa-")) && t.len() < 40
            && b.len() < 1024 && b.bytes().all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c)))
}

pub fn valid_user(u: &str) -> bool {
    !u.is_empty()
        && u.len() <= 64
        && !u.starts_with('-')
        && u.chars().all(|c| c.is_ascii_alphanumeric() || "._-@\\".contains(c))
}

/// `devbridge-studio-mac-01m5v7yh`
pub fn alias_for(host_name: &str, host_device_id: &str) -> String {
    let slug: String = host_name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(24).collect();
    let id: String = host_device_id.chars().take(8).collect::<String>().to_lowercase();
    if slug.is_empty() { format!("devbridge-{id}") } else { format!("devbridge-{slug}-{id}") }
}

pub fn config_path(home: &Path) -> PathBuf {
    ssh_dir(home).join("devbridge_config")
}

pub fn known_hosts_path(home: &Path) -> PathBuf {
    ssh_dir(home).join("devbridge_known_hosts")
}

pub struct HostEntry<'a> {
    pub alias: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub user: &'a str,
    pub key_path: &'a Path,
    pub host_keys: &'a [String],
}

fn block_markers(alias: &str) -> (String, String) {
    (format!("# >>> {alias} (managed by DevBridge)"), format!("# <<< {alias}"))
}

fn strip_block(text: &str, alias: &str) -> String {
    let (start, end) = block_markers(alias);
    let mut out = Vec::new();
    let mut skipping = false;
    for l in text.lines() {
        if l == start {
            skipping = true;
            continue;
        }
        if skipping {
            if l == end {
                skipping = false;
            }
            continue;
        }
        out.push(l);
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// Writes (or replaces) the `Host` block for `e.alias` in the DevBridge-owned
/// config file and pins the host keys.
pub fn write_host_entry(home: &Path, e: &HostEntry<'_>) -> Result<()> {
    if !valid_user(e.user) || e.host.contains(char::is_whitespace) || e.host.is_empty() {
        return Err(RemoteError::Invalid("host details"));
    }
    ensure_ssh_dir(home)?;
    let keys: Vec<&String> = e.host_keys.iter().filter(|k| valid_host_key(k)).collect();
    let kh = known_hosts_path(home);
    let host_pattern = if e.port == 22 { e.host.to_owned() } else { format!("[{}]:{}", e.host, e.port) };
    // Known hosts: replace this host's lines.
    let existing = std::fs::read_to_string(&kh).unwrap_or_default();
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|l| l.split_whitespace().next() != Some(host_pattern.as_str()))
        .map(str::to_owned)
        .collect();
    for k in &keys {
        lines.push(format!("{host_pattern} {k}"));
    }
    write_atomic(&kh, (lines.join("\n") + "\n").as_bytes())?;

    let (start, end) = block_markers(e.alias);
    let mut block = vec![
        start,
        format!("Host {}", e.alias),
        format!("  HostName {}", e.host),
        format!("  Port {}", e.port),
        format!("  User {}", e.user),
        format!("  IdentityFile \"{}\"", e.key_path.display()),
        "  IdentitiesOnly yes".into(),
        "  ServerAliveInterval 15".into(),
    ];
    if keys.is_empty() {
        block.push("  StrictHostKeyChecking ask".into());
    } else {
        block.push(format!("  UserKnownHostsFile \"{}\"", kh.display()));
        block.push("  StrictHostKeyChecking yes".into());
    }
    block.push(end);
    let cfg = config_path(home);
    let mut text = strip_block(&std::fs::read_to_string(&cfg).unwrap_or_default(), e.alias);
    text.push_str(&block.join("\n"));
    text.push('\n');
    write_atomic(&cfg, text.as_bytes())?;
    set_private(&cfg, false);
    Ok(())
}

pub fn remove_host_entry(home: &Path, alias: &str) -> Result<()> {
    let cfg = config_path(home);
    if let Ok(t) = std::fs::read_to_string(&cfg) {
        write_atomic(&cfg, strip_block(&t, alias).as_bytes())?;
    }
    Ok(())
}

const INCLUDE_LINE: &str = "Include devbridge_config";

/// Whether `~/.ssh/config` includes the DevBridge file.
pub fn include_present(home: &Path) -> bool {
    std::fs::read_to_string(ssh_dir(home).join("config"))
        .map(|t| t.lines().any(|l| l.trim().eq_ignore_ascii_case(INCLUDE_LINE)))
        .unwrap_or(false)
}

/// Adds `Include devbridge_config` at the top of `~/.ssh/config` (top level,
/// so it applies globally), keeping a backup. Only called after the user agrees.
pub fn ensure_include(home: &Path) -> Result<bool> {
    ensure_ssh_dir(home)?;
    if include_present(home) {
        return Ok(false);
    }
    let path = ssh_dir(home).join("config");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if !existing.is_empty() {
        std::fs::write(path.with_extension("devbridge-backup"), &existing)?;
    }
    let text = format!("# Added by DevBridge for Remote Workspace connections\n{INCLUDE_LINE}\n\n{existing}");
    write_atomic(&path, text.as_bytes())?;
    set_private(&path, false);
    Ok(true)
}

/// `vscode-remote://ssh-remote+<alias>/<path>`; Windows paths become `/C:/…`.
pub fn folder_uri(alias: &str, host_is_windows: bool, path: &str) -> String {
    let p = if host_is_windows { format!("/{}", path.replace('\\', "/")) } else { path.to_owned() };
    let encoded: String = p
        .chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '#' => "%23".to_string(),
            '?' => "%3F".to_string(),
            '%' => "%25".to_string(),
            c => c.to_string(),
        })
        .collect();
    format!("vscode-remote://ssh-remote+{alias}{encoded}")
}

pub fn port_reachable(addr: SocketAddr) -> bool {
    TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok()
}

/// Is an SSH server accepting connections on this computer?
pub fn local_ssh_server_running() -> bool {
    port_reachable(SocketAddr::from(([127, 0, 0, 1], 22)))
}

pub fn ssh_server_guidance() -> &'static str {
    if cfg!(target_os = "macos") {
        "Turn on Remote Login: System Settings → General → Sharing → Remote Login. Allow access for your user only."
    } else if cfg!(windows) {
        "Install and start OpenSSH Server: Settings → System → Optional features → Add “OpenSSH Server”, then in an administrator PowerShell run: Start-Service sshd; Set-Service sshd -StartupType Automatic. If your account is an administrator, sshd reads keys from C:\\ProgramData\\ssh\\administrators_authorized_keys instead of your own authorized_keys."
    } else {
        "Install and start the OpenSSH server (for example: sudo apt install openssh-server)."
    }
}

pub fn remote_ssh_installed(home: &Path) -> bool {
    std::fs::read_dir(home.join(".vscode/extensions"))
        .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().starts_with("ms-vscode-remote.remote-ssh-")))
        .unwrap_or(false)
}

pub fn current_user() -> String {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

    #[test]
    fn validates_keys_strictly() {
        assert_eq!(validate_public_key(&format!("{KEY} me@host")).unwrap(), KEY);
        assert!(validate_public_key(&format!("command=\"rm -rf ~\" {KEY}")).is_err());
        assert!(validate_public_key(&format!("{KEY}\nssh-rsa AAAA")).is_err());
        assert!(validate_public_key("ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQ").is_err());
    }

    #[test]
    fn authorize_and_revoke_only_touch_our_lines() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".ssh")).unwrap();
        std::fs::write(authorized_keys_path(h.path()), "ssh-ed25519 AAAAexisting user@laptop").unwrap();
        assert!(authorize_key(h.path(), KEY, "01JDEV").unwrap());
        assert!(!authorize_key(h.path(), KEY, "01JDEV").unwrap(), "idempotent");
        let t = std::fs::read_to_string(authorized_keys_path(h.path())).unwrap();
        assert_eq!(t.lines().count(), 2);
        assert!(t.contains("devbridge:01JDEV"));
        assert_eq!(remove_keys(h.path(), "01JDEV").unwrap(), 1);
        assert_eq!(
            std::fs::read_to_string(authorized_keys_path(h.path())).unwrap(),
            "ssh-ed25519 AAAAexisting user@laptop\n"
        );
        assert!(authorize_key(h.path(), KEY, "../x").is_err());
    }

    #[test]
    fn host_entry_round_trip_and_include() {
        let h = tempfile::tempdir().unwrap();
        std::fs::create_dir(h.path().join(".ssh")).unwrap();
        std::fs::write(h.path().join(".ssh/config"), "Host work\n  HostName work.example.com\n").unwrap();
        let keys = vec!["ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHostKey".to_string()];
        let e = HostEntry {
            alias: "devbridge-mac-01m5",
            host: "192.168.1.20",
            port: 22,
            user: "nick",
            key_path: Path::new("/k"),
            host_keys: &keys,
        };
        write_host_entry(h.path(), &e).unwrap();
        write_host_entry(h.path(), &e).unwrap();
        let cfg = std::fs::read_to_string(config_path(h.path())).unwrap();
        assert_eq!(cfg.matches("Host devbridge-mac-01m5").count(), 1, "replaced, not duplicated");
        assert!(cfg.contains("StrictHostKeyChecking yes"));
        assert!(std::fs::read_to_string(known_hosts_path(h.path())).unwrap().starts_with("192.168.1.20 ssh-ed25519"));

        assert!(!include_present(h.path()));
        assert!(ensure_include(h.path()).unwrap());
        assert!(!ensure_include(h.path()).unwrap());
        let main = std::fs::read_to_string(h.path().join(".ssh/config")).unwrap();
        assert!(main.contains("Include devbridge_config") && main.contains("Host work"), "existing config preserved");
        assert!(h.path().join(".ssh/config.devbridge-backup").exists());

        remove_host_entry(h.path(), "devbridge-mac-01m5").unwrap();
        assert!(!std::fs::read_to_string(config_path(h.path())).unwrap().contains("devbridge-mac-01m5"));
        let bad = HostEntry { user: "x; rm", ..e };
        assert!(write_host_entry(h.path(), &bad).is_err());
    }

    #[test]
    fn uris_and_aliases() {
        assert_eq!(
            folder_uri("devbridge-pc", true, r"C:\Users\Nick\My App"),
            "vscode-remote://ssh-remote+devbridge-pc/C:/Users/Nick/My%20App"
        );
        assert_eq!(
            folder_uri("devbridge-mac", false, "/Users/nick/site"),
            "vscode-remote://ssh-remote+devbridge-mac/Users/nick/site"
        );
        assert_eq!(alias_for("Nick's MacBook Air", "01M5V7YHRN"), "devbridge-nick-s-macbook-air-01m5v7yh");
    }

    #[test]
    fn generates_a_real_keypair() {
        if which_keygen().is_none() {
            return;
        }
        let h = tempfile::tempdir().unwrap();
        let line = ensure_keypair(h.path(), "01HOSTID", "01MYDEV").unwrap();
        assert!(line.starts_with("ssh-ed25519 "));
        assert_eq!(ensure_keypair(h.path(), "01HOSTID", "01MYDEV").unwrap(), line, "reused");
    }
}
