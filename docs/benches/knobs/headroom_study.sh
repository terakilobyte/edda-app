#!/usr/bin/env bash
# Item 31 (b): the fuel-headroom matrix. Reruns the eager+lean matrix at
# ED_FUEL_HEADROOM 0 and 2 (shipped default: 10 t via the capacity
# clamp), archiving every route via --route-out per the maintainer's standing
# directive. Compare against prune_gain_P.jsonl (headroom 10) with
# prune_gain_report/decompose tooling; then fuel-replay the headroom-0
# routes through the SHIPPED model to count hops the safe margin would
# forbid -- the load-bearing test.
set -u
ROOT="${EDDA_ROOT:-$(git rev-parse --show-toplevel)}"
K="$ROOT/docs/benches/knobs"
export BENCH="$ROOT/.claude/worktrees/target-a04/release/examples/bench.exe"
for H in 0 2; do
  echo "== headroom $H t: $(date) =="
  export ED_FUEL_HEADROOM=$H
  export ROUTE_OUT="$K/routes_h$H.jsonl"
  : > "$ROUTE_OUT"
  bash "$K/prune_gain.sh" "$K/prune_gain_h$H.jsonl"
done
echo "== headroom study complete: $(date) =="
