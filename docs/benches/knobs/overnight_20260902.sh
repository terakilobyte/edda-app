#!/usr/bin/env bash
# Maintainer-greenlit overnight sequence (2026-09-02): three studies in order.
#   S1  item 25c — min-fuel portfolio (P) matrix, lean arm vs 25b's B arm
#   S2  item 24d — field-density expansion census over archived corridors
#   S3  item 29  — fuel-judge rank agreement vs the seconds judge
# All on the worktree binary (portfolio + ED_JUDGE + census flags).
set -u
ROOT="${EDDA_ROOT:-$(git rev-parse --show-toplevel)}"
cd "$ROOT" || exit 1
K="$ROOT/docs/benches/knobs"
export BENCH="$ROOT/.claude/worktrees/target-a04/release/examples/bench.exe"

echo "== S1 (25c) P-matrix: $(date) =="
bash "$K/prune_gain.sh" "$K/prune_gain_P.jsonl"

echo "== S2 (24d) field census: $(date) =="
"$ROOT/.claude/worktrees/target-a04/release/examples/field_density_probe.exe" .data/galaxy --save "$K/voxels.csv"
: > "$K/census.csv"
census() { # from to ship budget
  "$BENCH" .data/galaxy "$1" "$2" --ship "$3" --thorough --budget "$4" --census "$K/census.csv" >/dev/null 2>&1
}
for pair in "Wongi|Colonia|30" "Colonia|Wongi|30" "Sol|Sagittarius A*|30" "Sagittarius A*|Sol|30" "Colonia|Spase AA-A a108-0|60" "Spase AA-A a108-0|Colonia|60" "Wongi|Beagle Point|60" "Beagle Point|Wongi|60"; do
  IFS='|' read -r F T BUD <<< "$pair"
  for ship in explorer mandalay; do
    echo "  census $F -> $T [$ship]"
    census "$F" "$T" "$ship" "$BUD"
  done
done
python "$K/field_census_report.py" "$K/voxels.csv" "$K/census.csv" > "$K/field_census_report.txt" 2>&1

echo "== S3 (29) fuel-judge matrix: $(date) =="
OUT="$K/judge_fuel.jsonl"
: > "$OUT"
jrun() {
  "$BENCH" .data/galaxy "$1" "$2" --ship "$3" --thorough --budget "$4" --judge fuel --json 2>/dev/null | grep '^{' >> "$OUT"
}
ROUTES=(
  "Wongi|Spase AA-A a108-0|30" "Sol|Byoomao AA-A c1|30" "Pheia Auscs AA-A d0|Spase AA-A a108-0|30"
  "Wongi|Plielou RN-R d5-27|30" "Wongi|Colonia|30" "Sol|Sagittarius A*|30"
  "Wongi|Blaa Hypai AA-A a96-19|30" "Wongi|Beagle Point|60" "Colonia|Spase AA-A a108-0|60"
)
for r in "${ROUTES[@]}"; do
  IFS='|' read -r A B BUD <<< "$r"
  for pair in "$A|$B" "$B|$A"; do
    IFS='|' read -r F T <<< "$pair"
    for ship in explorer mandalay explorer86; do
      echo "  judge-fuel $F -> $T [$ship]"
      jrun "$F" "$T" "$ship" "$BUD"
    done
  done
done

echo "== overnight complete: $(date) =="
