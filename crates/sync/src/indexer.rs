//! Turns the state of files on disk into recorded versions.
//!
//! Comparison uses `(size, mtime)` as a fast filter and the content hash as
//! the truth: a touched-but-unchanged file never creates a version, which is
//! also what stops sync loops (a file DevBridge just applied hashes the same
//! as its manifest entry).
//!
//! The store lock is never held while hashing. Before writing, each candidate
//! is re-checked against the manifest so a concurrent apply wins safely.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use devbridge_core::time::now_ms;
use devbridge_core::version::{child_ancestry, child_bases, content_key};
use devbridge_core::{DeviceId, RelPath, WorkspaceId};
use devbridge_fs::hash::hash_file;
use devbridge_fs::rules::Rules;
use devbridge_fs::scan::{IssueKind, ScanEntry, ScanIssue, ScanResult, scan_from, stat_entry};
use devbridge_store::{Head, ManifestEntry, Store, VersionRow};

use crate::{SyncError, lock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    Path(RelPath),
}

#[derive(Debug, Default)]
pub struct IndexReport {
    pub versions: Vec<VersionRow>,
    pub issues: usize,
    pub excluded: u64,
    pub gitignore_changed: bool,
}

pub struct Indexer<'a> {
    pub store: &'a Mutex<Store>,
    pub ws: WorkspaceId,
    pub root: &'a Path,
    pub rules: &'a Rules,
    pub device: DeviceId,
}

enum Candidate {
    Upsert { entry: ScanEntry, hash: String, parent: Option<ManifestEntry> },
    Delete { parent: ManifestEntry },
    Touch { entry: ScanEntry },
    Forget { path: RelPath },
}

fn issue_tuple(i: &ScanIssue) -> (String, String, String) {
    let (kind, detail) = match &i.kind {
        IssueKind::Unportable(d) => ("unportable_path", d.clone()),
        IssueKind::CaseCollision { other } => ("case_collision", format!("Differs only by letter case from {other}")),
        IssueKind::Symlink => ("symlink", "Symbolic links aren't synced".into()),
        IssueKind::Unreadable(d) => ("unreadable", d.clone()),
    };
    (i.path.clone(), kind.into(), detail)
}

impl Indexer<'_> {
    fn disk_state(&self, scope: &Scope) -> ScanResult {
        match scope {
            Scope::All => scan_from(self.root, None, self.rules),
            Scope::Path(p) => {
                let abs = p.to_fs(self.root);
                match std::fs::symlink_metadata(&abs) {
                    Ok(m) if m.is_dir() => scan_from(self.root, Some(p), self.rules),
                    Ok(m) if m.file_type().is_symlink() => {
                        let mut r = ScanResult::default();
                        if self.rules.is_excluded(p, false).is_none() {
                            r.issues.push(ScanIssue { path: p.to_string(), kind: IssueKind::Symlink });
                        }
                        r
                    }
                    Ok(_) => {
                        let mut r = ScanResult::default();
                        if self.rules.is_excluded(p, false).is_some() {
                            r.excluded = 1;
                        } else if let Ok(Some(e)) = stat_entry(self.root, p) {
                            r.files.push(e);
                        }
                        r
                    }
                    Err(_) => ScanResult::default(),
                }
            }
        }
    }

    pub fn refresh(&self, scope: Scope) -> Result<IndexReport, SyncError> {
        let disk = self.disk_state(&scope);
        let (manifest, issue_paths): (HashMap<RelPath, ManifestEntry>, HashSet<String>) = {
            let s = lock(self.store);
            let entries = match &scope {
                Scope::All => s.manifest_all(self.ws)?,
                Scope::Path(p) => s.manifest_under(self.ws, p)?,
            };
            let mut issues: HashSet<String> = s.issue_paths(self.ws)?.into_iter().collect();
            issues.extend(disk.issues.iter().map(|i| i.path.clone()));
            (entries.into_iter().map(|e| (e.path.clone(), e)).collect(), issues)
        };

        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for entry in disk.files {
            seen.insert(entry.path.clone());
            let m = manifest.get(&entry.path);
            let live = m.filter(|m| !m.deleted);
            if let Some(m) = live
                && m.size == entry.size
                && m.mtime_ns == Some(entry.mtime_ns)
                && modes_equal(m.mode, entry.mode)
            {
                continue;
            }
            let hash = match hash_file(&entry.path.to_fs(self.root)) {
                Ok((h, _)) => h,
                // Vanished or locked mid-scan: the next event/reconcile handles it.
                Err(e) => {
                    tracing::debug!(path = %entry.path, error = %e, "skipping unreadable file");
                    continue;
                }
            };
            match live {
                Some(m) if m.content_hash.as_deref() == Some(hash.as_str()) && modes_equal(m.mode, entry.mode) => {
                    candidates.push(Candidate::Touch { entry })
                }
                _ => candidates.push(Candidate::Upsert { entry, hash, parent: m.cloned() }),
            }
        }
        for (path, m) in &manifest {
            if m.deleted || seen.contains(path) || issue_paths.contains(path.as_str()) {
                continue;
            }
            if self.rules.is_excluded(path, false).is_some() {
                // Newly excluded: forget it here, never delete it elsewhere.
                candidates.push(Candidate::Forget { path: path.clone() });
                continue;
            }
            if std::fs::symlink_metadata(path.to_fs(self.root)).is_ok() {
                continue; // still there (e.g. became unreadable); don't tombstone
            }
            candidates.push(Candidate::Delete { parent: m.clone() });
        }

        let mut report = IndexReport { issues: disk.issues.len(), excluded: disk.excluded, ..Default::default() };
        let s = lock(self.store);
        for c in candidates {
            match c {
                Candidate::Touch { entry } => s.manifest_touch(self.ws, &entry.path, entry.size, entry.mtime_ns)?,
                Candidate::Forget { path } => s.manifest_forget(self.ws, &path)?,
                Candidate::Upsert { entry, hash, parent } => {
                    if !head_unchanged(&s, self.ws, &entry.path, parent.as_ref())? {
                        continue;
                    }
                    let (ancestry, base_hashes) = self.lineage_of(&s, parent.as_ref())?;
                    // Windows has no permission bits: carry the last known mode.
                    let mode = entry.mode.or(parent.as_ref().and_then(|p| p.mode));
                    let v = VersionRow {
                        version_id: ulid::Ulid::generate().to_string(),
                        workspace_id: self.ws,
                        path: entry.path.clone(),
                        ancestry,
                        base_hashes,
                        content_hash: Some(hash),
                        size: entry.size,
                        mode,
                        deleted: false,
                        origin_device_id: self.device,
                        created_at: now_ms(),
                    };
                    s.record_version(&v, Head::Set, Some(entry.mtime_ns), None)?;
                    report.gitignore_changed |= v.path.file_name() == ".gitignore";
                    report.versions.push(v);
                }
                Candidate::Delete { parent } => {
                    if !head_unchanged(&s, self.ws, &parent.path, Some(&parent))? {
                        continue;
                    }
                    let (ancestry, base_hashes) = self.lineage_of(&s, Some(&parent))?;
                    let v = VersionRow {
                        version_id: ulid::Ulid::generate().to_string(),
                        workspace_id: self.ws,
                        path: parent.path.clone(),
                        ancestry,
                        base_hashes,
                        content_hash: None,
                        size: 0,
                        mode: parent.mode,
                        deleted: true,
                        origin_device_id: self.device,
                        created_at: now_ms(),
                    };
                    s.record_version(&v, Head::Set, None, None)?;
                    report.gitignore_changed |= v.path.file_name() == ".gitignore";
                    report.versions.push(v);
                }
            }
        }
        let issues: Vec<_> = disk.issues.iter().map(issue_tuple).collect();
        match &scope {
            Scope::All => s.issues_replace(self.ws, None, &issues)?,
            Scope::Path(p) => s.issues_replace(self.ws, Some(p), &issues)?,
        }
        Ok(report)
    }

    /// Manifest entries in the same folders as `paths` whose files no longer
    /// exist. File watchers sometimes report only the new name of a rename;
    /// this catches the old one without waiting for a full reconcile.
    pub fn vanished_siblings(&self, paths: &[RelPath]) -> Result<Vec<RelPath>, SyncError> {
        let mut dirs: Vec<Option<RelPath>> = paths.iter().map(|p| p.parent()).collect();
        dirs.sort();
        dirs.dedup();
        let mut out = Vec::new();
        let s = lock(self.store);
        for dir in dirs {
            let entries = match &dir {
                Some(d) => s.manifest_under(self.ws, d)?,
                None => s.manifest_all(self.ws)?,
            };
            for e in entries {
                if e.deleted || e.path.parent() != dir {
                    continue;
                }
                if std::fs::symlink_metadata(e.path.to_fs(self.root)).is_err() && !paths.contains(&e.path) {
                    out.push(e.path);
                }
            }
        }
        Ok(out)
    }

    /// Ancestry and ancestor content keys for a child of `parent`.
    fn lineage_of(&self, s: &Store, parent: Option<&ManifestEntry>) -> Result<(Vec<String>, Vec<String>), SyncError> {
        Ok(match parent {
            None => (Vec::new(), Vec::new()),
            Some(p) => {
                let (anc, bases) =
                    s.version_get(&p.version_id)?.map(|v| (v.ancestry, v.base_hashes)).unwrap_or_default();
                let key = content_key(p.content_hash.as_deref(), p.deleted);
                (child_ancestry(&[(p.version_id.as_str(), &anc)]), child_bases(&[(key, &bases)]))
            }
        })
    }
}

fn modes_equal(a: Option<u32>, b: Option<u32>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true, // unknown on one side (Windows): not a change
    }
}

/// True if the manifest head for `path` is still what the scan compared against.
fn head_unchanged(
    s: &Store,
    ws: WorkspaceId,
    path: &RelPath,
    expected: Option<&ManifestEntry>,
) -> Result<bool, SyncError> {
    let now = s.manifest_get(ws, path)?;
    Ok(now.as_ref().map(|m| &m.version_id) == expected.map(|m| &m.version_id))
}

/// Order-independent digest of a manifest, used to skip reconciliation when
/// two devices already agree.
pub fn manifest_digest(entries: &[ManifestEntry]) -> String {
    let mut lines: Vec<String> = entries.iter().map(|e| format!("{}\0{}", e.path, e.version_id)).collect();
    lines.sort();
    let mut h = blake3::Hasher::new();
    for l in lines {
        h.update(l.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use devbridge_core::{OsKind, SyncState, Workspace, WorkspaceColor, WorkspaceIcon, WorkspaceMode};
    use devbridge_fs::rules::RuleConfig;

    struct Fx {
        _t: tempfile::TempDir,
        root: std::path::PathBuf,
        store: Mutex<Store>,
        ws: WorkspaceId,
        dev: DeviceId,
        rules: Rules,
    }

    fn fx() -> Fx {
        let t = tempfile::tempdir().unwrap();
        let root = devbridge_core::fsutil::canonicalize(t.path()).unwrap();
        let store = Store::open_in_memory().unwrap();
        let dev = store.ensure_local_device("Mac", OsKind::MacOs, None).unwrap().id;
        let ws = WorkspaceId::new();
        store
            .insert_workspace(&Workspace {
                id: ws,
                name: "W".into(),
                root_path: root.display().to_string(),
                color: WorkspaceColor::Blue,
                icon: WorkspaceIcon::Code,
                mode: WorkspaceMode::Synced,
                sync_state: SyncState::LocalOnly,
                active_device_id: None,
                last_synced_at: None,
                created_at: 0,
                updated_at: 0,
            })
            .unwrap();
        let rules = Rules::new(&root, &RuleConfig::default());
        Fx { _t: t, root, store: Mutex::new(store), ws, dev, rules }
    }

    impl Fx {
        fn idx(&self) -> Indexer<'_> {
            Indexer { store: &self.store, ws: self.ws, root: &self.root, rules: &self.rules, device: self.dev }
        }
        fn write(&self, p: &str, d: &str) {
            let f = self.root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, d).unwrap();
        }
        fn head(&self, p: &str) -> Option<ManifestEntry> {
            self.store.lock().unwrap().manifest_get(self.ws, &RelPath::parse(p).unwrap()).unwrap()
        }
    }

    #[test]
    fn creates_versions_with_ancestry_and_tombstones() {
        let f = fx();
        f.write("a.txt", "1");
        f.write("src/b.rs", "b");
        f.write(".env", "secret");
        let r = f.idx().refresh(Scope::All).unwrap();
        assert_eq!(r.versions.len(), 2, "secret file excluded");
        let v1 = f.head("a.txt").unwrap();

        // Unchanged rescan creates nothing.
        assert!(f.idx().refresh(Scope::All).unwrap().versions.is_empty());

        f.write("a.txt", "22");
        let r = f.idx().refresh(Scope::Path(RelPath::parse("a.txt").unwrap())).unwrap();
        assert_eq!(r.versions.len(), 1);
        assert_eq!(r.versions[0].ancestry, vec![v1.version_id.clone()]);

        std::fs::remove_dir_all(f.root.join("src")).unwrap();
        let r = f.idx().refresh(Scope::Path(RelPath::parse("src").unwrap())).unwrap();
        assert_eq!(r.versions.len(), 1);
        assert!(r.versions[0].deleted);
        assert!(f.head("src/b.rs").unwrap().deleted);

        // Re-created after deletion: parent is the tombstone.
        f.write("src/b.rs", "b2");
        let r = f.idx().refresh(Scope::All).unwrap();
        assert_eq!(
            r.versions[0].ancestry[0],
            f.store.lock().unwrap().version_get(&r.versions[0].ancestry[0]).unwrap().unwrap().version_id
        );
        assert!(!f.head("src/b.rs").unwrap().deleted);
    }

    #[test]
    fn touch_without_content_change_is_not_a_version() {
        let f = fx();
        f.write("a.txt", "same");
        f.idx().refresh(Scope::All).unwrap();
        let before = f.head("a.txt").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        f.write("a.txt", "same"); // new mtime, same bytes
        assert!(f.idx().refresh(Scope::All).unwrap().versions.is_empty());
        let after = f.head("a.txt").unwrap();
        assert_eq!(before.version_id, after.version_id);
        assert_ne!(before.mtime_ns, after.mtime_ns, "stat cache refreshed");
    }

    #[test]
    fn newly_excluded_files_are_forgotten_not_deleted() {
        let mut f = fx();
        f.write("build.log", "x");
        f.idx().refresh(Scope::All).unwrap();
        assert!(f.head("build.log").is_some());
        f.rules = Rules::new(&f.root, &RuleConfig { user_patterns: vec!["*.log".into()], ..Default::default() });
        let r = f.idx().refresh(Scope::All).unwrap();
        assert!(r.versions.is_empty(), "no tombstone: peers keep their copy");
        assert!(f.head("build.log").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn records_issues_and_never_tombstones_their_paths() {
        let f = fx();
        f.write("ok.txt", "x");
        f.write("bad?.txt", "x");
        let r = f.idx().refresh(Scope::All).unwrap();
        assert_eq!(r.issues, 1);
        let issues = f.store.lock().unwrap().issues_list(Some(f.ws)).unwrap();
        assert_eq!(issues[0].kind, "unportable_path");
        assert_eq!(r.versions.len(), 1);
    }

    #[test]
    fn gitignore_change_is_reported() {
        let f = fx();
        f.write(".gitignore", "*.tmp\n");
        assert!(f.idx().refresh(Scope::All).unwrap().gitignore_changed);
    }

    #[test]
    fn digest_is_order_independent() {
        let e = |p: &str, v: &str| ManifestEntry {
            path: RelPath::parse(p).unwrap(),
            content_hash: None,
            size: 0,
            mode: None,
            mtime_ns: None,
            version_id: v.into(),
            deleted: false,
        };
        let a = manifest_digest(&[e("a", "1"), e("b", "2")]);
        assert_eq!(a, manifest_digest(&[e("b", "2"), e("a", "1")]));
        assert_ne!(a, manifest_digest(&[e("a", "1"), e("b", "3")]));
    }
}
