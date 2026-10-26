//! Applying file changes safely.
//!
//! - Content is written to a staging file first and moved into place with an
//!   atomic rename, so a crash never leaves a half-written project file.
//! - Right before the rename, the current file is re-checked against what the
//!   sync engine expected. If the user edited it in the meantime, the apply is
//!   refused (`Changed`) and the engine turns it into a conflict instead.
//! - Every directory on the way to the target is checked with
//!   `symlink_metadata`: a symlink anywhere in the path aborts the write, so a
//!   peer can't redirect writes outside the workspace.

use std::io;
use std::path::{Path, PathBuf};

use devbridge_core::RelPath;
use thiserror::Error;

use crate::hash::hash_file;

#[derive(Debug, Error)]
pub enum ApplyError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("refusing to write through a symbolic link at {0}")]
    SymlinkInPath(String),
    #[error("{0} exists and is not a folder")]
    NotADirectory(String),
    #[error("target resolves outside the workspace")]
    OutsideRoot,
    #[error("the file changed on this device before the update could be applied")]
    Changed,
    #[error("received content doesn't match its hash")]
    HashMismatch,
}

/// What the engine believes is currently on disk at the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    Absent,
    Hash(String),
}

/// Picks where to stage incoming files for `root`: the app's staging dir when
/// it's on the same volume (so the final rename is atomic), otherwise a hidden
/// `.devbridge/staging` folder inside the workspace (always excluded from sync).
pub fn staging_dir(root: &Path, app_staging: &Path) -> PathBuf {
    if same_volume(root, app_staging) { app_staging.to_path_buf() } else { root.join(".devbridge").join("staging") }
}

#[cfg(unix)]
fn same_volume(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let _ = std::fs::create_dir_all(b);
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev(),
        _ => false,
    }
}

#[cfg(windows)]
fn same_volume(a: &Path, b: &Path) -> bool {
    let _ = std::fs::create_dir_all(b);
    let prefix = |p: &Path| {
        devbridge_core::fsutil::canonicalize(p)
            .ok()
            .and_then(|c| c.components().next().map(|c| c.as_os_str().to_ascii_lowercase()))
    };
    matches!((prefix(a), prefix(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(not(any(unix, windows)))]
fn same_volume(_a: &Path, _b: &Path) -> bool {
    false
}

/// Creates missing parent directories of `rel` below `root`, refusing to pass
/// through symlinks or files. Returns the absolute target path.
pub fn prepare_parent(root: &Path, rel: &RelPath) -> Result<PathBuf, ApplyError> {
    let segs: Vec<&str> = rel.segments().collect();
    let mut cur = root.to_path_buf();
    for seg in &segs[..segs.len() - 1] {
        cur.push(seg);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Err(ApplyError::SymlinkInPath(display(root, &cur))),
            Ok(m) if !m.is_dir() => return Err(ApplyError::NotADirectory(display(root, &cur))),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => std::fs::create_dir(&cur)?,
            Err(e) => return Err(e.into()),
        }
    }
    // Defence in depth: the canonical parent must still be inside the root.
    let parent = devbridge_core::fsutil::canonicalize(&cur)?;
    let canon_root = devbridge_core::fsutil::canonicalize(root)?;
    if !devbridge_core::fsutil::is_within(&parent, &canon_root) {
        return Err(ApplyError::OutsideRoot);
    }
    let target = cur.join(segs[segs.len() - 1]);
    if let Ok(m) = std::fs::symlink_metadata(&target) {
        if m.file_type().is_symlink() {
            return Err(ApplyError::SymlinkInPath(rel.to_string()));
        }
        if m.is_dir() {
            return Err(ApplyError::NotADirectory(rel.to_string()));
        }
    }
    Ok(target)
}

fn display(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Checks the on-disk state at `target` against `expect`.
pub fn check_expectation(target: &Path, expect: &Expect) -> Result<(), ApplyError> {
    let exists = match std::fs::symlink_metadata(target) {
        Ok(_) => true,
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };
    match expect {
        Expect::Absent if exists => Err(ApplyError::Changed),
        Expect::Absent => Ok(()),
        Expect::Hash(_) if !exists => Err(ApplyError::Changed),
        Expect::Hash(h) => {
            if &hash_file(target)?.0 == h {
                Ok(())
            } else {
                Err(ApplyError::Changed)
            }
        }
    }
}

/// Atomically replaces (or creates) `rel` with the verified `staged` file.
pub fn commit_file(
    root: &Path,
    rel: &RelPath,
    staged: &Path,
    expected_hash: &str,
    expect: &Expect,
    mode: Option<u32>,
) -> Result<(), ApplyError> {
    if hash_file(staged)?.0 != expected_hash {
        return Err(ApplyError::HashMismatch);
    }
    let target = prepare_parent(root, rel)?;
    check_expectation(&target, expect)?;
    set_mode(staged, mode);
    if std::fs::rename(staged, &target).is_err() {
        // Cross-volume staging (rare): copy next to the target, then rename.
        let tmp = target.with_file_name(format!(".{}.devbridge-tmp", ulid::Ulid::generate()));
        std::fs::copy(staged, &tmp)?;
        std::fs::rename(&tmp, &target).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        let _ = std::fs::remove_file(staged);
    }
    Ok(())
}

/// Copies `src` (e.g. a recovery blob) into a staging file and commits it.
pub fn commit_copy(
    root: &Path,
    rel: &RelPath,
    src: &Path,
    staging: &Path,
    hash: &str,
    expect: &Expect,
    mode: Option<u32>,
) -> Result<(), ApplyError> {
    std::fs::create_dir_all(staging)?;
    let staged = staging.join(format!("copy-{}", ulid::Ulid::generate()));
    std::fs::copy(src, &staged)?;
    let r = commit_file(root, rel, &staged, hash, expect, mode);
    if r.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    r
}

/// Deletes `rel` if it still matches `expect`, then prunes now-empty parent
/// folders (never the root). The caller saves a recovery copy first.
pub fn remove_file(root: &Path, rel: &RelPath, expect: &Expect) -> Result<(), ApplyError> {
    let target = rel.to_fs(root);
    match std::fs::symlink_metadata(&target) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
        Ok(m) if m.file_type().is_symlink() || m.is_dir() => return Err(ApplyError::NotADirectory(rel.to_string())),
        Ok(_) => {}
    }
    prepare_parent(root, rel)?; // symlink check on the way down
    check_expectation(&target, expect)?;
    std::fs::remove_file(&target)?;
    let mut dir = target.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || std::fs::remove_dir(&d).is_err() {
            break; // not empty (or root): stop
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(())
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: Option<u32>) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(m) = mode {
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(m & 0o777));
    }
}

#[cfg(not(unix))]
fn set_mode(_p: &Path, _mode: Option<u32>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_bytes;

    fn rp(s: &str) -> RelPath {
        RelPath::parse(s).unwrap()
    }

    fn stage(dir: &Path, data: &[u8]) -> (PathBuf, String) {
        let p = dir.join(ulid::Ulid::generate().to_string());
        std::fs::write(&p, data).unwrap();
        (p, hash_bytes(data))
    }

    #[test]
    fn creates_new_file_with_parents() {
        let w = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let (f, h) = stage(s.path(), b"new");
        commit_file(w.path(), &rp("a/b/c.txt"), &f, &h, &Expect::Absent, Some(0o755)).unwrap();
        assert_eq!(std::fs::read(w.path().join("a/b/c.txt")).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(w.path().join("a/b/c.txt")).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o755);
        }
    }

    #[test]
    fn refuses_when_local_file_changed() {
        let w = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        std::fs::write(w.path().join("x.txt"), b"user edit").unwrap();
        let (f, h) = stage(s.path(), b"incoming");
        let expect = Expect::Hash(hash_bytes(b"old"));
        assert!(matches!(commit_file(w.path(), &rp("x.txt"), &f, &h, &expect, None), Err(ApplyError::Changed)));
        assert_eq!(std::fs::read(w.path().join("x.txt")).unwrap(), b"user edit", "user's edit preserved");
        assert!(matches!(commit_file(w.path(), &rp("x.txt"), &f, &h, &Expect::Absent, None), Err(ApplyError::Changed)));
    }

    #[test]
    fn replaces_expected_file() {
        let w = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        std::fs::write(w.path().join("x.txt"), b"old").unwrap();
        let (f, h) = stage(s.path(), b"incoming");
        commit_file(w.path(), &rp("x.txt"), &f, &h, &Expect::Hash(hash_bytes(b"old")), None).unwrap();
        assert_eq!(std::fs::read(w.path().join("x.txt")).unwrap(), b"incoming");
    }

    #[test]
    fn rejects_corrupted_transfer() {
        let w = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        let (f, _) = stage(s.path(), b"corrupt");
        let r = commit_file(w.path(), &rp("x.txt"), &f, &hash_bytes(b"intended"), &Expect::Absent, None);
        assert!(matches!(r, Err(ApplyError::HashMismatch)));
        assert!(!w.path().join("x.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn never_writes_through_symlinks() {
        let w = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let s = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), w.path().join("evil")).unwrap();
        let (f, h) = stage(s.path(), b"pwn");
        let r = commit_file(w.path(), &rp("evil/authorized_keys"), &f, &h, &Expect::Absent, None);
        assert!(matches!(r, Err(ApplyError::SymlinkInPath(_))), "{r:?}");
        assert!(!outside.path().join("authorized_keys").exists());

        std::os::unix::fs::symlink(outside.path().join("t"), w.path().join("t.txt")).unwrap();
        let (f, h) = stage(s.path(), b"pwn");
        assert!(commit_file(w.path(), &rp("t.txt"), &f, &h, &Expect::Absent, None).is_err());
        assert!(!outside.path().join("t").exists());
    }

    #[test]
    fn remove_prunes_empty_dirs_but_checks_expectation() {
        let w = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(w.path().join("a/b")).unwrap();
        std::fs::write(w.path().join("a/b/c.txt"), b"v1").unwrap();
        assert!(matches!(
            remove_file(w.path(), &rp("a/b/c.txt"), &Expect::Hash(hash_bytes(b"other"))),
            Err(ApplyError::Changed)
        ));
        remove_file(w.path(), &rp("a/b/c.txt"), &Expect::Hash(hash_bytes(b"v1"))).unwrap();
        assert!(!w.path().join("a").exists());
        assert!(w.path().exists());
    }
}
