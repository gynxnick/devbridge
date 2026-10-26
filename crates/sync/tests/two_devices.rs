//! Two real engines ("mac" and "pc") with separate folders, databases and
//! identities, connected over loopback TLS. Covers the spec's integration
//! scenarios: propagation both ways, rename/delete, concurrent edits,
//! interrupted transfers, offline changes, Git independence, secret
//! exclusion, case collisions, revocation and restart recovery.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devbridge_core::time::now_ms;
use devbridge_core::{
    DeviceId, OsKind, SyncState, Workspace, WorkspaceColor, WorkspaceIcon, WorkspaceId, WorkspaceMode,
};
use devbridge_net::{Identity, MemoryStore};
use devbridge_store::Store;
use devbridge_sync::apply::Resolution;
use devbridge_sync::{Engine, EngineConfig, EngineEvent, ListenConfig, Tuning};
use tempfile::TempDir;

struct Node {
    _tmp: TempDir,
    data: PathBuf,
    root: PathBuf,
    store: Arc<Mutex<Store>>,
    identity: Identity,
    id: DeviceId,
    name: &'static str,
    engine: Engine,
    port: u16,
}

fn tuning() -> Tuning {
    Tuning { reconcile_every: Duration::from_secs(3600), ..Tuning::default() }
}

async fn start_engine(
    data: &Path,
    store: Arc<Mutex<Store>>,
    identity: Identity,
    id: DeviceId,
    name: &str,
    port: u16,
) -> Engine {
    Engine::start(EngineConfig {
        data_dir: data.to_path_buf(),
        store,
        identity,
        device_id: id,
        device_name: name.into(),
        os: OsKind::current(),
        app_version: "test".into(),
        listen: ListenConfig { lan: false, port },
        discovery: false,
        tuning: tuning(),
    })
    .await
    .unwrap()
}

fn init_logs() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("DEVBRIDGE_LOG").unwrap_or_else(|_| "off".into()))
        .with_test_writer()
        .try_init();
}

impl Node {
    async fn new(name: &'static str) -> Self {
        init_logs();
        let tmp = tempfile::tempdir().unwrap();
        let base = devbridge_core::fsutil::canonicalize(tmp.path()).unwrap();
        let data = base.join("data");
        let root = base.join("project");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let store = Store::open(&data.join("db.sqlite3")).unwrap();
        let id = store.ensure_local_device(name, OsKind::current(), None).unwrap().id;
        let store = Arc::new(Mutex::new(store));
        let identity = Identity::load_or_create(&MemoryStore::default()).unwrap();
        let engine = start_engine(&data, store.clone(), identity.clone(), id, name, 0).await;
        let port: u16 = engine.status().listen_address.unwrap().parse::<SocketAddr>().unwrap().port();
        Node { _tmp: tmp, data, root, store, identity, id, name, engine, port }
    }

    /// Simulates quitting and relaunching the app (same data, identity, port).
    async fn restart(&mut self) {
        self.engine.shutdown();
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.engine =
            start_engine(&self.data, self.store.clone(), self.identity.clone(), self.id, self.name, self.port).await;
    }

    fn write(&self, p: &str, data: &[u8]) {
        let f = self.root.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, data).unwrap();
    }

    fn read(&self, p: &str) -> Option<Vec<u8>> {
        std::fs::read(self.root.join(p)).ok()
    }

    fn register(&self, ws: WorkspaceId) {
        self.store
            .lock()
            .unwrap()
            .insert_workspace(&Workspace {
                id: ws,
                name: "Demo".into(),
                root_path: self.root.display().to_string(),
                color: WorkspaceColor::Blue,
                icon: WorkspaceIcon::Code,
                mode: WorkspaceMode::Synced,
                sync_state: SyncState::LocalOnly,
                active_device_id: Some(self.id),
                last_synced_at: None,
                created_at: now_ms(),
                updated_at: now_ms(),
            })
            .unwrap();
    }

    fn open_conflicts(&self, ws: WorkspaceId) -> Vec<devbridge_store::ConflictRecord> {
        self.store.lock().unwrap().conflicts_list(Some(ws), true).unwrap()
    }
}

async fn eventually(what: &str, timeout_s: u64, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_s);
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

async fn pair(a: &Node, b: &Node) {
    let mut events = a.engine.subscribe();
    let w = a.engine.pairing_open().unwrap();
    let addr: SocketAddr = format!("127.0.0.1:{}", w.port).parse().unwrap();
    let joiner = b.engine.clone();
    let code = w.code.clone();
    let join = tokio::spawn(async move { joiner.pair_join(addr, &code).await });
    let approve = async {
        loop {
            if let Ok(EngineEvent::PairingRequest { request_id, device_name, .. }) = events.recv().await {
                assert_eq!(device_name, b.name);
                assert!(a.engine.pairing_decide(&request_id, true));
                return;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), approve).await.expect("pairing request");
    let dev = join.await.unwrap().unwrap();
    assert_eq!(dev.id, a.id);
    eventually("devices connected", 20, || a.engine.is_online(b.id) && b.engine.is_online(a.id)).await;
}

/// Pairs, registers the workspace on `a`, shares it, and joins on `b`.
async fn setup() -> (Node, Node, WorkspaceId) {
    let a = Node::new("mac").await;
    let b = Node::new("pc").await;
    pair(&a, &b).await;
    let ws = WorkspaceId::new();
    a.register(ws);
    a.engine.workspace_added(ws);
    let mut ev = b.engine.subscribe();
    a.engine.share(ws, b.id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(EngineEvent::OfferReceived { workspace_id, .. }) = ev.recv().await
                && workspace_id == ws
            {
                return;
            }
        }
    })
    .await
    .expect("offer received");
    b.register(ws);
    b.engine.join(ws, a.id).await.unwrap();
    (a, b, ws)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changes_propagate_both_ways_including_rename_and_delete() {
    let (a, b, ws) = setup().await;
    a.write("src/main.rs", b"fn main() {}\n");
    a.write("README.md", b"# Demo\n");
    eventually("mac → pc", 20, || b.read("src/main.rs").as_deref() == Some(b"fn main() {}\n")).await;
    eventually("README", 20, || b.read("README.md").is_some()).await;

    b.write("src/lib.rs", b"pub fn x() {}\n");
    eventually("pc → mac", 20, || a.read("src/lib.rs").is_some()).await;

    a.write("src/main.rs", b"fn main() { println!(\"hi\"); }\n");
    eventually("edit propagates", 20, || {
        b.read("src/main.rs").as_deref() == Some(&b"fn main() { println!(\"hi\"); }\n"[..])
    })
    .await;

    std::fs::rename(a.root.join("README.md"), a.root.join("docs.md")).unwrap();
    eventually("rename propagates", 20, || {
        b.read("README.md").is_none() && b.read("docs.md").as_deref() == Some(b"# Demo\n")
    })
    .await;

    std::fs::remove_file(b.root.join("src/lib.rs")).unwrap();
    eventually("delete propagates", 20, || a.read("src/lib.rs").is_none()).await;

    // Deleted content is kept in the recovery history.
    let rec = a.store.lock().unwrap().recovery_list(Some(ws)).unwrap();
    assert!(rec.iter().any(|r| r.path == "src/lib.rs" && r.reason == "deleted"), "{rec:?}");
    assert!(a.open_conflicts(ws).is_empty() && b.open_conflicts(ws).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_become_a_recoverable_conflict() {
    let (mut a, mut b, ws) = setup().await;
    a.write("notes.txt", b"base\n");
    eventually("base synced", 20, || b.read("notes.txt").is_some()).await;

    // Disconnect both (simulate offline), edit independently, reconnect.
    a.engine.shutdown();
    b.engine.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;
    a.write("notes.txt", b"mac edit\n");
    b.write("notes.txt", b"pc edit\n");
    a.restart().await;
    b.restart().await;

    eventually("conflict on both devices", 30, || !a.open_conflicts(ws).is_empty() && !b.open_conflicts(ws).is_empty())
        .await;
    // Nothing was overwritten.
    assert_eq!(a.read("notes.txt").as_deref(), Some(&b"mac edit\n"[..]));
    assert_eq!(b.read("notes.txt").as_deref(), Some(&b"pc edit\n"[..]));
    let c = a.open_conflicts(ws).remove(0);
    let (local, remote) = a.engine.conflict_contents(ws, &c.id, 1 << 20).await.unwrap();
    assert_eq!(local.as_deref(), Some(&b"mac edit\n"[..]));
    assert_eq!(remote.as_deref(), Some(&b"pc edit\n"[..]));

    // Resolve on the mac by keeping the PC's version: both converge, and the
    // PC's conflict closes automatically.
    a.engine.resolve_conflict(ws, &c.id, Resolution::KeepRemote).await.unwrap();
    assert_eq!(a.read("notes.txt").as_deref(), Some(&b"pc edit\n"[..]));
    eventually("pc conflict resolved elsewhere", 30, || b.open_conflicts(ws).is_empty()).await;
    assert_eq!(b.read("notes.txt").as_deref(), Some(&b"pc edit\n"[..]));
    // The mac's own edit is still recoverable.
    let rec = a.store.lock().unwrap().recovery_list(Some(ws)).unwrap();
    let mac_copy = rec.iter().find(|r| r.reason == "conflict_local").unwrap();
    assert_eq!(a.engine.recovery().read(&mac_copy.content_hash).unwrap(), b"mac edit\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn save_both_and_merge_resolutions() {
    let (mut a, mut b, ws) = setup().await;
    a.write("a.txt", b"0\n");
    a.write("b.txt", b"0\n");
    eventually("synced", 20, || b.read("a.txt").is_some() && b.read("b.txt").is_some()).await;
    a.engine.shutdown();
    b.engine.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;
    a.write("a.txt", b"mac\n");
    b.write("a.txt", b"pc\n");
    a.write("b.txt", b"mac\n");
    b.write("b.txt", b"pc\n");
    a.restart().await;
    b.restart().await;
    eventually("two conflicts", 30, || a.open_conflicts(ws).len() == 2).await;
    for c in a.open_conflicts(ws) {
        let res = if c.path.as_str() == "a.txt" {
            Resolution::SaveBoth { suffix: "(from pc)".into() }
        } else {
            Resolution::Merged(b"mac\npc\n".to_vec())
        };
        a.engine.resolve_conflict(ws, &c.id, res).await.unwrap();
    }
    assert_eq!(a.read("a.txt").as_deref(), Some(&b"mac\n"[..]));
    assert_eq!(a.read("a (from pc).txt").as_deref(), Some(&b"pc\n"[..]));
    eventually("pc gets merged + saved copy", 30, || {
        b.read("b.txt").as_deref() == Some(b"mac\npc\n")
            && b.read("a.txt").as_deref() == Some(b"mac\n")
            && b.read("a (from pc).txt").is_some()
    })
    .await;
    eventually("pc conflicts closed", 30, || b.open_conflicts(ws).is_empty()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn offline_changes_sync_after_reconnect() {
    let (a, mut b, _ws) = setup().await;
    b.engine.shutdown();
    tokio::time::sleep(Duration::from_millis(200)).await;
    a.write("while-offline-mac.txt", b"m");
    b.write("while-offline-pc.txt", b"p");
    tokio::time::sleep(Duration::from_millis(500)).await;
    b.restart().await;
    eventually("both directions after reconnect", 30, || {
        b.read("while-offline-mac.txt").is_some() && a.read("while-offline-pc.txt").is_some()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn git_internals_and_secrets_are_never_synced() {
    let (a, b, ws) = setup().await;
    a.write(".git/HEAD", b"ref: refs/heads/main\n");
    a.write(".git/config", b"[core]\n");
    a.write(".env", b"API_KEY=secret\n");
    a.write("config/server.pem", b"-----BEGIN PRIVATE KEY-----\n");
    a.write(".env.example", b"API_KEY=\n");
    a.write("node_modules/x/index.js", b"x");
    a.write("visible.txt", b"ok");
    eventually("visible files arrive", 20, || b.read("visible.txt").is_some() && b.read(".env.example").is_some())
        .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    for p in [".git/HEAD", ".git/config", ".env", "config/server.pem", "node_modules/x/index.js"] {
        assert!(b.read(p).is_none(), "{p} must not be synced");
    }
    // And nothing secret is even in the manifest the peer can request.
    let m = a.store.lock().unwrap().manifest_all(ws).unwrap();
    assert!(m.iter().all(|e| !e.path.as_str().starts_with(".git") && e.path.as_str() != ".env"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn case_only_collisions_are_reported_not_applied() {
    let a = Node::new("mac").await;
    let b = Node::new("pc").await;
    pair(&a, &b).await;
    let ws = WorkspaceId::new();
    a.write("Data.txt", b"from mac");
    b.write("data.txt", b"from pc");
    a.register(ws);
    a.engine.workspace_added(ws);
    a.engine.share(ws, b.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    b.register(ws);
    b.engine.join(ws, a.id).await.unwrap();
    eventually("collision reported", 30, || {
        b.store.lock().unwrap().issues_list(Some(ws)).unwrap().iter().any(|i| i.kind == "case_collision")
    })
    .await;
    assert_eq!(b.read("data.txt").as_deref(), Some(&b"from pc"[..]));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_device_is_cut_off() {
    let (a, b, ws) = setup().await;
    a.write("one.txt", b"1");
    eventually("synced", 20, || b.read("one.txt").is_some()).await;
    a.engine.revoke(b.id).await.unwrap();
    eventually("disconnected", 10, || !a.engine.is_online(b.id)).await;
    eventually("pc learns it was removed", 10, || {
        b.store
            .lock()
            .unwrap()
            .device_get(a.id)
            .map(|d| d.status == devbridge_core::DeviceStatus::Revoked)
            .unwrap_or(false)
    })
    .await;
    b.write("after-revoke.txt", b"x");
    a.write("after-revoke-mac.txt", b"y");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(a.read("after-revoke.txt").is_none());
    assert!(b.read("after-revoke-mac.txt").is_none());
    assert!(!a.engine.is_online(b.id) && !b.engine.is_online(a.id));
    assert!(!a.store.lock().unwrap().is_synced_with(ws, b.id).unwrap());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_pairing_code_is_rejected_and_window_closes_after_three() {
    let a = Node::new("mac").await;
    let b = Node::new("pc").await;
    let w = a.engine.pairing_open().unwrap();
    let addr: SocketAddr = format!("127.0.0.1:{}", w.port).parse().unwrap();
    for _ in 0..3 {
        let r = b.engine.pair_join(addr, "0000-0000").await;
        assert!(r.is_err());
    }
    assert!(a.engine.pairing_current().is_none(), "window closed after three wrong codes");
    let r = b.engine.pair_join(addr, &w.code).await;
    assert!(r.is_err(), "the old code is no longer valid");
    assert!(a.store.lock().unwrap().paired_devices().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_methods_work_outside_the_runtime() {
    // Tauri runs non-async commands on the main thread, which has no Tokio
    // context. Opening a pairing window there used to abort the app.
    let a = Node::new("mac").await;
    let ws = WorkspaceId::new();
    a.register(ws);
    a.write("hello.txt", b"hi\n");
    let engine = a.engine.clone();
    let w = std::thread::spawn(move || {
        engine.workspace_added(ws);
        engine.pairing_open()
    })
    .join()
    .expect("no panic outside the runtime")
    .unwrap();
    assert_eq!(a.engine.pairing_current().map(|c| c.code), Some(w.code));
    eventually("workspace indexed", 10, || !a.store.lock().unwrap().manifest_all(ws).unwrap().is_empty()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupted_large_transfer_resumes_after_restart() {
    let (a, mut b, ws) = setup().await;
    // 24 MiB of incompressible-ish data.
    let data: Vec<u8> = (0..24 * 1024 * 1024u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    let mut ev = b.engine.subscribe();
    a.write("assets/big.bin", &data);
    // Wait until some bytes arrived, then "quit" the receiving app.
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(EngineEvent::TransferProgress { done, .. }) = ev.recv().await
                && done > 0
            {
                return;
            }
        }
    })
    .await
    .expect("transfer started");
    b.engine.shutdown();
    let partial = b.store.lock().unwrap().transfers_list(ws).unwrap();
    assert!(!partial.is_empty(), "transfer is persisted");
    // A leftover from a crash elsewhere in staging must be cleaned up on start.
    let staging = b.data.join("staging").join(ws.to_string());
    std::fs::create_dir_all(&staging).unwrap();
    std::fs::write(staging.join("local-stale"), b"junk").unwrap();
    b.restart().await;
    assert!(!staging.join("local-stale").exists(), "stale staging file removed at startup");
    eventually("big file complete and verified", 60, || b.read("assets/big.bin").is_some_and(|d| d == data)).await;
    eventually("no transfers left", 10, || b.store.lock().unwrap().transfers_list(ws).unwrap().is_empty()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_edits_converge_without_conflict() {
    let (mut a, mut b, ws) = setup().await;
    a.engine.shutdown();
    b.engine.shutdown();
    tokio::time::sleep(Duration::from_millis(200)).await;
    a.write("same.txt", b"same content");
    b.write("same.txt", b"same content");
    a.restart().await;
    b.restart().await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(a.open_conflicts(ws).is_empty() && b.open_conflicts(ws).is_empty());
    let ha = a
        .store
        .lock()
        .unwrap()
        .manifest_get(ws, &devbridge_core::RelPath::parse("same.txt").unwrap())
        .unwrap()
        .unwrap();
    eventually("same head version", 20, || {
        b.store
            .lock()
            .unwrap()
            .manifest_get(ws, &devbridge_core::RelPath::parse("same.txt").unwrap())
            .unwrap()
            .unwrap()
            .version_id
            == a.store
                .lock()
                .unwrap()
                .manifest_get(ws, &devbridge_core::RelPath::parse("same.txt").unwrap())
                .unwrap()
                .unwrap()
                .version_id
    })
    .await;
    let _ = ha;
}

fn git(dir: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

fn copy_dir(from: &Path, to: &Path) {
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&dest).unwrap();
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), &dest).unwrap();
        }
    }
}

/// The sample project in real Git clones on both devices: files sync, Git
/// history doesn't, and "catch up" aligns the second clone without touching files.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sample_repo_files_sync_but_git_history_stays_independent() {
    let a = Node::new("mac").await;
    let b = Node::new("pc").await;
    let origin = a.root.parent().unwrap().join("origin.git");
    git(a.root.parent().unwrap(), &["init", "-q", "--bare", "origin.git"]);
    git(a.root.parent().unwrap(), &["clone", "-q", origin.to_str().unwrap(), "project"]);
    git(b.root.parent().unwrap(), &["clone", "-q", origin.to_str().unwrap(), "project"]);
    let sample = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/hello-sync");
    copy_dir(&sample, &a.root);
    git(&a.root, &["add", "."]);
    git(&a.root, &["commit", "-qm", "initial"]);
    git(&a.root, &["push", "-q", "origin", "HEAD"]);
    git(&b.root, &["pull", "-q", "origin", "main"]);

    pair(&a, &b).await;
    let ws = WorkspaceId::new();
    a.register(ws);
    a.engine.workspace_added(ws);
    a.engine.share(ws, b.id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    b.register(ws);
    b.engine.join(ws, a.id).await.unwrap();

    // Work on the Mac: edit, commit and push.
    std::fs::write(a.root.join("src/greet.ts"), "export const greet = (n: string) => `Hi, ${n}!`;\n").unwrap();
    std::fs::write(a.root.join("src/new.ts"), "export {};\n").unwrap();
    git(&a.root, &["add", "."]);
    git(&a.root, &["commit", "-qm", "edit on mac"]);
    git(&a.root, &["push", "-q", "origin", "HEAD"]);

    eventually("files reach the PC", 30, || {
        b.read("src/new.ts").is_some() && b.read("src/greet.ts").is_some_and(|c| c.starts_with(b"export const greet"))
    })
    .await;
    // .env (secret) and build/ (generated) were never sent.
    assert!(
        !a.store
            .lock()
            .unwrap()
            .manifest_all(ws)
            .unwrap()
            .iter()
            .any(|e| e.path.as_str() == ".env" || e.path.as_str().starts_with("build/"))
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(b.root.join("scripts/run.sh")).unwrap().permissions().mode();
        assert!(mode & 0o111 != 0, "executable bit preserved");
    }

    // The PC's Git history is untouched by sync: its HEAD is still the old commit.
    let g = devbridge_git::Git::locate(None).unwrap();
    let mac_head = g.status(&a.root).unwrap().head;
    assert_ne!(g.status(&b.root).unwrap().head, mac_head, ".git is never synced");
    // Fetch + catch up moves the PC's branch without changing any file.
    g.fetch(&b.root).unwrap();
    let before = b.read("src/greet.ts");
    let r = g.catch_up(&b.root).unwrap();
    assert_eq!(r.files, 2);
    let st = g.status(&b.root).unwrap();
    assert_eq!(st.head, mac_head);
    assert!(st.is_clean(), "{st:?}");
    assert_eq!(b.read("src/greet.ts"), before);
}
