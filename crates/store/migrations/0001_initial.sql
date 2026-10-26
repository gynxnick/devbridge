-- DevBridge schema v1.
-- Conventions: IDs are ULID text; timestamps are Unix epoch milliseconds;
-- paths inside a workspace are relative, '/'-separated, NFC-normalized.
-- Secrets (private keys, session tokens) are NEVER stored here; they live in
-- the OS credential store.

CREATE TABLE devices (
    id               TEXT PRIMARY KEY,
    name             TEXT NOT NULL,
    os               TEXT NOT NULL CHECK (os IN ('macos','windows','linux','other')),
    os_version       TEXT,
    status           TEXT NOT NULL CHECK (status IN ('local','pending','paired','revoked')),
    role             TEXT NOT NULL DEFAULT 'peer' CHECK (role IN ('peer','host','client')),
    is_local         INTEGER NOT NULL DEFAULT 0 CHECK (is_local IN (0,1)),
    -- SHA-256 of the peer's TLS SubjectPublicKeyInfo, pinned at pairing.
    cert_fingerprint TEXT UNIQUE,
    last_address     TEXT,
    last_seen_at     INTEGER,
    created_at       INTEGER NOT NULL,
    revoked_at       INTEGER
);
-- Exactly one row may describe this installation.
CREATE UNIQUE INDEX devices_one_local ON devices(is_local) WHERE is_local = 1;

CREATE TABLE workspaces (
    id               TEXT PRIMARY KEY,
    name             TEXT NOT NULL,
    root_path        TEXT NOT NULL UNIQUE,
    color            TEXT NOT NULL,
    icon             TEXT NOT NULL,
    mode             TEXT NOT NULL DEFAULT 'synced' CHECK (mode IN ('synced','remote')),
    sync_state       TEXT NOT NULL DEFAULT 'local_only'
                     CHECK (sync_state IN ('local_only','paused','idle','syncing','offline','error')),
    active_device_id TEXT REFERENCES devices(id) ON DELETE SET NULL,
    last_synced_at   INTEGER,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL
);

-- Which devices share which workspace, and in what role.
CREATE TABLE workspace_devices (
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    device_id     TEXT NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    role          TEXT NOT NULL DEFAULT 'peer' CHECK (role IN ('peer','host','client')),
    shared        INTEGER NOT NULL DEFAULT 1 CHECK (shared IN (0,1)),
    last_sync_at  INTEGER,
    PRIMARY KEY (workspace_id, device_id)
);

-- Current known state of each path on this device.
CREATE TABLE file_manifest (
    workspace_id  TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path          TEXT NOT NULL,
    -- Case-folded path used to detect collisions across case-insensitive filesystems.
    path_key      TEXT NOT NULL,
    content_hash  TEXT,            -- hex BLAKE3; NULL for tombstones
    size          INTEGER NOT NULL DEFAULT 0,
    mode          INTEGER,         -- Unix permission bits when known
    mtime_ns      INTEGER,         -- fast-path change filter only
    version_id    TEXT NOT NULL,
    deleted       INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0,1)),
    updated_at    INTEGER NOT NULL,
    PRIMARY KEY (workspace_id, path)
);
CREATE INDEX file_manifest_path_key ON file_manifest(workspace_id, path_key);

-- Version ancestry for every path. Used to tell fast-forwards from conflicts.
CREATE TABLE file_versions (
    version_id        TEXT PRIMARY KEY,
    workspace_id      TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path              TEXT NOT NULL,
    parent_version_id TEXT,
    content_hash      TEXT,
    size              INTEGER NOT NULL DEFAULT 0,
    mode              INTEGER,
    deleted           INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0,1)),
    origin_device_id  TEXT NOT NULL,
    created_at        INTEGER NOT NULL
);
CREATE INDEX file_versions_path ON file_versions(workspace_id, path);

-- Durable queue of changes to send (outgoing) or apply (incoming).
CREATE TABLE change_events (
    change_id          TEXT PRIMARY KEY,
    workspace_id       TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    direction          TEXT NOT NULL CHECK (direction IN ('outgoing','incoming')),
    peer_device_id     TEXT NOT NULL,
    origin_device_id   TEXT NOT NULL,
    op                 TEXT NOT NULL CHECK (op IN ('upsert','delete','rename')),
    path               TEXT NOT NULL,
    rename_from        TEXT,
    version_id         TEXT NOT NULL,
    parent_version_id  TEXT,
    content_hash       TEXT,
    size               INTEGER,
    mode               INTEGER,
    protocol_version   INTEGER NOT NULL,
    state              TEXT NOT NULL CHECK (state IN
                         ('queued','in_flight','acked','applied','duplicate','conflict','rejected','failed')),
    attempts           INTEGER NOT NULL DEFAULT 0,
    next_attempt_at    INTEGER,
    last_error         TEXT,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);
CREATE INDEX change_events_pending ON change_events(state, next_attempt_at);

-- Chunked content transfers, resumable after restart.
CREATE TABLE transfer_jobs (
    id             TEXT PRIMARY KEY,
    change_id      TEXT NOT NULL REFERENCES change_events(change_id) ON DELETE CASCADE,
    peer_device_id TEXT NOT NULL,
    content_hash   TEXT NOT NULL,
    total_size     INTEGER NOT NULL,
    bytes_done     INTEGER NOT NULL DEFAULT 0,
    chunk_size     INTEGER NOT NULL,
    staging_path   TEXT,
    state          TEXT NOT NULL CHECK (state IN ('pending','active','paused','verifying','done','failed','cancelled')),
    attempts       INTEGER NOT NULL DEFAULT 0,
    last_error     TEXT,
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE TABLE conflicts (
    id                 TEXT PRIMARY KEY,
    workspace_id       TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path               TEXT NOT NULL,
    kind               TEXT NOT NULL CHECK (kind IN ('content','delete_modify','case_collision','unportable_path')),
    local_version_id   TEXT,
    remote_version_id  TEXT,
    local_device_id    TEXT,
    remote_device_id   TEXT,
    -- Recovery copy location relative to the app's recovery directory.
    recovery_path      TEXT,
    state              TEXT NOT NULL DEFAULT 'open' CHECK (state IN ('open','resolved')),
    resolution         TEXT CHECK (resolution IN ('keep_local','keep_remote','save_both','merged')),
    detected_at        INTEGER NOT NULL,
    resolved_at        INTEGER,
    -- Recovery copies are kept until the user removes them.
    recovery_removed_at INTEGER
);
CREATE INDEX conflicts_open ON conflicts(workspace_id, state);

CREATE TABLE sync_sessions (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    peer_device_id  TEXT NOT NULL,
    state           TEXT NOT NULL CHECK (state IN ('active','completed','failed','interrupted')),
    files_sent      INTEGER NOT NULL DEFAULT 0,
    files_received  INTEGER NOT NULL DEFAULT 0,
    bytes_sent      INTEGER NOT NULL DEFAULT 0,
    bytes_received  INTEGER NOT NULL DEFAULT 0,
    error           TEXT,
    started_at      INTEGER NOT NULL,
    ended_at        INTEGER
);

CREATE TABLE git_repositories (
    workspace_id     TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    toplevel         TEXT NOT NULL,
    last_branch      TEXT,
    last_head        TEXT,
    last_checked_at  INTEGER
);

CREATE TABLE app_settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,   -- JSON
    updated_at INTEGER NOT NULL
);

-- Append-only record of security- and data-relevant actions.
-- detail is JSON and must never contain file contents or secrets.
CREATE TABLE audit_events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    at            INTEGER NOT NULL,
    kind          TEXT NOT NULL,
    workspace_id  TEXT,
    device_id     TEXT,
    detail        TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX audit_events_at ON audit_events(at);
