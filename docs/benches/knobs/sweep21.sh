#!/usr/bin/env bash
# Sweep21: the item-42 judge gate. Unfitted defaults become the flat
# public model (60 s/jump + 120 s/stop, tonnage-blind) on worktree
# dac890f; this is the full-target eager sweep in sweep20's format so
# sweep_compare.py can grade the judge change before the merge.
# Env: BENCH (binary), OUT (csv path).
set -u
ROOT="${EDDA_ROOT:-$(git rev-parse --show-toplevel)}"
cd "$ROOT" || exit 1
B="${BENCH:-$ROOT/.claude/worktrees/target-a04/release/examples/bench.exe}"
OUT="${OUT:-$ROOT/docs/benches/2026-09-03-sweep21-item42-judge.csv}"
{
  echo "# 2026-09-03 sweep21: item-42 judge gate — unfitted judge = flat public model (60 s/jump + 120 s/stop, tonnage-blind; worktree dac890f). Grade vs sweep20 h0 era before merging to main."
  echo "route_from,route_to,ship,jumps,boosted,refuels,wall_ms,note"
} > "$OUT"
row() { # from to ship budget [note] [extra bench args...]
  local F="$1" T="$2" S="$3" BUD="$4" NOTE="${5:-}"
  shift 5 2>/dev/null || shift $#
  local line
  line=$("$B" .data/galaxy "$F" "$T" --ship "$S" --thorough --budget "$BUD" "$@" --json 2>/dev/null | grep '^{' | tail -1)
  if [ -n "$line" ]; then
    printf '%s' "$line" | python -c "import json,sys;r=json.load(sys.stdin);print(','.join(map(str,[sys.argv[1],sys.argv[2],sys.argv[3],r['jumps'],r['boosted'],r['refuel_stops'],r['elapsed_ms'],sys.argv[4]])))" "$F" "$T" "$S" "$NOTE" >> "$OUT"
  else
    echo "$F,$T,$S,,,,,${NOTE:-no route}" >> "$OUT"
  fi
  echo "  $F -> $T [$S] done" >&2
}
for ship in explorer mandalay explorer86; do :; done
PAIRS=(
  "Wongi|Spase AA-A a108-0|30" "Spase AA-A a108-0|Wongi|30"
  "Pheia Auscs AA-A d0|Spase AA-A a108-0|30" "Spase AA-A a108-0|Pheia Auscs AA-A d0|30"
  "Wongi|Colonia|30" "Colonia|Wongi|30"
  "Colonia|Spase AA-A a108-0|60" "Spase AA-A a108-0|Colonia|60"
  "Sol|Byoomao AA-A c1|30" "Byoomao AA-A c1|Sol|30"
  "Sol|Sagittarius A*|30" "Sagittarius A*|Sol|30"
  "Wongi|Blaa Hypai AA-A a96-19|30" "Blaa Hypai AA-A a96-19|Wongi|30"
  "Wongi|Plielou RN-R d5-27|30" "Plielou RN-R d5-27|Wongi|30"
  "Wongi|Beagle Point|60" "Beagle Point|Wongi|60"
)
for p in "${PAIRS[@]}"; do
  IFS='|' read -r F T BUD <<< "$p"
  for ship in explorer mandalay explorer86; do
    row "$F" "$T" "$ship" "$BUD" ""
  done
done
# The Oevasy rim pair, sweep-standard special-casing: explorer runs and
# reports its no-route honestly; mandalay is skipped (item-15 budget
# hole, 18 min CPU); explorer86 flies it with one basic injection.
for p in "Beagle Point|Oevasy CA-A d0" "Oevasy CA-A d0|Beagle Point"; do
  IFS='|' read -r F T <<< "$p"
  row "$F" "$T" "explorer" 30 "no route"
  echo "$F,$T,mandalay,,,,,no route (skipped: item-15 budget hole)" >> "$OUT"
  row "$F" "$T" "explorer86" 30 "one basic injection" --inject basic 1
done
echo "sweep21 complete: $(date)" >&2
