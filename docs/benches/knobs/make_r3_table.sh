#!/usr/bin/env bash
# make_r3_table.sh <after_pc_arm> <after_3thr_arm> <date> <reverse_note>
# Regenerates docs/benches/2026-10-18-planner-matrix-r3-table.md: the shipped
# planner (before / before3) and the R2 head (thin6 / thin6-3) against the
# given after arms, both thread counts, seconds judge.
set -euo pipefail
PC="$1"; TH="$2"; DATE="$3"; NOTE="$4"
D=docs/benches
T=$D/2026-10-18-planner-matrix-r3-table.md
REF=$D/2026-10-17-planner-matrix-ref.jsonl
{
  echo "# Planner matrix, before and after ($DATE): the shipped planner and the R2 branch head vs branch planner/r3 (the feasibility witness, its segments and the descent seed, LazySP repair, the bound-gap stop, the quality floor, the heavy slice, the witness in the plot's judging currency, the leg precheck), 28 routes, warm, 100 s, seconds judge"
  echo
  echo "Rows from docs/benches/2026-10-17-planner-matrix-{before,before3,thin6,thin6-3}.jsonl and 2026-10-18-planner-matrix-{$PC,$TH}.jsonl via knobs/planner_before_after.py --judge seconds. Columns: route, straight ly, jumps b/a, stops b/a, pilot seconds b/a, first route s b/a, wall s b/a, the external reference's jumps / seconds (for the eye), verdict. $NOTE"
  for sec in \
    "Three threads (the box's long lane): the shipped planner (before3) -> $TH|2026-10-17-planner-matrix-before3|2026-10-18-planner-matrix-$TH" \
    "Three threads: the R2 branch head (thin6-3) -> $TH|2026-10-17-planner-matrix-thin6-3|2026-10-18-planner-matrix-$TH" \
    "PC, 24 threads: the shipped planner (before) -> $PC|2026-10-17-planner-matrix-before|2026-10-18-planner-matrix-$PC" \
    "PC, 24 threads: the R2 branch head (thin6) -> $PC|2026-10-17-planner-matrix-thin6|2026-10-18-planner-matrix-$PC"; do
    IFS='|' read -r title a b <<< "$sec"
    echo; echo "## $title"; echo
    PYTHONIOENCODING=utf-8 python docs/benches/knobs/planner_before_after.py "$D/$a.jsonl" "$D/$b.jsonl" --ref "$REF" --judge seconds || true
  done
} > "$T"
sed -i 's/\r$//' "$T"
grep -E "^better" "$T"
