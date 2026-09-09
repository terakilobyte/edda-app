#!/usr/bin/env bash
# Item 34 rung 1 matrix: crossing-replan A/B at the shipped h0 default.
# Min-fuel arm (the pass runs post-rewrite), --route-out on, x3 on the
# heavy-crossing movers.
set -u
ROOT="${EDDA_ROOT:-$(git rev-parse --show-toplevel)}"
cd "$ROOT" || exit 1
B="$ROOT/target/release/examples/bench.exe"
K="$ROOT/docs/benches/knobs"
OUT="$K/crossing_matrix.jsonl"
: > "$OUT"
ROUTES=(
  "Wongi|Spase AA-A a108-0|30" "Sol|Byoomao AA-A c1|30" "Pheia Auscs AA-A d0|Spase AA-A a108-0|30"
  "Wongi|Plielou RN-R d5-27|30" "Wongi|Colonia|30" "Sol|Sagittarius A*|30"
  "Wongi|Blaa Hypai AA-A a96-19|30" "Wongi|Beagle Point|60" "Colonia|Spase AA-A a108-0|60"
)
MOVERS="Spase AA-A a108-0->Colonia Wongi->Beagle Point Beagle Point->Wongi Colonia->Spase AA-A a108-0"
for r in "${ROUTES[@]}"; do
  IFS='|' read -r A T BUD <<< "$r"
  for pair in "$A|$T" "$T|$A"; do
    IFS='|' read -r F TO <<< "$pair"
    reps=1
    case "$MOVERS" in *"$F->$TO"*) reps=3;; esac
    for ship in explorer mandalay explorer86; do
      for cr in 0 1; do
        for i in $(seq 1 $reps); do
          "$B" .data/galaxy "$F" "$TO" --ship "$ship" --thorough --budget "$BUD" --min-fuel --crossing-replan $cr --route-out "$K/routes_cr$cr.jsonl" --json 2>/dev/null | grep '^{' | sed "s/^{/{\"cr\":$cr,\"rep\":$i,/" >> "$OUT"
        done
      done
    done
    echo "== $F -> $TO done" >&2
  done
done
echo "crossing matrix complete: $(date)" >&2
