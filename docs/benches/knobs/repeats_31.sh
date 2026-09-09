#!/usr/bin/env bash
# Item 31 repeats gate: the ~15 mover cells x headroom {10,2,0} x 3
# reps. Medians decide which rung deltas are real vs re-roll noise.
set -u
ROOT="${EDDA_ROOT:-$(git rev-parse --show-toplevel)}"
cd "$ROOT" || exit 1
B="$ROOT/.claude/worktrees/target-a04/release/examples/bench.exe"
OUT="$ROOT/docs/benches/knobs/repeats_31.jsonl"
: > "$OUT"
CELLS=(
  "Beagle Point|Wongi|mandalay|60|--min-fuel"
  "Beagle Point|Wongi|mandalay|60|"
  "Wongi|Beagle Point|mandalay|60|--min-fuel"
  "Wongi|Beagle Point|mandalay|60|"
  "Wongi|Beagle Point|explorer|60|"
  "Beagle Point|Wongi|explorer|60|--min-fuel"
  "Wongi|Plielou RN-R d5-27|mandalay|30|"
  "Pheia Auscs AA-A d0|Spase AA-A a108-0|mandalay|30|"
  "Wongi|Spase AA-A a108-0|mandalay|30|--min-fuel"
  "Spase AA-A a108-0|Colonia|explorer|60|--min-fuel"
  "Spase AA-A a108-0|Colonia|explorer86|60|--min-fuel"
  "Sagittarius A*|Sol|explorer|30|"
  "Wongi|Colonia|explorer86|30|--min-fuel"
  "Colonia|Spase AA-A a108-0|mandalay|60|"
  "Colonia|Spase AA-A a108-0|mandalay|60|--min-fuel"
)
for H in 10 2 0; do
  echo "== headroom $H: $(date) =="
  for c in "${CELLS[@]}"; do
    IFS='|' read -r A T SHIP BUD MF <<< "$c"
    for rep in 1 2 3; do
      ED_FUEL_HEADROOM=$H "$B" .data/galaxy "$A" "$T" --ship "$SHIP" --thorough --budget "$BUD" $MF --json 2>/dev/null | grep '^{' | sed "s/^{/{\"headroom\":$H,\"rep\":$rep,/" >> "$OUT"
    done
  done
done
echo "repeats done: $(date)"
