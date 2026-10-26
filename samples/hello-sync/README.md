# hello-sync — sample project for testing DevBridge

A tiny project used in the manual end-to-end checklist (docs/TESTING.md).

- `src/` — ordinary source files that should sync.
- `.env` — a fake secret (`API_KEY=example-not-a-real-key`). It must **not** reach
  the other device unless you opt in to `.env` in Settings → Sync rules.
- `.env.example` — template that *does* sync.
- `build/` — generated output (excluded by default).
- `scripts/run.sh` — executable bit should survive a round trip via Windows.
