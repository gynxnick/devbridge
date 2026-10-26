//! Per-workspace actor. All local indexing, incoming applies, transfer
//! completions and conflict resolutions for one workspace run here, one at a
//! time, so they can never race with each other.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use devbridge_core::protocol::{Ack, AckResult, Change, ChangeOp, MAX_BATCH, Message};
use devbridge_core::{DeviceId, RelPath, SyncState, WorkspaceId};
use devbridge_fs::apply::{ApplyError, staging_dir};
use devbridge_fs::rules::Rules;
use devbridge_fs::watch::{WatchEvent, WorkspaceWatcher};
use devbridge_store::VersionRow;
use tokio::sync::{mpsc, oneshot};

use super::{EngineEvent, Inner, WsHandle, guard};
use crate::apply::{self, ConflictContents, Ctx, Outcome, Resolution};
use crate::indexer::{Scope, manifest_digest};
use crate::{SyncError, lock};

pub(crate) enum WsCmd {
    Paths(Vec<PathBuf>),
    Rescan { announce: bool },
    Incoming { peer: DeviceId, changes: Vec<Change> },
    TransferDone { transfer_id: String },
    TransferFailed { transfer_id: String, error: String, retry: bool },
    PeerOnline(DeviceId),
    PeerOffline(DeviceId),
    Digest { peer: DeviceId, digest: String },
    ManifestRequested(DeviceId),
    StateChanged,
    PausedChanged,
    RulesChanged,
    Retry,
    Resolve { conflict_id: String, res: Resolution, reply: oneshot::Sender<Result<(), SyncError>> },
    Contents { conflict_id: String, max: u64, reply: oneshot::Sender<Result<ConflictContents, SyncError>> },
    Shutdown,
}

struct Actor {
    inner: Arc<Inner>,
    ws: WorkspaceId,
    ctx: Ctx,
    paused: Arc<AtomicBool>,
    rx: mpsc::UnboundedReceiver<WsCmd>,
    tx: mpsc::UnboundedSender<WsCmd>,
    watcher: Option<WorkspaceWatcher>,
    last_state: Option<SyncState>,
    sessions: Vec<(DeviceId, String)>,
}

pub(crate) fn spawn(inner: Arc<Inner>, ws: WorkspaceId) -> Result<WsHandle, SyncError> {
    let w = lock(&inner.store).get_workspace(ws)?;
    let root = PathBuf::from(&w.root_path);
    let rules = Arc::new(Rules::new(&root, &guard(&inner.tuning).rules));
    let staging = staging_dir(&root, &inner.staging_root().join(ws.to_string()));
    let ctx = Ctx {
        store: inner.store.clone(),
        ws,
        root,
        rules,
        device: inner.device_id,
        recovery: inner.recovery.clone(),
        staging,
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let paused = Arc::new(AtomicBool::new(false));
    let mut actor = Actor {
        inner,
        ws,
        ctx,
        paused: paused.clone(),
        rx,
        tx: tx.clone(),
        watcher: None,
        last_state: None,
        sessions: vec![],
    };
    actor.start_watcher();
    let _ = tx.send(WsCmd::Rescan { announce: true });
    let task = actor.run();
    tokio::spawn(task);
    Ok(WsHandle { tx, paused })
}

impl Actor {
    fn start_watcher(&mut self) {
        if !self.ctx.root.is_dir() {
            return;
        }
        let tx = self.tx.clone();
        match WorkspaceWatcher::start(&self.ctx.root, move |ev| {
            let _ = tx.send(match ev {
                WatchEvent::Paths(p) => WsCmd::Paths(p),
                WatchEvent::Rescan => WsCmd::Rescan { announce: false },
            });
        }) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => {
                tracing::warn!(workspace = %self.ws, error = %e, "file watching unavailable; relying on periodic reconcile");
            }
        }
    }

    async fn run(mut self) {
        while let Some(first) = self.rx.recv().await {
            // Drain what's queued so bursts of file events are handled at once.
            let mut batch = vec![first];
            while batch.len() < 1000 {
                match self.rx.try_recv() {
                    Ok(c) => batch.push(c),
                    Err(_) => break,
                }
            }
            let mut paths = Vec::new();
            let mut rest = Vec::new();
            for c in batch {
                match c {
                    WsCmd::Paths(p) => paths.extend(p),
                    other => rest.push(other),
                }
            }
            if !paths.is_empty() {
                self.on_paths(paths).await;
            }
            for c in rest {
                if matches!(c, WsCmd::Shutdown) {
                    self.end_sessions("completed").await;
                    return;
                }
                self.handle(c).await;
            }
            self.update_state().await;
        }
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Ctx) -> Result<T, SyncError> + Send + 'static,
    ) -> Result<T, SyncError> {
        let ctx = self.ctx.clone();
        Inner::blocking(move || f(&ctx)).await
    }

    fn report(&self, e: &SyncError) {
        tracing::warn!(workspace = %self.ws, error = %e, "sync error");
        self.inner.emit(EngineEvent::Error { workspace_id: Some(self.ws), message: e.to_string() });
    }

    async fn handle(&mut self, cmd: WsCmd) {
        match cmd {
            WsCmd::Paths(_) | WsCmd::Shutdown => {}
            WsCmd::Rescan { announce } => self.on_rescan(announce).await,
            WsCmd::Incoming { peer, changes } => self.on_incoming(peer, changes).await,
            WsCmd::TransferDone { transfer_id } => self.on_transfer_done(transfer_id).await,
            WsCmd::TransferFailed { transfer_id, error, retry } => {
                self.on_transfer_failed(transfer_id, error, retry).await
            }
            WsCmd::PeerOnline(peer) => self.on_peer_online(peer).await,
            WsCmd::PeerOffline(peer) => {
                if let Some(pos) = self.sessions.iter().position(|(p, _)| *p == peer) {
                    let (_, id) = self.sessions.remove(pos);
                    let _ = self.blocking(move |c| Ok(lock(&c.store).session_end(&id, "interrupted", None)?)).await;
                }
            }
            WsCmd::Digest { peer, digest } => self.on_digest(peer, digest).await,
            WsCmd::ManifestRequested(peer) => self.on_manifest_requested(peer).await,
            WsCmd::StateChanged => {}
            WsCmd::PausedChanged => {
                if !self.paused.load(Ordering::Relaxed) {
                    self.on_rescan(true).await;
                    self.dispatch_pending_transfers(None).await;
                }
            }
            WsCmd::RulesChanged => {
                let cfg = guard(&self.inner.tuning).rules.clone();
                self.ctx.rules = Arc::new(Rules::new(&self.ctx.root, &cfg));
                self.on_rescan(true).await;
            }
            WsCmd::Retry => {
                let ws = self.ws;
                let _ = self.blocking(move |c| Ok(lock(&c.store).transfers_retry_failed(ws)?)).await;
                self.dispatch_pending_transfers(None).await;
                self.on_rescan(true).await;
            }
            WsCmd::Resolve { conflict_id, res, reply } => {
                let r = self.blocking(move |c| apply::resolve(c, &conflict_id, &res)).await;
                let ok = r.is_ok();
                let _ = reply.send(r);
                if ok {
                    self.flush_outgoing().await;
                    self.inner.emit(EngineEvent::WorkspaceChanged { workspace_id: self.ws });
                }
            }
            WsCmd::Contents { conflict_id, max, reply } => {
                let r = self.blocking(move |c| apply::conflict_contents(c, &conflict_id, max)).await;
                let _ = reply.send(r);
            }
        }
    }

    async fn on_paths(&mut self, paths: Vec<PathBuf>) {
        if !self.ctx.root.is_dir() {
            return;
        }
        let root = self.ctx.root.clone();
        let mut rels: Vec<RelPath> = paths.iter().filter_map(|p| RelPath::from_fs(&root, p).ok()).collect();
        // Unportable names can't become RelPaths; a rescan reports them.
        let unparsable = rels.len() < paths.iter().filter(|p| p.starts_with(&root) && **p != root).count();
        rels.sort();
        rels.dedup();
        let full = rels.len() > 200 || unparsable || paths.contains(&root);
        let r = self
            .blocking(move |c| {
                let mut scopes: Vec<Scope> =
                    if full { vec![Scope::All] } else { rels.iter().cloned().map(Scope::Path).collect() };
                if !full {
                    scopes.extend(c.indexer().vanished_siblings(&rels)?.into_iter().map(Scope::Path));
                }
                let mut gitignore = false;
                let mut n = 0;
                for s in scopes {
                    let rep = c.index(s)?;
                    gitignore |= rep.gitignore_changed;
                    n += rep.versions.len();
                }
                if gitignore {
                    c.rules.invalidate_gitignores();
                    n += c.index(Scope::All)?.versions.len();
                }
                Ok(n)
            })
            .await;
        match r {
            Ok(n) if n > 0 => {
                let me = self.inner.device_id;
                let ws = self.ws;
                let _ = self.blocking(move |c| Ok(lock(&c.store).set_active_device(ws, me)?)).await;
                self.flush_outgoing().await;
                self.inner.emit(EngineEvent::WorkspaceChanged { workspace_id: self.ws });
            }
            Ok(_) => {}
            Err(e) => self.report(&e),
        }
    }

    async fn on_rescan(&mut self, announce: bool) {
        if !self.ctx.root.is_dir() {
            return;
        }
        if self.watcher.is_none() {
            self.start_watcher();
        }
        match self.blocking(|c| c.index(Scope::All)).await {
            Ok(rep) => {
                if !rep.versions.is_empty() {
                    self.inner.emit(EngineEvent::WorkspaceChanged { workspace_id: self.ws });
                }
            }
            Err(e) => self.report(&e),
        }
        self.flush_outgoing().await;
        if announce && !self.paused.load(Ordering::Relaxed) {
            for peer in self.online_synced_peers().await {
                self.send_digest(peer).await;
            }
        }
    }

    async fn online_synced_peers(&self) -> Vec<DeviceId> {
        let ws = self.ws;
        let peers = self.blocking(move |c| Ok(lock(&c.store).share_states(ws)?)).await.unwrap_or_default();
        peers
            .into_iter()
            .filter(|p| p.shared && p.joined && self.inner.is_online(p.device_id))
            .map(|p| p.device_id)
            .collect()
    }

    async fn send_digest(&self, peer: DeviceId) {
        let ws = self.ws;
        if let Ok((digest, count)) = self
            .blocking(move |c| {
                let m = lock(&c.store).manifest_all(ws)?;
                Ok((manifest_digest(&m), m.len() as u64))
            })
            .await
        {
            self.inner.send(peer, Message::ManifestDigest { ws, digest, count }).await;
        }
    }

    /// Sends queued change notifications to every online, joined peer.
    async fn flush_outgoing(&self) {
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        for peer in self.online_synced_peers().await {
            loop {
                let ws = self.ws;
                let batch =
                    match self.blocking(move |c| Ok(lock(&c.store).pending_changes(ws, peer, MAX_BATCH as u32)?)).await
                    {
                        Ok(b) if !b.is_empty() => b,
                        _ => break,
                    };
                let ids: Vec<String> = batch.iter().map(|o| o.change_id.clone()).collect();
                let changes = batch.into_iter().map(|o| to_change(o.change_id, &o.version)).collect();
                if !self.inner.send(peer, Message::Changes { ws, changes, reconcile: false }).await {
                    break;
                }
                let _ = self.blocking(move |c| Ok(lock(&c.store).mark_changes_sent(&ids)?)).await;
            }
        }
    }

    async fn on_incoming(&mut self, peer: DeviceId, changes: Vec<Change>) {
        if self.paused.load(Ordering::Relaxed) || !self.ctx.root.is_dir() {
            let reason = if self.paused.load(Ordering::Relaxed) { "paused" } else { "project folder unavailable" };
            let acks = changes
                .iter()
                .map(|c| Ack {
                    change_id: c.change_id.clone(),
                    result: AckResult::Pending,
                    reason: Some(reason.into()),
                })
                .collect();
            self.inner.send(peer, Message::Acks { ws: self.ws, acks }).await;
            return;
        }
        let out = self.blocking(move |c| Ok(apply::process_incoming(c, peer, &changes))).await;
        match out {
            Ok(out) => self.after_outcome(peer, out).await,
            Err(e) => self.report(&e),
        }
    }

    async fn after_outcome(&mut self, peer: DeviceId, out: Outcome) {
        if !out.acks.is_empty() {
            for chunk in out.acks.chunks(MAX_BATCH) {
                self.inner.send(peer, Message::Acks { ws: self.ws, acks: chunk.to_vec() }).await;
            }
        }
        for p in &out.conflicts {
            self.inner.emit(EngineEvent::ConflictDetected { workspace_id: self.ws, path: p.to_string() });
        }
        if let Some(h) = self.inner.peer(peer) {
            for job in out.transfers {
                let _ = h.transfers.send(job);
            }
        }
        if out.applied > 0 || !out.conflicts.is_empty() {
            if let Some((_, sid)) = self.sessions.iter().find(|(p, _)| *p == peer) {
                let sid = sid.clone();
                let n = out.applied as u64;
                let _ = self.blocking(move |c| Ok(lock(&c.store).session_add(&sid, 0, n, 0, 0)?)).await;
            }
            self.inner.emit(EngineEvent::WorkspaceChanged { workspace_id: self.ws });
            // Forward to other peers (multi-device).
            self.flush_outgoing().await;
        }
    }

    async fn on_transfer_done(&mut self, transfer_id: String) {
        tracing::debug!(transfer = %transfer_id, "transfer complete; applying");
        let id = transfer_id.clone();
        let r = self
            .blocking(move |c| {
                let mut out = Outcome::default();
                let res = apply::finish_transfer(c, &id, &mut out)?;
                Ok((res, out))
            })
            .await;
        match r {
            Ok((Some((peer, ack)), mut out)) => {
                out.acks.push(ack);
                self.after_outcome(peer, out).await;
            }
            Ok((None, _)) => {}
            Err(SyncError::Apply(ApplyError::HashMismatch)) => {
                self.on_transfer_failed(transfer_id, "Received data didn't match its checksum".into(), true).await;
            }
            Err(e) => self.on_transfer_failed(transfer_id, e.to_string(), false).await,
        }
    }

    async fn on_transfer_failed(&mut self, transfer_id: String, error: String, retry: bool) {
        tracing::debug!(transfer = %transfer_id, %error, retry, "transfer failed");
        let id = transfer_id.clone();
        let err = error.clone();
        let r = self
            .blocking(move |c| {
                let s = lock(&c.store);
                let Some(job) = s.transfer_get(&id)? else { return Ok(None) };
                if !matches!(job.state.as_str(), "pending" | "active") {
                    return Ok(None);
                }
                if retry && job.attempts < 3 {
                    s.transfer_update(&id, "pending", 0, Some(&err))?;
                    let _ = std::fs::remove_file(c.transfer_path(&id));
                    Ok(Some((job, true)))
                } else {
                    s.transfer_update(&id, "failed", job.bytes_done, Some(&err))?;
                    let _ = std::fs::remove_file(c.transfer_path(&id));
                    Ok(Some((job, false)))
                }
            })
            .await;
        match r {
            Ok(Some((job, true))) => {
                if let Some(h) = self.inner.peer(job.peer_device_id) {
                    let _ = h.transfers.send(job);
                }
            }
            Ok(Some((job, false))) => {
                self.inner.emit(EngineEvent::Error {
                    workspace_id: Some(self.ws),
                    message: format!("Couldn't receive {}: {error}", job.path),
                });
                let ack = Ack { change_id: job.change_id, result: AckResult::Rejected, reason: Some(error) };
                self.inner.send(job.peer_device_id, Message::Acks { ws: self.ws, acks: vec![ack] }).await;
            }
            Ok(None) => {}
            Err(e) => self.report(&e),
        }
    }

    async fn dispatch_pending_transfers(&self, only: Option<DeviceId>) {
        let ws = self.ws;
        let jobs = self.blocking(move |c| Ok(lock(&c.store).transfers_list(ws)?)).await.unwrap_or_default();
        for job in jobs {
            if only.is_some_and(|p| p != job.peer_device_id) {
                continue;
            }
            if let Some(h) = self.inner.peer(job.peer_device_id) {
                let _ = h.transfers.send(job);
            }
        }
    }

    async fn on_peer_online(&mut self, peer: DeviceId) {
        tracing::debug!(workspace = %self.ws, %peer, "peer online for workspace");
        let ws = self.ws;
        let synced = self.blocking(move |c| Ok(lock(&c.store).is_synced_with(ws, peer)?)).await.unwrap_or(false);
        if !synced || !self.inner.is_online(peer) {
            return;
        }
        if !self.sessions.iter().any(|(p, _)| *p == peer)
            && let Ok(id) = self.blocking(move |c| Ok(lock(&c.store).session_start(ws, peer)?)).await
        {
            self.sessions.push((peer, id));
        }
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        self.flush_outgoing().await;
        self.dispatch_pending_transfers(Some(peer)).await;
        self.send_digest(peer).await;
    }

    async fn on_digest(&mut self, peer: DeviceId, digest: String) {
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        let ws = self.ws;
        let mine =
            self.blocking(move |c| Ok(manifest_digest(&lock(&c.store).manifest_all(ws)?))).await.unwrap_or_default();
        if mine == digest {
            let _ = self.blocking(move |c| Ok(lock(&c.store).mark_synced(ws, peer)?)).await;
            self.inner.emit(EngineEvent::WorkspaceChanged { workspace_id: ws });
        } else {
            self.inner.send(peer, Message::ManifestRequest { ws }).await;
        }
    }

    /// Sends the full manifest as reconciliation changes.
    async fn on_manifest_requested(&mut self, peer: DeviceId) {
        if self.paused.load(Ordering::Relaxed) {
            return;
        }
        let ws = self.ws;
        let versions = match self.blocking(move |c| Ok(lock(&c.store).manifest_versions(ws)?)).await {
            Ok(v) => v,
            Err(e) => return self.report(&e),
        };
        for chunk in versions.chunks(MAX_BATCH) {
            let changes = chunk.iter().map(|v| to_change(format!("r-{}", v.version_id), v)).collect();
            if !self.inner.send(peer, Message::Changes { ws, changes, reconcile: true }).await {
                return;
            }
        }
    }

    async fn end_sessions(&mut self, state: &'static str) {
        let ids: Vec<String> = self.sessions.drain(..).map(|(_, id)| id).collect();
        let _ = self
            .blocking(move |c| {
                let s = lock(&c.store);
                for id in ids {
                    s.session_end(&id, state, None)?;
                }
                Ok(())
            })
            .await;
    }

    async fn update_state(&mut self) {
        let ws = self.ws;
        let paused = self.paused.load(Ordering::Relaxed);
        let root_ok = self.ctx.root.is_dir();
        let online: HashSet<DeviceId> = super::guard(&self.inner.peers).keys().copied().collect();
        let r = self
            .blocking(move |c| {
                let s = lock(&c.store);
                let shares: Vec<_> = s.share_states(ws)?.into_iter().filter(|p| p.shared && p.joined).collect();
                let state = if !root_ok {
                    SyncState::Error
                } else if paused {
                    SyncState::Paused
                } else if shares.is_empty() {
                    SyncState::LocalOnly
                } else if !shares.iter().any(|p| online.contains(&p.device_id)) {
                    SyncState::Offline
                } else if s.count_queued_for(ws)? + s.transfers_active_count(ws)? > 0 {
                    SyncState::Syncing
                } else {
                    SyncState::Idle
                };
                s.set_sync_state(ws, state)?;
                if state == SyncState::Idle {
                    for p in shares.iter().filter(|p| online.contains(&p.device_id)) {
                        s.mark_synced(ws, p.device_id)?;
                    }
                }
                Ok(state)
            })
            .await;
        if let Ok(state) = r
            && self.last_state != Some(state)
        {
            if state == SyncState::Error {
                self.inner.emit(EngineEvent::Error {
                    workspace_id: Some(ws),
                    message: format!("The project folder {} isn't available.", self.ctx.root.display()),
                });
            }
            self.last_state = Some(state);
            self.inner.emit(EngineEvent::WorkspaceChanged { workspace_id: ws });
        }
    }
}

pub(crate) fn to_change(change_id: String, v: &VersionRow) -> Change {
    Change {
        change_id,
        origin_device: v.origin_device_id,
        path: v.path.clone(),
        op: if v.deleted { ChangeOp::Delete } else { ChangeOp::Upsert },
        version_id: v.version_id.clone(),
        ancestry: v.ancestry.clone(),
        base_hashes: v.base_hashes.clone(),
        hash: v.content_hash.clone(),
        size: v.size,
        mode: v.mode,
    }
}
