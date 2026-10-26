# DevBridge

Continue the same software project on your Mac and your Windows PC — with VS Code and
Claude Code — without copying files by hand.

- **Synced projects:** each computer keeps a copy; changes flow between paired devices over
  an encrypted, mutually authenticated LAN connection. Conflicting edits are never
  overwritten: both versions are kept and you decide.
- **Remote workspace:** one computer keeps the project; the other opens it through VS Code
  Remote-SSH, set up for you over the paired connection.
- Git stays Git: history moves through your remote; DevBridge helps your second checkout
  catch up after a sync without touching files.

Start with the [User Guide](docs/USER_GUIDE.md).

## Status
All six phases are implemented and tested on macOS (142 automated tests, measured
performance). **Not yet verified on Windows or across two physical machines** — see
[docs/TESTING.md](docs/TESTING.md) for the honest gap list and the manual checklist.

## Docs
[Architecture](docs/ARCHITECTURE.md) ·
[Protocol](docs/PROTOCOL.md) ·
[Security](docs/SECURITY.md) ·
[User guide](docs/USER_GUIDE.md) ·
[Troubleshooting](docs/TROUBLESHOOTING.md) ·
[Testing](docs/TESTING.md) ·
[Performance](docs/PERFORMANCE.md) ·
[Release](docs/RELEASE.md) ·
[Plan](docs/PLAN.md)

## Develop
Requirements: Node.js ≥ 22, Rust stable (≥ 1.88), Git. macOS: Xcode Command Line Tools.
Windows: Microsoft C++ Build Tools and WebView2 (preinstalled on Windows 11).

```sh
npm install
npm run tauri dev      # run the app with hot reload
npm run check          # typecheck, lint, UI tests, rustfmt, clippy -D warnings, all Rust tests
npm run tauri build    # installers (unsigned unless signing is configured, see docs/RELEASE.md)
```

Useful environment variables:
- `DEVBRIDGE_LOG=debug` — more detailed logs (stderr and `<data dir>/logs`).
- `DEVBRIDGE_DATA_DIR=/some/dir`, `DEVBRIDGE_PORT=47831` — run a second, independent instance
  on the same computer (`scripts/second-instance.sh`) to try pairing and sync locally.
- `DEVBRIDGE_KEYCHAIN=1` — make a *development* build store its device key in the system
  keychain. By default dev builds keep it in memory only, because every rebuild is re-signed
  and macOS would ask for your login keychain password each time. Release builds always use
  the keychain.

## Layout
```
crates/core       domain types, portable paths, version ancestry, protocol, redaction
crates/store      SQLite schema, migrations, repositories
crates/fs         hashing, exclusion rules, scanning, atomic apply, recovery store, watcher
crates/git        safe Git CLI adapter
crates/net        identity, TLS, framing, sessions, pairing crypto, discovery
crates/sync       indexer, apply/conflicts, sync engine
crates/remote     Remote Workspace (SSH/VS Code Remote-SSH) setup
crates/workspace  project import/validation, tool detection, launching
src-tauri         Tauri shell: commands, events, logging, notifications
src               React + TypeScript UI
samples/hello-sync  sample project for manual end-to-end tests
```
