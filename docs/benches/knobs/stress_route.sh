#!/usr/bin/env bash
# Stress route against production: one POST /v1/route per direction with
# The Caspian request that reproduces the app (fuel_model + premium injections):
# curl -X POST $API/v1/route -H content-type:application/json --data-binary @docs/benches/knobs/stress_caspian_body.json
# the canary's request shape (range moves with the minute to defeat the
# plot cache), the full answer saved, one summary line each.
#
#   stress_route.sh "<from>" "<to>" [range_ly] [out_dir] [api_base]
set -uo pipefail
FROM="$1"; TO="$2"; RANGE="${3:-72.$(date -u +%M)}"; OUT="${4:-.}"; API="${5:-https://api.edda-app.com}"
mkdir -p "$OUT"
plot() { # from to tag
    local from="$1" to="$2" tag="$3" out t0 t1 secs code
    out="$OUT/$tag.json"; t0=$(date +%s.%N)
    code=$(curl -sS -m 400 -o "$out" -w '%{http_code}' -X POST "$API/v1/route" -H 'content-type: application/json' \
        -d "{\"from\":\"$from\",\"to\":\"$to\",\"range_ly\":$RANGE,\"supercharge\":true,\"white_dwarfs\":false,\"min_fuel\":true}" || echo 000)
    t1=$(date +%s.%N); secs=$(python -c "print(round($t1-$t0,1))")
    echo "http $code in ${secs}s wall: $from -> $to (range $RANGE)"
    [ "$code" = 200 ] || { head -c 600 "$out"; echo; return; }
    python - "$out" <<'PY'
import json,sys
v=json.load(open(sys.argv[1]))
keys=["elapsed_ms","jumps","boosted_jumps","refuel_stops","total_ly","expansions","variants_finished","variants_total","early_answer","highway_pending","cached","injections","note"]
print("  "+", ".join(f"{k}={v.get(k)}" for k in keys if k in v))
hops=v.get("hops") or []
if hops: print(f"  hops={len(hops)} first={hops[0].get('name')} last={hops[-1].get('name')}")
PY
}
plot "$FROM" "$TO" fwd
plot "$TO" "$FROM" back
