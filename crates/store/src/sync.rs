//! Persistence for the sync engine: manifest, version history, outgoing change
//! queue, transfers, conflicts, issues, recovery items, sharing and peers.

use devbridge_core::time::now_ms;
use devbridge_core::{
    Device, DeviceId, DeviceRole, DeviceStatus, OsKind, RelPath, WorkspaceColor, WorkspaceIcon, WorkspaceId,
};
use rusqlite::{OptionalExtension, Row, params};
use serde::Serialize;

use crate::{Result, Store, StoreError, device_from_row, map_corrupt};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    pub path: RelPath,
    pub content_hash: Option<String>,
    pub size: u64,
    pub mode: Option<u32>,
    pub mtime_ns: Option<i64>,
    pub version_id: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionRow {
    pub version_id: String,
    pub workspace_id: WorkspaceId,
    pub path: RelPath,
    pub ancestry: Vec<String>,
    pub base_hashes: Vec<String>,
    pub content_hash: Option<String>,
    pub size: u64,
    pub mode: Option<u32>,
    pub deleted: bool,
    pub origin_device_id: DeviceId,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutgoingChange {
    pub change_id: String,
    pub version: VersionRow,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferJob {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub peer_device_id: DeviceId,
    pub change_id: String,
    pub path: RelPath,
    pub version_id: String,
    pub ancestry: Vec<String>,
    pub base_hashes: Vec<String>,
    pub origin_device_id: DeviceId,
    pub content_hash: String,
    pub total_size: u64,
    pub mode: Option<u32>,
    pub bytes_done: u64,
    pub state: String,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictRecord {
    pub id: String,
    pub workspace_id: WorkspaceId,
    pub path: RelPath,
    /// `content`, `local_deleted` or `remote_deleted`.
    pub kind: String,
    pub local_version_id: Option<String>,
    pub local_hash: Option<String>,
    pub local_size: Option<u64>,
    pub remote_version_id: String,
    pub remote_hash: Option<String>,
    pub remote_size: Option<u64>,
    pub remote_mode: Option<u32>,
    pub remote_ancestry: Vec<String>,
    pub remote_device_id: DeviceId,
    pub state: String,
    pub resolution: Option<String>,
    pub detected_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncIssue {
    pub workspace_id: WorkspaceId,
    pub path: String,
    pub kind: String,
    pub detail: String,
    pub first_seen_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryItem {
    pub id: i64,
    pub workspace_id: String,
    pub path: String,
    pub content_hash: String,
    pub size: u64,
    pub reason: String,
    pub conflict_id: Option<String>,
    pub pinned: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceOffer {
    pub workspace_id: WorkspaceId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub name: String,
    pub color: WorkspaceColor,
    pub icon: WorkspaceIcon,
    pub offered_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareState {
    pub device_id: DeviceId,
    pub device_name: String,
    /// This device shares the workspace with the peer.
    pub shared: bool,
    /// The peer has joined (sync is active when both are true).
    pub joined: bool,
    pub last_sync_at: Option<i64>,
}

fn rel(row: &Row<'_>, col: &str) -> rusqlite::Result<RelPath> {
    let s: String = row.get(col)?;
    RelPath::parse(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, format!("{col}: {e}").into())
    })
}

fn parse_col<T: std::str::FromStr>(row: &Row<'_>, col: &str) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = row.get(col)?;
    s.parse::<T>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, format!("{col}: {e}").into())
    })
}

fn json_vec(row: &Row<'_>, col: &str) -> rusqlite::Result<Vec<String>> {
    let s: String = row.get(col)?;
    serde_json::from_str(&s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, format!("{col}: {e}").into())
    })
}

fn opt_u32(row: &Row<'_>, col: &str) -> rusqlite::Result<Option<u32>> {
    Ok(row.get::<_, Option<i64>>(col)?.map(|v| v as u32))
}

fn manifest_from_row(row: &Row<'_>) -> rusqlite::Result<ManifestEntry> {
    Ok(ManifestEntry {
        path: rel(row, "path")?,
        content_hash: row.get("content_hash")?,
        size: row.get::<_, i64>("size")? as u64,
        mode: opt_u32(row, "mode")?,
        mtime_ns: row.get("mtime_ns")?,
        version_id: row.get("version_id")?,
        deleted: row.get("deleted")?,
    })
}

fn version_from_row(row: &Row<'_>) -> rusqlite::Result<VersionRow> {
    Ok(VersionRow {
        version_id: row.get("version_id")?,
        workspace_id: parse_col(row, "workspace_id")?,
        path: rel(row, "path")?,
        ancestry: json_vec(row, "ancestry")?,
        base_hashes: json_vec(row, "base_hashes")?,
        content_hash: row.get("content_hash")?,
        size: row.get::<_, i64>("size")? as u64,
        mode: opt_u32(row, "mode")?,
        deleted: row.get("deleted")?,
        origin_device_id: parse_col(row, "origin_device_id")?,
        created_at: row.get("created_at")?,
    })
}

fn transfer_from_row(row: &Row<'_>) -> rusqlite::Result<TransferJob> {
    Ok(TransferJob {
        id: row.get("id")?,
        workspace_id: parse_col(row, "workspace_id")?,
        peer_device_id: parse_col(row, "peer_device_id")?,
        change_id: row.get("change_id")?,
        path: rel(row, "path")?,
        version_id: row.get("version_id")?,
        ancestry: json_vec(row, "ancestry")?,
        base_hashes: json_vec(row, "base_hashes")?,
        origin_device_id: parse_col(row, "origin_device_id")?,
        content_hash: row.get("content_hash")?,
        total_size: row.get::<_, i64>("total_size")? as u64,
        mode: opt_u32(row, "mode")?,
        bytes_done: row.get::<_, i64>("bytes_done")? as u64,
        state: row.get("state")?,
        attempts: row.get::<_, i64>("attempts")? as u32,
        last_error: row.get("last_error")?,
    })
}

fn conflict_from_row(row: &Row<'_>) -> rusqlite::Result<ConflictRecord> {
    Ok(ConflictRecord {
        id: row.get("id")?,
        workspace_id: parse_col(row, "workspace_id")?,
        path: rel(row, "path")?,
        kind: row.get("kind")?,
        local_version_id: row.get("local_version_id")?,
        local_hash: row.get("local_hash")?,
        local_size: row.get::<_, Option<i64>>("local_size")?.map(|v| v as u64),
        remote_version_id: row.get("remote_version_id")?,
        remote_hash: row.get("remote_hash")?,
        remote_size: row.get::<_, Option<i64>>("remote_size")?.map(|v| v as u64),
        remote_mode: opt_u32(row, "remote_mode")?,
        remote_ancestry: json_vec(row, "remote_ancestry")?,
        remote_device_id: parse_col(row, "remote_device_id")?,
        state: row.get("state")?,
        resolution: row.get("resolution")?,
        detected_at: row.get("detected_at")?,
        resolved_at: row.get("resolved_at")?,
    })
}

fn collect<T>(rows: impl Iterator<Item = rusqlite::Result<T>>) -> Result<Vec<T>> {
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_corrupt)
}

/// What to do with a version being recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    /// Make it the path's current version (manifest head).
    Set,
    /// Only add to history (e.g. the losing side of a convergence).
    HistoryOnly,
}

impl Store {
    // ---------- manifest ----------

    pub fn manifest_get(&self, ws: WorkspaceId, path: &RelPath) -> Result<Option<ManifestEntry>> {
        self.conn
            .query_row(
                "SELECT * FROM file_manifest WHERE workspace_id = ?1 AND path = ?2",
                params![ws.to_string(), path.as_str()],
                manifest_from_row,
            )
            .optional()
            .map_err(map_corrupt)
    }

    /// All entries, including tombstones, ordered by path.
    pub fn manifest_all(&self, ws: WorkspaceId) -> Result<Vec<ManifestEntry>> {
        let mut st = self.conn.prepare("SELECT * FROM file_manifest WHERE workspace_id = ?1 ORDER BY path")?;
        collect(st.query_map([ws.to_string()], manifest_from_row)?)
    }

    /// Entries at `prefix` itself or below `prefix/`.
    pub fn manifest_under(&self, ws: WorkspaceId, prefix: &RelPath) -> Result<Vec<ManifestEntry>> {
        let mut st = self.conn.prepare(
            "SELECT * FROM file_manifest WHERE workspace_id = ?1 AND (path = ?2 OR substr(path, 1, ?3) = ?4) ORDER BY path",
        )?;
        let dir = format!("{}/", prefix.as_str());
        collect(st.query_map(params![ws.to_string(), prefix.as_str(), dir.len() as i64, dir], manifest_from_row)?)
    }

    /// Live (non-deleted) entries whose case-folded path equals `path`'s.
    pub fn manifest_case_variants(&self, ws: WorkspaceId, path: &RelPath) -> Result<Vec<ManifestEntry>> {
        let mut st = self.conn.prepare(
            "SELECT * FROM file_manifest WHERE workspace_id = ?1 AND path_key = ?2 AND path <> ?3 AND deleted = 0",
        )?;
        collect(st.query_map(params![ws.to_string(), path.key(), path.as_str()], manifest_from_row)?)
    }

    /// A live path in the workspace whose current content has `hash`.
    pub fn manifest_find_hash(&self, ws: WorkspaceId, hash: &str) -> Result<Option<RelPath>> {
        self.conn
            .query_row(
                "SELECT path FROM file_manifest WHERE workspace_id = ?1 AND content_hash = ?2 AND deleted = 0 LIMIT 1",
                params![ws.to_string(), hash],
                |r| rel(r, "path"),
            )
            .optional()
            .map_err(map_corrupt)
    }

    /// Updates the cached stat data after a re-hash found no content change.
    pub fn manifest_touch(&self, ws: WorkspaceId, path: &RelPath, size: u64, mtime_ns: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE file_manifest SET size = ?1, mtime_ns = ?2 WHERE workspace_id = ?3 AND path = ?4",
            params![size as i64, mtime_ns, ws.to_string(), path.as_str()],
        )?;
        Ok(())
    }

    /// Forgets a path without creating a tombstone (e.g. it became excluded:
    /// excluding a file must not delete it on other devices).
    pub fn manifest_forget(&self, ws: WorkspaceId, path: &RelPath) -> Result<()> {
        self.conn.execute(
            "DELETE FROM file_manifest WHERE workspace_id = ?1 AND path = ?2",
            params![ws.to_string(), path.as_str()],
        )?;
        Ok(())
    }

    /// Point the manifest head at an already-recorded version with identical
    /// content (convergence). Keeps stat data.
    pub fn manifest_set_head(&self, ws: WorkspaceId, path: &RelPath, version_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE file_manifest SET version_id = ?1, updated_at = ?2 WHERE workspace_id = ?3 AND path = ?4",
            params![version_id, now_ms(), ws.to_string(), path.as_str()],
        )?;
        Ok(())
    }

    // ---------- versions ----------

    pub fn version_get(&self, version_id: &str) -> Result<Option<VersionRow>> {
        self.conn
            .query_row("SELECT * FROM file_versions WHERE version_id = ?1", [version_id], version_from_row)
            .optional()
            .map_err(map_corrupt)
    }

    pub fn version_known(&self, version_id: &str) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM file_versions WHERE version_id = ?1)",
            [version_id],
            |r| r.get(0),
        )?)
    }

    /// Records a version, optionally makes it the head (with `mtime_ns` as the
    /// stat cache), and queues it for every joined peer except `except`.
    /// All in one transaction.
    pub fn record_version(
        &self,
        v: &VersionRow,
        head: Head,
        mtime_ns: Option<i64>,
        except: Option<DeviceId>,
    ) -> Result<Vec<String>> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO file_versions (version_id, workspace_id, path, parent_version_id, content_hash, size,
                 mode, deleted, origin_device_id, created_at, ancestry, base_hashes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                v.version_id,
                v.workspace_id.to_string(),
                v.path.as_str(),
                v.ancestry.first(),
                v.content_hash,
                v.size as i64,
                v.mode.map(i64::from),
                v.deleted,
                v.origin_device_id.to_string(),
                v.created_at,
                serde_json::to_string(&v.ancestry)?,
                serde_json::to_string(&v.base_hashes)?,
            ],
        )?;
        let mut queued = Vec::new();
        if head == Head::Set {
            tx.execute(
                "INSERT INTO file_manifest (workspace_id, path, path_key, content_hash, size, mode, mtime_ns, version_id, deleted, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(workspace_id, path) DO UPDATE SET content_hash = excluded.content_hash, size = excluded.size,
                   mode = excluded.mode, mtime_ns = excluded.mtime_ns, version_id = excluded.version_id,
                   deleted = excluded.deleted, updated_at = excluded.updated_at",
                params![
                    v.workspace_id.to_string(),
                    v.path.as_str(),
                    v.path.key(),
                    v.content_hash,
                    v.size as i64,
                    v.mode.map(i64::from),
                    mtime_ns,
                    v.version_id,
                    v.deleted,
                    now_ms(),
                ],
            )?;
            let peers: Vec<String> = {
                let mut st = tx.prepare(
                    "SELECT device_id FROM workspace_devices wd JOIN devices d ON d.id = wd.device_id
                     WHERE wd.workspace_id = ?1 AND wd.shared = 1 AND wd.joined = 1 AND d.status = 'paired'",
                )?;
                st.query_map([v.workspace_id.to_string()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
            };
            let now = now_ms();
            for p in peers {
                if except.is_some_and(|e| e.to_string() == p) {
                    continue;
                }
                let change_id = ulid::Ulid::generate().to_string();
                // Older undelivered notifications for the same path are superseded.
                tx.execute(
                    "UPDATE change_events SET state = 'superseded', updated_at = ?1
                     WHERE workspace_id = ?2 AND peer_device_id = ?3 AND path = ?4 AND state IN ('queued','sent')",
                    params![now, v.workspace_id.to_string(), p, v.path.as_str()],
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO change_events (change_id, workspace_id, peer_device_id, version_id, path, state, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?6)",
                    params![change_id, v.workspace_id.to_string(), p, v.version_id, v.path.as_str(), now],
                )?;
                queued.push(change_id);
            }
        }
        tx.commit()?;
        Ok(queued)
    }

    // ---------- outgoing queue ----------

    /// Changes not yet sent to a peer (oldest first, at most `limit`).
    /// Sent-but-unacknowledged ones are re-queued by [`Store::requeue_unacked`].
    pub fn pending_changes(&self, ws: WorkspaceId, peer: DeviceId, limit: u32) -> Result<Vec<OutgoingChange>> {
        let mut st = self.conn.prepare(
            "SELECT c.change_id, v.* FROM change_events c JOIN file_versions v ON v.version_id = c.version_id
             WHERE c.workspace_id = ?1 AND c.peer_device_id = ?2 AND c.state = 'queued'
             ORDER BY c.created_at LIMIT ?3",
        )?;
        collect(st.query_map(params![ws.to_string(), peer.to_string(), limit], |r| {
            Ok(OutgoingChange { change_id: r.get("change_id")?, version: version_from_row(r)? })
        })?)
    }

    /// Every manifest entry with its version's ancestry and origin, for
    /// reconciliation with a peer.
    pub fn manifest_versions(&self, ws: WorkspaceId) -> Result<Vec<VersionRow>> {
        let mut st = self.conn.prepare(
            "SELECT v.* FROM file_manifest m JOIN file_versions v ON v.version_id = m.version_id
             WHERE m.workspace_id = ?1 ORDER BY m.path",
        )?;
        collect(st.query_map([ws.to_string()], version_from_row)?)
    }

    pub fn mark_changes_sent(&self, ids: &[String]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for id in ids {
            tx.execute(
                "UPDATE change_events SET state = 'sent', attempts = attempts + 1, updated_at = ?1 WHERE change_id = ?2 AND state = 'queued'",
                params![now_ms(), id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records a final ack. Pending acks leave the change in 'sent'.
    pub fn ack_change(&self, peer: DeviceId, change_id: &str, result: &str, reason: Option<&str>) -> Result<()> {
        let state = if result == "rejected" { "failed" } else { "acked" };
        self.conn.execute(
            "UPDATE change_events SET state = ?1, ack_result = ?2, last_error = ?3, updated_at = ?4
             WHERE change_id = ?5 AND peer_device_id = ?6",
            params![state, result, reason, now_ms(), change_id, peer.to_string()],
        )?;
        Ok(())
    }

    /// Re-queues notifications that were sent but never acknowledged (e.g. the
    /// connection dropped). Idempotent on the receiver.
    pub fn requeue_unacked(&self, peer: DeviceId) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE change_events SET state = 'queued' WHERE peer_device_id = ?1 AND state = 'sent'",
            [peer.to_string()],
        )?)
    }

    pub fn count_queued_for(&self, ws: WorkspaceId) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM change_events WHERE workspace_id = ?1 AND state IN ('queued','sent')",
            [ws.to_string()],
            |r| r.get(0),
        )?)
    }

    // ---------- transfers ----------

    pub fn transfer_insert(&self, t: &TransferJob) -> Result<()> {
        self.conn.execute(
            "INSERT INTO transfer_jobs (id, workspace_id, peer_device_id, change_id, path, version_id, ancestry,
                origin_device_id, content_hash, total_size, mode, bytes_done, state, attempts, created_at, updated_at, base_hashes)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?15,?16)",
            params![
                t.id,
                t.workspace_id.to_string(),
                t.peer_device_id.to_string(),
                t.change_id,
                t.path.as_str(),
                t.version_id,
                serde_json::to_string(&t.ancestry)?,
                t.origin_device_id.to_string(),
                t.content_hash,
                t.total_size as i64,
                t.mode.map(i64::from),
                t.bytes_done as i64,
                t.state,
                t.attempts as i64,
                now_ms(),
                serde_json::to_string(&t.base_hashes)?,
            ],
        )?;
        Ok(())
    }

    pub fn transfer_get(&self, id: &str) -> Result<Option<TransferJob>> {
        self.conn
            .query_row("SELECT * FROM transfer_jobs WHERE id = ?1", [id], transfer_from_row)
            .optional()
            .map_err(map_corrupt)
    }

    pub fn transfer_for_version(&self, version_id: &str) -> Result<Option<TransferJob>> {
        self.conn
            .query_row(
                "SELECT * FROM transfer_jobs WHERE version_id = ?1 AND state IN ('pending','active')",
                [version_id],
                transfer_from_row,
            )
            .optional()
            .map_err(map_corrupt)
    }

    /// Unfinished transfers from a peer (resumed on reconnect / restart).
    pub fn transfers_unfinished(&self, peer: DeviceId) -> Result<Vec<TransferJob>> {
        let mut st = self.conn.prepare(
            "SELECT * FROM transfer_jobs WHERE peer_device_id = ?1 AND state IN ('pending','active') ORDER BY created_at",
        )?;
        collect(st.query_map([peer.to_string()], transfer_from_row)?)
    }

    pub fn transfers_active_count(&self, ws: WorkspaceId) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT count(*) FROM transfer_jobs WHERE workspace_id = ?1 AND state IN ('pending','active')",
            [ws.to_string()],
            |r| r.get(0),
        )?)
    }

    /// After a crash, transfers marked active have no connection: make them
    /// pending again (their partial staging files are resumed).
    pub fn transfers_reset_active(&self) -> Result<usize> {
        Ok(self.conn.execute("UPDATE transfer_jobs SET state = 'pending' WHERE state = 'active'", [])?)
    }

    /// Puts failed transfers back in the queue (user chose Retry).
    pub fn transfers_retry_failed(&self, ws: WorkspaceId) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE transfer_jobs SET state = 'pending', attempts = 0, last_error = NULL, updated_at = ?1
             WHERE workspace_id = ?2 AND state = 'failed'",
            params![now_ms(), ws.to_string()],
        )?)
    }

    pub fn transfers_failed(&self, ws: WorkspaceId) -> Result<Vec<TransferJob>> {
        let mut st = self.conn.prepare("SELECT * FROM transfer_jobs WHERE workspace_id = ?1 AND state = 'failed' ORDER BY updated_at DESC LIMIT 200")?;
        collect(st.query_map([ws.to_string()], transfer_from_row)?)
    }

    pub fn transfers_list(&self, ws: WorkspaceId) -> Result<Vec<TransferJob>> {
        let mut st = self.conn.prepare(
            "SELECT * FROM transfer_jobs WHERE workspace_id = ?1 AND state IN ('pending','active') ORDER BY created_at",
        )?;
        collect(st.query_map([ws.to_string()], transfer_from_row)?)
    }

    pub fn transfer_update(&self, id: &str, state: &str, bytes_done: u64, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE transfer_jobs SET state = ?1, bytes_done = ?2, last_error = COALESCE(?3, last_error), updated_at = ?4,
               attempts = attempts + CASE WHEN ?1 = 'active' AND state <> 'active' THEN 1 ELSE 0 END
             WHERE id = ?5",
            params![state, bytes_done as i64, error, now_ms(), id],
        )?;
        Ok(())
    }

    /// Marks pending transfers for an older announcement of `path` superseded.
    pub fn transfers_supersede_path(&self, ws: WorkspaceId, path: &RelPath, except: &str) -> Result<Vec<String>> {
        let mut st = self.conn.prepare(
            "SELECT id FROM transfer_jobs WHERE workspace_id = ?1 AND path = ?2 AND id <> ?3 AND state IN ('pending','active')",
        )?;
        let ids: Vec<String> = st
            .query_map(params![ws.to_string(), path.as_str(), except], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in &ids {
            self.transfer_update(id, "superseded", 0, None)?;
        }
        Ok(ids)
    }

    // ---------- conflicts ----------

    pub fn conflict_insert(&self, c: &ConflictRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conflicts (id, workspace_id, path, kind, local_version_id, local_hash, local_size,
                remote_version_id, remote_hash, remote_size, remote_mode, remote_ancestry, remote_device_id, state, detected_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,'open',?14)",
            params![
                c.id,
                c.workspace_id.to_string(),
                c.path.as_str(),
                c.kind,
                c.local_version_id,
                c.local_hash,
                c.local_size.map(|v| v as i64),
                c.remote_version_id,
                c.remote_hash,
                c.remote_size.map(|v| v as i64),
                c.remote_mode.map(i64::from),
                serde_json::to_string(&c.remote_ancestry)?,
                c.remote_device_id.to_string(),
                c.detected_at,
            ],
        )?;
        self.audit(
            "conflict.detected",
            Some(&c.workspace_id.to_string()),
            Some(&c.remote_device_id.to_string()),
            serde_json::json!({ "path": c.path.as_str(), "kind": c.kind }),
        )
    }

    pub fn conflict_get(&self, id: &str) -> Result<ConflictRecord> {
        self.conn
            .query_row("SELECT * FROM conflicts WHERE id = ?1", [id], conflict_from_row)
            .optional()
            .map_err(map_corrupt)?
            .ok_or(StoreError::NotFound("conflict"))
    }

    /// Open conflicts for a path (there can be several when two peers diverge).
    pub fn conflicts_open_for_path(&self, ws: WorkspaceId, path: &RelPath) -> Result<Vec<ConflictRecord>> {
        let mut st =
            self.conn.prepare("SELECT * FROM conflicts WHERE workspace_id = ?1 AND path = ?2 AND state = 'open'")?;
        collect(st.query_map(params![ws.to_string(), path.as_str()], conflict_from_row)?)
    }

    pub fn conflict_for_remote_version(&self, version_id: &str) -> Result<Option<ConflictRecord>> {
        self.conn
            .query_row("SELECT * FROM conflicts WHERE remote_version_id = ?1", [version_id], conflict_from_row)
            .optional()
            .map_err(map_corrupt)
    }

    /// All conflicts (`open_only`) across workspaces, newest first.
    pub fn conflicts_list(&self, ws: Option<WorkspaceId>, open_only: bool) -> Result<Vec<ConflictRecord>> {
        let mut st = self.conn.prepare(
            "SELECT * FROM conflicts WHERE (?1 IS NULL OR workspace_id = ?1) AND (?2 = 0 OR state = 'open')
             ORDER BY detected_at DESC LIMIT 1000",
        )?;
        collect(st.query_map(params![ws.map(|w| w.to_string()), open_only], conflict_from_row)?)
    }

    pub fn conflict_resolve(&self, id: &str, resolution: &str) -> Result<()> {
        let c = self.conflict_get(id)?;
        self.conn.execute(
            "UPDATE conflicts SET state = 'resolved', resolution = ?1, resolved_at = ?2 WHERE id = ?3",
            params![resolution, now_ms(), id],
        )?;
        self.audit(
            "conflict.resolved",
            Some(&c.workspace_id.to_string()),
            None,
            serde_json::json!({ "path": c.path.as_str(), "resolution": resolution }),
        )
    }

    // ---------- issues ----------

    /// Replaces the issue set for a workspace (or only those under `scope`).
    pub fn issues_replace(
        &self,
        ws: WorkspaceId,
        scope: Option<&RelPath>,
        issues: &[(String, String, String)],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        match scope {
            None => {
                tx.execute("DELETE FROM sync_issues WHERE workspace_id = ?1 AND kind <> 'rejected'", [ws.to_string()])?;
            }
            Some(p) => {
                let dir = format!("{}/", p.as_str());
                tx.execute(
                    "DELETE FROM sync_issues WHERE workspace_id = ?1 AND kind <> 'rejected'
                       AND (path = ?2 OR substr(path, 1, ?3) = ?4)",
                    params![ws.to_string(), p.as_str(), dir.len() as i64, dir],
                )?;
            }
        }
        let now = now_ms();
        for (path, kind, detail) in issues {
            tx.execute(
                "INSERT OR IGNORE INTO sync_issues (workspace_id, path, kind, detail, first_seen_at) VALUES (?1,?2,?3,?4,?5)",
                params![ws.to_string(), path, kind, detail, now],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn issue_add(&self, ws: WorkspaceId, path: &str, kind: &str, detail: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sync_issues (workspace_id, path, kind, detail, first_seen_at) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT DO UPDATE SET detail = excluded.detail",
            params![ws.to_string(), path, kind, detail, now_ms()],
        )?;
        Ok(())
    }

    pub fn issue_dismiss(&self, ws: WorkspaceId, path: &str, kind: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM sync_issues WHERE workspace_id = ?1 AND path = ?2 AND kind = ?3",
            params![ws.to_string(), path, kind],
        )?;
        Ok(())
    }

    pub fn issues_list(&self, ws: Option<WorkspaceId>) -> Result<Vec<SyncIssue>> {
        let mut st = self.conn.prepare(
            "SELECT * FROM sync_issues WHERE (?1 IS NULL OR workspace_id = ?1) ORDER BY workspace_id, path LIMIT 2000",
        )?;
        collect(st.query_map([ws.map(|w| w.to_string())], |r| {
            Ok(SyncIssue {
                workspace_id: parse_col(r, "workspace_id")?,
                path: r.get("path")?,
                kind: r.get("kind")?,
                detail: r.get("detail")?,
                first_seen_at: r.get("first_seen_at")?,
            })
        })?)
    }

    pub fn issue_paths(&self, ws: WorkspaceId) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT DISTINCT path FROM sync_issues WHERE workspace_id = ?1")?;
        Ok(st.query_map([ws.to_string()], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    pub fn count_issues(&self, ws: WorkspaceId) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM sync_issues WHERE workspace_id = ?1", [ws.to_string()], |r| r.get(0))?)
    }

    // ---------- recovery ----------

    #[allow(clippy::too_many_arguments)]
    pub fn recovery_add(
        &self,
        ws: WorkspaceId,
        path: &RelPath,
        hash: &str,
        size: u64,
        reason: &str,
        conflict_id: Option<&str>,
        pinned: bool,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO recovery_items (workspace_id, path, content_hash, size, reason, conflict_id, pinned, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![ws.to_string(), path.as_str(), hash, size as i64, reason, conflict_id, pinned, now_ms()],
        )?;
        Ok(())
    }

    pub fn recovery_list(&self, ws: Option<WorkspaceId>) -> Result<Vec<RecoveryItem>> {
        let mut st = self.conn.prepare(
            "SELECT * FROM recovery_items WHERE removed_at IS NULL AND (?1 IS NULL OR workspace_id = ?1)
             ORDER BY created_at DESC LIMIT 2000",
        )?;
        collect(st.query_map([ws.map(|w| w.to_string())], |r| {
            Ok(RecoveryItem {
                id: r.get("id")?,
                workspace_id: r.get("workspace_id")?,
                path: r.get("path")?,
                content_hash: r.get("content_hash")?,
                size: r.get::<_, i64>("size")? as u64,
                reason: r.get("reason")?,
                conflict_id: r.get("conflict_id")?,
                pinned: r.get("pinned")?,
                created_at: r.get("created_at")?,
            })
        })?)
    }

    pub fn recovery_get(&self, id: i64) -> Result<RecoveryItem> {
        self.recovery_list(None)?.into_iter().find(|r| r.id == id).ok_or(StoreError::NotFound("recovery item"))
    }

    pub fn recovery_for_conflict(&self, conflict_id: &str) -> Result<Vec<RecoveryItem>> {
        Ok(self.recovery_list(None)?.into_iter().filter(|r| r.conflict_id.as_deref() == Some(conflict_id)).collect())
    }

    /// Marks an item removed; returns its hash if no other live item uses it
    /// (so the caller can delete the blob).
    pub fn recovery_remove(&self, id: i64) -> Result<Option<String>> {
        let hash: Option<String> = self
            .conn
            .query_row("SELECT content_hash FROM recovery_items WHERE id = ?1 AND removed_at IS NULL", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let Some(hash) = hash else { return Ok(None) };
        self.conn.execute("UPDATE recovery_items SET removed_at = ?1 WHERE id = ?2", params![now_ms(), id])?;
        let still_used: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM recovery_items WHERE content_hash = ?1 AND removed_at IS NULL)",
            [&hash],
            |r| r.get(0),
        )?;
        Ok((!still_used).then_some(hash))
    }

    /// Unpinned items older than `cutoff_ms`.
    pub fn recovery_expired(&self, cutoff_ms: i64) -> Result<Vec<i64>> {
        let mut st = self
            .conn
            .prepare("SELECT id FROM recovery_items WHERE removed_at IS NULL AND pinned = 0 AND created_at < ?1")?;
        Ok(st.query_map([cutoff_ms], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
    }

    // ---------- sharing ----------

    pub fn share_workspace(&self, ws: WorkspaceId, peer: DeviceId) -> Result<()> {
        self.conn.execute(
            "INSERT INTO workspace_devices (workspace_id, device_id, role, shared) VALUES (?1, ?2, 'peer', 1)
             ON CONFLICT DO UPDATE SET shared = 1",
            params![ws.to_string(), peer.to_string()],
        )?;
        self.audit("workspace.shared", Some(&ws.to_string()), Some(&peer.to_string()), serde_json::json!({}))
    }

    pub fn set_peer_joined(&self, ws: WorkspaceId, peer: DeviceId, joined: bool) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE workspace_devices SET joined = ?1 WHERE workspace_id = ?2 AND device_id = ?3 AND shared = 1",
            params![joined, ws.to_string(), peer.to_string()],
        )? > 0)
    }

    pub fn unshare_workspace(&self, ws: WorkspaceId, peer: DeviceId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM workspace_devices WHERE workspace_id = ?1 AND device_id = ?2",
            params![ws.to_string(), peer.to_string()],
        )?;
        self.conn.execute(
            "UPDATE change_events SET state = 'superseded' WHERE workspace_id = ?1 AND peer_device_id = ?2 AND state IN ('queued','sent')",
            params![ws.to_string(), peer.to_string()],
        )?;
        self.audit("workspace.unshared", Some(&ws.to_string()), Some(&peer.to_string()), serde_json::json!({}))
    }

    /// True when `ws` actively syncs with `peer` (shared here and joined there).
    pub fn is_synced_with(&self, ws: WorkspaceId, peer: DeviceId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_devices wd JOIN devices d ON d.id = wd.device_id
              WHERE wd.workspace_id = ?1 AND wd.device_id = ?2 AND wd.shared = 1 AND wd.joined = 1 AND d.status = 'paired')",
            params![ws.to_string(), peer.to_string()],
            |r| r.get(0),
        )?)
    }

    pub fn is_shared_with(&self, ws: WorkspaceId, peer: DeviceId) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM workspace_devices WHERE workspace_id = ?1 AND device_id = ?2 AND shared = 1)",
            params![ws.to_string(), peer.to_string()],
            |r| r.get(0),
        )?)
    }

    pub fn share_states(&self, ws: WorkspaceId) -> Result<Vec<ShareState>> {
        let mut st = self.conn.prepare(
            "SELECT wd.device_id, d.name, wd.shared, wd.joined, wd.last_sync_at FROM workspace_devices wd
             JOIN devices d ON d.id = wd.device_id WHERE wd.workspace_id = ?1 AND d.status = 'paired'",
        )?;
        collect(st.query_map([ws.to_string()], |r| {
            Ok(ShareState {
                device_id: parse_col(r, "device_id")?,
                device_name: r.get(1)?,
                shared: r.get(2)?,
                joined: r.get(3)?,
                last_sync_at: r.get(4)?,
            })
        })?)
    }

    /// Workspaces actively synced with `peer`.
    pub fn synced_workspaces_with(&self, peer: DeviceId) -> Result<Vec<WorkspaceId>> {
        let mut st = self
            .conn
            .prepare("SELECT workspace_id FROM workspace_devices WHERE device_id = ?1 AND shared = 1 AND joined = 1")?;
        collect(st.query_map([peer.to_string()], |r| parse_col(r, "workspace_id"))?)
    }

    /// Workspaces this device offered to `peer` that the peer hasn't joined yet.
    pub fn unjoined_shares_with(&self, peer: DeviceId) -> Result<Vec<WorkspaceId>> {
        let mut st = self
            .conn
            .prepare("SELECT workspace_id FROM workspace_devices WHERE device_id = ?1 AND shared = 1 AND joined = 0")?;
        collect(st.query_map([peer.to_string()], |r| parse_col(r, "workspace_id"))?)
    }

    pub fn mark_synced(&self, ws: WorkspaceId, peer: DeviceId) -> Result<()> {
        let now = now_ms();
        self.conn.execute(
            "UPDATE workspace_devices SET last_sync_at = ?1 WHERE workspace_id = ?2 AND device_id = ?3",
            params![now, ws.to_string(), peer.to_string()],
        )?;
        self.conn.execute("UPDATE workspaces SET last_synced_at = ?1 WHERE id = ?2", params![now, ws.to_string()])?;
        Ok(())
    }

    pub fn set_sync_state(&self, ws: WorkspaceId, state: devbridge_core::SyncState) -> Result<()> {
        self.conn
            .execute("UPDATE workspaces SET sync_state = ?1 WHERE id = ?2", params![state.as_str(), ws.to_string()])?;
        Ok(())
    }

    pub fn set_active_device(&self, ws: WorkspaceId, device: DeviceId) -> Result<()> {
        self.conn.execute(
            "UPDATE workspaces SET active_device_id = ?1 WHERE id = ?2",
            params![device.to_string(), ws.to_string()],
        )?;
        Ok(())
    }

    // ---------- offers ----------

    pub fn offer_upsert(&self, o: &WorkspaceOffer) -> Result<()> {
        self.conn.execute(
            "INSERT INTO workspace_offers (workspace_id, device_id, name, color, icon, offered_at) VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT DO UPDATE SET name = excluded.name, color = excluded.color, icon = excluded.icon",
            params![
                o.workspace_id.to_string(),
                o.device_id.to_string(),
                o.name,
                o.color.as_str(),
                o.icon.as_str(),
                o.offered_at
            ],
        )?;
        Ok(())
    }

    pub fn offers_list(&self) -> Result<Vec<WorkspaceOffer>> {
        let mut st = self.conn.prepare(
            "SELECT o.*, d.name AS device_name FROM workspace_offers o JOIN devices d ON d.id = o.device_id
             WHERE d.status = 'paired'
               AND NOT EXISTS (SELECT 1 FROM workspace_devices wd WHERE wd.workspace_id = o.workspace_id
                               AND wd.device_id = o.device_id AND wd.joined = 1)
             ORDER BY offered_at DESC",
        )?;
        collect(st.query_map([], |r| {
            Ok(WorkspaceOffer {
                workspace_id: parse_col(r, "workspace_id")?,
                device_id: parse_col(r, "device_id")?,
                device_name: r.get("device_name")?,
                name: r.get("name")?,
                color: parse_col(r, "color")?,
                icon: parse_col(r, "icon")?,
                offered_at: r.get("offered_at")?,
            })
        })?)
    }

    pub fn offer_get(&self, ws: WorkspaceId, peer: DeviceId) -> Result<Option<WorkspaceOffer>> {
        Ok(self.offers_list()?.into_iter().find(|o| o.workspace_id == ws && o.device_id == peer))
    }

    pub fn offer_delete(&self, ws: WorkspaceId, peer: DeviceId) -> Result<()> {
        self.conn.execute(
            "DELETE FROM workspace_offers WHERE workspace_id = ?1 AND device_id = ?2",
            params![ws.to_string(), peer.to_string()],
        )?;
        Ok(())
    }

    // ---------- peers ----------

    /// Stores a newly paired device (or re-pairs a previously revoked one).
    pub fn add_paired_device(
        &self,
        id: DeviceId,
        name: &str,
        os: OsKind,
        fingerprint: &str,
        address: Option<&str>,
    ) -> Result<Device> {
        let now = now_ms();
        let tx = self.conn.unchecked_transaction()?;
        // A fingerprint identifies exactly one device; drop stale rows for it.
        tx.execute(
            "DELETE FROM devices WHERE cert_fingerprint = ?1 AND id <> ?2 AND is_local = 0",
            params![fingerprint, id.to_string()],
        )?;
        tx.execute(
            "INSERT INTO devices (id, name, os, status, role, is_local, cert_fingerprint, last_address, last_seen_at, created_at)
             VALUES (?1, ?2, ?3, 'paired', 'peer', 0, ?4, ?5, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, os = excluded.os, status = 'paired',
               cert_fingerprint = excluded.cert_fingerprint, last_address = excluded.last_address,
               last_seen_at = excluded.last_seen_at, revoked_at = NULL",
            params![id.to_string(), name, os.as_str(), fingerprint, address, now],
        )?;
        tx.commit()?;
        self.audit(
            "device.paired",
            None,
            Some(&id.to_string()),
            serde_json::json!({ "name": name, "os": os.as_str() }),
        )?;
        self.device_get(id)
    }

    pub fn device_get(&self, id: DeviceId) -> Result<Device> {
        self.conn
            .query_row("SELECT * FROM devices WHERE id = ?1", [id.to_string()], device_from_row)
            .optional()
            .map_err(map_corrupt)?
            .ok_or(StoreError::NotFound("device"))
    }

    /// The paired (non-revoked) device whose certificate has `fingerprint`.
    pub fn paired_by_fingerprint(&self, fingerprint: &str) -> Result<Option<Device>> {
        self.conn
            .query_row(
                "SELECT * FROM devices WHERE cert_fingerprint = ?1 AND status = 'paired' AND is_local = 0",
                [fingerprint],
                device_from_row,
            )
            .optional()
            .map_err(map_corrupt)
    }

    pub fn paired_devices(&self) -> Result<Vec<Device>> {
        Ok(self.list_devices()?.into_iter().filter(|d| d.status == DeviceStatus::Paired).collect())
    }

    pub fn device_address(&self, id: DeviceId) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT last_address FROM devices WHERE id = ?1", [id.to_string()], |r| r.get(0))
            .optional()?
            .flatten())
    }

    pub fn device_seen(&self, id: DeviceId, address: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE devices SET last_seen_at = ?1, last_address = COALESCE(?2, last_address) WHERE id = ?3",
            params![now_ms(), address, id.to_string()],
        )?;
        Ok(())
    }

    pub fn device_set_role(&self, id: DeviceId, role: DeviceRole) -> Result<()> {
        self.conn.execute("UPDATE devices SET role = ?1 WHERE id = ?2", params![role.as_str(), id.to_string()])?;
        Ok(())
    }

    /// Revokes a device: removes its pin, its shares and its queued changes.
    pub fn revoke_device(&self, id: DeviceId) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let n = tx.execute(
            "UPDATE devices SET status = 'revoked', revoked_at = ?1, cert_fingerprint = NULL WHERE id = ?2 AND is_local = 0",
            params![now_ms(), id.to_string()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound("device"));
        }
        tx.execute("DELETE FROM workspace_devices WHERE device_id = ?1", [id.to_string()])?;
        tx.execute("DELETE FROM workspace_offers WHERE device_id = ?1", [id.to_string()])?;
        tx.execute(
            "UPDATE change_events SET state = 'superseded' WHERE peer_device_id = ?1 AND state IN ('queued','sent')",
            [id.to_string()],
        )?;
        tx.execute(
            "UPDATE transfer_jobs SET state = 'cancelled' WHERE peer_device_id = ?1 AND state IN ('pending','active')",
            [id.to_string()],
        )?;
        tx.commit()?;
        self.audit("device.revoked", None, Some(&id.to_string()), serde_json::json!({}))
    }

    // ---------- sessions ----------

    pub fn session_start(&self, ws: WorkspaceId, peer: DeviceId) -> Result<String> {
        let id = ulid::Ulid::generate().to_string();
        self.conn.execute(
            "INSERT INTO sync_sessions (id, workspace_id, peer_device_id, state, started_at) VALUES (?1,?2,?3,'active',?4)",
            params![id, ws.to_string(), peer.to_string(), now_ms()],
        )?;
        Ok(id)
    }

    pub fn session_add(
        &self,
        id: &str,
        files_sent: u64,
        files_received: u64,
        bytes_sent: u64,
        bytes_received: u64,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE sync_sessions SET files_sent = files_sent + ?1, files_received = files_received + ?2,
               bytes_sent = bytes_sent + ?3, bytes_received = bytes_received + ?4 WHERE id = ?5",
            params![files_sent as i64, files_received as i64, bytes_sent as i64, bytes_received as i64, id],
        )?;
        Ok(())
    }

    pub fn session_end(&self, id: &str, state: &str, error: Option<&str>) -> Result<()> {
        self.conn.execute(
            "UPDATE sync_sessions SET state = ?1, error = ?2, ended_at = ?3 WHERE id = ?4 AND state = 'active'",
            params![state, error, now_ms(), id],
        )?;
        Ok(())
    }

    /// Sessions left 'active' by a crash are marked interrupted at startup.
    pub fn sessions_mark_interrupted(&self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE sync_sessions SET state = 'interrupted', ended_at = ?1 WHERE state = 'active'",
            [now_ms()],
        )?)
    }

    /// Stores/reads small JSON blobs (e.g. the public identity certificate).
    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM app_settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now_ms()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devbridge_core::{SyncState, Workspace, WorkspaceMode};

    fn setup() -> (Store, WorkspaceId, DeviceId, DeviceId) {
        let s = Store::open_in_memory().unwrap();
        let me = s.ensure_local_device("Mac", OsKind::MacOs, None).unwrap().id;
        let ws = WorkspaceId::new();
        s.insert_workspace(&Workspace {
            id: ws,
            name: "W".into(),
            root_path: "/w".into(),
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
        let peer = DeviceId::new();
        s.add_paired_device(peer, "PC", OsKind::Windows, "fp-pc", Some("10.0.0.2:47821")).unwrap();
        (s, ws, me, peer)
    }

    fn ver(ws: WorkspaceId, origin: DeviceId, id: &str, path: &str, anc: &[&str], hash: Option<&str>) -> VersionRow {
        VersionRow {
            version_id: id.into(),
            workspace_id: ws,
            path: RelPath::parse(path).unwrap(),
            ancestry: anc.iter().map(|s| s.to_string()).collect(),
            base_hashes: vec![],
            content_hash: hash.map(Into::into),
            size: 1,
            mode: Some(0o644),
            deleted: hash.is_none(),
            origin_device_id: origin,
            created_at: 0,
        }
    }

    #[test]
    fn versions_queue_only_for_joined_peers_and_supersede() {
        let (s, ws, me, peer) = setup();
        let q = s.record_version(&ver(ws, me, "v1", "a.txt", &[], Some("h1")), Head::Set, Some(1), None).unwrap();
        assert!(q.is_empty(), "not shared yet");

        s.share_workspace(ws, peer).unwrap();
        assert!(!s.is_synced_with(ws, peer).unwrap());
        assert!(s.set_peer_joined(ws, peer, true).unwrap());
        assert!(s.is_synced_with(ws, peer).unwrap());

        s.record_version(&ver(ws, me, "v2", "a.txt", &["v1"], Some("h2")), Head::Set, Some(2), None).unwrap();
        s.record_version(&ver(ws, me, "v3", "a.txt", &["v2", "v1"], Some("h3")), Head::Set, Some(3), None).unwrap();
        let pending = s.pending_changes(ws, peer, 100).unwrap();
        assert_eq!(pending.len(), 1, "older notification superseded");
        assert_eq!(s.manifest_versions(ws).unwrap()[0].version_id, "v3");
        assert_eq!(pending[0].version.version_id, "v3");
        assert_eq!(pending[0].version.ancestry, ["v2", "v1"]);

        // Forwarded versions aren't echoed back to their sender.
        let q = s.record_version(&ver(ws, peer, "v4", "b.txt", &[], Some("h")), Head::Set, None, Some(peer)).unwrap();
        assert!(q.is_empty());

        let m = s.manifest_get(ws, &RelPath::parse("a.txt").unwrap()).unwrap().unwrap();
        assert_eq!((m.version_id.as_str(), m.content_hash.as_deref()), ("v3", Some("h3")));
        assert!(s.version_known("v2").unwrap());

        s.mark_changes_sent(&[pending[0].change_id.clone()]).unwrap();
        assert!(s.pending_changes(ws, peer, 100).unwrap().is_empty(), "sent, awaiting ack");
        assert_eq!(s.requeue_unacked(peer).unwrap(), 1);
        assert_eq!(s.pending_changes(ws, peer, 100).unwrap().len(), 1);
        s.ack_change(peer, &pending[0].change_id, "applied", None).unwrap();
        assert!(s.pending_changes(ws, peer, 100).unwrap().is_empty());
    }

    #[test]
    fn manifest_queries() {
        let (s, ws, me, _) = setup();
        for (id, p, h) in [("v1", "src/A.ts", Some("h1")), ("v2", "src/b.ts", Some("h2")), ("v3", "srcx/c", Some("h3"))]
        {
            s.record_version(&ver(ws, me, id, p, &[], h), Head::Set, None, None).unwrap();
        }
        let under = s.manifest_under(ws, &RelPath::parse("src").unwrap()).unwrap();
        assert_eq!(under.len(), 2, "prefix match respects segment boundary");
        assert_eq!(s.manifest_case_variants(ws, &RelPath::parse("src/a.ts").unwrap()).unwrap().len(), 1);
        assert_eq!(s.manifest_find_hash(ws, "h2").unwrap().unwrap().as_str(), "src/b.ts");
        s.manifest_forget(ws, &RelPath::parse("src/b.ts").unwrap()).unwrap();
        assert!(s.manifest_find_hash(ws, "h2").unwrap().is_none());
    }

    #[test]
    fn transfers_conflicts_issues_recovery() {
        let (s, ws, _me, peer) = setup();
        let t = TransferJob {
            id: "t1".into(),
            workspace_id: ws,
            peer_device_id: peer,
            change_id: "c1".into(),
            path: RelPath::parse("a").unwrap(),
            version_id: "v9".into(),
            ancestry: vec!["v8".into()],
            base_hashes: vec![],
            origin_device_id: peer,
            content_hash: "h".into(),
            total_size: 10,
            mode: None,
            bytes_done: 0,
            state: "pending".into(),
            attempts: 0,
            last_error: None,
        };
        s.transfer_insert(&t).unwrap();
        assert_eq!(s.transfer_for_version("v9").unwrap().unwrap().id, "t1");
        s.transfer_update("t1", "active", 5, None).unwrap();
        assert_eq!(s.transfers_unfinished(peer).unwrap()[0].bytes_done, 5);
        s.transfer_update("t1", "done", 10, None).unwrap();
        assert!(s.transfers_unfinished(peer).unwrap().is_empty());

        let c = ConflictRecord {
            id: "k1".into(),
            workspace_id: ws,
            path: RelPath::parse("a").unwrap(),
            kind: "content".into(),
            local_version_id: Some("v1".into()),
            local_hash: Some("hl".into()),
            local_size: Some(1),
            remote_version_id: "v9".into(),
            remote_hash: Some("hr".into()),
            remote_size: Some(2),
            remote_mode: None,
            remote_ancestry: vec![],
            remote_device_id: peer,
            state: "open".into(),
            resolution: None,
            detected_at: 1,
            resolved_at: None,
        };
        s.conflict_insert(&c).unwrap();
        assert_eq!(s.count_open_conflicts(ws).unwrap(), 1);
        s.conflict_resolve("k1", "keep_local").unwrap();
        assert_eq!(s.count_open_conflicts(ws).unwrap(), 0);
        assert_eq!(s.conflict_get("k1").unwrap().resolution.as_deref(), Some("keep_local"));

        s.issues_replace(ws, None, &[("x?".into(), "unportable_path".into(), "bad".into())]).unwrap();
        s.issue_add(ws, "y", "rejected", "excluded").unwrap();
        s.issues_replace(ws, None, &[]).unwrap();
        let issues = s.issues_list(Some(ws)).unwrap();
        assert_eq!(issues.len(), 1, "rejections persist across rescans");

        s.recovery_add(ws, &RelPath::parse("a").unwrap(), "hh", 3, "overwritten", None, false).unwrap();
        s.recovery_add(ws, &RelPath::parse("b").unwrap(), "hh", 3, "conflict_local", Some("k1"), true).unwrap();
        let items = s.recovery_list(Some(ws)).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(s.recovery_expired(i64::MAX).unwrap().len(), 1, "pinned items never expire");
        let unpinned = items.iter().find(|i| !i.pinned).unwrap().id;
        assert_eq!(s.recovery_remove(unpinned).unwrap(), None, "blob still referenced");
        let pinned = items.iter().find(|i| i.pinned).unwrap().id;
        assert_eq!(s.recovery_remove(pinned).unwrap().as_deref(), Some("hh"));
    }

    #[test]
    fn revocation_blocks_sync_and_clears_queues() {
        let (s, ws, me, peer) = setup();
        assert_eq!(s.paired_by_fingerprint("fp-pc").unwrap().unwrap().id, peer);
        s.share_workspace(ws, peer).unwrap();
        s.set_peer_joined(ws, peer, true).unwrap();
        s.record_version(&ver(ws, me, "v1", "a", &[], Some("h")), Head::Set, None, None).unwrap();
        s.revoke_device(peer).unwrap();
        assert!(s.paired_by_fingerprint("fp-pc").unwrap().is_none());
        assert!(!s.is_synced_with(ws, peer).unwrap());
        assert!(s.pending_changes(ws, peer, 10).unwrap().is_empty());
        // Re-pairing restores it.
        s.add_paired_device(peer, "PC", OsKind::Windows, "fp-new", None).unwrap();
        assert_eq!(s.device_get(peer).unwrap().status, DeviceStatus::Paired);
    }

    #[test]
    fn offers_hide_once_joined() {
        let (s, ws, _me, peer) = setup();
        let other = WorkspaceId::new();
        s.offer_upsert(&WorkspaceOffer {
            workspace_id: other,
            device_id: peer,
            device_name: String::new(),
            name: "Site".into(),
            color: WorkspaceColor::Red,
            icon: WorkspaceIcon::Globe,
            offered_at: 1,
        })
        .unwrap();
        assert_eq!(s.offers_list().unwrap()[0].device_name, "PC");
        // An offer for a workspace already joined with that device is hidden.
        s.offer_upsert(&WorkspaceOffer { workspace_id: ws, ..s.offers_list().unwrap()[0].clone() }).unwrap();
        assert_eq!(s.offers_list().unwrap().len(), 2);
        s.share_workspace(ws, peer).unwrap();
        s.set_peer_joined(ws, peer, true).unwrap();
        assert_eq!(s.offers_list().unwrap().len(), 1);
    }
}
