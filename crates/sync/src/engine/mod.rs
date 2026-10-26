//! The running sync engine: listener, sessions with paired devices, one actor
//! per workspace, pairing, discovery and reconnection.

mod pair;
mod peer;
mod ws;

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devbridge_core::protocol::{DeviceInfo, Message};
use devbridge_core::{DeviceId, OsKind, WorkspaceId};
use devbridge_fs::recovery::RecoveryStore;
use devbridge_fs::rules::RuleConfig;
use devbridge_net::discovery::{Discovered, Discovery};
use devbridge_net::{Identity, Listener, SessionTx, TlsConn};
use devbridge_store::{Store, TransferJob};
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::apply::{ConflictContents, Resolution};
use crate::{SyncError, lock};

pub use pair::PairingWindow;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenConfig {
    /// Accept connections from the local network (otherwise loopback only).
    pub lan: bool,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct Tuning {
    pub rules: RuleConfig,
    pub max_transfers: usize,
    pub max_file_size: u64,
    pub reconcile_every: Duration,
    pub recovery_retention: Duration,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            rules: RuleConfig { respect_gitignore: true, ..Default::default() },
            max_transfers: 4,
            max_file_size: 2048 * 1024 * 1024,
            reconcile_every: Duration::from_secs(600),
            recovery_retention: Duration::from_secs(30 * 86400),
        }
    }
}

pub struct EngineConfig {
    pub data_dir: PathBuf,
    pub store: Arc<Mutex<Store>>,
    pub identity: Identity,
    pub device_id: DeviceId,
    pub device_name: String,
    pub os: OsKind,
    pub app_version: String,
    pub listen: ListenConfig,
    pub discovery: bool,
    pub tuning: Tuning,
}

/// Events for the UI. Never contain file contents.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum EngineEvent {
    #[serde(rename_all = "camelCase")]
    WorkspaceChanged {
        workspace_id: WorkspaceId,
    },
    DevicesChanged,
    #[serde(rename_all = "camelCase")]
    PeerOnline {
        device_id: DeviceId,
    },
    #[serde(rename_all = "camelCase")]
    PeerOffline {
        device_id: DeviceId,
    },
    #[serde(rename_all = "camelCase")]
    PairingRequest {
        request_id: String,
        device_name: String,
        os: OsKind,
        verification_code: String,
    },
    #[serde(rename_all = "camelCase")]
    PairingVerification {
        verification_code: String,
    },
    #[serde(rename_all = "camelCase")]
    PairingClosed {
        reason: String,
    },
    #[serde(rename_all = "camelCase")]
    OfferReceived {
        workspace_id: WorkspaceId,
        device_id: DeviceId,
        name: String,
    },
    #[serde(rename_all = "camelCase")]
    ConflictDetected {
        workspace_id: WorkspaceId,
        path: String,
    },
    #[serde(rename_all = "camelCase")]
    TransferProgress {
        workspace_id: WorkspaceId,
        path: String,
        done: u64,
        total: u64,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        workspace_id: Option<WorkspaceId>,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    ListenerChanged {
        address: Option<String>,
        error: Option<String>,
    },
    /// Remote-workspace protocol message for the remote module.
    #[serde(rename_all = "camelCase")]
    Remote {
        device_id: DeviceId,
        message: serde_json::Value,
    },
}

#[derive(Clone)]
pub(crate) struct PeerHandle {
    pub session_id: u64,
    pub tx: SessionTx,
    pub outbound: bool,
    pub transfers: mpsc::UnboundedSender<TransferJob>,
}

pub(crate) struct WsHandle {
    pub tx: mpsc::UnboundedSender<ws::WsCmd>,
    pub paused: Arc<AtomicBool>,
}

struct ListenerState {
    task: JoinHandle<()>,
    addr: SocketAddr,
}

struct Backoff {
    next: Instant,
    delay: Duration,
}

pub(crate) struct Inner {
    pub store: Arc<Mutex<Store>>,
    pub identity: Identity,
    pub device_id: DeviceId,
    pub device_name: Mutex<String>,
    pub os: OsKind,
    pub app_version: String,
    pub data_dir: PathBuf,
    pub recovery: Arc<RecoveryStore>,
    pub events: broadcast::Sender<EngineEvent>,
    pub peers: Mutex<HashMap<DeviceId, PeerHandle>>,
    pub workspaces: Mutex<HashMap<WorkspaceId, WsHandle>>,
    pub tuning: Mutex<Tuning>,
    listen: Mutex<ListenConfig>,
    listener: Mutex<Option<ListenerState>>,
    conn_tx: mpsc::Sender<TlsConn>,
    pub pairing: Mutex<Option<pair::PairingWindow>>,
    pub approvals: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    pub discovered: Mutex<HashMap<String, Discovered>>,
    discovery: Mutex<Option<Discovery>>,
    backoff: Mutex<HashMap<DeviceId, Backoff>>,
    session_seq: AtomicU64,
    shutdown: AtomicBool,
    /// The runtime the engine was started on, entered by sync methods that
    /// spawn tasks (callers such as Tauri's sync commands run outside it).
    rt: tokio::runtime::Handle,
}

fn guard<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Inner {
    pub fn emit(&self, e: EngineEvent) {
        let _ = self.events.send(e);
    }

    pub fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            device_id: self.device_id,
            name: guard(&self.device_name).clone(),
            os: self.os,
            app_version: self.app_version.clone(),
        }
    }

    pub fn peer(&self, id: DeviceId) -> Option<PeerHandle> {
        guard(&self.peers).get(&id).cloned()
    }

    pub fn is_online(&self, id: DeviceId) -> bool {
        guard(&self.peers).contains_key(&id)
    }

    pub async fn send(&self, to: DeviceId, msg: Message) -> bool {
        match self.peer(to) {
            Some(p) => p.tx.send(msg).await.is_ok(),
            None => false,
        }
    }

    pub fn ws_cmd(&self, ws: WorkspaceId, cmd: ws::WsCmd) -> bool {
        guard(&self.workspaces).get(&ws).is_some_and(|h| h.tx.send(cmd).is_ok())
    }

    pub fn is_paused(&self, ws: WorkspaceId) -> bool {
        guard(&self.workspaces).get(&ws).is_some_and(|h| h.paused.load(Ordering::Relaxed))
    }

    pub fn listen_port(&self) -> Option<u16> {
        guard(&self.listener).as_ref().map(|l| l.addr.port())
    }

    pub fn staging_root(&self) -> PathBuf {
        self.data_dir.join("staging")
    }

    pub async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, SyncError> + Send + 'static,
    ) -> Result<T, SyncError> {
        tokio::task::spawn_blocking(f).await.map_err(|e| SyncError::Other(e.to_string()))?
    }
}

/// Handle to the running engine. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub listen_address: Option<String>,
    pub lan: bool,
    pub port: u16,
    pub online_peers: Vec<DeviceId>,
    pub fingerprint: String,
}

impl Engine {
    pub async fn start(cfg: EngineConfig) -> Result<Self, SyncError> {
        let recovery = Arc::new(RecoveryStore::new(cfg.data_dir.join("recovery"))?);
        std::fs::create_dir_all(cfg.data_dir.join("staging"))?;
        {
            let s = lock(&cfg.store);
            s.sessions_mark_interrupted()?;
            s.transfers_reset_active()?;
            cleanup_staging(&s, &cfg.data_dir);
        }
        let (events, _) = broadcast::channel(512);
        let (conn_tx, conn_rx) = mpsc::channel(32);
        let inner = Arc::new(Inner {
            store: cfg.store,
            identity: cfg.identity,
            device_id: cfg.device_id,
            device_name: Mutex::new(cfg.device_name),
            os: cfg.os,
            app_version: cfg.app_version,
            data_dir: cfg.data_dir,
            recovery,
            events,
            peers: Mutex::new(HashMap::new()),
            workspaces: Mutex::new(HashMap::new()),
            tuning: Mutex::new(cfg.tuning),
            listen: Mutex::new(cfg.listen),
            listener: Mutex::new(None),
            conn_tx,
            pairing: Mutex::new(None),
            approvals: Mutex::new(HashMap::new()),
            discovered: Mutex::new(HashMap::new()),
            discovery: Mutex::new(None),
            backoff: Mutex::new(HashMap::new()),
            session_seq: AtomicU64::new(1),
            shutdown: AtomicBool::new(false),
            rt: tokio::runtime::Handle::current(),
        });
        let engine = Engine { inner };
        engine.spawn_accept_loop(conn_rx);
        engine.restart_listener().await;
        if cfg.discovery {
            engine.start_discovery();
        }
        let workspaces = lock(&engine.inner.store).list_workspaces()?;
        for w in workspaces {
            engine.workspace_added(w.id);
        }
        engine.spawn_connector();
        engine.spawn_housekeeping();
        Ok(engine)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    pub fn device_id(&self) -> DeviceId {
        self.inner.device_id
    }

    pub fn fingerprint(&self) -> &str {
        &self.inner.identity.fingerprint
    }

    pub fn set_device_name(&self, name: &str) {
        *guard(&self.inner.device_name) = name.to_owned();
    }

    pub fn status(&self) -> EngineStatus {
        let l = *guard(&self.inner.listen);
        EngineStatus {
            listen_address: guard(&self.inner.listener).as_ref().map(|l| l.addr.to_string()),
            lan: l.lan,
            port: l.port,
            online_peers: guard(&self.inner.peers).keys().copied().collect(),
            fingerprint: self.inner.identity.fingerprint.clone(),
        }
    }

    pub fn is_online(&self, device: DeviceId) -> bool {
        self.inner.is_online(device)
    }

    pub fn discovered(&self) -> Vec<Discovered> {
        guard(&self.inner.discovered).values().cloned().collect()
    }

    // ---------------- listener / connections ----------------

    /// Applies new listen settings (rebinding if needed).
    pub async fn set_listen(&self, cfg: ListenConfig) {
        let changed = {
            let mut l = guard(&self.inner.listen);
            let changed = *l != cfg;
            *l = cfg;
            changed
        };
        if changed {
            self.restart_listener().await;
        }
    }

    pub fn listen_config(&self) -> ListenConfig {
        *guard(&self.inner.listen)
    }

    async fn restart_listener(&self) {
        if let Some(old) = guard(&self.inner.listener).take() {
            old.task.abort();
        }
        let cfg = *guard(&self.inner.listen);
        let ip = if cfg.lan { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
        match Listener::bind(SocketAddr::new(ip, cfg.port), &self.inner.identity).await {
            Ok(l) => {
                let addr = l.local_addr().unwrap_or(SocketAddr::new(ip, cfg.port));
                let task = tokio::spawn(l.run(self.inner.conn_tx.clone()));
                *guard(&self.inner.listener) = Some(ListenerState { task, addr });
                tracing::info!(%addr, lan = cfg.lan, "listening for paired devices");
                self.inner.emit(EngineEvent::ListenerChanged { address: Some(addr.to_string()), error: None });
            }
            Err(e) => {
                let msg = if e.kind() == std::io::ErrorKind::AddrInUse {
                    format!("Port {} is already in use. Choose another port in Settings → Network.", cfg.port)
                } else {
                    format!("Couldn't listen for connections: {e}")
                };
                tracing::warn!(error = %e, "listener failed");
                self.inner.emit(EngineEvent::ListenerChanged { address: None, error: Some(msg) });
            }
        }
    }

    fn spawn_accept_loop(&self, mut rx: mpsc::Receiver<TlsConn>) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            while let Some(conn) = rx.recv().await {
                let inner = inner.clone();
                tokio::spawn(async move { peer::handle_connection(inner, conn, None).await });
            }
        });
    }

    fn start_discovery(&self) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let port = self.listen_config().port;
        match Discovery::start(&self.inner.device_id.to_string(), port, tx) {
            Ok(d) => *guard(&self.inner.discovery) = Some(d),
            Err(e) => {
                tracing::warn!(error = %e, "LAN discovery unavailable");
                return;
            }
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            while let Some(d) = rx.recv().await {
                guard(&inner.discovered).insert(d.device_id.clone(), d);
                inner.emit(EngineEvent::DevicesChanged);
            }
        });
    }

    pub(crate) fn advertise_pairing(&self, name: Option<&str>) {
        if let Some(d) = guard(&self.inner.discovery).as_mut() {
            let _ = d.advertise(name);
        }
    }

    /// Dials paired devices that aren't connected, with per-device
    /// exponential backoff (1 s … 60 s, jittered).
    fn spawn_connector(&self) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(2));
            loop {
                tick.tick().await;
                if inner.shutdown.load(Ordering::Relaxed) {
                    return;
                }
                let devices = match Inner::blocking({
                    let store = inner.store.clone();
                    move || Ok(lock(&store).paired_devices()?)
                })
                .await
                {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                for d in devices {
                    if inner.is_online(d.id) {
                        guard(&inner.backoff).remove(&d.id);
                        continue;
                    }
                    let Some(fp) = d.cert_fingerprint.clone() else { continue };
                    {
                        let b = guard(&inner.backoff);
                        if b.get(&d.id).is_some_and(|b| b.next > Instant::now()) {
                            continue;
                        }
                    }
                    let mut candidates: Vec<SocketAddr> =
                        guard(&inner.discovered).get(&d.id.to_string()).map(|x| x.addrs.clone()).unwrap_or_default();
                    let stored = lock(&inner.store).device_address(d.id).ok().flatten();
                    if let Some(a) = stored.and_then(|s| s.parse().ok())
                        && !candidates.contains(&a)
                    {
                        candidates.push(a);
                    }
                    if candidates.is_empty() {
                        continue;
                    }
                    // Reserve the attempt (and schedule the next) before dialing.
                    {
                        let mut b = guard(&inner.backoff);
                        let e =
                            b.entry(d.id).or_insert(Backoff { next: Instant::now(), delay: Duration::from_secs(1) });
                        let jitter = rand::random_range(0..=e.delay.as_millis() as u64 / 2);
                        e.next = Instant::now() + e.delay + Duration::from_millis(jitter);
                        e.delay = (e.delay * 2).min(Duration::from_secs(60));
                    }
                    let inner = inner.clone();
                    let device_id = d.id;
                    tokio::spawn(async move {
                        for addr in candidates {
                            match devbridge_net::connect(addr, &inner.identity, Some(&fp)).await {
                                Ok(conn) => {
                                    peer::handle_connection(inner.clone(), conn, Some(device_id)).await;
                                    return;
                                }
                                Err(e) => tracing::debug!(%addr, error = %e, "dial failed"),
                            }
                        }
                    });
                }
            }
        });
    }

    /// Periodic reconcile and recovery-store cleanup.
    fn spawn_housekeeping(&self) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut last_reconcile = Instant::now();
            let mut tick = tokio::time::interval(Duration::from_secs(30));
            loop {
                tick.tick().await;
                if inner.shutdown.load(Ordering::Relaxed) {
                    return;
                }
                let (every, retention) = {
                    let t = guard(&inner.tuning);
                    (t.reconcile_every, t.recovery_retention)
                };
                if last_reconcile.elapsed() >= every {
                    last_reconcile = Instant::now();
                    let ids: Vec<WorkspaceId> = guard(&inner.workspaces).keys().copied().collect();
                    for id in ids {
                        inner.ws_cmd(id, ws::WsCmd::Rescan { announce: true });
                    }
                }
                let store = inner.store.clone();
                let recovery = inner.recovery.clone();
                let _ = Inner::blocking(move || {
                    let cutoff = devbridge_core::time::now_ms() - retention.as_millis() as i64;
                    let s = lock(&store);
                    for id in s.recovery_expired(cutoff)? {
                        if let Some(h) = s.recovery_remove(id)? {
                            let _ = recovery.remove(&h);
                        }
                    }
                    Ok(())
                })
                .await;
            }
        });
    }

    pub fn shutdown(&self) {
        self.inner.shutdown.store(true, Ordering::Relaxed);
        for (_, p) in guard(&self.inner.peers).drain() {
            p.tx.close();
        }
        for (_, w) in guard(&self.inner.workspaces).drain() {
            let _ = w.tx.send(ws::WsCmd::Shutdown);
        }
        if let Some(l) = guard(&self.inner.listener).take() {
            l.task.abort();
        }
        guard(&self.inner.discovery).take();
    }

    // ---------------- workspaces ----------------

    /// Starts syncing a registered workspace (no-op if already running).
    pub fn workspace_added(&self, ws: WorkspaceId) {
        let _rt = self.inner.rt.enter();
        let mut map = guard(&self.inner.workspaces);
        if map.contains_key(&ws) {
            return;
        }
        match ws::spawn(self.inner.clone(), ws) {
            Ok(h) => {
                map.insert(ws, h);
            }
            Err(e) => {
                tracing::warn!(workspace = %ws, error = %e, "workspace not started");
                drop(map);
                self.inner.emit(EngineEvent::Error { workspace_id: Some(ws), message: e.to_string() });
            }
        }
    }

    /// Stops syncing a workspace (its registration was removed).
    pub fn workspace_removed(&self, ws: WorkspaceId) {
        if let Some(h) = guard(&self.inner.workspaces).remove(&ws) {
            let _ = h.tx.send(ws::WsCmd::Shutdown);
        }
    }

    pub fn rescan(&self, ws: WorkspaceId) {
        self.inner.ws_cmd(ws, ws::WsCmd::Rescan { announce: true });
    }

    pub fn set_paused(&self, ws: WorkspaceId, paused: bool) {
        if let Some(h) = guard(&self.inner.workspaces).get(&ws) {
            h.paused.store(paused, Ordering::Relaxed);
            let _ = h.tx.send(ws::WsCmd::PausedChanged);
        }
    }

    pub fn is_paused(&self, ws: WorkspaceId) -> bool {
        self.inner.is_paused(ws)
    }

    /// Re-requests failed transfers for a workspace.
    pub fn retry(&self, ws: WorkspaceId) {
        self.inner.ws_cmd(ws, ws::WsCmd::Retry);
    }

    pub fn set_tuning(&self, t: Tuning) {
        *guard(&self.inner.tuning) = t;
        let ids: Vec<WorkspaceId> = guard(&self.inner.workspaces).keys().copied().collect();
        for id in ids {
            self.inner.ws_cmd(id, ws::WsCmd::RulesChanged);
        }
    }

    // ---------------- sharing ----------------

    /// Offers a workspace to a paired device.
    pub async fn share(&self, ws: WorkspaceId, peer: DeviceId) -> Result<(), SyncError> {
        let store = self.inner.store.clone();
        let w = Inner::blocking(move || {
            let s = lock(&store);
            s.share_workspace(ws, peer)?;
            Ok(s.get_workspace(ws)?)
        })
        .await?;
        self.inner.send(peer, Message::WorkspaceOffer { ws, name: w.name, color: w.color, icon: w.icon }).await;
        self.inner.emit(EngineEvent::WorkspaceChanged { workspace_id: ws });
        Ok(())
    }

    /// Stops syncing a workspace with a device (both directions).
    pub async fn unshare(&self, ws: WorkspaceId, peer: DeviceId) -> Result<(), SyncError> {
        let store = self.inner.store.clone();
        Inner::blocking(move || Ok(lock(&store).unshare_workspace(ws, peer)?)).await?;
        self.inner.send(peer, Message::WorkspaceLeft { ws }).await;
        self.inner.ws_cmd(ws, ws::WsCmd::StateChanged);
        Ok(())
    }

    /// Accepts an offer. The caller has already registered the workspace
    /// locally with the offered id and a validated root folder.
    pub async fn join(&self, ws: WorkspaceId, peer: DeviceId) -> Result<(), SyncError> {
        let store = self.inner.store.clone();
        Inner::blocking(move || {
            let s = lock(&store);
            s.share_workspace(ws, peer)?;
            s.set_peer_joined(ws, peer, true)?;
            s.offer_delete(ws, peer)?;
            Ok(())
        })
        .await?;
        self.workspace_added(ws);
        self.inner.send(peer, Message::WorkspaceJoined { ws }).await;
        self.inner.ws_cmd(ws, ws::WsCmd::PeerOnline(peer));
        Ok(())
    }

    // ---------------- devices ----------------

    pub async fn revoke(&self, device: DeviceId) -> Result<(), SyncError> {
        let store = self.inner.store.clone();
        Inner::blocking(move || Ok(lock(&store).revoke_device(device)?)).await?;
        let session = guard(&self.inner.peers).remove(&device);
        if let Some(p) = session {
            let _ =
                p.tx.send(Message::Error {
                    code: "revoked".into(),
                    message: "This device was removed by its owner.".into(),
                })
                .await;
            p.tx.close();
        }
        self.inner.emit(EngineEvent::DevicesChanged);
        Ok(())
    }

    /// Sends a remote-workspace message to a connected peer.
    pub async fn send_remote(&self, device: DeviceId, msg: Message) -> bool {
        self.inner.send(device, msg).await
    }

    // ---------------- pairing ----------------

    pub fn pairing_open(&self) -> Result<PairingWindow, SyncError> {
        let _rt = self.inner.rt.enter();
        pair::open(self)
    }

    pub fn pairing_close(&self) {
        pair::close(&self.inner, "Cancelled");
        self.advertise_pairing(None);
    }

    pub fn pairing_current(&self) -> Option<PairingWindow> {
        guard(&self.inner.pairing).clone()
    }

    pub fn pairing_decide(&self, request_id: &str, approve: bool) -> bool {
        guard(&self.inner.approvals).remove(request_id).is_some_and(|tx| tx.send(approve).is_ok())
    }

    /// Joins a device that has a pairing window open. Resolves after the other
    /// person approves (or declines, or 3 minutes pass).
    pub async fn pair_join(&self, addr: SocketAddr, code: &str) -> Result<devbridge_core::Device, SyncError> {
        pair::join(self.inner.clone(), addr, code).await
    }

    // ---------------- conflicts ----------------

    pub async fn resolve_conflict(&self, ws: WorkspaceId, conflict_id: &str, res: Resolution) -> Result<(), SyncError> {
        let (tx, rx) = oneshot::channel();
        if !self.inner.ws_cmd(ws, ws::WsCmd::Resolve { conflict_id: conflict_id.to_owned(), res, reply: tx }) {
            return Err(SyncError::Other("This project isn't active.".into()));
        }
        rx.await.map_err(|_| SyncError::Other("The project stopped while resolving.".into()))?
    }

    pub async fn conflict_contents(
        &self,
        ws: WorkspaceId,
        conflict_id: &str,
        max: u64,
    ) -> Result<ConflictContents, SyncError> {
        let (tx, rx) = oneshot::channel();
        if !self.inner.ws_cmd(ws, ws::WsCmd::Contents { conflict_id: conflict_id.to_owned(), max, reply: tx }) {
            return Err(SyncError::Other("This project isn't active.".into()));
        }
        rx.await.map_err(|_| SyncError::Other("The project stopped.".into()))?
    }

    pub fn recovery(&self) -> &RecoveryStore {
        &self.inner.recovery
    }

    pub(crate) fn inner(&self) -> &Arc<Inner> {
        &self.inner
    }
}

impl Inner {
    pub(crate) fn next_session_id(&self) -> u64 {
        self.session_seq.fetch_add(1, Ordering::Relaxed)
    }
}

/// Startup recovery: removes staging leftovers that no unfinished transfer
/// owns (e.g. after a crash) and half-written recovery copies. Partial files
/// of unfinished transfers are kept so they can resume.
fn cleanup_staging(s: &Store, data_dir: &std::path::Path) {
    let Ok(workspaces) = s.list_workspaces() else { return };
    for w in workspaces {
        let keep: std::collections::HashSet<String> =
            s.transfers_list(w.id).unwrap_or_default().into_iter().map(|t| t.id).collect();
        let dir = devbridge_fs::apply::staging_dir(
            std::path::Path::new(&w.root_path),
            &data_dir.join("staging").join(w.id.to_string()),
        );
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !keep.contains(&name) && e.file_type().is_ok_and(|t| t.is_file()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(data_dir.join("recovery").join("tmp")) {
        for e in rd.flatten() {
            let _ = std::fs::remove_file(e.path());
        }
    }
}
