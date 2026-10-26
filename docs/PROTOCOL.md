# DevBridge Sync Protocol — v1

`crates/core/src/protocol.rs` is the normative definition; this document explains it.
Framing lives in `crates/net/src/frame.rs`.

## Transport

- TCP, then **TLS 1.3 only** (rustls, ring provider). Both sides present a self-signed
  Ed25519 certificate (generated once per device, key in the OS credential store).
- **Server** requires a client certificate and verifies the handshake signature. Whether
  the presented certificate is *trusted* is decided by the application: its SHA-256
  fingerprint must belong to a paired, non-revoked device; otherwise the connection may
  only run pairing (if a pairing window is open) and is then closed.
- **Client** pins the server's fingerprint when dialing a paired device (handshake fails
  on mismatch). While pairing it accepts any certificate; the PAKE binds fingerprints.
- Default port 47821. The listener binds `127.0.0.1` unless the user enables LAN
  connections, then `0.0.0.0`. At most 16 concurrent handshakes, 10 s timeout.
- One session per device pair: if both dial, the session dialed by the smaller device id
  is kept (both sides compute the same answer).

## Framing

```
u32 big-endian length (kind byte + payload, ≤ 1 MiB + 1 KiB)
u8  kind: 0 = JSON envelope, 1 = content chunk
payload:
  kind 0: {"v": 1, "msg": {"type": "<snake_case>", "body": {...}}}
  kind 1: 26-byte ASCII transfer id (ULID) | u64 BE offset | bytes
```
Unknown `v` or malformed frames close the connection. Every `path` field is parsed as a
validated `RelPath` during deserialization (traversal, absolute and Windows-invalid paths
are rejected before any handler runs).

Control messages are written with priority over chunks; `ContentEnd`/`ContentError` are
queued *behind* the chunks they terminate.

## Session

1. Both send `Hello { device {deviceId, name, os, appVersion}, protocol_versions: [1], listen_port }`.
   The claimed `deviceId` must match the pinned device for the certificate.
2. Unacknowledged change notifications are re-queued; for every workspace synced with
   the peer, queued `Changes` are sent and a `ManifestDigest` exchanged.
3. Keepalive: `Ping`/`Pong` every 15 s; idle 45 s closes the session.

## Messages

| Type | Purpose |
|---|---|
| `hello`, `ping`, `pong`, `error {code, message}`, `bye` | Session |
| `workspace_offer {ws, name, color, icon}` | Offer a project to a paired device |
| `workspace_joined {ws}` / `workspace_left {ws}` | Start / stop syncing |
| `manifest_digest {ws, digest, count}` | BLAKE3 over sorted `path\0version` lines |
| `manifest_request {ws}` | Ask for every manifest entry (answered as `changes` with `reconcile: true`) |
| `changes {ws, changes[≤500], reconcile}` | `Change {changeId, originDevice, path, op: upsert\|delete, versionId, ancestry[≤64], baseHashes[≤16], hash?, size, mode?}` — `baseHashes` are 16-hex-char content keys of ancestors (`-` = deletion) |
| `acks {ws, acks[]}` | `Ack {changeId, result: applied\|duplicate\|pending\|conflict\|rejected, reason?}` |
| `content_request {transferId, ws, hash, offset}` | Stream content with that hash from `offset` |
| `content_end {transferId, size}` / `content_error {transferId, code, message}` / `content_cancel {transferId}` | Transfer control (`unavailable` = sender has a newer version) |
| `pair_request {spake, device}` → `pair_response {spake}` → `pair_confirm {mac}` → `pair_accepted {mac, device}` \| `pair_rejected {reason}` | Pairing (only messages allowed before trust) |
| `remote_access_request {sshPublicKey}` → `remote_access_granted {sshUser, sshPort, hostOs, workspaces[], hostKeys[]}` \| `remote_access_denied {reason}`; `remote_access_revoked {}` | Remote Workspace setup |

## Authorization rules

- Workspace messages are processed only if this device shared the workspace with the
  peer **and** the peer joined (`workspace_devices.shared = 1 AND joined = 1`).
- Content is served only if the hash is the current content of a path in that workspace
  or a recovery copy belonging to it, so a peer can't fetch arbitrary files.
- The receiver enforces its own exclusion rules and rejects case-only collisions.

## Pairing

1. Host opens a window: 8-character Crockford-base32 code (40 bits), 5 minutes,
   single use, 3 wrong attempts close it.
2. SPAKE2 (symmetric, identity `devbridge-pair-v1`) with the normalized code.
3. Joiner sends `HMAC(K, "client"|server_fp|client_fp)`; host verifies using the
   fingerprints *it* observed. A man in the middle can't make these match.
4. Host shows the request with a 6-digit verification code derived from `K`; the joiner
   shows the same code. Nothing is stored until the host user approves.
5. Host sends `HMAC(K, "server"|…)` and its device info; both pin each other's fingerprint.

## Reliability

- Redelivery is idempotent (known version → `duplicate`; transfer already pending → `pending`).
- Transfers resume from the staging file length; a full-hash mismatch retries (max 3).
- Offline periods: changes queue durably; on reconnect queued notifications are sent and
  manifests reconciled, so nothing depends on a single message arriving.
