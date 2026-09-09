#!/usr/bin/env bash
# /v1/market/search latency bench (design (b) pre-registered targets:
# P50 < 150 ms, P95 < 600 ms server-side). Usage:
#   market_search_bench.sh [base_url] [iterations]
# Emits CSV: case,iterations,p50_ms,p95_ms,max_ms — paste into
# docs/benches/ with the box + row counts in the header per doctrine.
set -euo pipefail
BASE="${1:-http://127.0.0.1:8787}"
N="${2:-30}"

run_case() {
    local name="$1" body="$2"
    local times=()
    for _ in $(seq 1 "$N"); do
        # %{http_code} guards the instrument: the first run of this
        # bench measured 429s from the rate limiter as 0.6 ms query
        # times. A non-200 fails the case loudly, never silently.
        # (Server side: EDDA_API_MARKET_RATE_PER_HOUR raises the budget
        # for bench runs.)
        read -r t code <<< "$(curl -s -o /dev/null -w '%{time_total} %{http_code}' -X POST "$BASE/v1/market/search" \
            -H 'content-type: application/json' -d "$body")"
        if [ "$code" != "200" ]; then
            echo "$name,ERROR http $code" >&2
            echo "$name,0,ERROR,ERROR,ERROR"
            return
        fi
        times+=("$t")
    done
    printf '%s\n' "${times[@]}" | sort -n | awk -v name="$name" -v n="$N" '
        { a[NR] = $1 * 1000 }
        END {
            p50 = a[int(NR * 0.50) + (NR * 0.50 == int(NR * 0.50) ? 0 : 1)]
            p95 = a[int(NR * 0.95) + (NR * 0.95 == int(NR * 0.95) ? 0 : 1)]
            printf "%s,%d,%.1f,%.1f,%.1f\n", name, n, p50, p95, a[NR]
        }'
}

# One throwaway request first: connection setup + statement prep.
curl -s -o /dev/null -X POST "$BASE/v1/market/search" -H 'content-type: application/json' \
    -d '{"kind":"commodity","text":"Gold","system":"Sol","side":"sell"}' || true

echo "case,iterations,p50_ms,p95_ms,max_ms"
run_case "gold_sell_price_100ly"    '{"kind":"commodity","text":"Gold","system":"Sol","side":"sell"}'
run_case "gold_sell_distance_100ly" '{"kind":"commodity","text":"Gold","system":"Sol","side":"sell","sort":"distance"}'
run_case "gold_sell_price_500ly"    '{"kind":"commodity","text":"Gold","system":"Sol","side":"sell","radius_ly":500}'
run_case "palladium_buy_min500"     '{"kind":"commodity","text":"Palladium","system":"Sol","side":"buy","min_quantity":500}'
run_case "sapphire_sell_100ly"      '{"kind":"commodity","text":"Sapphire","system":"Sol","side":"sell","max_age_hours":720}'
run_case "deuterium_sell_500ly"     '{"kind":"commodity","text":"Deuterium","system":"Sol","side":"sell","radius_ly":500,"max_age_hours":720}'
run_case "module_fsd5_100ly"        '{"kind":"module","text":"size5_class5","system":"Sol"}'
run_case "ship_search_100ly"        '{"kind":"ship","text":"python","system":"Sol"}'
run_case "prohibited_optin_sell"    '{"kind":"commodity","text":"Gold","system":"Sol","side":"sell","include_prohibited":true,"min_pad":"l"}'
