#!/usr/bin/env bash
# The stress route on this PC's full index with the planner bench: one run,
# 2026-10-17 rim-to-rim stress: bench.exe built with CARGO_TARGET_DIR=edda-target-limits
# (cargo build --release --example bench -p ed-galaxy); index = the Sep 11 full
# build with boost.bin. Override with EDDA_BENCH / EDDA_INDEX / EDDA_STRESS_OUT.
# per-leg timings on (ED_PLOT_DEBUG), the leg lines summarised.
#
#   stress_pc.sh <tag> <budget_s> [extra bench args...]
set -uo pipefail
TAG="$1"; BUDGET="$2"; shift 2
B="${EDDA_BENCH:-/c/Users/terak/ED-Claude/edda-target-limits/release/examples/bench.exe}"
IDX="${EDDA_INDEX:-/d/galos/index-boost-secondary}"
S="${EDDA_STRESS_OUT:-docs/benches/stress-out}"; mkdir -p "$S"
t0=$(date +%s)
ED_PLOT_DEBUG=1 "$B" "$IDX" "Ood Fleau ZJ-I d9-0" "Sphiesi HX-L d7-0" --ship explorer --inject premium 22 --budget "$BUDGET" --min-fuel --json --route-out "$S/$TAG-route.jsonl" "$@" > "$S/$TAG.json" 2> "$S/$TAG.err"
echo "exit $? after $(( $(date +%s)-t0 )) s (tag $TAG, budget $BUDGET, extra: $*)"
grep -E 'found|refine [0-9]+/[0-9]+ at [0-9]+ ms$' "$S/$TAG.err" | grep -v 'refine 0/' | head -20
python - "$S/$TAG.err" <<'PY'
import re,sys
ok=[];bad=[]
for line in open(sys.argv[1],encoding="utf-8",errors="replace"):
    m=re.match(r'\s+leg\s+(\d+): (\d+) jumps (\d+) expansions (\d+) ms',line)
    if m: ok.append((int(m[4]),int(m[3]),int(m[2]))); continue
    m=re.match(r'\s+leg\s+(\d+): FAILED (.*?) (\d+) ms',line)
    if m: bad.append((int(m[3]),m[2][:120]))
ok.sort(reverse=True)
print(f"legs finished: {len(ok)}, failed: {len(bad)}")
if ok:
    ms=[o[0] for o in ok]; tot=sum(ms)
    print(f"  leg ms: max {ms[0]}, p90 {ms[len(ms)//10]}, median {ms[len(ms)//2]}, sum {tot} ms over all variants")
    print("  slowest 8:", ok[:8])
if bad: print("  failed (slowest 6):", bad[:6])
PY
grep '^{' "$S/$TAG.json" | head -c 600; echo
