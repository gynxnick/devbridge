//! Workspace-relative paths.
//!
//! A [`RelPath`] is the only path form that crosses module and network
//! boundaries. It is `/`-separated, NFC-normalized, never absolute, never
//! contains `.`/`..`/empty segments, and is portable to both macOS and Windows
//! (Windows forbids some names and characters that macOS allows; such files are
//! reported as sync issues rather than synced and then failing on the peer).

use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

pub const MAX_PATH_BYTES: usize = 1024;
/// Windows' classic per-component limit.
pub const MAX_COMPONENT_CHARS: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path must be relative to the workspace")]
    Absolute,
    #[error("path contains '.' or '..'")]
    DotSegment,
    #[error("path contains an empty segment")]
    EmptySegment,
    #[error("path contains a backslash")]
    Backslash,
    #[error("path contains control characters")]
    Control,
    #[error("“{0}” is a reserved name on Windows")]
    ReservedName(String),
    #[error("“{0}” ends with a dot or space, which Windows doesn't allow")]
    TrailingDotOrSpace(String),
    #[error("path contains “{0}”, which Windows doesn't allow in file names")]
    InvalidChar(char),
    #[error("path is too long")]
    TooLong,
    #[error("path is outside the workspace")]
    OutsideRoot,
    #[error("path is not valid Unicode")]
    NotUnicode,
}

impl PathError {
    /// True for errors that only mean "can't be synced to Windows" (the file is
    /// legitimate locally); false for malformed or hostile paths.
    pub fn is_portability(&self) -> bool {
        matches!(
            self,
            PathError::ReservedName(_)
                | PathError::TrailingDotOrSpace(_)
                | PathError::InvalidChar(_)
                | PathError::TooLong
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RelPath(String);

impl<'de> Deserialize<'de> for RelPath {
    /// Deserialization always validates: a peer can never smuggle in a bad path.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        RelPath::parse(&s).map_err(serde::de::Error::custom)
    }
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "COM¹", "COM²",
    "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

fn check_segment(seg: &str) -> Result<(), PathError> {
    if seg.is_empty() {
        return Err(PathError::EmptySegment);
    }
    if seg == "." || seg == ".." {
        return Err(PathError::DotSegment);
    }
    if seg.chars().any(|c| c.is_control()) {
        return Err(PathError::Control);
    }
    if seg.contains('\\') {
        return Err(PathError::Backslash);
    }
    if let Some(c) = seg.chars().find(|c| matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*')) {
        return Err(PathError::InvalidChar(c));
    }
    if seg.ends_with('.') || seg.ends_with(' ') {
        return Err(PathError::TrailingDotOrSpace(seg.to_owned()));
    }
    let stem = seg.split('.').next().unwrap_or(seg).trim_end();
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Err(PathError::ReservedName(seg.to_owned()));
    }
    if seg.chars().count() > MAX_COMPONENT_CHARS {
        return Err(PathError::TooLong);
    }
    Ok(())
}

impl RelPath {
    /// Validates and normalizes a `/`-separated relative path.
    pub fn parse(raw: &str) -> Result<Self, PathError> {
        if raw.is_empty() {
            return Err(PathError::Empty);
        }
        let b = raw.as_bytes();
        let drive =
            b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b.get(2), None | Some(b'/' | b'\\'));
        if raw.starts_with('/') || raw.starts_with('\\') || drive {
            return Err(PathError::Absolute);
        }
        let norm: String = raw.nfc().collect();
        if norm.len() > MAX_PATH_BYTES {
            return Err(PathError::TooLong);
        }
        for seg in norm.split('/') {
            check_segment(seg)?;
        }
        Ok(Self(norm))
    }

    /// Converts an absolute on-disk path below `root` into a `RelPath`.
    pub fn from_fs(root: &Path, abs: &Path) -> Result<Self, PathError> {
        let rel = abs.strip_prefix(root).map_err(|_| PathError::OutsideRoot)?;
        let mut parts = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_str().ok_or(PathError::NotUnicode)?.to_owned()),
                Component::CurDir | Component::ParentDir => return Err(PathError::DotSegment),
                _ => return Err(PathError::Absolute),
            }
        }
        Self::parse(&parts.join("/"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Case-folded key; two paths with the same key collide on macOS/Windows.
    pub fn key(&self) -> String {
        self.0.to_lowercase()
    }

    /// The on-disk location below `root`.
    pub fn to_fs(&self, root: &Path) -> PathBuf {
        let mut p = root.to_path_buf();
        for seg in self.0.split('/') {
            p.push(seg);
        }
        p
    }

    pub fn file_name(&self) -> &str {
        self.0.rsplit('/').next().unwrap_or(&self.0)
    }

    pub fn parent(&self) -> Option<RelPath> {
        self.0.rsplit_once('/').map(|(p, _)| RelPath(p.to_owned()))
    }

    /// Path segments, e.g. `["src", "main.rs"]`.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// `src/app.ts` → `src/app (conflict from Mac 2026-10-26).ts`
    pub fn with_suffix(&self, suffix: &str) -> Result<RelPath, PathError> {
        let name = self.file_name();
        let new_name = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => format!("{stem} {suffix}.{ext}"),
            _ => format!("{name} {suffix}"),
        };
        match self.parent() {
            Some(p) => RelPath::parse(&format!("{}/{new_name}", p.0)),
            None => RelPath::parse(&new_name),
        }
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for p in ["a.txt", "src/main.rs", ".gitignore", "a b/c-d_e.f", "日本/ファイル.txt", ".env.example"] {
            assert!(RelPath::parse(p).is_ok(), "{p}");
        }
    }

    #[test]
    fn rejects_traversal_and_absolute() {
        for (p, e) in [
            ("", PathError::Empty),
            ("/etc/passwd", PathError::Absolute),
            ("C:/x", PathError::Absolute),
            ("\\\\server\\x", PathError::Absolute),
            ("../x", PathError::DotSegment),
            ("a/../../x", PathError::DotSegment),
            ("a/./b", PathError::DotSegment),
            ("a//b", PathError::EmptySegment),
            ("a/", PathError::EmptySegment),
            ("a\\..\\b", PathError::Backslash),
            ("a\0b", PathError::Control),
        ] {
            assert_eq!(RelPath::parse(p), Err(e), "{p:?}");
        }
    }

    #[test]
    fn flags_windows_unportable_names() {
        assert!(matches!(RelPath::parse("con"), Err(PathError::ReservedName(_))));
        assert!(matches!(RelPath::parse("src/NUL.txt"), Err(PathError::ReservedName(_))));
        assert!(matches!(RelPath::parse("lpt1.tar.gz"), Err(PathError::ReservedName(_))));
        assert!(RelPath::parse("console.log").is_ok());
        assert!(matches!(RelPath::parse("notes."), Err(PathError::TrailingDotOrSpace(_))));
        assert!(matches!(RelPath::parse("a:b.txt"), Err(PathError::InvalidChar(':'))));
        assert!(matches!(RelPath::parse("what?.md"), Err(PathError::InvalidChar('?'))));
        assert!(RelPath::parse(&"x".repeat(256)).unwrap_err().is_portability());
    }

    #[test]
    fn normalizes_to_nfc() {
        let nfd = "cafe\u{301}.txt";
        let p = RelPath::parse(nfd).unwrap();
        assert_eq!(p.as_str(), "caf\u{e9}.txt");
    }

    #[test]
    fn deserialization_validates() {
        assert!(serde_json::from_str::<RelPath>("\"../../.ssh/authorized_keys\"").is_err());
        assert_eq!(serde_json::from_str::<RelPath>("\"a/b\"").unwrap().as_str(), "a/b");
    }

    #[test]
    fn fs_round_trip() {
        let root = Path::new("/w");
        let p = RelPath::parse("src/lib.rs").unwrap();
        let abs = p.to_fs(root);
        assert_eq!(RelPath::from_fs(root, &abs).unwrap(), p);
        assert_eq!(RelPath::from_fs(root, Path::new("/elsewhere/x")), Err(PathError::OutsideRoot));
    }

    #[test]
    fn case_key_and_suffix() {
        assert_eq!(RelPath::parse("Src/Main.RS").unwrap().key(), "src/main.rs");
        let p = RelPath::parse("src/app.ts").unwrap();
        assert_eq!(p.with_suffix("(conflict)").unwrap().as_str(), "src/app (conflict).ts");
        assert_eq!(RelPath::parse("Makefile").unwrap().with_suffix("(x)").unwrap().as_str(), "Makefile (x)");
        assert_eq!(RelPath::parse(".env").unwrap().with_suffix("(x)").unwrap().as_str(), ".env (x)");
    }
}
