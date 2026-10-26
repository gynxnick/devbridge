//! Walks a workspace and reports syncable files plus anything that can't be
//! synced (unportable names, case collisions, symlinks, unreadable entries).
//! Symlinks are never followed, so a link can't pull files from outside the
//! workspace into sync.

use std::collections::HashMap;
use std::fs::Metadata;
use std::path::Path;
use std::time::UNIX_EPOCH;

use devbridge_core::{PathError, RelPath};

use crate::rules::Rules;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanEntry {
    pub path: RelPath,
    pub size: u64,
    pub mtime_ns: i64,
    pub mode: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    /// Valid locally but can't exist on Windows (or is too long).
    Unportable(String),
    /// Differs from another path only by letter case.
    CaseCollision {
        other: String,
    },
    Symlink,
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanIssue {
    /// Display form, `/`-separated (may not be a valid `RelPath`).
    pub path: String,
    pub kind: IssueKind,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub files: Vec<ScanEntry>,
    pub issues: Vec<ScanIssue>,
    pub excluded: u64,
}

pub fn mtime_ns(m: &Metadata) -> i64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

#[cfg(unix)]
pub fn file_mode(m: &Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
pub fn file_mode(_m: &Metadata) -> Option<u32> {
    None
}

/// Builds an entry for one path, or `None` if it's not a regular file.
pub fn stat_entry(root: &Path, rel: &RelPath) -> std::io::Result<Option<ScanEntry>> {
    let m = match std::fs::symlink_metadata(rel.to_fs(root)) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !m.file_type().is_file() {
        return Ok(None);
    }
    Ok(Some(ScanEntry { path: rel.clone(), size: m.len(), mtime_ns: mtime_ns(&m), mode: file_mode(&m) }))
}

fn display(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

pub fn scan(root: &Path, rules: &Rules) -> ScanResult {
    scan_from(root, None, rules)
}

/// Scans the subtree at `start` (a directory below `root`), or everything.
pub fn scan_from(root: &Path, start: Option<&RelPath>, rules: &Rules) -> ScanResult {
    let mut out = ScanResult::default();
    if let Some(s) = start
        && rules.is_excluded(s, true).is_some()
    {
        out.excluded += 1;
        return out;
    }
    let mut stack = vec![start.map(|s| s.to_fs(root)).unwrap_or_else(|| root.to_path_buf())];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                out.issues.push(ScanIssue { path: display(root, &dir), kind: IssueKind::Unreadable(e.to_string()) });
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    out.issues
                        .push(ScanIssue { path: display(root, &dir), kind: IssueKind::Unreadable(e.to_string()) });
                    continue;
                }
            };
            let abs = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            let rel = match RelPath::from_fs(root, &abs) {
                Ok(r) => r,
                Err(e) => {
                    // Check exclusions on the raw name first so e.g. an
                    // unportable file inside node_modules isn't reported.
                    let raw = display(root, &abs);
                    let excluded_parent = raw
                        .rsplit_once('/')
                        .and_then(|(p, _)| RelPath::parse(p).ok())
                        .is_some_and(|p| rules.is_excluded(&p, true).is_some());
                    if excluded_parent {
                        out.excluded += 1;
                    } else if e.is_portability() {
                        out.issues.push(ScanIssue { path: raw, kind: IssueKind::Unportable(e.to_string()) });
                    } else if e != PathError::OutsideRoot {
                        out.issues.push(ScanIssue { path: raw, kind: IssueKind::Unreadable(e.to_string()) });
                    }
                    continue;
                }
            };
            if rules.is_excluded(&rel, ft.is_dir()).is_some() {
                out.excluded += 1;
                continue;
            }
            if ft.is_symlink() {
                out.issues.push(ScanIssue { path: rel.to_string(), kind: IssueKind::Symlink });
            } else if ft.is_dir() {
                stack.push(abs);
            } else if ft.is_file() {
                match entry.metadata() {
                    Ok(m) => out.files.push(ScanEntry {
                        path: rel,
                        size: m.len(),
                        mtime_ns: mtime_ns(&m),
                        mode: file_mode(&m),
                    }),
                    Err(e) => {
                        out.issues.push(ScanIssue { path: rel.to_string(), kind: IssueKind::Unreadable(e.to_string()) })
                    }
                }
            }
        }
    }
    out.files.sort_by(|a, b| a.path.cmp(&b.path));
    remove_case_collisions(&mut out);
    out
}

/// Files whose paths differ only by case can't coexist on macOS/Windows
/// defaults. Neither is synced; both are reported.
fn remove_case_collisions(out: &mut ScanResult) {
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in out.files.iter().enumerate() {
        by_key.entry(f.path.key()).or_default().push(i);
    }
    let mut drop = vec![false; out.files.len()];
    for idxs in by_key.values().filter(|v| v.len() > 1) {
        for &i in idxs {
            drop[i] = true;
            let other = idxs.iter().find(|&&j| j != i).map(|&j| out.files[j].path.to_string()).unwrap_or_default();
            out.issues
                .push(ScanIssue { path: out.files[i].path.to_string(), kind: IssueKind::CaseCollision { other } });
        }
    }
    let mut i = 0;
    out.files.retain(|_| {
        let keep = !drop[i];
        i += 1;
        keep
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::RuleConfig;

    fn write(root: &Path, p: &str, data: &str) {
        let f = root.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, data).unwrap();
    }

    #[test]
    fn scans_files_and_skips_exclusions() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        write(root, "src/main.rs", "fn main(){}");
        write(root, "README.md", "hi");
        write(root, ".env", "SECRET=1");
        write(root, "node_modules/x/index.js", "x");
        write(root, ".git/HEAD", "ref");
        let rules = Rules::new(root, &RuleConfig::default());
        let s = scan(root, &rules);
        let paths: Vec<_> = s.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["README.md", "src/main.rs"]);
        assert!(s.excluded >= 3);
        assert!(s.issues.is_empty(), "{:?}", s.issues);
        assert_eq!(s.files[0].size, 2);
    }

    #[cfg(unix)]
    #[test]
    fn reports_symlinks_and_unportable_names_without_following() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "secret.txt", "s");
        let root = d.path();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        write(root, "what?.txt", "x");
        write(root, "aux.c", "x");
        let s = scan(root, &Rules::new(root, &RuleConfig::default()));
        assert!(s.files.is_empty(), "{:?}", s.files);
        assert!(s.issues.iter().any(|i| i.path == "link" && i.kind == IssueKind::Symlink));
        assert_eq!(s.issues.iter().filter(|i| matches!(i.kind, IssueKind::Unportable(_))).count(), 2);
    }

    #[test]
    fn case_collisions_are_reported_not_synced() {
        let mut out = ScanResult::default();
        for p in ["a/Readme.md", "a/README.md", "b.txt"] {
            out.files.push(ScanEntry { path: RelPath::parse(p).unwrap(), size: 0, mtime_ns: 0, mode: None });
        }
        remove_case_collisions(&mut out);
        assert_eq!(out.files.len(), 1);
        assert_eq!(out.issues.len(), 2);
    }
}
