//! SQLite persistence for DevBridge.
//!
//! `Store` owns one connection. Callers that share it across threads wrap it in
//! a `Mutex`; SQLite work here is short and synchronous.

pub mod migrations;
pub mod remote;
pub mod sync;

pub use remote::*;
pub use sync::*;

use std::path::Path;
use std::str::FromStr;

use devbridge_core::time::now_ms;
use devbridge_core::{
    AppSettings, Device, DeviceId, DeviceRole, DeviceStatus, OsKind, SyncState, Workspace, WorkspaceColor,
    WorkspaceIcon, WorkspaceId, WorkspaceMode,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration to schema version {version} failed: {source}")]
    Migration { version: i64, source: rusqlite::Error },
    #[error(
        "the database was created by a newer version of DevBridge (schema {found}, this build supports {supported}); update the app"
    )]
    NewerSchema { found: i64, supported: i64 },
    #[error("stored data is invalid: {0}")]
    Corrupt(String),
    #[error("settings document is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} not found")]
    NotFound(&'static str),
    #[error("a workspace is already registered at this path")]
    DuplicateRoot,
}

pub type Result<T> = std::result::Result<T, StoreError>;

const SETTINGS_KEY: &str = "app";

pub struct Store {
    conn: Connection,
}

/// Parses a stable enum/ID string column, mapping failures to `Corrupt`.
fn parse<T: FromStr>(row: &Row<'_>, col: &str) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = row.get(col)?;
    s.parse::<T>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, format!("{col}: {e}").into())
    })
}

fn parse_opt<T: FromStr>(row: &Row<'_>, col: &str) -> rusqlite::Result<Option<T>>
where
    T::Err: std::fmt::Display,
{
    let s: Option<String> = row.get(col)?;
    s.map(|s| {
        s.parse::<T>().map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, format!("{col}: {e}").into())
        })
    })
    .transpose()
}

pub(crate) fn device_from_row(row: &Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        id: parse(row, "id")?,
        name: row.get("name")?,
        os: parse::<OsKind>(row, "os")?,
        os_version: row.get("os_version")?,
        status: parse::<DeviceStatus>(row, "status")?,
        role: parse::<DeviceRole>(row, "role")?,
        is_local: row.get("is_local")?,
        cert_fingerprint: row.get("cert_fingerprint")?,
        last_seen_at: row.get("last_seen_at")?,
        created_at: row.get("created_at")?,
    })
}

fn workspace_from_row(row: &Row<'_>) -> rusqlite::Result<Workspace> {
    Ok(Workspace {
        id: parse(row, "id")?,
        name: row.get("name")?,
        root_path: row.get("root_path")?,
        color: parse::<WorkspaceColor>(row, "color")?,
        icon: parse::<WorkspaceIcon>(row, "icon")?,
        mode: parse::<WorkspaceMode>(row, "mode")?,
        sync_state: parse::<SyncState>(row, "sync_state")?,
        active_device_id: parse_opt(row, "active_device_id")?,
        last_synced_at: row.get("last_synced_at")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

pub(crate) fn map_corrupt(e: rusqlite::Error) -> StoreError {
    match e {
        rusqlite::Error::FromSqlConversionFailure(_, _, inner) => StoreError::Corrupt(inner.to_string()),
        other => StoreError::Sqlite(other),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEvent {
    pub id: i64,
    pub at: i64,
    pub kind: String,
    pub workspace_id: Option<String>,
    pub device_id: Option<String>,
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRepoRecord {
    pub toplevel: String,
    pub last_branch: Option<String>,
    pub last_head: Option<String>,
    pub last_checked_at: Option<i64>,
}

impl Store {
    /// Opens (creating if needed) the database file and applies migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")?;
        migrations::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn schema_version(&self) -> Result<i64> {
        migrations::current_version(&self.conn)
    }

    // ---------- devices ----------

    /// Returns this installation's device record, creating it on first run.
    pub fn ensure_local_device(&self, default_name: &str, os: OsKind, os_version: Option<&str>) -> Result<Device> {
        if let Some(d) = self.local_device()? {
            // Keep the OS version current after upgrades; the name is user-owned.
            if d.os_version.as_deref() != os_version {
                self.conn.execute(
                    "UPDATE devices SET os_version = ?1 WHERE id = ?2",
                    params![os_version, d.id.to_string()],
                )?;
                return self.local_device()?.ok_or(StoreError::NotFound("local device"));
            }
            return Ok(d);
        }
        let id = DeviceId::new();
        self.conn.execute(
            "INSERT INTO devices (id, name, os, os_version, status, role, is_local, created_at)
             VALUES (?1, ?2, ?3, ?4, 'local', 'peer', 1, ?5)",
            params![id.to_string(), default_name, os.as_str(), os_version, now_ms()],
        )?;
        self.audit("device.local_created", None, Some(&id.to_string()), serde_json::json!({}))?;
        self.local_device()?.ok_or(StoreError::NotFound("local device"))
    }

    pub fn local_device(&self) -> Result<Option<Device>> {
        self.conn
            .query_row("SELECT * FROM devices WHERE is_local = 1", [], device_from_row)
            .optional()
            .map_err(map_corrupt)
    }

    pub fn list_devices(&self) -> Result<Vec<Device>> {
        let mut stmt = self.conn.prepare("SELECT * FROM devices ORDER BY is_local DESC, name COLLATE NOCASE")?;
        let rows = stmt.query_map([], device_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_corrupt)
    }

    /// `name` must already be validated by the caller.
    pub fn rename_device(&self, id: DeviceId, name: &str) -> Result<()> {
        let n = self.conn.execute("UPDATE devices SET name = ?1 WHERE id = ?2", params![name, id.to_string()])?;
        if n == 0 {
            return Err(StoreError::NotFound("device"));
        }
        self.audit("device.renamed", None, Some(&id.to_string()), serde_json::json!({ "name": name }))
    }

    // ---------- workspaces ----------

    pub fn insert_workspace(&self, ws: &Workspace) -> Result<()> {
        let res = self.conn.execute(
            "INSERT INTO workspaces (id, name, root_path, color, icon, mode, sync_state,
                                     active_device_id, last_synced_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                ws.id.to_string(),
                ws.name,
                ws.root_path,
                ws.color.as_str(),
                ws.icon.as_str(),
                ws.mode.as_str(),
                ws.sync_state.as_str(),
                ws.active_device_id.map(|d| d.to_string()),
                ws.last_synced_at,
                ws.created_at,
                ws.updated_at,
            ],
        );
        match res {
            Err(rusqlite::Error::SqliteFailure(e, Some(msg)))
                if e.code == rusqlite::ErrorCode::ConstraintViolation && msg.contains("root_path") =>
            {
                Err(StoreError::DuplicateRoot)
            }
            Err(e) => Err(e.into()),
            Ok(_) => self.audit(
                "workspace.imported",
                Some(&ws.id.to_string()),
                None,
                serde_json::json!({ "name": ws.name, "root": ws.root_path }),
            ),
        }
    }

    pub fn get_workspace(&self, id: WorkspaceId) -> Result<Workspace> {
        self.conn
            .query_row("SELECT * FROM workspaces WHERE id = ?1", [id.to_string()], workspace_from_row)
            .optional()
            .map_err(map_corrupt)?
            .ok_or(StoreError::NotFound("workspace"))
    }

    pub fn list_workspaces(&self) -> Result<Vec<Workspace>> {
        let mut stmt = self.conn.prepare("SELECT * FROM workspaces ORDER BY name COLLATE NOCASE")?;
        let rows = stmt.query_map([], workspace_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_corrupt)
    }

    /// `name` must already be validated by the caller.
    pub fn update_workspace_appearance(
        &self,
        id: WorkspaceId,
        name: &str,
        color: WorkspaceColor,
        icon: WorkspaceIcon,
    ) -> Result<Workspace> {
        let n = self.conn.execute(
            "UPDATE workspaces SET name = ?1, color = ?2, icon = ?3, updated_at = ?4 WHERE id = ?5",
            params![name, color.as_str(), icon.as_str(), now_ms(), id.to_string()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound("workspace"));
        }
        self.get_workspace(id)
    }

    /// Removes the workspace's *registration* and its sync metadata.
    /// Never touches files on disk.
    pub fn delete_workspace(&self, id: WorkspaceId) -> Result<()> {
        let ws = self.get_workspace(id)?;
        self.conn.execute("DELETE FROM workspaces WHERE id = ?1", [id.to_string()])?;
        self.audit(
            "workspace.removed",
            Some(&id.to_string()),
            None,
            serde_json::json!({ "name": ws.name, "root": ws.root_path, "filesDeleted": false }),
        )
    }

    pub fn count_open_conflicts(&self, id: WorkspaceId) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM conflicts WHERE workspace_id = ?1 AND state = 'open'",
            [id.to_string()],
            |r| r.get(0),
        )?)
    }

    pub fn count_pending_changes(&self, id: WorkspaceId) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM change_events WHERE workspace_id = ?1 AND state IN ('queued','in_flight')",
            [id.to_string()],
            |r| r.get(0),
        )?)
    }

    // ---------- git ----------

    pub fn upsert_git_repo(&self, id: WorkspaceId, rec: &GitRepoRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO git_repositories (workspace_id, toplevel, last_branch, last_head, last_checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(workspace_id) DO UPDATE SET toplevel = excluded.toplevel,
               last_branch = excluded.last_branch, last_head = excluded.last_head,
               last_checked_at = excluded.last_checked_at",
            params![id.to_string(), rec.toplevel, rec.last_branch, rec.last_head, rec.last_checked_at],
        )?;
        Ok(())
    }

    pub fn get_git_repo(&self, id: WorkspaceId) -> Result<Option<GitRepoRecord>> {
        Ok(self
            .conn
            .query_row(
                "SELECT toplevel, last_branch, last_head, last_checked_at FROM git_repositories WHERE workspace_id = ?1",
                [id.to_string()],
                |r| {
                    Ok(GitRepoRecord {
                        toplevel: r.get(0)?,
                        last_branch: r.get(1)?,
                        last_head: r.get(2)?,
                        last_checked_at: r.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    // ---------- settings ----------

    pub fn settings(&self) -> Result<AppSettings> {
        let raw: Option<String> = self
            .conn
            .query_row("SELECT value FROM app_settings WHERE key = ?1", [SETTINGS_KEY], |r| r.get(0))
            .optional()?;
        match raw {
            None => Ok(AppSettings::default()),
            Some(s) => Ok(serde_json::from_str(&s)?),
        }
    }

    /// Persists settings. Callers validate with `AppSettings::validate` first.
    pub fn save_settings(&self, settings: &AppSettings) -> Result<()> {
        let before = self.settings()?;
        self.conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![SETTINGS_KEY, serde_json::to_string(settings)?, now_ms()],
        )?;
        // Opting a secret pattern into sync is security-relevant: record it.
        if before.secret_optins != settings.secret_optins {
            self.audit(
                "settings.secret_optins_changed",
                None,
                None,
                serde_json::json!({ "before": before.secret_optins, "after": settings.secret_optins }),
            )?;
        }
        Ok(())
    }

    // ---------- audit ----------

    /// Appends an audit event. `detail` must never contain file contents or secrets.
    pub fn audit(
        &self,
        kind: &str,
        workspace_id: Option<&str>,
        device_id: Option<&str>,
        detail: serde_json::Value,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO audit_events (at, kind, workspace_id, device_id, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![now_ms(), kind, workspace_id, device_id, detail.to_string()],
        )?;
        Ok(())
    }

    pub fn recent_audit(&self, limit: u32) -> Result<Vec<AuditEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, at, kind, workspace_id, device_id, detail FROM audit_events ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |r| {
            let detail: String = r.get(5)?;
            Ok(AuditEvent {
                id: r.get(0)?,
                at: r.get(1)?,
                kind: r.get(2)?,
                workspace_id: r.get(3)?,
                device_id: r.get(4)?,
                detail: serde_json::from_str(&detail).unwrap_or(serde_json::Value::Null),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(root: &str) -> Workspace {
        let now = now_ms();
        Workspace {
            id: WorkspaceId::new(),
            name: "Demo".into(),
            root_path: root.into(),
            color: WorkspaceColor::Blue,
            icon: WorkspaceIcon::Code,
            mode: WorkspaceMode::Synced,
            sync_state: SyncState::LocalOnly,
            active_device_id: None,
            last_synced_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn local_device_is_created_once() {
        let s = Store::open_in_memory().unwrap();
        let a = s.ensure_local_device("Mac", OsKind::MacOs, Some("26.0")).unwrap();
        let b = s.ensure_local_device("Other", OsKind::MacOs, Some("26.0")).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(b.name, "Mac", "existing name must not be overwritten");
        let c = s.ensure_local_device("Mac", OsKind::MacOs, Some("26.1")).unwrap();
        assert_eq!(c.os_version.as_deref(), Some("26.1"));
        s.rename_device(a.id, "Studio").unwrap();
        assert_eq!(s.local_device().unwrap().unwrap().name, "Studio");
    }

    #[test]
    fn workspace_crud_and_duplicate_root() {
        let s = Store::open_in_memory().unwrap();
        let w = ws("/tmp/a");
        s.insert_workspace(&w).unwrap();
        assert_eq!(s.get_workspace(w.id).unwrap(), w);
        assert!(matches!(s.insert_workspace(&ws("/tmp/a")), Err(StoreError::DuplicateRoot)));

        let u = s.update_workspace_appearance(w.id, "Renamed", WorkspaceColor::Green, WorkspaceIcon::Globe).unwrap();
        assert_eq!((u.name.as_str(), u.color, u.icon), ("Renamed", WorkspaceColor::Green, WorkspaceIcon::Globe));

        s.delete_workspace(w.id).unwrap();
        assert!(matches!(s.get_workspace(w.id), Err(StoreError::NotFound(_))));
        let kinds: Vec<_> = s.recent_audit(10).unwrap().into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, ["workspace.removed", "workspace.imported"]);
    }

    #[test]
    fn corrupt_enum_value_is_reported_not_panicked() {
        let s = Store::open_in_memory().unwrap();
        let w = ws("/tmp/b");
        s.insert_workspace(&w).unwrap();
        s.conn.execute("PRAGMA ignore_check_constraints = ON", []).unwrap();
        s.conn.execute("UPDATE workspaces SET color = 'plaid'", []).unwrap();
        assert!(matches!(s.list_workspaces(), Err(StoreError::Corrupt(_))));
    }

    #[test]
    fn settings_round_trip_and_audit_secret_optins() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.settings().unwrap(), AppSettings::default());
        let mut st = AppSettings { theme: devbridge_core::ThemePreference::Dark, ..Default::default() };
        s.save_settings(&st).unwrap();
        assert_eq!(s.settings().unwrap(), st);
        assert!(s.recent_audit(5).unwrap().is_empty());

        st.secret_optins.push(".env.local".into());
        s.save_settings(&st).unwrap();
        assert_eq!(s.recent_audit(5).unwrap()[0].kind, "settings.secret_optins_changed");
    }

    #[test]
    fn deleting_workspace_cascades_sync_metadata() {
        let s = Store::open_in_memory().unwrap();
        let w = ws("/tmp/c");
        s.insert_workspace(&w).unwrap();
        s.upsert_git_repo(
            w.id,
            &GitRepoRecord {
                toplevel: "/tmp/c".into(),
                last_branch: Some("main".into()),
                last_head: None,
                last_checked_at: None,
            },
        )
        .unwrap();
        s.conn
            .execute(
                "INSERT INTO conflicts (id, workspace_id, path, kind, remote_version_id, remote_device_id, detected_at)
                 VALUES ('c1', ?1, 'a.txt', 'content', 'v', 'd', 0)",
                [w.id.to_string()],
            )
            .unwrap();
        assert_eq!(s.count_open_conflicts(w.id).unwrap(), 1);
        s.delete_workspace(w.id).unwrap();
        let n: i64 = s.conn.query_row("SELECT count(*) FROM conflicts", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
        assert!(s.get_git_repo(w.id).unwrap().is_none());
    }

    #[test]
    fn file_database_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite3");
        let id = {
            let s = Store::open(&path).unwrap();
            s.ensure_local_device("Mac", OsKind::MacOs, None).unwrap().id
        };
        let s = Store::open(&path).unwrap();
        assert_eq!(s.local_device().unwrap().unwrap().id, id);
        assert_eq!(s.schema_version().unwrap(), migrations::LATEST_VERSION);
    }
}
