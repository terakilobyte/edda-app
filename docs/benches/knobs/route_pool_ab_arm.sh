#!/usr/bin/env bash
# One arm of the planner-pool A/B: start an ed-api binary on a scratch
# port against the local full index, wait until it is ready, run
# route_pool_ab.sh against it, stop it. Env vars after the label and
# binary are passed to the server (RAYON_NUM_THREADS=6,
# EDDA_API_PLANNER_THREADS_INTERACTIVE=2, ...).
#
#   route_pool_ab_arm.sh <arm-label> <ed-api-binary> [VAR=value ...]
#
# Assumes the WSL dev database (edda_mirror on 55432) and the artifact
# dir with routing/51 + .highway/51 (elite-copilot/.data/api/artifacts).
set -u
LABEL="${1:?arm label}"; BIN="${2:?ed-api binary}"; shift 2
K="$(cd "$(dirname "$0")" && pwd)"
PORT=8790
export DATABASE_URL="${DATABASE_URL:-postgres://nathan@127.0.0.1:55432/edda_mirror}"
export EDDA_API_ARTIFACT_DIR="${EDDA_API_ARTIFACT_DIR:-/mnt/c/Users/terak/ED-Claude/elite-copilot/.data/api/artifacts}"
export EDDA_API_BIND="127.0.0.1:$PORT" EDDA_API_INGEST_BIND="127.0.0.1:$((PORT+1))" RUST_LOG=ed_api=info
for kv in "$@"; do export "$kv"; done
LOG="/tmp/route_pool_ab.$LABEL.log"
"$BIN" serve >"$LOG" 2>&1 &
PID=$!
for i in $(seq 1 120); do
  if curl -sf -m 2 "http://127.0.0.1:$PORT/readyz" >/dev/null 2>&1; then break; fi
  sleep 1
done
if ! curl -sf -m 2 "http://127.0.0.1:$PORT/readyz" >/dev/null 2>&1; then
  echo "$LABEL: server never became ready; log tail:" >&2; tail -5 "$LOG" >&2; kill "$PID" 2>/dev/null; exit 1
fi
# Let the highway sub-index open (it exists for routing/51) before timing.
sleep 3
bash "$K/route_pool_ab.sh" "$LABEL" "http://127.0.0.1:$PORT"
kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null
grep -c "route served" "$LOG" >/dev/null 2>&1 && echo "# $LABEL: $(grep -c 'route served' "$LOG") routes served; log $LOG" >&2
