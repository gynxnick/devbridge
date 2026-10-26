//! Forward-only schema migrations tracked with `PRAGMA user_version`.
//!
//! Each migration runs in its own transaction together with the version bump,
//! so a crash mid-migration leaves the database at the previous version.

use rusqlite::{Connection, TransactionBehavior};

use crate::StoreError;

/// Ordered list of migrations. Index + 1 is the schema version after applying.
/// Never edit a shipped migration; append a new one.
pub const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_sync_engine.sql"),
    include_str!("../migrations/0003_remote_workspace.sql"),
    include_str!("../migrations/0004_content_bases.sql"),
];

pub const LATEST_VERSION: i64 = MIGRATIONS.len() as i64;

pub fn current_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// Applies every pending migration. Returns the number applied.
pub fn migrate(conn: &mut Connection) -> Result<usize, StoreError> {
    migrate_to(conn, MIGRATIONS)
}

pub(crate) fn migrate_to(conn: &mut Connection, migrations: &[&str]) -> Result<usize, StoreError> {
    let current = current_version(conn)?;
    let latest = migrations.len() as i64;
    if current > latest {
        return Err(StoreError::NewerSchema { found: current, supported: latest });
    }
    let mut applied = 0;
    for (idx, sql) in migrations.iter().enumerate().skip(current as usize) {
        let version = idx as i64 + 1;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Exclusive)?;
        tx.execute_batch(sql).map_err(|e| StoreError::Migration { version, source: e })?;
        // PRAGMA can't take bound parameters; version is an integer we control.
        tx.execute_batch(&format!("PRAGMA user_version = {version}"))?;
        tx.commit()?;
        applied += 1;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_empty_db_to_latest_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), MIGRATIONS.len());
        assert_eq!(current_version(&conn).unwrap(), LATEST_VERSION);
        assert_eq!(migrate(&mut conn).unwrap(), 0);
    }

    #[test]
    fn creates_every_spec_table() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        for table in [
            "devices",
            "workspaces",
            "workspace_devices",
            "file_manifest",
            "file_versions",
            "change_events",
            "transfer_jobs",
            "conflicts",
            "sync_sessions",
            "git_repositories",
            "app_settings",
            "audit_events",
        ] {
            let n: i64 = conn
                .query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1", [table], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1, "missing table {table}");
        }
    }

    #[test]
    fn refuses_database_from_newer_app() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 999").unwrap();
        assert!(matches!(migrate(&mut conn), Err(StoreError::NewerSchema { found: 999, .. })));
    }

    #[test]
    fn failed_migration_rolls_back_to_previous_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        let migrations = ["CREATE TABLE a (x INTEGER);", "CREATE TABLE b (y INTEGER); THIS IS NOT SQL;"];
        let err = migrate_to(&mut conn, &migrations).unwrap_err();
        assert!(matches!(err, StoreError::Migration { version: 2, .. }));
        assert_eq!(current_version(&conn).unwrap(), 1);
        let b_exists: i64 =
            conn.query_row("SELECT count(*) FROM sqlite_master WHERE name='b'", [], |r| r.get(0)).unwrap();
        assert_eq!(b_exists, 0, "partial migration must be rolled back");
    }

    #[test]
    fn enforces_single_local_device() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let ins = "INSERT INTO devices (id,name,os,status,is_local,created_at) VALUES (?1,'d','macos','local',1,0)";
        conn.execute(ins, ["A"]).unwrap();
        assert!(conn.execute(ins, ["B"]).is_err());
    }
}
