# Implementation Plan & Checklist

Status as of 2026-10-26. ✅ = implemented and covered by automated tests on macOS.
⚠️ = implemented, not yet verified where noted.

## Phase 1 — Foundation ✅
- ✅ Cargo workspace + Tauri 2 + React/TS/Vite/Tailwind; lint, format, tests, CI (macOS + Windows matrix ⚠️ not yet run on CI).
- ✅ SQLite schema with all spec models; forward-only migrations (now v4) with rollback tests.
- ✅ Project import/create with path validation; remove without touching files.
- ✅ Tool detection (VS Code, Git, Claude Code CLI + extension); Open in VS Code / Terminal.
- ✅ Dashboard, project detail, devices, conflict center, settings; light/dark/system theme.

## Phase 2 — Local project management ✅
- ✅ Git adapter: status (porcelain v2), log, branches, remotes, fetch, ff-only pull, commit, push (never force), switch/create, remote add/edit/remove, init; confirmation gating; redaction; **Catch up Git** for synced checkouts.
- ✅ Path validation & portability, exclusion rules (defaults, secrets with opt-in, user rules, nested .gitignore), streaming hashing, scanning, case-collision detection.
- ✅ Watcher (debounced) + vanished-sibling check + startup/periodic/reconnect reconcile.
- ✅ File logging with redaction; diagnostics view and export.

## Phase 3 — Pairing & sync ✅
- ✅ Device identity in Keychain/Credential Manager (⚠️ Windows unverified); SPAKE2 pairing with approval and verification code; revocation.
- ✅ TLS 1.3 mutual auth with pinned certificates; framed protocol v1; mDNS discovery; dialer with backoff.
- ✅ Durable change queue, acks, idempotent redelivery, manifest digests, chunked resumable transfers, multi-device forwarding.
- ✅ Version ancestry + base keys, convergence, conflicts (content / delete-modify), recovery store, all four resolutions, resolved-elsewhere propagation, issues list.
- ✅ Two-engine loopback integration suite (11 scenarios).

## Phase 4 — Release-quality reliability ✅ (signing ⚠️)
- ✅ Restart recovery (transfers resume; interrupted sessions; stale staging cleanup), offline queueing, disk-space checks, max file size, large-file streaming, symlink hardening, atomic replace.
- ✅ macOS `.app` + `.dmg` built locally; Windows `.msi`/NSIS configured (⚠️ not built yet); release workflow with signing hooks. ⚠️ Signing needs your Apple Developer ID and a Windows code-signing certificate (docs/RELEASE.md).

## Phase 5 — Remote Workspace ✅ (two-machine run ⚠️)
- ✅ Host chooses projects; SSH server check + guidance; key request/approval over the paired channel; tagged authorized_keys entries; host-key pinning; DevBridge-owned SSH config + consented Include; open via VS Code Remote-SSH; connection check; revoke (also on unpair).
- ⚠️ Needs a real two-machine run (checklist in TESTING.md).

## Phase 6 — Polish ✅
- ✅ Onboarding checklist, notifications (conflicts, pairing, offers, errors), launch at login, start minimized, accessibility basics (labels, roles, focus via native dialogs, live regions, contrast-checked tokens).
- ✅ Performance measured (docs/PERFORMANCE.md).
- ✅ Docs: architecture, protocol, security, user guide, troubleshooting, release, testing, performance.

## MVP acceptance (spec §13)
| # | Criterion | Status |
|---|---|---|
| 1 | Launches on macOS and Windows | macOS ✅ (dev + release build); Windows ⚠️ not run yet |
| 2 | Secure pairing | ✅ tests 8 + real TLS |
| 3 | Import and open in VS Code on both | ✅ on macOS; ⚠️ Windows |
| 4 | Changes propagate | ✅ integration 1, 11 |
| 5 | Conflicts detected without loss | ✅ integration 2, 3, 10 |
| 6 | Offline changes reconciled | ✅ integration 4, 9 |
| 7 | Git via Git, not copied | ✅ integration 5, 11 |
| 8 | Claude Code usable normally | ✅ by design (no interference; detection only); ⚠️ manual check on both machines |
| 9 | Credentials/excluded files not transmitted | ✅ integration 5, 11; rules tests |
| 10 | Tests cover sync, recovery, security | ✅ 142 automated tests |
| 11 | Installers + setup docs | macOS ✅; Windows ⚠️ build not run; docs ✅ |
