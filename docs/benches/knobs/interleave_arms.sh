#!/usr/bin/env bash
# Two arms of the SAME code, interleaved pair by pair (the rule adopted in
# the 2026-10-19 audit: a default changes only when two same-code runs,
# interleaved, both hold the pre-registered row). For every pair in the
# route list, arm A plots both directions, then arm B, so drift in the
# page cache, the machine's thermals or the box's other work lands on
# both arms alike.
#   interleave_arms.sh <bench.exe> <tagA> <tagB> <routes.txt> <out_dir> [budget_s] [extra args]
# Environment (RAYON_NUM_THREADS, EDDA_INDEX, knobs) passes through to
# edda_matrix.sh, which appends under EDDA_APPEND=1.
set -uo pipefail
B="$1"; TA="$2"; TB="$3"; ROUTES="$4"; OUT="$5"; BUDGET="${6:-100}"; shift 6 || shift $#
HERE="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$OUT"; : > "$OUT/$TA.jsonl"; : > "$OUT/$TB.jsonl"
ONE="$OUT/.one-pair-$$.txt"
while IFS= read -r line; do
  [ -z "$line" ] && continue
  printf '%s\n' "$line" > "$ONE"
  for TAG in "$TA" "$TB"; do
    EDDA_APPEND=1 "$HERE/edda_matrix.sh" "$B" "$TAG" "$ONE" "$OUT" "$BUDGET" "$@"
  done
done < "$ROUTES"
rm -f "$ONE"
