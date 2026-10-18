#!/usr/bin/env bash
# Goal-end plain-run allowance probe (2026-10-17): the start end of a coarse
# plot may spend MAX_END_BRIDGE_JUMPS (60) ordinary jumps on its first edge,
# the goal end MAX_GOAL_BRIDGE_JUMPS (12) on its last. Toward an island or
# rim-star goal the coarse search therefore stalls 5-6 kly short (both
# reverse Jongou routes, three threads) and hands the exact planner the
# rest. ED_GOAL_BRIDGE raises the goal allowance; the probe plots the two
# lost routes and the Beagle Point pair (the route whose approach made an
# unbounded final edge exhaustive once) on the box's three threads.
#   goal_bridge_probe.sh <bench.exe> <out_dir> <goal_bridge>
set -uo pipefail
B="$1"; O="$2"; GB="$3"; mkdir -p "$O"
IDX="${EDDA_INDEX:-/d/galos/index-boost-secondary}"
run() { # tag from to
  local tag="$1-gb$GB" t0; t0=$(date +%s)
  RAYON_NUM_THREADS=3 ED_PLOT_DEBUG=1 ED_GOAL_BRIDGE="$GB" "$B" "$IDX" "$2" "$3" --ship explorer --inject premium 22 --budget 100 --min-fuel --json --route-out "$O/$tag.route.jsonl" > "$O/$tag.json" 2> "$O/$tag.err"
  echo "== $tag: $(( $(date +%s)-t0 )) s"
  grep -E "^    variant outcome: wave x1 #(0|1) " "$O/$tag.err" | sed 's/^ *//' | cut -c1-105
  grep -m2 -E "found [0-9]+ jumps" "$O/$tag.err"
  grep '^{' "$O/$tag.json" | python -c "import json,sys; [print('  ROUTE', {k:v.get(k) for k in ['jumps','refuel_stops','total_ly','elapsed_ms','variants_finished']}) for v in map(json.loads, sys.stdin)]" 2>/dev/null
}
run jrev1 "Byoi Fraae AT-U d2-1" "Jongou AA-A d0"
run jrev2 "Byoi Fraae CQ-G d10-0" "Jongou XM-W d1-0"
run beagle1 "Sol" "Beagle Point"
run beagle2 "Wongi" "Beagle Point"
run colsol "Colonia" "Sol"
echo "===== goal bridge $GB done"
