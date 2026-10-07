#!/usr/bin/env bash
# Route-latency canary against production: pinned routes, a bound each,
# red if any is slow or fails. Runs hourly from .github/workflows/route-canary.yml
# and by hand; a red run is an email to the maintainer, so a slow galaxy
# crossing can never again go unnoticed (maintainer, 2026-10-07: "we can
# NEVER have this happen again"). The cache key is the request tuple, so
# a range that moves with the minute defeats the one-hour plot cache and
# every run measures the planner, not the cache.
#
#   scripts/route_canary.sh [api_base]
#
# Bounds are the measured baseline with headroom, not the target: Sol ->
# Colonia answered in 0.96-2.0 s on 2026-10-06/07; Sol -> Beagle Point is
# the long lane's full budget today (see docs/benches/2026-10-07-route-latency-prod.csv)
# and its bound tightens when the interactive-lane ruling ships.
set -uo pipefail
API="${1:-https://api.edda-app.com}"
range="50.$(date -u +%M)"
red=0
plot() { # name from to bound_s
    local name="$1" from="$2" to="$3" bound="$4" out t0 t1 secs code
    out="$(mktemp)"; t0=$(date +%s.%N)
    code=$(curl -sS -m 200 -o "$out" -w '%{http_code}' -X POST "$API/v1/route" -H 'content-type: application/json' \
        -d "{\"from\":\"$from\",\"to\":\"$to\",\"range_ly\":$range,\"supercharge\":true,\"white_dwarfs\":false,\"min_fuel\":true}" || echo 000)
    t1=$(date +%s.%N); secs=$(python3 -c "print(round($t1-$t0,1))")
    if [ "$code" != 200 ]; then echo "RED  $name: http $code after ${secs}s"; red=1; rm -f "$out"; return; fi
    python3 - "$out" "$name" "$secs" "$bound" <<'PY'
import json,sys
v=json.load(open(sys.argv[1])); name,secs,bound=sys.argv[2],float(sys.argv[3]),float(sys.argv[4])
line=f"{name}: {secs}s wall, planner {v.get('elapsed_ms')} ms, {v.get('jumps')} jumps, {v.get('boosted_jumps')} boosted, {v.get('expansions')} expansions, variants {v.get('variants_finished')}/{v.get('variants_run')}, lane {v.get('lane')}, cached {v.get('cached')}, highway_pending {v.get('highway_pending')}"
if v.get('cached'): print("RED  "+line+" -- the canary must not be served from the cache"); sys.exit(3)
if v.get('highway_pending'): print("RED  "+line+" -- plotted without the highway"); sys.exit(3)
if secs>bound: print(f"RED  {line} -- over the {bound}s bound"); sys.exit(3)
print("ok   "+line)
PY
    [ $? -eq 0 ] || red=1
    rm -f "$out"
}
plot "Sol -> Alioth (interactive)"      "Sol" "Alioth"       5
plot "Sol -> Colonia (long)"            "Sol" "Colonia"      15
plot "Sol -> Beagle Point (long)"       "Sol" "Beagle Point" 150
exit $red
