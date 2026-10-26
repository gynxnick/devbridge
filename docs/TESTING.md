# Testing record

Run everything with `npm run check` (TypeScript, ESLint, Vitest, rustfmt, clippy with
`-D warnings`, and all Rust tests). Performance benchmarks are separate (docs/PERFORMANCE.md).

## Final run — 2026-10-26, macOS 26.6 (Apple Silicon), Rust 1.99.1, Node 24.22.1

| Suite | Tests | What it covers |
|---|---|---|
| `devbridge-core` | 32 | IDs, settings validation, portable path validation (traversal, Windows names, NFC), version ancestry & decisions incl. base keys, protocol wire format & hostile paths, credential redaction |
| `devbridge-store` | 17 | Migrations v1→v4 (incl. rollback & newer-schema refusal), manifest/versions/queue/acks, transfers, conflicts, issues, recovery retention, sharing, offers, revocation, remote tables |
| `devbridge-fs` | 18 | BLAKE3 hashing, exclusion rules (defaults, secrets & opt-ins, user re-includes, nested .gitignore), scanning (symlinks, unportable names, case collisions), atomic apply (expectation checks, corrupted transfers, symlink escapes), recovery store, watcher |
| `devbridge-git` | 10 | Porcelain v2 parsing, init/discover, stage/unstage/commit, confirmation gating for pull/switch, push/fetch with a bare remote, credential redaction in remotes, injection refusal, catch-up after sync (never rewrites files, even with `core.autocrlf=true`; refuses when content differs) |
| `devbridge-net` | 9 | Identity in a secret store, Ed25519 mutual TLS, pinned-fingerprint refusal, plain TCP rejected, framing limits & version check, SPAKE2 agreement/role/fingerprint binding, wrong codes, QR |
| `devbridge-remote` | 5 | SSH key validation (option injection refused), authorized_keys add/revoke only our lines, SSH config block management + include with backup, host-key pinning, folder URIs, real `ssh-keygen` |
| `devbridge-workspace` | 18 | Import validation (system/home/app-data/nested/duplicate), Git init on import, tool detection, timeouts |
| `devbridge-sync` (unit) | 6 | Indexer: versions, tombstones, touch-without-change, newly excluded files forgotten not deleted, issues, digest |
| `devbridge-sync` (two devices) | 11 | See below |
| `devbridge` (app) | 1 | Date formatting for conflict copies |
| Vitest (UI) | 15 | Dashboard, offers/join, sharing, remove confirmation, error toasts, Git confirmation flow, pairing approval + code display, conflict compare + resolve, settings secret opt-in, merge helpers |
| **Total** | **142** | all passing |

### Two-device integration scenarios (real engines, separate folders/databases/identities, loopback TLS)
1. Mac → PC and PC → Mac propagation, edits, rename (no re-transfer), delete (with recovery copy).
2. Concurrent offline edits → conflict on **both** devices, nothing overwritten, compare contents, *keep remote* on one side closes the other side's conflict automatically, local side recoverable.
3. *Save both* and *merged* resolutions propagate.
4. Offline changes on both sides sync after reconnect.
5. `.git/`, `.env`, `*.pem`, `node_modules/` never synced (not even in the manifest).
6. Case-only collision reported as an issue, not applied.
7. Revoked device disconnected, told it was removed, and later changes don't flow either way.
8. Three wrong pairing codes close the window; the old code stops working; nothing is paired.
9. 24 MiB transfer interrupted by quitting the receiving app resumes after restart and verifies; stale staging leftovers are cleaned at startup.
10. Identical independent edits converge without a conflict.
11. Sample project in real Git clones on both devices: files sync, executable bit preserved, secrets/generated files excluded, the PC's Git history untouched; *Catch up Git* then fast-forwards the PC with no file changes.

### Bugs found by these tests and fixed
- Content-end control messages could overtake queued chunks (priority queue) → large transfers always failed. Now terminators travel behind their chunks.
- Partial rename events left the old path behind until the next full reconcile → vanished-sibling check.
- Spurious conflicts when both devices start from identical files and one edits before reconciliation completes → ancestor content keys (`baseHashes`).
- Pairing window closed 200 ms after the third wrong code (race) → closed before replying.
- Catch up Git used stash + merge, which rewrites files; with `core.autocrlf=true` (common on
  Windows) that would turn synced LF files into CRLF → now moves only the branch and index.
- *Pair new device* aborted the real app (found on the first Windows run; every platform was
  affected): Tauri runs sync commands on the main thread, outside the Tokio runtime, and
  opening the window spawns its expiry timer. The engine now enters its own runtime in sync
  methods that spawn, and panics are written to `logs/crash.log` (release builds abort on
  panic, so they used to leave no trace).

## Not yet verified (honest gaps)
- **Windows:** nothing has been run on Windows yet. The CI workflow runs the full suite on
  `windows-latest` once the repository is pushed to GitHub. Windows-specific code paths
  (Credential Manager, `code.cmd`, `wt.exe`, path prefixes, OpenSSH Server) are implemented
  but unexecuted.
- **Two physical machines:** the integration suite uses two engines on one machine over
  loopback. Run the checklist below on a Mac and a Windows PC on the same network.
- **UI end-to-end automation:** Tauri's WebDriver isn't available on macOS; UI logic is covered
  by component tests with IPC mocks, and the real app was launched and inspected manually
  (database migration, keychain identity, listener).
- Remote Workspace end-to-end (actual Remote-SSH session) needs two machines with SSH servers.

## Manual two-machine checklist (Mac ↔ Windows)
1. Install the build on both. Settings → Tools shows VS Code, Git, Claude Code.
2. Mac: Devices → enable network connections → Pair new device. PC: Join a device → pick the Mac → enter the code. Verification codes match → Approve. Both show each other online.
3. Mac: import `samples/hello-sync` (as a Git clone) → Sync tab → Share with PC. PC: Shared with you → Join into an empty folder.
4. Verify on PC: `src/` arrived; `.env` and `build/` did not; Conflicts → Can't sync is empty.
5. Edit `src/greet.ts` on the PC, save → arrives on the Mac within ~1 s. Rename a file on the Mac → renamed on the PC.
6. Disconnect the PC's network; edit `src/main.ts` on both; reconnect → conflict on both; compare; resolve with Merge… on the Mac → PC conflict closes; both files identical.
7. Commit and push on the Mac → PC: Git tab → Fetch → Catch up Git → clean working tree at the Mac's commit.
8. Quit both apps during a large transfer (copy a 500 MB file in); relaunch → transfer resumes and completes.
9. Open in VS Code on each machine; Claude Code works normally in each.
10. Remote: enable SSH server on the Mac; project → Remote access → allow. PC: Devices → Remote access → Request access → approve on Mac → Open remotely → VS Code opens the Mac's folder; Claude Code runs on the Mac. Revoke → key removed from the Mac's `authorized_keys`.
11. Devices → Remove the PC on the Mac → PC shows the Mac as removed; nothing syncs.
