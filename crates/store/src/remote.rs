//! Persistence for Remote Workspace Mode.

use devbridge_core::protocol::RemoteWorkspace;
use devbridge_core::time::now_ms;
use devbridge_core::{DeviceId, OsKind, WorkspaceId};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::{Result, Store};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteGrant {
    pub device_id: DeviceId,
    pub granted_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHost {
    pub device_id: DeviceId,
    pub alias: String,
    pub ssh_user: String,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub host_os: OsKind,
    pub key_path: String,
    pub workspaces: Vec<RemoteWorkspace>,
    pub granted_at: i64,
}

impl Store {
    pub fn remote_hosting(&self) -> Result<Vec<WorkspaceId>> {
        let mut st = self.conn.prepare("SELECT workspace_id FROM remote_hosting")?;
        let ids: Vec<String> = st.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        Ok(ids.iter().filter_map(|s| s.parse().ok()).collect())
    }

    pub fn set_remote_hosting(&self, ws: WorkspaceId, enabled: bool) -> Result<()> {
        if enabled {
            self.conn.execute(
                "INSERT OR IGNORE INTO remote_hosting (workspace_id, enabled_at) VALUES (?1, ?2)",
                params![ws.to_string(), now_ms()],
            )?;
        } else {
            self.conn.execute("DELETE FROM remote_hosting WHERE workspace_id = ?1", [ws.to_string()])?;
        }
        self.audit(
            if enabled { "remote.hosting_enabled" } else { "remote.hosting_disabled" },
            Some(&ws.to_string()),
            None,
            serde_json::json!({}),
        )
    }

    pub fn remote_grant(&self, device: DeviceId, public_key: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO remote_grants (device_id, public_key, granted_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(device_id) DO UPDATE SET public_key = excluded.public_key, granted_at = excluded.granted_at",
            params![device.to_string(), public_key, now_ms()],
        )?;
        self.audit("remote.access_granted", None, Some(&device.to_string()), serde_json::json!({}))
    }

    pub fn remote_grants(&self) -> Result<Vec<RemoteGrant>> {
        let mut st = self.conn.prepare("SELECT device_id, granted_at FROM remote_grants")?;
        let rows: Vec<(String, i64)> =
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(d, t)| Some(RemoteGrant { device_id: d.parse().ok()?, granted_at: t }))
            .collect())
    }

    pub fn remote_ungrant(&self, device: DeviceId) -> Result<bool> {
        let n = self.conn.execute("DELETE FROM remote_grants WHERE device_id = ?1", [device.to_string()])?;
        if n > 0 {
            self.audit("remote.access_revoked", None, Some(&device.to_string()), serde_json::json!({}))?;
        }
        Ok(n > 0)
    }

    pub fn remote_host_save(&self, h: &RemoteHost) -> Result<()> {
        self.conn.execute(
            "INSERT INTO remote_hosts (device_id, alias, ssh_user, ssh_host, ssh_port, host_os, key_path, workspaces, granted_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(device_id) DO UPDATE SET alias = excluded.alias, ssh_user = excluded.ssh_user,
               ssh_host = excluded.ssh_host, ssh_port = excluded.ssh_port, host_os = excluded.host_os,
               key_path = excluded.key_path, workspaces = excluded.workspaces, granted_at = excluded.granted_at",
            params![
                h.device_id.to_string(),
                h.alias,
                h.ssh_user,
                h.ssh_host,
                h.ssh_port,
                h.host_os.as_str(),
                h.key_path,
                serde_json::to_string(&h.workspaces)?,
                h.granted_at
            ],
        )?;
        Ok(())
    }

    pub fn remote_hosts(&self) -> Result<Vec<RemoteHost>> {
        let mut st = self.conn.prepare("SELECT * FROM remote_hosts")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>("device_id")?,
                r.get::<_, String>("alias")?,
                r.get::<_, String>("ssh_user")?,
                r.get::<_, String>("ssh_host")?,
                r.get::<_, i64>("ssh_port")?,
                r.get::<_, String>("host_os")?,
                r.get::<_, String>("key_path")?,
                r.get::<_, String>("workspaces")?,
                r.get::<_, i64>("granted_at")?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (d, alias, user, host, port, os, key, ws, at) = row?;
            out.push(RemoteHost {
                device_id: d.parse().map_err(|_| crate::StoreError::Corrupt("remote host id".into()))?,
                alias,
                ssh_user: user,
                ssh_host: host,
                ssh_port: port as u16,
                host_os: os.parse().map_err(|_| crate::StoreError::Corrupt("remote host os".into()))?,
                key_path: key,
                workspaces: serde_json::from_str(&ws)?,
                granted_at: at,
            });
        }
        Ok(out)
    }

    pub fn remote_host_get(&self, device: DeviceId) -> Result<Option<RemoteHost>> {
        Ok(self.remote_hosts()?.into_iter().find(|h| h.device_id == device))
    }

    pub fn remote_host_delete(&self, device: DeviceId) -> Result<Option<RemoteHost>> {
        let h = self.remote_host_get(device)?;
        self.conn.execute("DELETE FROM remote_hosts WHERE device_id = ?1", [device.to_string()]).optional()?;
        Ok(h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_tables_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let peer = DeviceId::new();
        s.add_paired_device(peer, "PC", OsKind::Windows, "fp", None).unwrap();
        s.remote_grant(peer, "ssh-ed25519 AAAA").unwrap();
        assert_eq!(s.remote_grants().unwrap().len(), 1);
        assert!(s.remote_ungrant(peer).unwrap());
        let h = RemoteHost {
            device_id: peer,
            alias: "devbridge-pc".into(),
            ssh_user: "nick".into(),
            ssh_host: "10.0.0.2".into(),
            ssh_port: 22,
            host_os: OsKind::Windows,
            key_path: "/k".into(),
            workspaces: vec![RemoteWorkspace { ws: WorkspaceId::new(), name: "Site".into(), path: r"C:\site".into() }],
            granted_at: 1,
        };
        s.remote_host_save(&h).unwrap();
        assert_eq!(s.remote_hosts().unwrap()[0].workspaces[0].path, r"C:\site");
        assert!(s.remote_host_delete(peer).unwrap().is_some());
        assert!(s.remote_hosts().unwrap().is_empty());
    }
}
