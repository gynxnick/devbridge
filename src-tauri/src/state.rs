use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use devbridge_core::{AppSettings, DeviceId, OsKind, WorkspaceId};
use devbridge_fs::rules::RuleConfig;
use devbridge_git::Git;
use devbridge_net::{Identity, KeyringStore, MemoryStore, SecretStore};
use devbridge_store::Store;
use devbridge_sync::{Engine, EngineConfig, ListenConfig, Tuning};
use devbridge_workspace::Protected;

use crate::error::CmdError;

/// Shared application state.
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    pub local_device: DeviceId,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub protected: Protected,
    pub engine: Engine,
    /// Where the device's private key lives ("system keychain" or a warning).
    pub identity_storage: String,
}

/// Keychain service name, unique per data folder so two instances on one
/// computer (e.g. for testing) don't share an identity.
fn service_name(data_dir: &Path) -> String {
    let h = devbridge_fs::hash::hash_bytes(data_dir.to_string_lossy().as_bytes());
    format!("DevBridge ({})", &h[..8])
}

pub fn tuning_from(s: &AppSettings) -> Tuning {
    Tuning {
        rules: RuleConfig {
            user_patterns: s.exclude_patterns.clone(),
            secret_optins: s.secret_optins.clone(),
            respect_gitignore: s.respect_gitignore,
        },
        max_transfers: s.max_concurrent_transfers as usize,
        max_file_size: u64::from(s.max_file_size_mb) * 1024 * 1024,
        reconcile_every: Duration::from_secs(u64::from(s.reconcile_interval_minutes) * 60),
        recovery_retention: Duration::from_secs(u64::from(s.recovery_retention_days) * 86400),
    }
}

pub fn listen_from(s: &AppSettings) -> ListenConfig {
    ListenConfig { lan: s.lan_enabled, port: s.listen_port }
}

impl AppState {
    pub fn init(data_dir: PathBuf, logs_dir: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(&data_dir)?;
        let store = Store::open(&data_dir.join("devbridge.sqlite3"))?;
        let info = os_info::get();
        let device =
            store.ensure_local_device(&default_device_name(), OsKind::current(), Some(&info.version().to_string()))?;
        let mut settings = store.settings()?;
        // DEVBRIDGE_PORT gives a second instance on the same computer its own
        // port (see scripts/second-instance.sh). Applied only over the default.
        if let Some(port) = std::env::var("DEVBRIDGE_PORT").ok().and_then(|p| p.parse::<u16>().ok())
            && settings.listen_port == AppSettings::default().listen_port
            && port >= 1024
        {
            settings.listen_port = port;
            store.save_settings(&settings)?;
        }

        // The private key lives only in the OS credential store. If that's
        // unavailable we keep an in-memory identity for this session (pairings
        // then won't survive a restart) rather than writing the key to disk.
        //
        // Development builds are re-signed ad hoc on every rebuild, so macOS
        // would ask for the login keychain password after each one. They
        // therefore keep the key in memory only, unless DEVBRIDGE_KEYCHAIN=1.
        let use_keychain = !cfg!(debug_assertions) || std::env::var("DEVBRIDGE_KEYCHAIN").is_ok_and(|v| v == "1");
        let (identity, identity_storage) = if !use_keychain {
            (
                Identity::load_or_create(&MemoryStore::default())?,
                "temporary (development build; set DEVBRIDGE_KEYCHAIN=1 to use the system keychain)".to_string(),
            )
        } else {
            let keyring = KeyringStore::new(service_name(&data_dir));
            match Identity::load_or_create(&keyring as &dyn SecretStore) {
                Ok(id) => (id, "system keychain".to_string()),
                Err(e) => {
                    tracing::error!(error = %e, "credential store unavailable; using a temporary identity");
                    (
                        Identity::load_or_create(&MemoryStore::default())?,
                        format!("temporary (credential store unavailable: {e})"),
                    )
                }
            }
        };
        let store = Arc::new(Mutex::new(store));
        let engine = tauri::async_runtime::block_on(Engine::start(EngineConfig {
            data_dir: data_dir.clone(),
            store: store.clone(),
            identity,
            device_id: device.id,
            device_name: device.name.clone(),
            os: OsKind::current(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            listen: listen_from(&settings),
            discovery: true,
            tuning: tuning_from(&settings),
        }))?;
        tracing::info!(device = %device.id, data_dir = %data_dir.display(), "DevBridge started");
        Ok(Self {
            store,
            local_device: device.id,
            protected: Protected::for_app(Some(&data_dir)),
            data_dir,
            logs_dir,
            engine,
            identity_storage,
        })
    }

    pub fn store(&self) -> Result<MutexGuard<'_, Store>, CmdError> {
        lock(&self.store)
    }

    pub fn git(&self) -> Result<Option<Git>, CmdError> {
        let custom = self.store()?.settings()?.git_path.map(PathBuf::from);
        Ok(Git::locate(custom.as_deref()).ok())
    }

    pub fn workspace_root(&self, id: WorkspaceId) -> Result<PathBuf, CmdError> {
        let root = PathBuf::from(self.store()?.get_workspace(id)?.root_path);
        if !root.is_dir() {
            return Err(CmdError::new(
                "root_missing",
                format!(
                    "The project folder {} isn't available. Reconnect the disk or move the folder back.",
                    root.display()
                ),
            ));
        }
        Ok(root)
    }
}

pub fn lock(store: &Arc<Mutex<Store>>) -> Result<MutexGuard<'_, Store>, CmdError> {
    // A poisoned lock means a panic mid-operation; SQLite transactions keep the
    // data consistent, so continuing with the inner value is safe.
    Ok(store.lock().unwrap_or_else(|p| p.into_inner()))
}

/// The machine's name without the `.local` mDNS suffix.
fn default_device_name() -> String {
    let raw = gethostname::gethostname().to_string_lossy().into_owned();
    let name = raw.trim_end_matches(".local").trim();
    if name.is_empty() { "This computer".into() } else { name.chars().take(64).collect() }
}
