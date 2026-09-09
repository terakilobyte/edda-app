#!/usr/bin/env bash
# Prod API LOAD harness (doctrine rules 1, 3, 4: measure before the default
# flips; harnesses are files; pre-register the expectation).
#
# Question it answers (maintainer, 2026-09-07, "the server is a ferrari and we
# are riding it like a burro"; API-first is a lean, not a ruling, until
# this exists): at what concurrency does each route bend, and where —
# CPU, Postgres, or the EDDN queue backing up behind reads?
#
# PRE-REGISTERED EXPECTATION (2026-09-07, before the first run):
#   - route (planner, CPU-bound) and trade/search (100 M-row scans) bend
#     first: p95 more than doubles between c=4 and c=16, and by c=64 the
#     8-core box is CPU-saturated on them.
#   - market/search sits between: index-served but wide.
#   - names/complete, market/station, knowledge/system stay under 50 ms
#     p95 all the way to c=64 (index range scans / mmap binary search).
#   - EDDN queue depth stays flat while reads are hammered; if it climbs,
#     that is the finding that matters most (serving starves ingest).
#   - Zero 5xx at every step. A 429 from the per-source limiters IS
#     expected at high c on rate-limited routes — the harness reports it
#     separately so a limiter is never mistaken for capacity. Set
#     EDDA_*_LIMIT env overrides on the box for a capacity run (see
#     http.rs) or accept the 429 column as the limiter's own measurement.
#
# Usage (from a machine with `oha`; brew install oha):
#   docs/benches/knobs/api-load.sh [base_url] [duration_s] [concurrency list]
#     > docs/benches/api-load-YYYY-MM-DD.csv
#   Defaults: prod, 30 s per step, "1 4 16 64".
#   Set SSH_BOX=edda to also sample the box (load, EDDN queue depth, pool)
#   between steps, from VictoriaMetrics on 127.0.0.1:8428.
#
# Each (route, concurrency) row: requests, rps, p50/p95/p99 ms, 2xx, 429,
# 5xx, other. Pinned request bodies are the same as api-latency.sh so the
# two harnesses diff against each other.
set -uo pipefail

API="${1:-https://api.edda-app.com}"
DUR="${2:-30}"
CONC="${3:-1 4 16 64}"
SSH_BOX="${SSH_BOX:-}"

command -v oha >/dev/null || { echo "oha not found (brew install oha)" >&2; exit 1; }

box_sample() {
  [ -n "$SSH_BOX" ] || return 0
  ssh -o BatchMode=yes -o ConnectTimeout=8 "$SSH_BOX" '
    q() { curl -s "http://127.0.0.1:8428/api/v1/query" --data-urlencode "query=$1" | python3 -c "import sys,json
r=json.load(sys.stdin)[\"data\"][\"result\"]
print(round(float(r[0][\"value\"][1]),2) if r else \"\")"; }
    printf "box,load1=%s,eddn_queue=%s,pool_idle=%s,pool_conns=%s\n" \
      "$(cut -d" " -f1 /proc/loadavg)" "$(q edda_eddn_queue_depth)" "$(q edda_db_pool_idle)" "$(q edda_db_pool_connections)"' 2>/dev/null
}

# name, concurrency, then oha args (method/body/url).
step() {
  local name="$1" c="$2"; shift 2
  local out
  out="$(oha --no-tui -z "${DUR}s" -c "$c" --output-format json "$@" 2>/dev/null)" || { echo "$name,$c,oha_failed"; return; }
  OHA_JSON="$out" python3 - "$name" "$c" <<'PY'
import sys, json, os
name, c = sys.argv[1], sys.argv[2]
d = json.loads(os.environ["OHA_JSON"])
s = d["summary"]; lat = d["latencyPercentiles"]; codes = d["statusCodeDistribution"]
ok = sum(v for k, v in codes.items() if k.startswith("2"))
r429 = codes.get("429", 0)
r5 = sum(v for k, v in codes.items() if k.startswith("5"))
other = sum(codes.values()) - ok - r429 - r5
# A rung with NO responses (2026-09-07: the market-search pool deadlock —
# every request "aborted due to deadline") has null percentiles; print
# the row with blanks so the error column tells the story instead of a
# traceback losing the row.
ms = lambda k: round(lat[k] * 1000, 1) if lat.get(k) is not None else ""
errs = sum((d.get("errorDistribution") or {}).values())
print(f'{name},{c},{sum(codes.values())},{round(s["requestsPerSec"],2)},{ms("p50")},{ms("p95")},{ms("p99")},{ok},{r429},{r5},{other},{errs}')
PY
}

json=(-H 'content-type: application/json' -m POST)

echo "route,concurrency,requests,rps,p50_ms,p95_ms,p99_ms,http_2xx,http_429,http_5xx,http_other,transport_errors"
for c in $CONC; do
  box_sample
  step "names_complete_system_so"   "$c" "$API/v1/names/complete?kind=system&prefix=so&limit=12"
  step "names_complete_station_jam" "$c" "$API/v1/names/complete?kind=station&prefix=jam&limit=12"
  step "knowledge_system_sol"       "$c" "$API/v1/knowledge/system?name=Sol"
  step "market_station"             "$c" "$API/v1/market/station/3228843264"
  step "market_search_gold_50ly"    "$c" "${json[@]}" -d '{"kind":"commodity","text":"gold","system":"Sol","radius_ly":50,"side":"sell","limit":10}' "$API/v1/market/search"
  step "trade_search_sol_40ly"      "$c" "${json[@]}" -d '{"system":"Sol","radius_ly":40,"limit":10}' "$API/v1/trade/search"
  step "route_sol_deciat"           "$c" "${json[@]}" -d '{"from":"Sol","to":"Deciat","range_ly":30}' "$API/v1/route"
  step "route_sol_colonia"          "$c" "${json[@]}" -d '{"from":"Sol","to":"Colonia","range_ly":50}' "$API/v1/route"
done
box_sample
