-- DevBridge schema v2: sync engine.
-- change_events / transfer_jobs / conflicts were placeholders in v1 and are
-- never written by v1 builds, so they are rebuilt rather than altered.

ALTER TABLE file_versions ADD COLUMN ancestry TEXT NOT NULL DEFAULT '[]'; -- JSON array, nearest first

DROP TABLE transfer_jobs;
DROP TABLE change_events;
DROP TABLE conflicts;

-- Outgoing change notifications, one per (version, peer). Durable: survives
-- restarts and offline periods; redelivery is idempotent on the receiver.
CREATE TABLE change_events (
    change_id        TEXT PRIMARY KEY,
    workspace_id     TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    peer_device_id   TEXT NOT NULL,
    version_id       TEXT NOT NULL,
    path             TEXT NOT NULL,
    state            TEXT NOT NULL CHECK (state IN ('queued','sent','acked','superseded','failed')),
    ack_result       TEXT,
    attempts         INTEGER NOT NULL DEFAULT 0,
    last_error       TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    UNIQUE (peer_device_id, version_id)
);
CREATE INDEX change_events_pending ON change_events(workspace_id, peer_device_id, state);

-- Incoming content transfers. The announced version is kept here (not in
-- file_versions) until its content has been verified and applied.
CREATE TABLE transfer_jobs (
    id                 TEXT PRIMARY KEY,
    workspace_id       TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    peer_device_id     TEXT NOT NULL,
    change_id          TEXT NOT NULL,
    path               TEXT NOT NULL,
    version_id         TEXT NOT NULL,
    ancestry           TEXT NOT NULL DEFAULT '[]',
    origin_device_id   TEXT NOT NULL,
    content_hash       TEXT NOT NULL,
    total_size         INTEGER NOT NULL,
    mode               INTEGER,
    bytes_done         INTEGER NOT NULL DEFAULT 0,
    state              TEXT NOT NULL CHECK (state IN ('pending','active','done','failed','cancelled','superseded')),
    attempts           INTEGER NOT NULL DEFAULT 0,
    last_error         TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);
CREATE INDEX transfer_jobs_active ON transfer_jobs(peer_device_id, state);
CREATE UNIQUE INDEX transfer_jobs_version ON transfer_jobs(version_id) WHERE state IN ('pending','active');

CREATE TABLE conflicts (
    id                 TEXT PRIMARY KEY,
    workspace_id       TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path               TEXT NOT NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('content','local_deleted','remote_deleted')),
    local_version_id   TEXT,
    local_hash         TEXT,
    local_size         INTEGER,
    remote_version_id  TEXT NOT NULL,
    remote_hash        TEXT,
    remote_size        INTEGER,
    remote_mode        INTEGER,
    remote_ancestry    TEXT NOT NULL DEFAULT '[]',
    remote_device_id   TEXT NOT NULL,
    state              TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','resolved')),
    resolution         TEXT CHECK (resolution IN ('keep_local','keep_remote','save_both','merged','resolved_elsewhere')),
    detected_at        INTEGER NOT NULL,
    resolved_at        INTEGER
);
CREATE INDEX conflicts_open ON conflicts(workspace_id, state);

-- Paths that can't be synced (refreshed on every scan).
CREATE TABLE sync_issues (
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path          TEXT NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('unportable_path','case_collision','symlink','unreadable','rejected')),
    detail        TEXT NOT NULL DEFAULT '',
    first_seen_at INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, path, kind)
);

-- Copies kept in the recovery store. Conflict copies are pinned (kept until
-- the user removes them); overwrite/delete history expires after retention.
-- No FK: recovery data outlives a workspace's registration.
CREATE TABLE recovery_items (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    workspace_id  TEXT NOT NULL,
    path          TEXT NOT NULL,
    content_hash  TEXT NOT NULL,
    size          INTEGER NOT NULL,
    reason        TEXT NOT NULL CHECK (reason IN ('conflict_local','conflict_remote','overwritten','deleted')),
    conflict_id   TEXT,
    pinned        INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0,1)),
    created_at    INTEGER NOT NULL,
    removed_at    INTEGER
);
CREATE INDEX recovery_items_ws ON recovery_items(workspace_id, removed_at);
CREATE INDEX recovery_items_hash ON recovery_items(content_hash);

-- Sharing: a workspace syncs with a peer only when this device shared it AND
-- the peer joined (both rows present on both sides).
ALTER TABLE workspace_devices ADD COLUMN joined INTEGER NOT NULL DEFAULT 0 CHECK (joined IN (0,1));

-- Workspaces offered to this device by a paired peer, awaiting a decision.
CREATE TABLE workspace_offers (
    workspace_id  TEXT NOT NULL,
    device_id     TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    color         TEXT NOT NULL,
    icon          TEXT NOT NULL,
    offered_at    INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, device_id)
);
