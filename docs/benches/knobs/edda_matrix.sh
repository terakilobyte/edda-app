#!/usr/bin/env bash
# EDDA planner over the route list, both directions: one JSON row per
# route (jumps, stops, ly, first-found, wall) and the full route per file.
#   edda_matrix.sh <bench.exe> <tag> <routes.txt> <out_dir> [budget_s] [extra args]
set -uo pipefail
B="$1"; TAG="$2"; ROUTES="$3"; OUT="$4"; BUDGET="${5:-100}"; shift 5 || shift $#
IDX="${EDDA_INDEX:-/d/galos/index-prod-af6bc099}"
# EDDA_APPEND=1 keeps an existing row file (interleave_arms.sh feeds one
# pair at a time, alternating arms).
mkdir -p "$OUT"; [ -n "${EDDA_APPEND:-}" ] || : > "$OUT/$TAG.jsonl"
# Warm the index in the page cache first (2026-10-17: the same baseline
# binary plotted Sol -> Beagle Point in 18 s warm and 77 s after a build
# had evicted the index; wall times across arms are only comparable warm).
cat "$IDX/stars.bin" "$IDX/cells.bin" "$IDX/byname.bin" "$IDX"/boost250/*.bin > /dev/null 2>&1
while IFS='|' read -r A Bq; do
  [ -z "$A" ] && continue
  for pair in "$A|$Bq" "$Bq|$A"; do
    IFS='|' read -r F T <<< "$pair"
    slug=$(printf '%s__%s' "$F" "$T" | tr -c 'A-Za-z0-9_.-' '_')
    t0=$(date +%s.%N)
    line=$("$B" "$IDX" "$F" "$T" --ship explorer --budget "$BUDGET" --min-fuel --inject premium 22 --json --route-out "$OUT/$TAG-$slug.route.jsonl" "$@" 2>"$OUT/$TAG-$slug.err" | grep '^{' | head -1)
    t1=$(date +%s.%N)
    first=$(grep -m1 -oE 'found [0-9]+ jumps at [0-9]+ ms' "$OUT/$TAG-$slug.err" | grep -oE '[0-9]+ ms' | grep -oE '[0-9]+')
    wall=$(python -c "print(round($t1-$t0,1))")
    if [ -n "$line" ]; then
      echo "$line" | python -c "
import json,sys
v=json.load(sys.stdin)
print(json.dumps({'from':'$F','to':'$T','tag':'$TAG','wall_s':$wall,'first_found_ms':${first:-None},'jumps':v.get('jumps'),'stops':v.get('refuel_stops'),'boosted':v.get('boosted_jumps'),'injections':v.get('injections'),'total_ly':round(v.get('total_ly') or 0),'straight_ly':round(v.get('straight_ly') or 0),'elapsed_ms':v.get('elapsed_ms'),'variants_finished':v.get('variants_finished')}))" >> "$OUT/$TAG.jsonl"
    else
      echo "{\"from\":\"$F\",\"to\":\"$T\",\"tag\":\"$TAG\",\"wall_s\":$wall,\"error\":\"$(tail -1 "$OUT/$TAG-$slug.err" | tr -d '"' | cut -c1-80)\"}" >> "$OUT/$TAG.jsonl"
    fi
    tail -1 "$OUT/$TAG.jsonl" | cut -c1-200
  done
done < "$ROUTES"
