#!/usr/bin/env bash
# The feasibility search alone over the route list, both directions: one
# JSON row per route (hops, boosted departures, injections, total ly,
# expansions, ms). Warm the index first (edda_matrix.sh does the same).
#   witness_matrix.sh <witness.exe> <routes.txt> <out.jsonl> [max_ms]
set -uo pipefail
W="$1"; ROUTES="$2"; OUT="$3"; MAX="${4:-120000}"
IDX="${EDDA_INDEX:-/d/galos/index-boost-secondary}"
cat "$IDX/stars.bin" "$IDX/cells.bin" "$IDX/byname.bin" "$IDX"/boost250/*.bin > /dev/null 2>&1
: > "$OUT"
while IFS='|' read -r A B; do
  [ -z "$A" ] && continue
  for pair in "$A|$B" "$B|$A"; do
    IFS='|' read -r F T <<< "$pair"
    "$W" "$IDX" "$F" "$T" --inject premium 22 --max-ms "$MAX" 2>/dev/null | grep '^{' | tail -1 >> "$OUT"
    tail -1 "$OUT" | cut -c1-160
  done
done < "$ROUTES"
