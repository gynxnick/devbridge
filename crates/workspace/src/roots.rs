//! Validation of a folder the user wants to register as a workspace root.

use std::path::{Path, PathBuf};

use devbridge_core::fsutil::{canonicalize, is_within, path_key};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RootError {
    #[error("The folder {0} doesn't exist.")]
    NotFound(String),
    #[error("{0} is a file, not a folder.")]
    NotADirectory(String),
    #[error(
        "DevBridge can't read {0}. Check the folder's permissions (on macOS, also System Settings → Privacy & Security → Files and Folders)."
    )]
    NotReadable(String),
    #[error("{0} is a system or home folder. Choose a specific project folder instead.")]
    Protected(String),
    #[error("This folder is already registered as \"{0}\".")]
    AlreadyRegistered(String),
    #[error("This folder is inside the workspace \"{0}\". Nested workspaces would sync the same files twice.")]
    InsideWorkspace(String),
    #[error("This folder contains the workspace \"{0}\". Nested workspaces would sync the same files twice.")]
    ContainsWorkspace(String),
}

/// An already-registered workspace, for overlap checks.
pub struct Registered<'a> {
    pub name: &'a str,
    pub root: &'a Path,
}

/// Folders that must not become workspace roots.
#[derive(Debug, Default, Clone)]
pub struct Protected {
    /// Forbidden as a root themselves, but their subfolders are fine (e.g. home).
    pub exact: Vec<PathBuf>,
    /// Forbidden together with everything inside them (e.g. DevBridge's app data).
    pub subtree: Vec<PathBuf>,
}

impl Protected {
    pub fn for_app(app_data_dir: Option<&Path>) -> Self {
        Self {
            exact: dirs::home_dir().into_iter().collect(),
            subtree: app_data_dir.map(Path::to_path_buf).into_iter().collect(),
        }
    }
}

/// Validates a candidate root and returns its canonical form.
pub fn validate_root(raw: &Path, existing: &[Registered<'_>], protected: &Protected) -> Result<PathBuf, RootError> {
    let shown = raw.display().to_string();
    let meta = std::fs::metadata(raw).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => RootError::NotFound(shown.clone()),
        _ => RootError::NotReadable(shown.clone()),
    })?;
    if !meta.is_dir() {
        return Err(RootError::NotADirectory(shown));
    }
    let root = canonicalize(raw).map_err(|_| RootError::NotReadable(shown.clone()))?;
    std::fs::read_dir(&root).map_err(|_| RootError::NotReadable(shown.clone()))?;

    let key = path_key(&root);
    // A filesystem root ("/" or "C:\") or any protected folder itself.
    if root.parent().is_none() || protected.exact.iter().any(|p| canonicalize(p).is_ok_and(|p| path_key(&p) == key)) {
        return Err(RootError::Protected(root.display().to_string()));
    }
    // System directories and everything under them.
    if system_dirs().iter().any(|s| is_within(&root, Path::new(s))) {
        return Err(RootError::Protected(root.display().to_string()));
    }
    // App data must never become a workspace (it holds recovery copies and the DB).
    if protected.subtree.iter().any(|p| canonicalize(p).is_ok_and(|p| is_within(&root, &p))) {
        return Err(RootError::Protected(root.display().to_string()));
    }

    for ws in existing {
        let other = ws.root;
        if path_key(other) == key {
            return Err(RootError::AlreadyRegistered(ws.name.to_owned()));
        }
        if is_within(&root, other) {
            return Err(RootError::InsideWorkspace(ws.name.to_owned()));
        }
        if is_within(other, &root) {
            return Err(RootError::ContainsWorkspace(ws.name.to_owned()));
        }
    }
    Ok(root)
}

fn system_dirs() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &["/System", "/usr", "/bin", "/sbin", "/etc", "/private/etc", "/Library", "/Applications", "/Volumes/Recovery"]
    } else if cfg!(windows) {
        &[r"C:\Windows", r"C:\Program Files", r"C:\Program Files (x86)", r"C:\ProgramData"]
    } else {
        &["/usr", "/bin", "/sbin", "/etc", "/proc", "/sys", "/dev", "/boot"]
    }
}

/// Non-fatal observations shown before importing.
pub fn warnings_for(root: &Path) -> Vec<String> {
    let key = path_key(root);
    let mut w = Vec::new();
    let cloud = [
        ("library/mobile documents", "iCloud Drive"),
        ("/onedrive", "OneDrive"),
        ("/dropbox", "Dropbox"),
        ("/google drive", "Google Drive"),
        ("/library/cloudstorage/", "a cloud storage provider"),
    ];
    let key_ci = key.to_lowercase();
    if let Some((_, name)) = cloud.iter().find(|(needle, _)| key_ci.contains(needle)) {
        w.push(format!(
            "This folder is synced by {name}. Two sync engines on the same files can cause conflicts and corrupt Git data; a folder outside cloud storage is strongly recommended."
        ));
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> Protected {
        Protected::default()
    }

    fn dirs3() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let a = t.path().join("a");
        let b = a.join("b");
        std::fs::create_dir_all(&b).unwrap();
        (t, canonicalize(&a).unwrap(), canonicalize(&b).unwrap())
    }

    #[test]
    fn accepts_plain_folder_and_canonicalizes() {
        let (_t, a, _) = dirs3();
        let dotted = a.join("b").join("..");
        assert_eq!(validate_root(&dotted, &[], &none()).unwrap(), a);
    }

    #[test]
    fn rejects_missing_and_files() {
        let (t, ..) = dirs3();
        assert!(matches!(validate_root(&t.path().join("nope"), &[], &none()), Err(RootError::NotFound(_))));
        let f = t.path().join("f.txt");
        std::fs::write(&f, "x").unwrap();
        assert!(matches!(validate_root(&f, &[], &none()), Err(RootError::NotADirectory(_))));
    }

    #[test]
    fn rejects_overlap_with_existing_workspaces() {
        let (_t, a, b) = dirs3();
        let reg = [Registered { name: "A", root: &a }];
        assert_eq!(validate_root(&a, &reg, &none()), Err(RootError::AlreadyRegistered("A".into())));
        assert_eq!(validate_root(&b, &reg, &none()), Err(RootError::InsideWorkspace("A".into())));
        let reg = [Registered { name: "B", root: &b }];
        assert_eq!(validate_root(&a, &reg, &none()), Err(RootError::ContainsWorkspace("B".into())));
    }

    #[test]
    fn rejects_protected_folders() {
        let (_t, a, b) = dirs3();
        // `a` acts as "home": itself is protected, its children are fine.
        let home = Protected { exact: vec![a.clone()], subtree: vec![] };
        assert!(matches!(validate_root(&a, &[], &home), Err(RootError::Protected(_))));
        assert!(validate_root(&b, &[], &home).is_ok());
        // `a` as app data: it and everything inside are protected.
        let data = Protected { exact: vec![], subtree: vec![a.clone()] };
        assert!(matches!(validate_root(&b, &[], &data), Err(RootError::Protected(_))));
        assert!(matches!(validate_root(Path::new("/"), &[], &none()), Err(RootError::Protected(_))));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_system_dirs() {
        assert!(matches!(validate_root(Path::new("/usr/bin"), &[], &none()), Err(RootError::Protected(_))));
    }

    #[test]
    fn warns_about_cloud_folders() {
        assert_eq!(warnings_for(Path::new("/Users/x/Library/Mobile Documents/com~apple~CloudDocs/p")).len(), 1);
        assert_eq!(warnings_for(Path::new("/Users/x/Dropbox/p")).len(), 1);
        assert!(warnings_for(Path::new("/Users/x/Developer/p")).is_empty());
    }
}
