#!/usr/bin/env bash
# Prod API latency harness (doctrine rule 3: harnesses live as files,
# never retyped inline).
#
# Exercises every query route in ed-api against a live server, cold then
# warm, and prints CSV to stdout so successive runs diff directly. The
# cold/warm split is the point: the trade-search work (migration 0012 +
# force_custom_plan + the MATERIALIZED CTE) was all about first-query
# cost, and knowledge/sphere still shows an ~8x first-call penalty that
# nobody has chased yet.
#
#   docs/benches/knobs/api-latency.sh [base_url] > docs/benches/api-latency-YYYY-MM-DD.csv
#
# Default target is prod. Point it at http://127.0.0.1:8787 to compare a
# dev server, or at a candidate build to A/B a change.
set -uo pipefail

API="${1:-https://api.edda-app.com}"
TIMEOUT="${TIMEOUT:-120}"

# One timed call. Prints "<http_code>,<seconds>".
call() {
  curl -s -m "$TIMEOUT" -o /dev/null -w '%{http_code},%{time_total}' "$@"
}

# name, then curl args. Runs twice: cold (as the server found it) then warm.
row() {
  local name="$1"; shift
  local cold warm
  cold="$(call "$@")"
  warm="$(call "$@")"
  printf '%s,%s,%s\n' "$name" "$cold" "$warm"
}

json=(-H 'content-type: application/json' -X POST)

echo "endpoint,cold_http,cold_s,warm_http,warm_s"
row "healthz"                    "$API/healthz"
row "readyz"                     "$API/readyz"
row "manifest"                   "$API/v1/manifest"
row "stars_ids"                  "$API/v1/stars?ids=10477373803,29258525255073"
row "market_station"             "$API/v1/market/station/3228843264"
row "knowledge_sphere_r20"       "$API/v1/knowledge/sphere?x=0&y=0&z=0&radius_ly=20"
row "knowledge_system_known"     "$API/v1/knowledge/system?name=Sol"
row "trade_search_sol_40ly"      "${json[@]}" "$API/v1/trade/search" \
    -d '{"system":"Sol","radius_ly":40,"limit":10}'
row "trade_search_deciat_60ly"   "${json[@]}" "$API/v1/trade/search" \
    -d '{"system":"Deciat","radius_ly":60,"limit":10}'
row "market_search_gold_50ly"    "${json[@]}" "$API/v1/market/search" \
    -d '{"kind":"commodity","text":"gold","system":"Sol","radius_ly":50,"side":"sell","limit":10}'
row "route_sol_deciat"           "${json[@]}" "$API/v1/route" \
    -d '{"from":"Sol","to":"Deciat","range_ly":30}'
row "route_sol_colonia"          "${json[@]}" "$API/v1/route" \
    -d '{"from":"Sol","to":"Colonia","range_ly":50}'
