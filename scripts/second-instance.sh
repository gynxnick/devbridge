#!/bin/sh
# Runs a second, independent DevBridge on this computer (its own database,
# device identity and port) so you can try pairing and sync without a second
# machine. Start the first instance normally (npm run tauri dev), then run this.
#
# The second instance uses the debug build and the dev server of the first.
set -eu
DATA="${DEVBRIDGE_DATA_DIR:-${TMPDIR:-/tmp}/devbridge-second}"
mkdir -p "$DATA"
# Give it its own port on first run (the first instance uses 47821).
if [ ! -f "$DATA/devbridge.sqlite3" ]; then
  echo "Creating $DATA with port 47831"
fi
cd "$(dirname "$0")/.."
DEVBRIDGE_DATA_DIR="$DATA" DEVBRIDGE_PORT=47831 ./target/debug/devbridge
