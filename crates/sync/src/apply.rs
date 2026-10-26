//! Applying incoming changes and resolving conflicts. Every function here is
//! blocking and is called from a workspace's actor, which serializes all work
//! for that workspace (so local indexing and incoming applies never race).
//!
//! Invariants:
//! - A local file is only replaced or deleted if its current content matches
//!   the version the decision was based on (`Expect`); otherwise the local
//!   edit is indexed first and the change is re-evaluated (usually a conflict).
//! - Overwritten or deleted content is copied to the recovery store first.
//! - In a conflict nothing on disk changes; the remote content is kept in the
//!   recovery store until the user decides.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use devbridge_core::protocol::{Ack, AckResult, Change, ChangeOp};
use devbridge_core::time::now_ms;
use devbridge_core::version::{
    Decision, IncomingVersion, LocalHead, child_ancestry, child_bases, classify, content_key,
};
use devbridge_core::{DeviceId, RelPath, WorkspaceId};
use devbridge_fs::apply::{ApplyError, Expect, commit_copy, commit_file, remove_file};
use devbridge_fs::hash::{hash_file, is_valid_hash};
use devbridge_fs::recovery::RecoveryStore;
use devbridge_fs::rules::Rules;
use devbridge_fs::scan::stat_entry;
use devbridge_store::{ConflictRecord, Head, ManifestEntry, Store, TransferJob, VersionRow};

use crate::indexer::{IndexReport, Indexer, Scope};
use crate::{SyncError, lock};

/// Files larger than this aren't copied to the (expiring) overwrite history.
/// Conflict copies are always kept regardless of size.
const HISTORY_COPY_LIMIT: u64 = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Ctx {
    pub store: Arc<Mutex<Store>>,
    pub ws: WorkspaceId,
    pub root: PathBuf,
    pub rules: Arc<Rules>,
    pub device: DeviceId,
    pub recovery: Arc<RecoveryStore>,
    pub staging: PathBuf,
}

#[derive(Debug, Default)]
pub struct Outcome {
    pub acks: Vec<Ack>,
    pub transfers: Vec<TransferJob>,
    pub conflicts: Vec<RelPath>,
    pub applied: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    KeepLocal,
    KeepRemote,
    /// Keep local at the original path and write the remote version next to
    /// it with this suffix, e.g. `(conflict from PC 2026-10-26)`.
    SaveBoth {
        suffix: String,
    },
    Merged(Vec<u8>),
}

/// An incoming version, from a `Change` or a completed transfer.
#[derive(Debug, Clone)]
struct Inc {
    change_id: String,
    origin: DeviceId,
    path: RelPath,
    version_id: String,
    ancestry: Vec<String>,
    base_hashes: Vec<String>,
    hash: Option<String>,
    size: u64,
    mode: Option<u32>,
    deleted: bool,
}

impl Inc {
    fn from_change(c: &Change) -> Self {
        Self {
            change_id: c.change_id.clone(),
            origin: c.origin_device,
            path: c.path.clone(),
            version_id: c.version_id.clone(),
            ancestry: c.ancestry.clone(),
            base_hashes: c.base_hashes.clone(),
            hash: c.hash.clone(),
            size: c.size,
            mode: c.mode,
            deleted: c.op == ChangeOp::Delete,
        }
    }

    fn from_job(j: &TransferJob) -> Self {
        Self {
            change_id: j.change_id.clone(),
            origin: j.origin_device_id,
            path: j.path.clone(),
            version_id: j.version_id.clone(),
            ancestry: j.ancestry.clone(),
            base_hashes: j.base_hashes.clone(),
            hash: Some(j.content_hash.clone()),
            size: j.total_size,
            mode: j.mode,
            deleted: false,
        }
    }

    fn version(&self, ws: WorkspaceId) -> VersionRow {
        VersionRow {
            version_id: self.version_id.clone(),
            workspace_id: ws,
            path: self.path.clone(),
            ancestry: self.ancestry.clone(),
            base_hashes: self.base_hashes.clone(),
            content_hash: self.hash.clone(),
            size: if self.deleted { 0 } else { self.size },
            mode: self.mode,
            deleted: self.deleted,
            origin_device_id: self.origin,
            created_at: now_ms(),
        }
    }
}

fn ack(change_id: &str, result: AckResult, reason: Option<String>) -> Ack {
    Ack { change_id: change_id.to_owned(), result, reason }
}

impl Ctx {
    pub fn indexer(&self) -> Indexer<'_> {
        Indexer { store: &self.store, ws: self.ws, root: &self.root, rules: &self.rules, device: self.device }
    }

    pub fn index(&self, scope: Scope) -> Result<IndexReport, SyncError> {
        self.indexer().refresh(scope)
    }

    fn head(&self, path: &RelPath) -> Result<Option<ManifestEntry>, SyncError> {
        Ok(lock(&self.store).manifest_get(self.ws, path)?)
    }

    /// A recorded version's (ancestry, ancestor content keys).
    fn lineage_of(&self, version_id: &str) -> Result<(Vec<String>, Vec<String>), SyncError> {
        Ok(lock(&self.store).version_get(version_id)?.map(|v| (v.ancestry, v.base_hashes)).unwrap_or_default())
    }

    fn staging_file(&self) -> Result<PathBuf, SyncError> {
        std::fs::create_dir_all(&self.staging)?;
        Ok(self.staging.join(format!("local-{}", ulid::Ulid::generate())))
    }

    /// Path of the staging file for a transfer.
    pub fn transfer_path(&self, transfer_id: &str) -> PathBuf {
        self.staging.join(transfer_id)
    }

    /// Copies the current local file into the recovery store.
    fn save_local(
        &self,
        path: &RelPath,
        reason: &str,
        conflict_id: Option<&str>,
        pinned: bool,
    ) -> Result<(), SyncError> {
        let abs = path.to_fs(&self.root);
        let Ok(meta) = std::fs::symlink_metadata(&abs) else { return Ok(()) };
        if !meta.is_file() || (!pinned && meta.len() > HISTORY_COPY_LIMIT) {
            return Ok(());
        }
        let (hash, size) = self.recovery.put_file(&abs)?;
        lock(&self.store).recovery_add(self.ws, path, &hash, size, reason, conflict_id, pinned)?;
        Ok(())
    }

    /// Finds content with `hash` already on this device (another path in the
    /// workspace — e.g. a rename — or the recovery store) and stages a copy.
    fn local_content(&self, hash: &str) -> Result<Option<PathBuf>, SyncError> {
        let src = {
            let s = lock(&self.store);
            s.manifest_find_hash(self.ws, hash)?.map(|p| p.to_fs(&self.root))
        };
        let src = src.or_else(|| self.recovery.blob_path(hash).filter(|p| p.is_file()));
        let Some(src) = src else { return Ok(None) };
        let staged = self.staging_file()?;
        if std::fs::copy(&src, &staged).is_err() {
            return Ok(None);
        }
        // The source may have changed since it was indexed: verify.
        if hash_file(&staged)?.0 != hash {
            let _ = std::fs::remove_file(&staged);
            return Ok(None);
        }
        Ok(Some(staged))
    }

    fn record(&self, v: &VersionRow, head: Head, except: Option<DeviceId>) -> Result<(), SyncError> {
        let mtime =
            if head == Head::Set && !v.deleted { stat_entry(&self.root, &v.path)?.map(|e| e.mtime_ns) } else { None };
        lock(&self.store).record_version(v, head, mtime, except)?;
        Ok(())
    }

    /// Marks open conflicts for `path` resolved when a version that includes
    /// their remote side arrives (the other device resolved them).
    fn resolve_elsewhere(&self, path: &RelPath, ancestry: &[String], version_id: &str) -> Result<(), SyncError> {
        let s = lock(&self.store);
        for c in s.conflicts_open_for_path(self.ws, path)? {
            if c.remote_version_id == version_id || ancestry.contains(&c.remote_version_id) {
                s.conflict_resolve(&c.id, "resolved_elsewhere")?;
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Incoming changes
// ---------------------------------------------------------------------------

pub fn process_incoming(ctx: &Ctx, peer: DeviceId, changes: &[Change]) -> Outcome {
    let mut out = Outcome::default();
    for c in changes {
        let a = match process_one(ctx, peer, c, &mut out) {
            Ok(a) => a,
            Err(e) => {
                tracing::warn!(path = %c.path, error = %e, "failed to process incoming change");
                ack(&c.change_id, AckResult::Rejected, Some(e.to_string()))
            }
        };
        out.acks.push(a);
    }
    out
}

fn process_one(ctx: &Ctx, peer: DeviceId, c: &Change, out: &mut Outcome) -> Result<Ack, SyncError> {
    if let Some(ex) = ctx.rules.is_excluded(&c.path, false) {
        lock(&ctx.store).issue_add(
            ctx.ws,
            c.path.as_str(),
            "rejected",
            &format!("Not accepted from the other device: excluded here ({ex:?})"),
        )?;
        return Ok(ack(&c.change_id, AckResult::Rejected, Some("excluded by the receiver's rules".into())));
    }
    if c.op == ChangeOp::Upsert && !c.hash.as_deref().is_some_and(is_valid_hash) {
        return Ok(ack(&c.change_id, AckResult::Rejected, Some("invalid content hash".into())));
    }
    {
        let s = lock(&ctx.store);
        if s.version_known(&c.version_id)? {
            return Ok(ack(&c.change_id, AckResult::Duplicate, None));
        }
        if s.transfer_for_version(&c.version_id)?.is_some() {
            return Ok(ack(&c.change_id, AckResult::Pending, None));
        }
        if let Some(other) = s.manifest_case_variants(ctx.ws, &c.path)?.first() {
            let detail = format!("Differs only by letter case from {}", other.path);
            s.issue_add(ctx.ws, c.path.as_str(), "case_collision", &detail)?;
            return Ok(ack(&c.change_id, AckResult::Rejected, Some(detail)));
        }
    }
    // A case-insensitive disk can report a differently-cased existing file.
    if exists_with_other_case(&ctx.root, &c.path) {
        let detail = "A file with the same name but different letter case exists here".to_string();
        lock(&ctx.store).issue_add(ctx.ws, c.path.as_str(), "case_collision", &detail)?;
        return Ok(ack(&c.change_id, AckResult::Rejected, Some(detail)));
    }
    // Capture any local edit the watcher hasn't reported yet.
    ctx.index(Scope::Path(c.path.clone()))?;
    let inc = Inc::from_change(c);
    let result = decide(ctx, peer, &inc, None, out, 0)?;
    Ok(ack(&c.change_id, result, None))
}

/// True if `rel` resolves on disk but its final component's exact name isn't
/// present in the parent directory (case-insensitive filesystem collision).
fn exists_with_other_case(root: &Path, rel: &RelPath) -> bool {
    let abs = rel.to_fs(root);
    if std::fs::symlink_metadata(&abs).is_err() {
        return false;
    }
    let Some(parent) = abs.parent() else { return false };
    let Ok(rd) = std::fs::read_dir(parent) else { return false };
    !rd.flatten().any(|e| e.file_name().to_str() == Some(rel.file_name()))
}

fn local_head(m: &Option<ManifestEntry>) -> Option<LocalHead> {
    m.as_ref().map(|m| LocalHead {
        version_id: m.version_id.clone(),
        content_hash: m.content_hash.clone(),
        deleted: m.deleted,
    })
}

fn expect_for(m: &Option<ManifestEntry>) -> Expect {
    match m {
        Some(m) if !m.deleted => Expect::Hash(m.content_hash.clone().unwrap_or_default()),
        _ => Expect::Absent,
    }
}

fn existing_mode(root: &Path, rel: &RelPath) -> Option<u32> {
    std::fs::metadata(rel.to_fs(root)).ok().and_then(|m| devbridge_fs::scan::file_mode(&m))
}

/// Core decision + application for one incoming version. `staged` holds its
/// verified content when it arrived by transfer.
fn decide(
    ctx: &Ctx,
    peer: DeviceId,
    inc: &Inc,
    staged: Option<&Path>,
    out: &mut Outcome,
    depth: u8,
) -> Result<AckResult, SyncError> {
    if depth > 2 {
        return Err(SyncError::Other("file keeps changing locally; will retry".into()));
    }
    let head = ctx.head(&inc.path)?;
    let known = lock(&ctx.store).version_known(&inc.version_id)?;
    let incoming = IncomingVersion {
        version_id: &inc.version_id,
        ancestry: &inc.ancestry,
        content_hash: inc.hash.as_deref(),
        deleted: inc.deleted,
        base_hashes: &inc.base_hashes,
    };
    let decision = classify(local_head(&head).as_ref(), &incoming, known);
    tracing::debug!(path = %inc.path, version = %inc.version_id, ?decision, staged = staged.is_some(), "incoming version");
    let discard = |p: Option<&Path>| {
        if let Some(p) = p {
            let _ = std::fs::remove_file(p);
        }
    };
    match decision {
        Decision::Duplicate => {
            discard(staged);
            Ok(AckResult::Duplicate)
        }
        Decision::Converge { head: winner } => {
            discard(staged);
            ctx.record(&inc.version(ctx.ws), Head::HistoryOnly, None)?;
            if winner == inc.version_id {
                lock(&ctx.store).manifest_set_head(ctx.ws, &inc.path, &inc.version_id)?;
            }
            ctx.resolve_elsewhere(&inc.path, &inc.ancestry, &inc.version_id)?;
            Ok(AckResult::Applied)
        }
        Decision::FastForward if inc.deleted => {
            if head.as_ref().is_some_and(|h| !h.deleted) {
                ctx.save_local(&inc.path, "deleted", None, false)?;
                match remove_file(&ctx.root, &inc.path, &expect_for(&head)) {
                    Err(ApplyError::Changed) => {
                        ctx.index(Scope::Path(inc.path.clone()))?;
                        return decide(ctx, peer, inc, staged, out, depth + 1);
                    }
                    r => r?,
                }
            }
            ctx.record(&inc.version(ctx.ws), Head::Set, Some(peer))?;
            ctx.resolve_elsewhere(&inc.path, &inc.ancestry, &inc.version_id)?;
            out.applied += 1;
            Ok(AckResult::Applied)
        }
        Decision::FastForward => {
            let hash = inc.hash.clone().unwrap_or_default();
            let src = match staged {
                Some(p) => Some(p.to_path_buf()),
                None => ctx.local_content(&hash)?,
            };
            let Some(src) = src else {
                queue_transfer(ctx, peer, inc, out)?;
                return Ok(AckResult::Pending);
            };
            if head.as_ref().is_some_and(|h| !h.deleted) {
                ctx.save_local(&inc.path, "overwritten", None, false)?;
            }
            let mode = inc.mode.or_else(|| existing_mode(&ctx.root, &inc.path));
            match commit_file(&ctx.root, &inc.path, &src, &hash, &expect_for(&head), mode) {
                Ok(()) => {}
                Err(ApplyError::Changed) => {
                    ctx.index(Scope::Path(inc.path.clone()))?;
                    return decide(ctx, peer, inc, Some(&src), out, depth + 1);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&src);
                    return Err(e.into());
                }
            }
            ctx.record(&inc.version(ctx.ws), Head::Set, Some(peer))?;
            ctx.resolve_elsewhere(&inc.path, &inc.ancestry, &inc.version_id)?;
            out.applied += 1;
            Ok(AckResult::Applied)
        }
        Decision::Conflict => {
            let local = head.clone().expect("conflict implies a local head");
            let conflict_id = ulid::Ulid::generate().to_string();
            let kind = if inc.deleted {
                "remote_deleted"
            } else if local.deleted {
                "local_deleted"
            } else {
                "content"
            };
            if !inc.deleted {
                let hash = inc.hash.clone().unwrap_or_default();
                if !ctx.recovery.contains(&hash) {
                    let src = match staged {
                        Some(p) => Some(p.to_path_buf()),
                        None => ctx.local_content(&hash)?,
                    };
                    let Some(src) = src else {
                        queue_transfer(ctx, peer, inc, out)?;
                        return Ok(AckResult::Pending);
                    };
                    ctx.recovery.adopt(&src, &hash)?;
                } else {
                    discard(staged);
                }
                lock(&ctx.store).recovery_add(
                    ctx.ws,
                    &inc.path,
                    &hash,
                    inc.size,
                    "conflict_remote",
                    Some(&conflict_id),
                    true,
                )?;
            }
            if !local.deleted {
                ctx.save_local(&inc.path, "conflict_local", Some(&conflict_id), true)?;
            }
            ctx.record(&inc.version(ctx.ws), Head::HistoryOnly, None)?;
            lock(&ctx.store).conflict_insert(&ConflictRecord {
                id: conflict_id,
                workspace_id: ctx.ws,
                path: inc.path.clone(),
                kind: kind.into(),
                local_version_id: Some(local.version_id.clone()),
                local_hash: local.content_hash.clone(),
                local_size: Some(local.size),
                remote_version_id: inc.version_id.clone(),
                remote_hash: inc.hash.clone(),
                remote_size: Some(inc.size),
                remote_mode: inc.mode,
                remote_ancestry: inc.ancestry.clone(),
                remote_device_id: inc.origin,
                state: "open".into(),
                resolution: None,
                detected_at: now_ms(),
                resolved_at: None,
            })?;
            out.conflicts.push(inc.path.clone());
            Ok(AckResult::Conflict)
        }
    }
}

fn queue_transfer(ctx: &Ctx, peer: DeviceId, inc: &Inc, out: &mut Outcome) -> Result<(), SyncError> {
    let job = TransferJob {
        id: ulid::Ulid::generate().to_string(),
        workspace_id: ctx.ws,
        peer_device_id: peer,
        change_id: inc.change_id.clone(),
        path: inc.path.clone(),
        version_id: inc.version_id.clone(),
        ancestry: inc.ancestry.clone(),
        base_hashes: inc.base_hashes.clone(),
        origin_device_id: inc.origin,
        content_hash: inc.hash.clone().unwrap_or_default(),
        total_size: inc.size,
        mode: inc.mode,
        bytes_done: 0,
        state: "pending".into(),
        attempts: 0,
        last_error: None,
    };
    let s = lock(&ctx.store);
    s.transfer_insert(&job)?;
    s.transfers_supersede_path(ctx.ws, &inc.path, &job.id)?;
    drop(s);
    out.transfers.push(job);
    Ok(())
}

/// Called when a transfer's content has fully arrived in its staging file.
/// Returns the final ack for the announcing peer, or `None` if the job was
/// superseded/cancelled meanwhile. A hash mismatch is returned as an error
/// (the caller retries or gives up).
pub fn finish_transfer(ctx: &Ctx, transfer_id: &str, out: &mut Outcome) -> Result<Option<(DeviceId, Ack)>, SyncError> {
    let Some(job) = lock(&ctx.store).transfer_get(transfer_id)? else { return Ok(None) };
    let staged = ctx.transfer_path(transfer_id);
    if !matches!(job.state.as_str(), "pending" | "active") {
        let _ = std::fs::remove_file(&staged);
        return Ok(None);
    }
    let (hash, size) = hash_file(&staged)?;
    if hash != job.content_hash || size != job.total_size {
        let _ = std::fs::remove_file(&staged);
        return Err(ApplyError::HashMismatch.into());
    }
    ctx.index(Scope::Path(job.path.clone()))?;
    let inc = Inc::from_job(&job);
    let result = decide(ctx, job.peer_device_id, &inc, Some(&staged), out, 0);
    let _ = std::fs::remove_file(&staged);
    let result = result?;
    lock(&ctx.store).transfer_update(transfer_id, "done", job.total_size, None)?;
    Ok(Some((job.peer_device_id, ack(&job.change_id, result, None))))
}

// ---------------------------------------------------------------------------
// Conflict resolution
// ---------------------------------------------------------------------------

pub fn resolve(ctx: &Ctx, conflict_id: &str, res: &Resolution) -> Result<(), SyncError> {
    let c = lock(&ctx.store).conflict_get(conflict_id)?;
    if c.state != "open" {
        return Err(SyncError::Other("This conflict was already resolved.".into()));
    }
    ctx.index(Scope::Path(c.path.clone()))?;
    let head = ctx.head(&c.path)?.ok_or_else(|| SyncError::Other("The file's history is missing.".into()))?;
    let (head_ancestry, head_bases) = ctx.lineage_of(&head.version_id)?;
    let ancestry = child_ancestry(&[
        (head.version_id.as_str(), &head_ancestry),
        (c.remote_version_id.as_str(), &c.remote_ancestry),
    ]);
    // The merge result descends from both sides' content.
    let base_hashes = child_bases(&[
        (content_key(head.content_hash.as_deref(), head.deleted), &head_bases),
        (content_key(c.remote_hash.as_deref(), c.kind == "remote_deleted"), &[]),
    ]);
    let local_expect =
        if head.deleted { Expect::Absent } else { Expect::Hash(head.content_hash.clone().unwrap_or_default()) };

    let write_new =
        |content_hash: Option<String>, size: u64, mode: Option<u32>, deleted: bool| -> Result<(), SyncError> {
            let v = VersionRow {
                version_id: ulid::Ulid::generate().to_string(),
                workspace_id: ctx.ws,
                path: c.path.clone(),
                ancestry: ancestry.clone(),
                base_hashes: base_hashes.clone(),
                content_hash,
                size,
                mode,
                deleted,
                origin_device_id: ctx.device,
                created_at: now_ms(),
            };
            ctx.record(&v, Head::Set, None)
        };
    let keep_local = || write_new(head.content_hash.clone(), head.size, head.mode, head.deleted);

    let resolution = match res {
        Resolution::KeepLocal => {
            keep_local()?;
            "keep_local"
        }
        Resolution::KeepRemote => {
            match &c.remote_hash {
                None => {
                    if !head.deleted {
                        ctx.save_local(&c.path, "overwritten", None, false)?;
                        remove_file(&ctx.root, &c.path, &local_expect)?;
                    }
                    write_new(None, 0, head.mode, true)?;
                }
                Some(h) => {
                    let blob = ctx.recovery.blob_path(h).filter(|p| p.is_file()).ok_or_else(|| {
                        SyncError::Other("The other device's version is no longer in the recovery store.".into())
                    })?;
                    if !head.deleted {
                        ctx.save_local(&c.path, "overwritten", None, false)?;
                    }
                    let mode = c.remote_mode.or(head.mode);
                    commit_copy(&ctx.root, &c.path, &blob, &ctx.staging, h, &local_expect, mode)?;
                    write_new(Some(h.clone()), c.remote_size.unwrap_or(0), mode, false)?;
                }
            }
            "keep_remote"
        }
        Resolution::SaveBoth { suffix } => {
            if let Some(h) = &c.remote_hash {
                let blob = ctx.recovery.blob_path(h).filter(|p| p.is_file()).ok_or_else(|| {
                    SyncError::Other("The other device's version is no longer in the recovery store.".into())
                })?;
                let mut target = c.path.with_suffix(suffix).map_err(|e| SyncError::Other(e.to_string()))?;
                let mut n = 2;
                while std::fs::symlink_metadata(target.to_fs(&ctx.root)).is_ok() {
                    target =
                        c.path.with_suffix(&format!("{suffix} {n}")).map_err(|e| SyncError::Other(e.to_string()))?;
                    n += 1;
                }
                commit_copy(&ctx.root, &target, &blob, &ctx.staging, h, &Expect::Absent, c.remote_mode)?;
                ctx.index(Scope::Path(target))?;
            }
            keep_local()?;
            "save_both"
        }
        Resolution::Merged(bytes) => {
            let staged = ctx.staging_file()?;
            std::fs::write(&staged, bytes)?;
            let hash = devbridge_fs::hash::hash_bytes(bytes);
            if !head.deleted {
                ctx.save_local(&c.path, "overwritten", None, false)?;
            }
            let mode = head.mode.or(c.remote_mode);
            if let Err(e) = commit_file(&ctx.root, &c.path, &staged, &hash, &local_expect, mode) {
                let _ = std::fs::remove_file(&staged);
                return Err(e.into());
            }
            write_new(Some(hash), bytes.len() as u64, mode, false)?;
            "merged"
        }
    };
    lock(&ctx.store).conflict_resolve(conflict_id, resolution)?;
    Ok(())
}

/// Both sides of a conflict for comparison: (local bytes, remote bytes);
/// `None` where that side is deleted or unavailable.
pub type ConflictContents = (Option<Vec<u8>>, Option<Vec<u8>>);

pub fn conflict_contents(ctx: &Ctx, conflict_id: &str, max: u64) -> Result<ConflictContents, SyncError> {
    let c = lock(&ctx.store).conflict_get(conflict_id)?;
    let read_limited = |p: &Path| -> Option<Vec<u8>> {
        let m = std::fs::metadata(p).ok()?;
        if m.len() > max {
            return None;
        }
        std::fs::read(p).ok()
    };
    let local = read_limited(&c.path.to_fs(&ctx.root));
    let remote = c.remote_hash.as_deref().and_then(|h| ctx.recovery.blob_path(h)).and_then(|p| read_limited(&p));
    Ok((local, remote))
}
