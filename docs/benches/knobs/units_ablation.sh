#!/usr/bin/env bash
# R2 Task 3 ablation (2026-10-17): the one-currency arm lost against r1 on
# the matrix, so each priced component is switched off in turn (study
# knobs in long_range.rs: ED_UNITS_COARSE, ED_UNITS_VIA, ED_UNITS_EXACT)
# and the 28 routes re-run warm against the r1 arm under the seconds judge.
#   units_ablation.sh <bench.exe> <out_dir> [budget_s]
# Arms: r2-nocoarse (coarse edges and h as before R2), r2-novia (the coarse
# scoop-stop edge at the old 2.0), r2-noexact (the exact planner's old 0.01
# a boost), r2-judgeonly (all three off: the judge alone). One arm after
# another; no compiles while it runs (the index must stay in the page cache).
set -uo pipefail
B="$1"; OUT="$2"; BUDGET="${3:-100}"
cd "$(dirname "$0")/../../.."
R1=docs/benches/2026-10-17-planner-matrix-r1.jsonl
run() { # tag, then env assignments
  local tag="$1"; shift
  echo "===== $tag ($*)"
  env "$@" bash docs/benches/knobs/edda_matrix.sh "$B" "$tag" docs/benches/knobs/planner_matrix_routes.txt "$OUT" "$BUDGET" 2>&1 | tail -1 | cut -c1-120
  python docs/benches/knobs/rejudge_seconds.py "$OUT/$tag.jsonl" "$OUT" "$tag"
  echo "----- $tag vs r1 (seconds judge)"
  python docs/benches/knobs/planner_before_after.py "$R1" "$OUT/$tag.jsonl" --judge seconds 2>&1 | grep -E "WORSE|better" | cut -c1-200
}
# The control first: with every Task 3 price off the arm is Tasks 1-2
# (scoop byte, refuel reach) plus the judge, so it tells whether the loss
# is Task 3's at all.
run r2-judgeonly ED_UNITS_COARSE=0 ED_UNITS_VIA=0 ED_UNITS_EXACT=0
run r2-novia ED_UNITS_VIA=0
run r2-nocoarse ED_UNITS_COARSE=0
run r2-noexact ED_UNITS_EXACT=0
echo "===== ablation done"
