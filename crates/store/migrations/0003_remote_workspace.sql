-- DevBridge schema v3: Remote Workspace Mode.

-- Workspaces this device (as host) makes available to approved clients.
CREATE TABLE remote_hosting (
    workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    enabled_at   INTEGER NOT NULL
);

-- Client devices whose SSH key this host added to authorized_keys.
CREATE TABLE remote_grants (
    device_id   TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    public_key  TEXT NOT NULL,
    granted_at  INTEGER NOT NULL
);

-- Hosts this device (as client) may connect to.
CREATE TABLE remote_hosts (
    device_id   TEXT PRIMARY KEY REFERENCES devices(id) ON DELETE CASCADE,
    alias       TEXT NOT NULL,
    ssh_user    TEXT NOT NULL,
    ssh_host    TEXT NOT NULL,
    ssh_port    INTEGER NOT NULL,
    host_os     TEXT NOT NULL,
    key_path    TEXT NOT NULL,
    workspaces  TEXT NOT NULL DEFAULT '[]',   -- JSON [{ws, name, path}]
    granted_at  INTEGER NOT NULL
);
