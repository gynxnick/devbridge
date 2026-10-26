//! Small filesystem helpers shared across crates.

use std::path::{Path, PathBuf};

/// Canonicalizes `p` (resolving symlinks and `..`). On Windows it strips the
/// `\\?\` verbatim prefix from local-drive paths so results compare equal to
/// paths printed by other tools (Git, VS Code) and are readable in the UI.
pub fn canonicalize(p: &Path) -> std::io::Result<PathBuf> {
    let c = p.canonicalize()?;
    #[cfg(windows)]
    {
        let s = c.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\")
            && !rest.starts_with("UNC\\")
            && rest.len() < 260
        {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(c)
}

/// Whether the platform's default filesystems compare names case-insensitively.
/// (APFS and NTFS defaults are case-insensitive; Linux filesystems are not.)
pub const CASE_INSENSITIVE_FS: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// Comparable key for a path on this platform.
pub fn path_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let s = s.trim_end_matches('/').to_owned();
    if CASE_INSENSITIVE_FS { s.to_lowercase() } else { s }
}

/// True if `inner` is `outer` or lies inside it (both should be canonical).
pub fn is_within(inner: &Path, outer: &Path) -> bool {
    let (i, o) = (path_key(inner), path_key(outer));
    i == o || (i.starts_with(&o) && i.as_bytes().get(o.len()) == Some(&b'/'))
        // A filesystem root ("/" or "c:") ends with no separator after trimming.
        || (o.is_empty() || o.ends_with(':')) && i.starts_with(&o)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_respects_component_boundaries() {
        assert!(is_within(Path::new("/a/b/c"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/b"), Path::new("/a/b")));
        assert!(is_within(Path::new("/a/b/"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a/bc"), Path::new("/a/b")));
        assert!(!is_within(Path::new("/a"), Path::new("/a/b")));
        assert!(is_within(Path::new("/anything"), Path::new("/")));
    }

    #[test]
    fn case_handling_matches_platform() {
        assert_eq!(is_within(Path::new("/A/B/c"), Path::new("/a/b")), CASE_INSENSITIVE_FS);
    }

    #[test]
    fn canonicalize_resolves_dotdot() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("x")).unwrap();
        let a = canonicalize(&d.path().join("x").join("..").join("x")).unwrap();
        let b = canonicalize(&d.path().join("x")).unwrap();
        assert_eq!(a, b);
    }
}
