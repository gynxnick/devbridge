//! Sessions with paired devices.
//!
//! Every inbound connection is authenticated by TLS; here it's *authorized*:
//! the certificate fingerprint must belong to a paired, non-revoked device,
//! and the device id it claims in `Hello` must match. Every workspace-level
//! message is additionally checked against the sharing table.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devbridge_core::protocol::{AckResult, CHUNK_SIZE, MAX_BATCH, Message, PROTOCOL_VERSION};
use devbridge_core::{Device, DeviceId, WorkspaceId};
use devbridge_fs::hash::is_valid_hash;
use devbridge_net::session::{self, Session};
use devbridge_net::{Frame, SessionTx, TlsConn};
use devbridge_store::{TransferJob, WorkspaceOffer};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::AbortHandle;

use super::ws::WsCmd;
use super::{EngineEvent, Inner, PeerHandle, guard};
use crate::lock;

const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(15);
const IDLE_LIMIT: Duration = Duration::from_secs(45);
/// Free space kept in reserve when accepting a transfer.
const DISK_RESERVE: u64 = 256 * 1024 * 1024;
const PROGRESS_EVERY: u64 = 4 * 1024 * 1024;

pub(crate) async fn handle_connection(inner: Arc<Inner>, conn: TlsConn, expected: Option<DeviceId>) {
    let fp = conn.peer_fingerprint.clone();
    let device = {
        let store = inner.store.clone();
        Inner::blocking(move || Ok(lock(&store).paired_by_fingerprint(&fp)?)).await.ok().flatten()
    };
    match device {
        Some(d) if expected.is_none_or(|e| e == d.id) => run_session(inner, conn, d).await,
        None if !conn.outbound => super::pair::respond(inner, conn).await,
        _ => {}
    }
}

struct RecvState {
    ws: WorkspaceId,
    path: String,
    file: tokio::fs::File,
    expected: u64,
    total: u64,
    last_report: u64,
    _permit: OwnedSemaphorePermit,
}

struct Ctx {
    inner: Arc<Inner>,
    peer: DeviceId,
    tx: SessionTx,
    recv: Arc<Mutex<HashMap<String, RecvState>>>,
    serving: Arc<Mutex<HashMap<String, AbortHandle>>>,
    serve_slots: Arc<Semaphore>,
}

async fn run_session(inner: Arc<Inner>, conn: TlsConn, device: Device) {
    let outbound = conn.outbound;
    let addr = conn.peer_addr;
    let mut s: Session = session::spawn(conn);
    let hello = Message::Hello {
        device: inner.device_info(),
        protocol_versions: vec![PROTOCOL_VERSION],
        listen_port: inner.listen_port(),
    };
    if s.tx.send(hello).await.is_err() {
        return;
    }
    let peer_hello = match tokio::time::timeout(HELLO_TIMEOUT, s.rx.recv()).await {
        Ok(Some(Frame::Msg(Message::Hello { device: info, protocol_versions, listen_port }))) => {
            if info.device_id != device.id || !protocol_versions.contains(&PROTOCOL_VERSION) {
                let _ =
                    s.tx.send(Message::Error { code: "identity_mismatch".into(), message: "Unexpected device".into() })
                        .await;
                s.tx.close();
                return;
            }
            (info, listen_port)
        }
        _ => {
            s.tx.close();
            return;
        }
    };
    let (info, listen_port) = peer_hello;
    let address = listen_port.map(|p| SocketAddr::new(addr.ip(), p).to_string());
    {
        let store = inner.store.clone();
        let id = device.id;
        let name = info.name.clone();
        let changed_name = name != device.name;
        let _ = Inner::blocking(move || {
            let st = lock(&store);
            st.device_seen(id, address.as_deref())?;
            if changed_name
                && let Ok(n) = devbridge_core::validate_display_name(&name, devbridge_core::DEVICE_NAME_MAX_CHARS)
            {
                st.rename_device(id, &n)?;
            }
            Ok(())
        })
        .await;
    }

    // Register, keeping exactly one session per device: the one dialed by
    // the device with the smaller id (both sides compute the same answer).
    let session_id = inner.next_session_id();
    let (tr_tx, tr_rx) = mpsc::unbounded_channel();
    let handle = PeerHandle { session_id, tx: s.tx.clone(), outbound, transfers: tr_tx };
    let keep = {
        let mut peers = guard(&inner.peers);
        let preferred = outbound == (inner.device_id < device.id);
        match peers.get(&device.id) {
            Some(existing) if existing.outbound == outbound || preferred => {
                existing.tx.close();
                peers.insert(device.id, handle);
                true
            }
            Some(_) => false,
            None => {
                peers.insert(device.id, handle);
                true
            }
        }
    };
    if !keep {
        s.tx.close();
        return;
    }
    tracing::info!(peer = %device.name, %addr, "device connected");
    inner.emit(EngineEvent::PeerOnline { device_id: device.id });
    inner.emit(EngineEvent::DevicesChanged);

    let ctx = Arc::new(Ctx {
        inner: inner.clone(),
        peer: device.id,
        tx: s.tx.clone(),
        recv: Arc::new(Mutex::new(HashMap::new())),
        serving: Arc::new(Mutex::new(HashMap::new())),
        serve_slots: Arc::new(Semaphore::new(4)),
    });

    // Re-send anything unacknowledged, start reconciliation, re-offer shares.
    let synced: Vec<WorkspaceId> = {
        let store = inner.store.clone();
        let peer = device.id;
        Inner::blocking(move || {
            let st = lock(&store);
            st.requeue_unacked(peer)?;
            Ok(st.synced_workspaces_with(peer)?)
        })
        .await
        .unwrap_or_default()
    };
    for ws in &synced {
        inner.ws_cmd(*ws, WsCmd::PeerOnline(device.id));
    }
    reoffer_shares(&inner, device.id).await;

    let scheduler = tokio::spawn(transfer_scheduler(ctx.clone(), tr_rx));

    let mut last_rx = Instant::now();
    let mut ping = tokio::time::interval(PING_EVERY);
    let mut nonce = 0u64;
    loop {
        tokio::select! {
            f = s.rx.recv() => match f {
                None => break,
                Some(frame) => {
                    last_rx = Instant::now();
                    if !handle_frame(&ctx, frame).await {
                        break;
                    }
                }
            },
            _ = ping.tick() => {
                if last_rx.elapsed() > IDLE_LIMIT {
                    tracing::info!(peer = %device.name, "connection timed out");
                    break;
                }
                nonce += 1;
                if s.tx.send(Message::Ping { nonce }).await.is_err() {
                    break;
                }
            }
        }
    }

    // Cleanup.
    s.tx.close();
    scheduler.abort();
    for (_, h) in guard(&ctx.serving).drain() {
        h.abort();
    }
    let interrupted: Vec<(String, WorkspaceId)> = guard(&ctx.recv).drain().map(|(id, st)| (id, st.ws)).collect();
    let removed = {
        let mut peers = guard(&inner.peers);
        if peers.get(&device.id).is_some_and(|p| p.session_id == session_id) {
            peers.remove(&device.id);
            true
        } else {
            false
        }
    };
    {
        let store = inner.store.clone();
        let ids: Vec<String> = interrupted.iter().map(|(id, _)| id.clone()).collect();
        let _ = Inner::blocking(move || {
            let st = lock(&store);
            for id in ids {
                // Keep bytes_done: the staging file is resumed on reconnect.
                if let Some(j) = st.transfer_get(&id)?
                    && j.state == "active"
                {
                    st.transfer_update(&id, "pending", j.bytes_done, None)?;
                }
            }
            Ok(())
        })
        .await;
    }
    if removed {
        tracing::info!(peer = %device.name, "device disconnected");
        for ws in synced {
            inner.ws_cmd(ws, WsCmd::PeerOffline(device.id));
            inner.ws_cmd(ws, WsCmd::StateChanged);
        }
        inner.emit(EngineEvent::PeerOffline { device_id: device.id });
        inner.emit(EngineEvent::DevicesChanged);
    }
}

async fn reoffer_shares(inner: &Arc<Inner>, peer: DeviceId) {
    let store = inner.store.clone();
    let offers = Inner::blocking(move || {
        let st = lock(&store);
        let mut v = Vec::new();
        for ws in st.unjoined_shares_with(peer)? {
            if let Ok(w) = st.get_workspace(ws) {
                v.push(w);
            }
        }
        Ok(v)
    })
    .await
    .unwrap_or_default();
    for w in offers {
        inner.send(peer, Message::WorkspaceOffer { ws: w.id, name: w.name, color: w.color, icon: w.icon }).await;
    }
}

/// Returns false to end the session.
async fn handle_frame(ctx: &Arc<Ctx>, frame: Frame) -> bool {
    let inner = &ctx.inner;
    let peer = ctx.peer;
    match frame {
        Frame::Chunk { transfer_id, offset, data } => on_chunk(ctx, transfer_id, offset, data).await,
        Frame::Msg(msg) => {
            match msg {
                Message::Ping { nonce } => {
                    let _ = ctx.tx.send(Message::Pong { nonce }).await;
                }
                Message::Pong { .. } | Message::Hello { .. } => {}
                Message::Bye { .. } => return false,
                Message::Error { code, message } => {
                    tracing::warn!(%code, %message, "peer reported an error");
                    if code == "revoked" {
                        let store = inner.store.clone();
                        let _ = Inner::blocking(move || Ok(lock(&store).revoke_device(peer)?)).await;
                        inner.emit(EngineEvent::DevicesChanged);
                        return false;
                    }
                }
                Message::WorkspaceOffer { ws, name, color, icon } => {
                    let store = inner.store.clone();
                    let n = name.clone();
                    let rejoin = Inner::blocking(move || {
                        let st = lock(&store);
                        // Re-offer of a workspace already shared both ways: just rejoin.
                        if st.get_workspace(ws).is_ok() && st.is_shared_with(ws, peer)? {
                            st.set_peer_joined(ws, peer, true)?;
                            return Ok(true);
                        }
                        let name = devbridge_core::validate_display_name(&n, devbridge_core::WORKSPACE_NAME_MAX_CHARS)
                            .unwrap_or_else(|_| "Shared project".into());
                        st.offer_upsert(&WorkspaceOffer {
                            workspace_id: ws,
                            device_id: peer,
                            device_name: String::new(),
                            name,
                            color,
                            icon,
                            offered_at: devbridge_core::time::now_ms(),
                        })?;
                        Ok(false)
                    })
                    .await;
                    match rejoin {
                        Ok(true) => {
                            let _ = ctx.tx.send(Message::WorkspaceJoined { ws }).await;
                            inner.ws_cmd(ws, WsCmd::PeerOnline(peer));
                        }
                        Ok(false) => inner.emit(EngineEvent::OfferReceived { workspace_id: ws, device_id: peer, name }),
                        Err(_) => {}
                    }
                }
                Message::WorkspaceJoined { ws } => {
                    let store = inner.store.clone();
                    if let Ok(true) = Inner::blocking(move || Ok(lock(&store).set_peer_joined(ws, peer, true)?)).await {
                        inner.ws_cmd(ws, WsCmd::PeerOnline(peer));
                        inner.emit(EngineEvent::WorkspaceChanged { workspace_id: ws });
                    }
                }
                Message::WorkspaceLeft { ws } => {
                    let store = inner.store.clone();
                    let _ = Inner::blocking(move || Ok(lock(&store).set_peer_joined(ws, peer, false)?)).await;
                    inner.ws_cmd(ws, WsCmd::StateChanged);
                    inner.emit(EngineEvent::WorkspaceChanged { workspace_id: ws });
                }
                Message::ManifestDigest { ws, digest, .. } => {
                    if authorized(ctx, ws).await {
                        inner.ws_cmd(ws, WsCmd::Digest { peer, digest });
                    }
                }
                Message::ManifestRequest { ws } => {
                    if authorized(ctx, ws).await {
                        inner.ws_cmd(ws, WsCmd::ManifestRequested(peer));
                    }
                }
                Message::Changes { ws, changes, .. } => {
                    if changes.len() > MAX_BATCH {
                        let _ = ctx
                            .tx
                            .send(Message::Error { code: "batch_too_large".into(), message: String::new() })
                            .await;
                        return false;
                    }
                    if authorized(ctx, ws).await {
                        inner.ws_cmd(ws, WsCmd::Incoming { peer, changes });
                    } else {
                        let _ = ctx
                            .tx
                            .send(Message::Error {
                                code: "not_shared".into(),
                                message: "That project isn't shared with this device.".into(),
                            })
                            .await;
                    }
                }
                Message::Acks { ws, acks } => {
                    let store = inner.store.clone();
                    let _ = Inner::blocking(move || {
                        let st = lock(&store);
                        for a in acks.iter().filter(|a| a.result != AckResult::Pending) {
                            let r = serde_json::to_value(a.result)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_owned))
                                .unwrap_or_default();
                            st.ack_change(peer, &a.change_id, &r, a.reason.as_deref())?;
                        }
                        Ok(())
                    })
                    .await;
                    inner.ws_cmd(ws, WsCmd::StateChanged);
                }
                Message::ContentRequest { transfer_id, ws, hash, offset } => {
                    if !is_valid_hash(&hash) || transfer_id.len() != 26 || !authorized(ctx, ws).await {
                        let _ = ctx
                            .tx
                            .send(Message::ContentError { transfer_id, code: "denied".into(), message: String::new() })
                            .await;
                    } else {
                        let c = ctx.clone();
                        let id = transfer_id.clone();
                        let task = tokio::spawn(async move { serve(c, id, ws, hash, offset).await });
                        guard(&ctx.serving).insert(transfer_id, task.abort_handle());
                    }
                }
                Message::ContentCancel { transfer_id } => {
                    if let Some(h) = guard(&ctx.serving).remove(&transfer_id) {
                        h.abort();
                    }
                }
                Message::ContentEnd { transfer_id, size } => {
                    tracing::debug!(transfer = %transfer_id, size, "content end");
                    let st = guard(&ctx.recv).remove(&transfer_id);
                    if let Some(mut st) = st {
                        let ok =
                            st.file.flush().await.is_ok() && st.file.sync_all().await.is_ok() && st.expected == size;
                        drop(st.file);
                        let cmd = if ok {
                            WsCmd::TransferDone { transfer_id }
                        } else {
                            WsCmd::TransferFailed { transfer_id, error: "transfer ended early".into(), retry: true }
                        };
                        inner.ws_cmd(st.ws, cmd);
                    }
                }
                Message::ContentError { transfer_id, code, message } => {
                    if let Some(st) = guard(&ctx.recv).remove(&transfer_id) {
                        // "unavailable": the sender has a newer version; it'll announce it.
                        let (error, retry) = if code == "unavailable" {
                            ("The other device no longer has this version".to_string(), false)
                        } else {
                            (format!("{code} {message}").trim().to_owned(), true)
                        };
                        inner.ws_cmd(st.ws, WsCmd::TransferFailed { transfer_id, error, retry });
                    }
                }
                m @ (Message::RemoteAccessRequest { .. }
                | Message::RemoteAccessGranted { .. }
                | Message::RemoteAccessDenied { .. }
                | Message::RemoteAccessRevoked {}) => {
                    if let Ok(v) = serde_json::to_value(&m) {
                        inner.emit(EngineEvent::Remote { device_id: peer, message: v });
                    }
                }
                Message::PairRequest { .. }
                | Message::PairResponse { .. }
                | Message::PairConfirm { .. }
                | Message::PairAccepted { .. }
                | Message::PairRejected { .. } => {}
            }
            true
        }
    }
}

async fn authorized(ctx: &Ctx, ws: WorkspaceId) -> bool {
    let store = ctx.inner.store.clone();
    let peer = ctx.peer;
    Inner::blocking(move || Ok(lock(&store).is_synced_with(ws, peer)?)).await.unwrap_or(false)
}

async fn on_chunk(ctx: &Arc<Ctx>, transfer_id: String, offset: u64, data: Vec<u8>) -> bool {
    let mut progress = None;
    let mut failed = None;
    {
        // Take the state out to write without holding the lock across await.
        let st = guard(&ctx.recv).remove(&transfer_id);
        let Some(mut st) = st else { return true };
        if offset != st.expected || st.expected + data.len() as u64 > st.total {
            failed = Some((st.ws, "out-of-order or oversized data".to_string()));
        } else if let Err(e) = st.file.write_all(&data).await {
            failed = Some((st.ws, format!("couldn't write: {e}")));
        } else {
            st.expected += data.len() as u64;
            if st.expected - st.last_report >= PROGRESS_EVERY {
                st.last_report = st.expected;
                progress = Some((st.ws, st.path.clone(), st.expected, st.total));
            }
            guard(&ctx.recv).insert(transfer_id.clone(), st);
        }
    }
    if let Some((ws, err)) = failed {
        let _ = ctx.tx.send(Message::ContentCancel { transfer_id: transfer_id.clone() }).await;
        ctx.inner.ws_cmd(ws, WsCmd::TransferFailed { transfer_id, error: err, retry: true });
        return true;
    }
    if let Some((ws, path, done, total)) = progress {
        let store = ctx.inner.store.clone();
        let id = transfer_id.clone();
        let _ = Inner::blocking(move || Ok(lock(&store).transfer_update(&id, "active", done, None)?)).await;
        ctx.inner.emit(EngineEvent::TransferProgress { workspace_id: ws, path, done, total });
    }
    true
}

/// Streams content with `hash` to the peer. Only serves files that are the
/// current content of a path in this workspace, or recovery copies that
/// belong to it.
async fn serve(ctx: Arc<Ctx>, transfer_id: String, ws: WorkspaceId, hash: String, offset: u64) {
    let _permit = ctx.serve_slots.clone().acquire_owned().await;
    let store = ctx.inner.store.clone();
    let recovery = ctx.inner.recovery.clone();
    let h = hash.clone();
    let source: Option<PathBuf> = Inner::blocking(move || {
        let st = lock(&store);
        let w = st.get_workspace(ws)?;
        if let Some(p) = st.manifest_find_hash(ws, &h)? {
            return Ok(Some(p.to_fs(std::path::Path::new(&w.root_path))));
        }
        let in_recovery = st.recovery_list(Some(ws))?.iter().any(|r| r.content_hash == h);
        Ok(in_recovery.then(|| recovery.blob_path(&h)).flatten())
    })
    .await
    .ok()
    .flatten();
    let result: std::io::Result<u64> = async {
        let Some(path) = source else { return Err(std::io::ErrorKind::NotFound.into()) };
        let mut f = tokio::fs::File::open(&path).await?;
        f.seek(std::io::SeekFrom::Start(offset)).await?;
        let mut pos = offset;
        let mut buf = vec![0u8; CHUNK_SIZE];
        loop {
            let n = f.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            if ctx.tx.send_chunk(&transfer_id, pos, buf[..n].to_vec()).await.is_err() {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            pos += n as u64;
        }
        Ok(pos)
    }
    .await;
    tracing::debug!(transfer = %transfer_id, ok = result.is_ok(), "finished serving content");
    let msg = match result {
        Ok(size) => Message::ContentEnd { transfer_id: transfer_id.clone(), size },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Message::ContentError {
            transfer_id: transfer_id.clone(),
            code: "unavailable".into(),
            message: String::new(),
        },
        Err(e) => Message::ContentError { transfer_id: transfer_id.clone(), code: "io".into(), message: e.to_string() },
    };
    // Must follow the chunks it terminates: control frames have priority.
    let _ = ctx.tx.send_after_chunks(msg).await;
    guard(&ctx.serving).remove(&transfer_id);
}

/// Starts queued transfers from this peer, at most `max_transfers` at once.
async fn transfer_scheduler(ctx: Arc<Ctx>, mut jobs: mpsc::UnboundedReceiver<TransferJob>) {
    let limit = guard(&ctx.inner.tuning).max_transfers.max(1);
    let slots = Arc::new(Semaphore::new(limit));
    while let Some(job) = jobs.recv().await {
        tracing::debug!(transfer = %job.id, "transfer queued");
        if guard(&ctx.recv).contains_key(&job.id) {
            continue; // already running
        }
        let Ok(permit) = slots.clone().acquire_owned().await else { return };
        if let Err((error, retry)) = start_transfer(&ctx, &job, permit).await {
            ctx.inner.ws_cmd(job.workspace_id, WsCmd::TransferFailed { transfer_id: job.id.clone(), error, retry });
        }
    }
}

async fn start_transfer(ctx: &Arc<Ctx>, job: &TransferJob, permit: OwnedSemaphorePermit) -> Result<(), (String, bool)> {
    let store = ctx.inner.store.clone();
    let id = job.id.clone();
    let current = Inner::blocking(move || Ok(lock(&store).transfer_get(&id)?)).await.ok().flatten();
    let Some(job) = current.filter(|j| matches!(j.state.as_str(), "pending" | "active")) else { return Ok(()) };
    let max = guard(&ctx.inner.tuning).max_file_size;
    if job.total_size > max {
        return Err((format!("It's larger than the maximum file size in Settings ({} MB).", max / 1024 / 1024), false));
    }
    let staging = {
        let ws_dir = ctx.inner.staging_root().join(job.workspace_id.to_string());
        let store = ctx.inner.store.clone();
        let wsid = job.workspace_id;
        let root = Inner::blocking(move || Ok(lock(&store).get_workspace(wsid)?.root_path))
            .await
            .map_err(|e| (e.to_string(), false))?;
        devbridge_fs::apply::staging_dir(std::path::Path::new(&root), &ws_dir)
    };
    tokio::fs::create_dir_all(&staging).await.map_err(|e| (e.to_string(), true))?;
    let path = staging.join(&job.id);
    let existing = tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0);
    let offset = if existing <= job.total_size { existing } else { 0 };
    let free = devbridge_fs::available_space(&staging).unwrap_or(u64::MAX);
    if free < job.total_size - offset + DISK_RESERVE {
        return Err((
            format!(
                "Not enough free disk space ({} MB needed). Free up space, then choose Retry.",
                (job.total_size - offset) / 1024 / 1024
            ),
            false,
        ));
    }
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .await
        .map_err(|e| (e.to_string(), true))?;
    file.set_len(offset).await.map_err(|e| (e.to_string(), true))?;
    let mut file = file;
    file.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| (e.to_string(), true))?;
    {
        let store = ctx.inner.store.clone();
        let id = job.id.clone();
        let _ = Inner::blocking(move || Ok(lock(&store).transfer_update(&id, "active", offset, None)?)).await;
    }
    guard(&ctx.recv).insert(
        job.id.clone(),
        RecvState {
            ws: job.workspace_id,
            path: job.path.to_string(),
            file,
            expected: offset,
            total: job.total_size,
            last_report: offset,
            _permit: permit,
        },
    );
    tracing::debug!(transfer = %job.id, path = %job.path, offset, total = job.total_size, "requesting content");
    let req = Message::ContentRequest {
        transfer_id: job.id.clone(),
        ws: job.workspace_id,
        hash: job.content_hash.clone(),
        offset,
    };
    if ctx.tx.send(req).await.is_err() {
        guard(&ctx.recv).remove(&job.id);
        return Err(("connection closed".into(), true));
    }
    Ok(())
}
