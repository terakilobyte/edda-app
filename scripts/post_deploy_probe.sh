#!/bin/bash
# scripts/post_deploy_probe.sh <expected-version>   (run after a release deploy)
# The checks that would have caught the 0.3.2 outage (stations 502 on every
# request) and the 0.3.3 Coriolis refusal: manifest version, readyz, a
# station read, typed material traders, a route plot, a Coriolis paste.
# Read-only against prod. Exit 1 if any probe is off.
set -u
API="${API_BASE:-https://api.edda-app.com}"
want="${1:?expected version, e.g. 0.3.4}"
S="$(dirname "$0")/fixtures"
bad=0
py() { python -c "import sys,json; d=json.load(sys.stdin); $1"; }

v=$(curl -fsS "$API/v1/app/latest.json" | py 'print(d.get("version"))')
echo "manifest: $v"; [ "$v" = "$want" ] || { echo "  !! expected $want"; bad=1; }

code=$(curl -s -o /dev/null -w '%{http_code}' "$API/readyz"); echo "readyz: $code"; [ "$code" = 200 ] || bad=1

code=$(curl -s -o /dev/null -w '%{http_code}' "$API/v1/stations?near=Sol"); echo "stations near Sol: $code"; [ "$code" = 200 ] || bad=1

typed=$(curl -fsS "$API/v1/stations?near=Sol&service=material_trader" | py 'rows=d if isinstance(d,list) else d.get("stations",d.get("rows",[])); print(len(rows), sum(1 for r in rows if r.get("primary_economy")))')
echo "material traders typed: $(echo $typed | awk '{print $1" rows, "$2" with economy"}')"; [ "$(echo $typed | awk '{print $2}')" -gt 0 ] || bad=1

code=$(curl -s -o /dev/null -w '%{http_code}' -H 'content-type: application/json' -d '{"from":"Sol","to":"Colonia","range_ly":60}' "$API/v1/route"); echo "route plot: $code"; [ "$code" = 200 ] || bad=1

cor=$(curl -s -w '\n%{http_code}' -H 'content-type: application/json' -d "{\"paste\": $(python -c "import json,sys; print(json.dumps(open(sys.argv[1]).read()))" "$S/coriolis_paste.json")}" "$API/v1/loadout/physics")
code=$(echo "$cor" | tail -1); body=$(echo "$cor" | head -n -1)
echo "coriolis paste: HTTP $code | $(echo "$body" | py 'print("missing", d.get("missing"), "| source", d.get("source"))' 2>/dev/null)"
[ "$code" = 400 ] && echo "$body" | grep -q '"MaxJumpRange"' || { echo "  !! expected a 400 naming MaxJumpRange"; bad=1; }

[ $bad = 0 ] && echo "ALL PROBES OK" || echo "PROBES FAILED"
exit $bad
