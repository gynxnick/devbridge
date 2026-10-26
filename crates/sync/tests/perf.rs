//! Performance measurements. Ignored by default; run with:
//!   cargo test --release -p devbridge-sync --test perf -- --ignored --nocapture --test-threads=1
//! Results are printed and recorded by hand in docs/PERFORMANCE.md.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devbridge_core::{OsKind, SyncState, Workspace, WorkspaceColor, WorkspaceIcon, WorkspaceId, WorkspaceMode};
use devbridge_fs::rules::{RuleConfig, Rules};
use devbridge_store::Store;
use devbridge_sync::indexer::{Indexer, Scope};

fn ws_store(root: &std::path::Path) -> (Mutex<Store>, WorkspaceId, devbridge_core::DeviceId) {
    let s = Store::open_in_memory().unwrap();
    let dev = s.ensure_local_device("bench", OsKind::current(), None).unwrap().id;
    let ws = WorkspaceId::new();
    s.insert_workspace(&Workspace {
        id: ws,
        name: "bench".into(),
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
    (Mutex::new(s), ws, dev)
}

#[test]
#[ignore]
fn index_10k_small_files() {
    let t = tempfile::tempdir().unwrap();
    let root = devbridge_core::fsutil::canonicalize(t.path()).unwrap();
    for d in 0..100 {
        let dir = root.join(format!("src/mod{d}"));
        std::fs::create_dir_all(&dir).unwrap();
        for f in 0..100 {
            std::fs::write(dir.join(format!("file{f}.ts")), format!("export const v{d}_{f} = {};\n", d * f).repeat(20))
                .unwrap();
        }
    }
    let (store, ws, dev) = ws_store(&root);
    let rules = Rules::new(&root, &RuleConfig::default());
    let idx = Indexer { store: &store, ws, root: &root, rules: &rules, device: dev };
    let t0 = Instant::now();
    let r = idx.refresh(Scope::All).unwrap();
    let first = t0.elapsed();
    let t1 = Instant::now();
    let r2 = idx.refresh(Scope::All).unwrap();
    let again = t1.elapsed();
    let t2 = Instant::now();
    idx.refresh(Scope::Path(devbridge_core::RelPath::parse("src/mod5/file5.ts").unwrap())).unwrap();
    let single = t2.elapsed();
    println!(
        "PERF index 10,000 files: initial {first:?} ({} versions); unchanged rescan {again:?} ({} versions); single path {single:?}",
        r.versions.len(),
        r2.versions.len()
    );
    assert_eq!(r.versions.len(), 10_000);
    assert!(r2.versions.is_empty());
}

#[test]
#[ignore]
fn hash_throughput_256mb() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("big.bin");
    let chunk: Vec<u8> = (0..1024 * 1024u32).map(|i| (i.wrapping_mul(2654435761) >> 7) as u8).collect();
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&p).unwrap();
        for _ in 0..256 {
            f.write_all(&chunk).unwrap();
        }
    }
    let t0 = Instant::now();
    let (_, n) = devbridge_fs::hash::hash_file(&p).unwrap();
    let el = t0.elapsed();
    println!(
        "PERF hash 256 MiB: {el:?} = {:.0} MiB/s (constant memory, 256 KiB buffer)",
        n as f64 / 1048576.0 / el.as_secs_f64()
    );
}

mod two {
    use super::*;
    use devbridge_net::{Identity, MemoryStore};
    use devbridge_sync::{Engine, EngineConfig, EngineEvent, ListenConfig, Tuning};

    pub struct N {
        pub _t: tempfile::TempDir,
        pub root: std::path::PathBuf,
        pub engine: Engine,
        pub id: devbridge_core::DeviceId,
        pub store: Arc<Mutex<Store>>,
    }

    pub async fn node(name: &str) -> N {
        let t = tempfile::tempdir().unwrap();
        let base = devbridge_core::fsutil::canonicalize(t.path()).unwrap();
        let data = base.join("data");
        let root = base.join("p");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let s = Store::open(&data.join("db")).unwrap();
        let id = s.ensure_local_device(name, OsKind::current(), None).unwrap().id;
        let store = Arc::new(Mutex::new(s));
        let engine = Engine::start(EngineConfig {
            data_dir: data,
            store: store.clone(),
            identity: Identity::load_or_create(&MemoryStore::default()).unwrap(),
            device_id: id,
            device_name: name.into(),
            os: OsKind::current(),
            app_version: "bench".into(),
            listen: ListenConfig { lan: false, port: 0 },
            discovery: false,
            tuning: Tuning { reconcile_every: Duration::from_secs(3600), ..Tuning::default() },
        })
        .await
        .unwrap();
        N { _t: t, root, engine, id, store }
    }

    pub async fn connect(a: &N, b: &N) -> WorkspaceId {
        let mut ev = a.engine.subscribe();
        let w = a.engine.pairing_open().unwrap();
        let addr = format!("127.0.0.1:{}", w.port).parse().unwrap();
        let e = b.engine.clone();
        let code = w.code.clone();
        let j = tokio::spawn(async move { e.pair_join(addr, &code).await });
        loop {
            if let Ok(EngineEvent::PairingRequest { request_id, .. }) = ev.recv().await {
                a.engine.pairing_decide(&request_id, true);
                break;
            }
        }
        j.await.unwrap().unwrap();
        let ws = WorkspaceId::new();
        for n in [a, b] {
            n.store
                .lock()
                .unwrap()
                .insert_workspace(&Workspace {
                    id: ws,
                    name: "p".into(),
                    root_path: n.root.display().to_string(),
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
        }
        while !(a.engine.is_online(b.id) && b.engine.is_online(a.id)) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        a.engine.workspace_added(ws);
        a.engine.share(ws, b.id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        b.engine.join(ws, a.id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        ws
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn small_edit_latency_and_large_transfer_throughput() {
    let a = two::node("a").await;
    let b = two::node("b").await;
    let _ws = two::connect(&a, &b).await;

    // Small edits: time from write on A until the new content is on B's disk
    // (includes the 300 ms watcher debounce).
    let mut lat = Vec::new();
    for i in 0..10 {
        let content = format!("edit {i}\n");
        let t0 = Instant::now();
        std::fs::write(a.root.join("notes.txt"), &content).unwrap();
        loop {
            if std::fs::read_to_string(b.root.join("notes.txt")).ok().as_deref() == Some(content.as_str()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
            assert!(t0.elapsed() < Duration::from_secs(20), "edit {i} never arrived");
        }
        lat.push(t0.elapsed());
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    lat.sort();
    println!(
        "PERF small edit A→B over loopback: median {:?}, max {:?} (incl. 300 ms debounce)",
        lat[lat.len() / 2],
        lat[lat.len() - 1]
    );

    // Large file: 200 MiB over loopback TLS.
    let size = 200 * 1024 * 1024usize;
    let chunk: Vec<u8> = (0..1024 * 1024u32).map(|i| (i.wrapping_mul(2654435761) >> 9) as u8).collect();
    {
        use std::io::Write;
        let mut f = std::fs::File::create(a.root.join("big.bin")).unwrap();
        for _ in 0..(size / chunk.len()) {
            f.write_all(&chunk).unwrap();
        }
    }
    let t0 = Instant::now();
    loop {
        if std::fs::metadata(b.root.join("big.bin")).map(|m| m.len() as usize == size).unwrap_or(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(t0.elapsed() < Duration::from_secs(300), "large file never arrived");
    }
    let el = t0.elapsed();
    println!(
        "PERF 200 MiB file A→B over loopback TLS: {el:?} = {:.0} MiB/s (hash, transfer, verify, atomic rename)",
        200.0 / el.as_secs_f64()
    );
}
