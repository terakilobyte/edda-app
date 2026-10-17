#!/usr/bin/env bash
# Coarse-weight sweep on the pinned long routes plus the rim-to-rim pair:
# one run per (route, weight), JSON rows with jumps, stops, ly, first-found
# and elapsed. "default" is the shipped COARSE_WEIGHT (1.5).
#
#   coarse_w_sweep.sh <budget_s> <out.jsonl> <w1> [w2 ...]
set -uo pipefail
BUDGET="$1"; OUT="$2"; shift 2
B="${EDDA_BENCH:-/c/Users/terak/ED-Claude/edda-target-limits/release/examples/bench.exe}"
IDX="${EDDA_INDEX:-/d/galos/index-boost-secondary}"
: > "$OUT"
ROUTES=(
  "Sol|Colonia" "Sol|Beagle Point" "Wongi|Colonia" "Wongi|Beagle Point"
  "Sol|Sagittarius A*" "Colonia|Spase AA-A a108-0" "Wongi|Spase AA-A a108-0"
  "Jongou XM-W d1-0|Byoi Fraae CQ-G d10-0" "Jongou AA-A d0|Byoi Fraae AT-U d2-1"
  "Ood Fleau ZJ-I d9-0|Sphiesi HX-L d7-0" "Sphiesi HX-L d7-0|Ood Fleau ZJ-I d9-0"
)
for r in "${ROUTES[@]}"; do
  IFS='|' read -r F T <<< "$r"
  for arm in "$@"; do
    t0=$(date +%s.%N)
    if [ "$arm" = default ]; then
      line=$("$B" "$IDX" "$F" "$T" --ship explorer --budget "$BUDGET" --min-fuel --inject premium 22 --json 2>/tmp/sweep.err | grep '^{' | head -1)
    else
      line=$(ED_COARSE_W="$arm" "$B" "$IDX" "$F" "$T" --ship explorer --budget "$BUDGET" --min-fuel --inject premium 22 --json 2>/tmp/sweep.err | grep '^{' | head -1)
    fi
    t1=$(date +%s.%N)
    first=$(grep -m1 -oE 'found [0-9]+ jumps at [0-9]+ ms' /tmp/sweep.err | grep -oE '[0-9]+ ms' | grep -oE '[0-9]+')
    wall=$(python -c "print(round($t1-$t0,1))")
    if [ -n "$line" ]; then
      echo "$line" | python -c "
import json,sys
v=json.load(sys.stdin)
print(json.dumps({'from':'$F','to':'$T','arm':'$arm','wall_s':$wall,'first_found_ms':${first:-None},'jumps':v.get('jumps'),'stops':v.get('refuel_stops'),'injections':v.get('injections'),'total_ly':round(v.get('total_ly') or 0),'elapsed_ms':v.get('elapsed_ms'),'variants_finished':v.get('variants_finished')}))" >> "$OUT"
    else
      echo "{\"from\":\"$F\",\"to\":\"$T\",\"arm\":\"$arm\",\"wall_s\":$wall,\"error\":\"$(tail -1 /tmp/sweep.err | tr -d '"' | cut -c1-80)\"}" >> "$OUT"
    fi
    tail -1 "$OUT" | cut -c1-220
  done
done
