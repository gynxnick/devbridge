# DevBridge — Architecture

DevBridge (working name) is a desktop app for macOS and Windows that lets one person
continue work on the same software project from several computers, using VS Code and
Claude Code exactly as they normally would.

| Mode | What it is | When to use |
|---|---|---|
| **Synced Project** | Each device keeps a full local copy. DevBridge exchanges file changes peer-to-peer over an encrypted, mutually authenticated connection, with version ancestry and explicit conflict handling. | Offline access; one computer at a time. |
| **Remote Workspace** | One computer (host) keeps the only copy and runs VS Code Server, Claude Code and builds. The other connects through **VS Code Remote-SSH**; DevBridge automates key exchange and SSH configuration over the paired channel. | One authoritative copy; no chance of two Claude Code sessions editing independent copies. |

DevBridge is **not** a real-time collaborative editor. It does not merge keystrokes and
never moves a running Claude Code conversation, terminal or process between machines.

## 1. Stack (as built)

| Concern | Choice |
|---|---|
| Shell | Tauri 2.12 (Rust 2024 edition) |
| UI | React 19 + TypeScript 6.0 + Vite 8 + Tailwind CSS 4, lucide icons, `diff` for comparisons |
| Database | SQLite (`rusqlite`, bundled) with forward-only migrations on `PRAGMA user_version` (v3) |
| Hashing | BLAKE3, streaming (256 KiB buffer) |
| Watching | `notify` 8 + `notify-debouncer-full` (300 ms) + periodic full reconcile (default 10 min) |
| Ignore rules | `ignore` crate gitignore matchers; nested `.gitignore` support |
| Git | Git CLI (porcelain v2), never a library, so user config/hooks/credential helpers apply |
| Transport | TLS 1.3 only (`rustls` + ring), **mutual** authentication with self-signed Ed25519 certificates, fingerprints pinned at pairing |
| Framing | Length-prefixed frames over the TLS stream (JSON control messages + binary chunks) |
| Pairing | SPAKE2 (Ed25519 group) keyed by a single-use 8-character code; HMAC binds both TLS fingerprints; explicit approval |
| Discovery | mDNS `_devbridge._tcp` (id + port; name only while pairing) plus manual address |
| Secrets | Device key + certificate in macOS Keychain / Windows Credential Manager (`keyring` 4) |
| Remote editor | VS Code Remote-SSH with OpenSSH; DevBridge manages keys, `authorized_keys` lines and a dedicated SSH config file |

**Deviation from the original spec:** the spec suggested "authenticated WebSocket or
equivalent". DevBridge uses its own length-prefixed framing directly on the TLS stream.
There is no HTTP layer to secure, it needs fewer dependencies, and it lets the session
writer prioritise control messages over bulk data while keeping per-transfer ordering.
The authentication and encryption properties are those of TLS 1.3 with mutual auth.

## 2. Modules

```
crates/core       pure domain: ids, model/settings, RelPath (portable path validation),
                  version ancestry + decision logic, protocol messages, default rules,
                  credential redaction
crates/store      SQLite persistence: migrations, manifest, versions, change queue,
                  transfers, conflicts, issues, recovery items, sharing, peers, remote
crates/fs         filesystem engine: hashing, exclusion rules, scanning, atomic staged
                  apply with symlink checks, recovery blob store, watcher
crates/git        Git CLI adapter with safety checks and "catch up" flow
crates/net        identity (keychain), TLS configs + verifiers, framing, sessions,
                  pairing crypto, mDNS discovery, QR
crates/sync       indexer, apply/conflict logic, engine (listener, sessions, workspace
                  actors, transfers, dialer, pairing orchestration, housekeeping)
crates/remote     Remote Workspace setup: SSH keys, authorized_keys, ssh config,
                  host-key pinning, VS Code folder URIs
crates/workspace  project import/validation, tool detection, launching VS Code/terminal
src-tauri         thin Tauri commands, event forwarding, logging, notifications
src               React UI
```

Spec module mapping: Desktop UI → `src`; Workspace Manager → `workspace` + `store`;
Filesystem Engine → `fs` + `sync::indexer`; Sync Engine → `sync`; Git Adapter → `git`;
Device Identity + Transport → `net`; Remote Agent → `remote` + remote messages in `sync`;
Persistence → `store`; Diagnostics → `src-tauri/logging.rs` + `diagnostics` commands.

Every crate except `src-tauri` builds and tests headless.

## 3. Synchronization model

**Unit:** a regular file at a `RelPath`: `/`-separated, NFC, no `.`/`..`/empty
segments, and portable to Windows (reserved names, `<>:"|?*`, trailing dot/space and
over-long names are rejected). Paths that can't be represented are *sync issues*
shown in the Conflict Center, never silent skips. Empty directories aren't synced.

**Never synced:** `.git/` and `.devbridge/` (always), default exclusions
(`node_modules/`, `target/`, …), secret patterns (`.env*`, keys, credentials, …)
unless opted in per pattern, user rules, and optionally `.gitignore`d paths. The
**receiver applies its own rules too**, so a peer can't write a file this device
excludes. Symlinks are reported, not followed or synced.

**Versions:** every change creates `FileVersion { id (ULID), content hash | tombstone,
size, mode, origin, ancestry[≤64], base keys[≤16] }`, where base keys are 64-bit content
keys of ancestors. A local edit's ancestry is its parent followed by
the parent's ancestry. Merge results (conflict resolutions) list both parents.

**Decision for an incoming version** (`core::version::classify`):

| Condition | Result |
|---|---|
| Version already known locally | Duplicate (idempotent redelivery) |
| No local version, or local version ∈ incoming ancestry | Fast-forward: apply |
| Same content (or both deleted) on different histories | Converge: both devices adopt the larger version id as head |
| Local content equals the content of one of the incoming version's ancestors | Fast-forward: the change was made on top of content we already have, so nothing is lost (e.g. both devices started from identical clones) |
| Otherwise | Conflict: local file untouched; remote content kept in recovery store |

Timestamps never decide anything.

**Detecting local changes:** watcher paths (plus vanished siblings in the same folders,
because rename events can be partial) are re-indexed; `(size, mtime)` is a fast filter,
the hash is the truth. A touched-but-identical file creates no version. That's also why
there are no sync loops: a file DevBridge just applied hashes equal to its manifest entry.
Full reconcile runs at startup, on reconnect and periodically.

**Applying** (`sync::apply`): rescan the path first (catches unscanned edits) → decide →
if content is needed, look for the same hash locally (renames don't re-transfer) or
request it from the peer → stage in the app's staging folder on the same volume
(else `.devbridge/staging` in the workspace) → verify BLAKE3 → copy the current local
file to the recovery store → re-check the target still matches what the decision was
based on → atomic rename. If the local file changed meanwhile, it's indexed and the
change is re-evaluated (usually becoming a conflict).

**Per-workspace actors** serialize all indexing, applying, transfer completion and
conflict resolution for a workspace, so they never race.

**Delivery:** each local version is queued per joined peer (`change_events`), sent in
batches, and acknowledged (`Applied`, `Duplicate`, `Conflict`, `Rejected`, or `Pending`
while content transfers). Unacked notifications are re-sent after reconnect. On connect
both sides compare manifest digests and exchange full manifests only if they differ.
Versions received from one peer are forwarded to other joined peers (multi-device).

**Transfers:** content is requested by hash and streamed in 1 MiB chunks; staging files
survive restarts and resume from their length; the full hash is verified before use;
three mismatches fail the transfer. Concurrency per peer and max file size are settings;
free disk space (+256 MiB reserve) is checked first.

**Conflicts:** kinds `content`, `local_deleted`, `remote_deleted`. Resolutions: keep
local, keep remote, save both (remote copy written next to the file with a
"(conflict from …)" suffix), or merged text. A resolution is a new version whose
ancestry contains both sides, so the other device fast-forwards and marks its own
conflict "resolved elsewhere". Conflict copies stay in the recovery store until the
user deletes them; overwrite/delete history expires after the retention period.

## 4. Platform risks and how they're handled

| Risk | Handling |
|---|---|
| Case-insensitive filesystems | Case-only collisions detected in scans and on receipt (manifest + exact on-disk name); reported, never applied |
| Windows-invalid names | Validated on every path; reported as issues |
| Unicode NFD on macOS | Paths normalized to NFC |
| Line endings | Bytes synced as-is; Git panel warns about `core.autocrlf`; `.gitattributes` recommended |
| Executable bits | Mode stored; applied on Unix; Windows carries the last known mode forward |
| Git HEAD divergence | Working trees sync, histories don't. After device A commits+pushes, device B uses **Catch up Git**: fast-forward only if every changed file already matches the upstream byte-for-byte, moving only the branch and index (files are never written, so no line-ending rewrites). Never reset/force. |
| Two Claude Code sessions on two copies | "Last edited on" indicator; conflicts are explicit; Remote Workspace mode recommended for simultaneous use |
| Missed watcher events | Sibling check, startup/periodic/reconnect reconcile |
| Large files | Streaming hash and transfer; memory bounded by chunk size × concurrency |
| Crash mid-write | Staging + atomic rename; transfers resume; stale staging cleaned at startup; sessions marked interrupted |

## 5. Remote Workspace

1. Host chooses which projects are reachable (`remote_hosting`), and needs an SSH server
   (macOS Remote Login / Windows OpenSSH Server). DevBridge checks and guides; it never
   enables system services.
2. Client generates `~/.ssh/devbridge_<host>` (ed25519) and sends the public key over the
   paired DevBridge session.
3. Host user approves; DevBridge appends exactly one validated `ssh-ed25519` line tagged
   `devbridge:<device-id>` to `authorized_keys` and replies with user, port, project paths
   and the host's SSH host keys.
4. Client writes a `Host devbridge-…` block to `~/.ssh/devbridge_config` with
   `StrictHostKeyChecking yes` against a DevBridge-owned known-hosts file (pinned over
   the authenticated channel), adds `Include devbridge_config` to `~/.ssh/config` only
   after the user agrees (with a backup), and opens
   `code --folder-uri vscode-remote://ssh-remote+<alias>/<path>`.
5. Revocation (or unpairing) removes the tagged key and the host block.
Disconnecting VS Code never stops host processes.

## 6. Test strategy (see TESTING.md for results)

- Unit tests in every crate (path validation, rules, hashing, manifest/indexer, ancestry,
  conflict decisions, migrations incl. rollback, Git parsing and safety, pairing crypto,
  framing limits, TLS pinning, SSH config management).
- Integration: two real engines over loopback TLS (`crates/sync/tests/two_devices.rs`):
  propagation both ways, rename/delete, concurrent edits → conflicts, all resolution types,
  convergence, offline changes, interrupted 24 MB transfer resumed after restart, Git
  internals and secrets excluded, case collisions, revocation, wrong pairing codes.
- Real Git scenarios with temporary repositories (`crates/git`).
- UI: Vitest + Testing Library with Tauri IPC mocks.
- Performance: `crates/sync/tests/perf.rs` (ignored by default), results in PERFORMANCE.md.
- Cross-platform: CI matrix macOS + Windows (`.github/workflows/ci.yml`); a manual two-machine
  checklist in TESTING.md.
