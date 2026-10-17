#!/usr/bin/env bash
# Discriminating runs for the rim-to-rim stall, one after another on the PC
# 2026-10-17 rim-to-rim stress: bench.exe built with CARGO_TARGET_DIR=edda-target-limits
# (cargo build --release --example bench -p ed-galaxy); index = the Sep 11 full
# build with boost.bin. Override with EDDA_BENCH / EDDA_INDEX / EDDA_STRESS_OUT.
# (budget 100 s each, per-leg debug on): which half fails, and whether the
# bidirectional variant engages when both ends are neutrons.
set -uo pipefail
B="${EDDA_BENCH:-/c/Users/terak/ED-Claude/edda-target-limits/release/examples/bench.exe}"
IDX="${EDDA_INDEX:-/d/galos/index-boost-secondary}"
S="${EDDA_STRESS_OUT:-docs/benches/stress-out}"; mkdir -p "$S"
run() { # tag from to
    local tag="$1" from="$2" to="$3" t0
    t0=$(date +%s)
    ED_PLOT_DEBUG=1 "$B" "$IDX" "$from" "$to" --ship explorer --inject premium 22 --budget 100 --min-fuel --json --route-out "$S/$tag-route.jsonl" > "$S/$tag.json" 2> "$S/$tag.err"
    echo "== $tag: exit $? after $(( $(date +%s)-t0 )) s  ($from -> $to)"
    head -1 "$S/$tag.err" | cut -c1-160
    grep -E 'coarse stalled|goal reachable|bidi x1|next wave|found [0-9]+ jumps|gateway' "$S/$tag.err" | sed 's/^ *//' | sort | uniq -c | sort -rn | head -6 | cut -c1-200
    grep '^{' "$S/$tag.json" | python -c "
import json,sys
for line in sys.stdin:
    v=json.loads(line); print('  ROUTE', {k:v.get(k) for k in ['jumps','boosted_jumps','injections','refuel_stops','total_ly','straight_ly','elapsed_ms','variants_finished','variants_total'] if k in v})" 2>/dev/null
}
run half2 "Leamuae YV-T d4-2760" "Sphiesi HX-L d7-0"
run half1 "Ood Fleau ZJ-I d9-0" "Leamuae YV-T d4-2760"
run neutron-ends "Byiae Aick ON-B d13-0" "Hypoe Groa NN-Q d6-0"
run reverse "Sphiesi HX-L d7-0" "Ood Fleau ZJ-I d9-0"
