#!/usr/bin/env bash
# Rebuild the local ed-api from the main tree and restart it on 8787, so the
# dev app (dev_api_local = true, the boss's 2026-09-29 rule: fly against the
# WSL server) previews the SAME server code as main. 2026-10-07: a local
# server built at 13:13Z from the tree of that moment lacked the day's
# planner fixes and every slow plot was blamed on prod.
#
#   bash scripts/dev-api-restart.sh            # build from the tree you run it in
#   bash scripts/dev-api-restart.sh --no-build # just restart the binary that exists
#
# Run from WSL inside the repo. Env you may override: EDDA_DEV_DB,
# EDDA_DEV_ARTIFACTS, EDDA_DEV_TARGET.
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
TARGET="${EDDA_DEV_TARGET:-$HOME/edda-target}"
BIN="$TARGET/release/ed-api"
DB="${EDDA_DEV_DB:-postgres://nathan@127.0.0.1:55432/edda_mirror}"
ART="${EDDA_DEV_ARTIFACTS:-/mnt/c/Users/terak/ED-Claude/elite-copilot/.data/api/artifacts}"
LOG="$HOME/ed-api-serve.log"
if [ "${1:-}" != "--no-build" ]; then
  echo "building ed-api from $(git -C "$ROOT" rev-parse --short HEAD) ($(git -C "$ROOT" log -1 --format=%s | cut -c1-60))"
  (cd "$ROOT" && CARGO_TARGET_DIR="$TARGET" cargo build --release -p ed-api 2>&1 | tail -1)
fi
# Anchored pattern: an unanchored one matches this very shell and kills it.
pkill -f "^$BIN serve" 2>/dev/null && sleep 1 || true
DATABASE_URL="$DB" EDDA_API_ARTIFACT_DIR="$ART" \
EDDA_API_BIND=127.0.0.1:8787 EDDA_API_INGEST_BIND=127.0.0.1:8788 RUST_LOG="${RUST_LOG:-ed_api=info}" \
nohup "$BIN" serve >"$LOG" 2>&1 &
for _ in $(seq 1 60); do curl -sf -m 2 http://127.0.0.1:8787/readyz >/dev/null 2>&1 && break; sleep 1; done
if curl -sf -m 2 http://127.0.0.1:8787/healthz >/dev/null; then
  echo "local ed-api up on 127.0.0.1:8787 (pid $(pgrep -f "^$BIN serve" | head -1)), built $(date -r "$BIN" +%F_%T), artifacts $ART, log $LOG"
else
  echo "local ed-api did not come up; last log lines:" >&2; tail -5 "$LOG" >&2; exit 1
fi
