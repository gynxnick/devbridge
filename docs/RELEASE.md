# Release checklist & packaging

## Build locally (unsigned)
```sh
npm ci
npm run check                     # all tests + lint + clippy
npm run tauri build               # macOS: .app + .dmg ; Windows: .msi + NSIS .exe
```
Outputs: `target/release/bundle/{macos,dmg,msi,nsis}`. Verified on macOS 26.6 (arm64):
`DevBridge.app` 11.6 MiB, `DevBridge_0.1.0_aarch64.dmg` 5.4 MiB (ad-hoc signed).

Universal macOS build: `rustup target add x86_64-apple-darwin` then
`npm run tauri build -- --target universal-apple-darwin`.

## Signing (needs your certificates)

### macOS — Developer ID + notarization
1. Apple Developer Program membership; create a **Developer ID Application** certificate.
2. Export it as `.p12`; base64 it into repository secret `APPLE_CERTIFICATE`, its password
   into `APPLE_CERTIFICATE_PASSWORD`, and the identity name (e.g. `Developer ID Application:
   Your Name (TEAMID)`) into `APPLE_SIGNING_IDENTITY`.
3. Notarization: `APPLE_ID`, an app-specific password in `APPLE_PASSWORD`, and `APPLE_TEAM_ID`.
4. `src-tauri/entitlements.plist` grants network client/server only; hardened runtime is on.

### Windows — Authenticode
1. Obtain an OV/EV code-signing certificate (or Azure Trusted Signing).
2. On the build machine import the `.pfx` into `Cert:\CurrentUser\My`, then build with
   `npm run tauri build -- --config '{"bundle":{"windows":{"certificateThumbprint":"<THUMBPRINT>"}}}'`.
   Timestamping uses `http://timestamp.digicert.com` (SHA-256).
3. The installer bootstraps WebView2 if missing (preinstalled on Windows 11).

## CI release
Push a tag `vX.Y.Z`, or run it manually (Actions → Release → Run workflow, or
`gh workflow run release.yml`). `.github/workflows/release.yml` builds the macOS (universal)
and Windows installers with `tauri-action`, attaches them to a **draft pre-release**
`v<version>` and uploads them as workflow artifacts. Tests run separately in `ci.yml` on every
push. Without signing secrets the installers are unsigned (Gatekeeper/SmartScreen will warn).

## Checklist before publishing
- [ ] Version bumped in `package.json`, `Cargo.toml` (`workspace.package.version`), `src-tauri/tauri.conf.json`.
- [ ] `npm run check` green on macOS **and** Windows (CI).
- [ ] Two-machine manual checklist in docs/TESTING.md passed (Mac ↔ Windows).
- [ ] Migrations: upgrade from the previous release's database tested (the runner refuses to open a newer DB).
- [ ] Installers signed (macOS notarized: `spctl -a -vv DevBridge.app` says "Notarized Developer ID").
- [ ] Release notes mention protocol version changes (peers must run compatible versions).
- [ ] Docs updated (USER_GUIDE, SECURITY, PROTOCOL).
