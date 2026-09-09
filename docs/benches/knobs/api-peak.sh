#!/usr/bin/env bash
# Combined-peak harness: several machines run THIS script at the same
# wall-clock second so the server sees one aggregate spike (2026-09-07,
# maintainer + the assistant session + the assistant session: 3 × c=64 = 192 in flight against a
# 10-connection Postgres pool). Each machine writes its own CSV; the
# server-side histograms on the box are the shared truth.
#
#   docs/benches/knobs/api-peak.sh <start HH:MM:SSZ> [base_url] [c] > docs/benches/api-peak-YYYY-MM-DD-<who>.csv
#
# Waits until the start time (UTC), then runs three 30 s rungs back to
# back, one route each, all at concurrency c (default 64):
#   T+0:00  route Sol → Colonia   (planner, CPU)
#   T+0:45  trade/search Sol 40ly (Postgres, wide)
#   T+1:30  names/complete "so"   (mmap binary search, should not bend)
# The 15 s gaps let the box settle so the rungs don't bleed together.
set -uo pipefail
START="${1:?start time, e.g. 07:30:00Z}"
API="${2:-https://api.edda-app.com}"
C="${3:-64}"
command -v oha >/dev/null || { echo "oha not found (brew install oha / cargo install oha)" >&2; exit 1; }

now_s() { date -u +%s; }
start_s=$(date -u -j -f "%H:%M:%SZ" "$START" +%s 2>/dev/null || date -u -d "today $START" +%s)
wait=$(( start_s - $(now_s) ))
[ "$wait" -gt 0 ] && { echo "waiting ${wait}s for $START" >&2; sleep "$wait"; }

rung() {
  local name="$1"; shift
  local out
  out="$(oha --no-tui -z 30s -c "$C" --output-format json "$@" 2>/dev/null)" || { echo "$name,$C,oha_failed"; return; }
  OHA_JSON="$out" python3 - "$name" "$C" <<'PY'
import sys, json, os
name, c = sys.argv[1], sys.argv[2]
d = json.loads(os.environ["OHA_JSON"])
s = d["summary"]; lat = d["latencyPercentiles"]; codes = d["statusCodeDistribution"]
ok = sum(v for k, v in codes.items() if k.startswith("2"))
r429 = codes.get("429", 0); r5 = sum(v for k, v in codes.items() if k.startswith("5"))
ms = lambda k: round(lat[k] * 1000, 1) if lat.get(k) is not None else ""
print(f'{name},{c},{sum(codes.values())},{round(s["requestsPerSec"],2)},{ms("p50")},{ms("p95")},{ms("p99")},{ok},{r429},{r5},{sum(codes.values())-ok-r429-r5},{sum((d.get("errorDistribution") or {}).values())}')
PY
}
json=(-H 'content-type: application/json' -m POST)

echo "route,concurrency,requests,rps,p50_ms,p95_ms,p99_ms,http_2xx,http_429,http_5xx,http_other,transport_errors"
echo "# started $(date -u +%H:%M:%SZ) on $(hostname)"
rung "route_sol_colonia"       "${json[@]}" -d '{"from":"Sol","to":"Colonia","range_ly":50}' "$API/v1/route"
sleep 15
rung "trade_search_sol_40ly"   "${json[@]}" -d '{"system":"Sol","radius_ly":40,"limit":10}' "$API/v1/trade/search"
sleep 15
rung "names_complete_system_so" "$API/v1/names/complete?kind=system&prefix=so&limit=12"
echo "# finished $(date -u +%H:%M:%SZ)"
