#!/usr/bin/env bash
# Item 25 follow-up (user hypothesis): measure total time-in-seat saved
# by the min-fuel toggle, post-pruned-truth judging — eager vs lean per
# matrix cell, fitted-seconds scored. Appends bench --json lines (they
# carry from/to/ship/min_fuel) to a jsonl; report via prune_gain_report.py.
#
#   docs/benches/knobs/prune_gain.sh [out.jsonl]
set -u
ROOT="${EDDA_ROOT:-$(git rev-parse --show-toplevel)}"
cd "$ROOT" || exit 1
OUT="${1:-$ROOT/docs/benches/knobs/prune_gain.jsonl}"
BENCH="${BENCH:-$ROOT/target/release/examples/bench.exe}"
: > "$OUT"
run() { # from to ship budget extra...
  local from="$1" to="$2" ship="$3" budget="$4"
  shift 4
  "$BENCH" .data/galaxy "$from" "$to" --ship "$ship" --thorough --budget "$budget" --json ${ROUTE_OUT:+--route-out "$ROUTE_OUT"} "$@" 2>/dev/null | grep '^{' >> "$OUT"
}
ROUTES=(
  "Wongi|Spase AA-A a108-0|30"
  "Sol|Byoomao AA-A c1|30"
  "Pheia Auscs AA-A d0|Spase AA-A a108-0|30"
  "Wongi|Plielou RN-R d5-27|30"
  "Wongi|Colonia|30"
  "Sol|Sagittarius A*|30"
  "Wongi|Blaa Hypai AA-A a96-19|30"
  "Wongi|Beagle Point|60"
  "Colonia|Spase AA-A a108-0|60"
)
for r in "${ROUTES[@]}"; do
  IFS='|' read -r A B BUD <<< "$r"
  for pair in "$A|$B" "$B|$A"; do
    IFS='|' read -r F T <<< "$pair"
    for ship in explorer mandalay explorer86; do
      echo "== $F -> $T [$ship]" >&2
      run "$F" "$T" "$ship" "$BUD"
      run "$F" "$T" "$ship" "$BUD" --min-fuel
    done
  done
done
echo "done: $OUT" >&2
