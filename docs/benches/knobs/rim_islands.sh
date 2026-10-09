#!/usr/bin/env bash
# Rim islands: the boss's 2026-10-09 plot, Jongou XM-W d1-0 -> Byoi Fraae
# CQ-G d10-0 (87 kly, both ends with their nearest known star 74-77 ly
# out), on a Caspian Explorer at its 72.2 ly full-tank reach with the
# Mk II SCO boosts. Three requests against one server: plain (expect a
# fast island refusal, not a 100 s budget), basic injections, premium
# injections (expect a route, with its injection count).
#
#   bash docs/benches/knobs/rim_islands.sh [http://127.0.0.1:8789] [label]
#
# Prints CSV rows: label,case,http,wall_s,jumps,injections,detail
API="${1:-http://127.0.0.1:8789}"
LABEL="${2:-local}"
FROM="Jongou XM-W d1-0"
TO="Byoi Fraae CQ-G d10-0"
one() {
  local case="$1" inj="$2"
  local body="{\"from\":\"$FROM\",\"to\":\"$TO\",\"range_ly\":72.19,\"boost\":{\"neutron\":6.0,\"white_dwarf\":3.0},\"supercharge\":true,\"thorough\":false$inj}"
  local s e out code json
  s=$(date +%s.%N)
  out=$(curl -s -m 200 -w '\n%{http_code}' -H 'Content-Type: application/json' -d "$body" "$API/v1/route")
  e=$(date +%s.%N)
  code=$(echo "$out" | tail -1)
  json=$(echo "$out" | sed '$d')
  python3 - "$LABEL" "$case" "$code" "$(echo "$e - $s" | bc)" <<'PY' "$json"
import sys, json
label, case, code, wall = sys.argv[1:5]
raw = sys.argv[5]
try:
    d = json.loads(raw)
except Exception:
    d = {}
jumps = d.get("jumps", "")
inj = d.get("injections", "")
detail = d.get("error", "") if "error" in d else ""
if d.get("why") == "island":
    detail = f"island {d.get('end')} {d.get('system')} reach={d.get('reach_ly'):.1f} nearest={d.get('nearest_ly')} light={d.get('light_reach_ly'):.1f} injected={d.get('injected_reach_ly')}"
print(f"{label},{case},{code},{float(wall):.2f},{jumps},{inj},{detail}")
PY
}
echo "label,case,http,wall_s,jumps,injections,detail"
one plain ""
one basic ',"injection":{"grade":"basic","max":10}'
one premium ',"injection":{"grade":"premium","max":10}'
