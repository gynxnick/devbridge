# DevBridge — Security & Threat Model

## Assets
Source code and uncommitted work; secrets that may sit in a workspace; the device's TLS
private key; SSH keys created for Remote Workspace; and the integrity of files outside
registered workspaces.

## Trust boundaries
1. **UI ↔ Rust.** The WebView is less trusted: commands accept workspace/device ids and
   validated relative paths, never arbitrary absolute paths for writes (except a folder the
   user picks to import, which is validated). Tauri capabilities allow only the dialog,
   notification, `https://` link and "reveal logs folder" APIs. CSP restricts scripts to the app.
2. **Device ↔ device.** Only devices paired through PAKE + explicit approval, identified by
   pinned certificate fingerprints, on TLS 1.3 with mutual authentication.
3. **Device ↔ disk.** Writes only inside registered workspace roots (symlink-checked) and
   the app data folder.

## Threats and mitigations

| Threat | Mitigation |
|---|---|
| Rogue device on the LAN | Listener is loopback-only until the user enables LAN. Unpaired certificates can only attempt pairing, and only while a window is open. Handshake limits (16 concurrent, 10 s). |
| Pairing code guessing | 40-bit code, 5 min, single use, 3 attempts then closed; PAKE prevents offline guessing. |
| MITM during pairing | SPAKE2-keyed MACs bind both TLS fingerprints as each side saw them; matching 6-digit codes; explicit approval. |
| Stolen/forged peer identity | Client pins server fingerprint; server maps fingerprint → device and checks the id in `Hello`. |
| Malicious paired peer writes outside the project | `RelPath` validation at deserialization; receiver-side exclusion rules; parent directories created with `symlink_metadata` checks; canonical parent must be inside root; target symlinks refused. |
| Peer fetches arbitrary files | Content served only by hash for the current manifest or that workspace's recovery copies; workspace must be shared both ways. |
| Peer overwrites my unsynced edits | Expectation re-check before every replace/delete; mismatches become conflicts; overwritten/deleted content is copied to the recovery store first. |
| Oversized frames / floods | 1 MiB+1 KiB frame cap, batch cap 500, bounded queues, per-peer transfer concurrency, max file size, disk-space reserve. |
| Secrets synced by accident | Default secret exclusions (incl. `.claude/settings.local.json`); opt-in per pattern, audited. Receiver enforces too. |
| Key theft from the database | No secrets in SQLite. Device key + cert live in Keychain / Credential Manager. If the store is unavailable, an in-memory identity is used for the session (shown in Diagnostics) rather than writing a key to disk. |
| Logs/diagnostics leak secrets | Logs contain paths, sizes, hashes and errors; every line written to disk is redacted (URL credentials, GitHub/GitLab/Anthropic/AWS/Slack tokens, bearer tokens, `password=`…). Git stderr and remote URLs are redacted. Diagnostic reports exclude file contents and custom tool paths and are redacted again. |
| Revoked device keeps access | Revocation clears the pin, closes the session, drops shares/queues/transfers, tells the peer, and removes any Remote Workspace SSH key DevBridge added. |
| Git command injection | No shell; refnames/remote names validated (no leading `-`); `ext::` URLs refused; `--` separators. |
| Accidental data loss | No reset/force push/clean anywhere. Removing a project never deletes files. Recovery copies require confirmation to delete. Catch-up Git verifies byte equality before moving HEAD. |
| Claude Code credentials | Never read, copied or synced; DevBridge only detects installations. |

## Remote Workspace specifics
- Granting access is equivalent to allowing an SSH login **as the host user**; the
  approval prompt says so. Keys are restricted to one validated `ssh-ed25519` line (no
  options can be injected) tagged with the device id for precise revocation.
- Host keys are pinned over the authenticated DevBridge channel (`StrictHostKeyChecking yes`
  with a dedicated known-hosts file), avoiding trust-on-first-use.
- DevBridge never enables SSH servers or changes firewall settings.
- On Windows, administrator accounts use `C:\ProgramData\ssh\administrators_authorized_keys`;
  DevBridge can't write there without elevation and tells the user what to do.

## Residual risks / out of scope
- A paired device is trusted for the projects you share with it; DevBridge can't protect
  against a compromised paired computer beyond path, rule and conflict safeguards.
- mDNS reveals that a DevBridge device (id, port) is on the LAN; the device name only while pairing.
- Files at rest are not encrypted by DevBridge (they're your project files); the database
  contains paths, hashes and device names.
- No relay or NAT traversal: devices must reach each other directly (same LAN or VPN).
- Development builds are not code-signed; macOS may ask to allow Keychain access after rebuilds.
