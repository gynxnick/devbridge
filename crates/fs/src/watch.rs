//! Native filesystem notifications, debounced. Events are treated as hints:
//! the indexer re-reads every reported path, and a periodic full reconcile
//! catches anything the OS failed to report.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// These absolute paths (files or directories) may have changed.
    Paths(Vec<PathBuf>),
    /// Events were lost (overflow or watcher error): rescan everything.
    Rescan,
}

pub struct WorkspaceWatcher {
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
}

pub const DEBOUNCE: Duration = Duration::from_millis(300);

impl WorkspaceWatcher {
    /// Starts watching `root` recursively. `sink` is called from the
    /// debouncer's thread; it must not block for long.
    pub fn start(root: &Path, sink: impl Fn(WatchEvent) + Send + 'static) -> notify::Result<Self> {
        let root_owned = root.to_path_buf();
        let mut debouncer = new_debouncer(DEBOUNCE, None, move |res: DebounceEventResult| match res {
            Ok(events) => {
                if events.iter().any(|e| e.need_rescan()) {
                    sink(WatchEvent::Rescan);
                    return;
                }
                let mut paths: Vec<PathBuf> = events
                    .into_iter()
                    .flat_map(|e| e.event.paths)
                    .filter(|p| p.starts_with(&root_owned) && !is_internal(&root_owned, p))
                    .collect();
                paths.sort();
                paths.dedup();
                if !paths.is_empty() {
                    sink(WatchEvent::Paths(paths));
                }
            }
            Err(errors) => {
                tracing::warn!(count = errors.len(), "file watcher reported errors; scheduling rescan");
                sink(WatchEvent::Rescan);
            }
        })?;
        debouncer.watch(root, RecursiveMode::Recursive)?;
        Ok(Self { _debouncer: debouncer })
    }
}

/// `.git` internals and DevBridge staging change constantly and are never synced.
fn is_internal(root: &Path, p: &Path) -> bool {
    let rel = p.strip_prefix(root).unwrap_or(p);
    matches!(rel.components().next().and_then(|c| c.as_os_str().to_str()), Some(".git" | ".devbridge"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn reports_changes_and_ignores_git_internals() {
        let d = tempfile::tempdir().unwrap();
        let root = devbridge_core::fsutil::canonicalize(d.path()).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let (tx, rx) = mpsc::channel();
        let _w = WorkspaceWatcher::start(&root, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(root.join(".git/index"), b"x").unwrap();
        std::fs::write(root.join("a.txt"), b"x").unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            if let Ok(WatchEvent::Paths(p)) = rx.recv_timeout(Duration::from_millis(200)) {
                seen.extend(p);
                if seen.iter().any(|p| p.ends_with("a.txt")) {
                    break;
                }
            }
        }
        assert!(seen.iter().any(|p| p.ends_with("a.txt")), "{seen:?}");
        assert!(!seen.iter().any(|p| p.starts_with(root.join(".git"))), "{seen:?}");
    }
}
