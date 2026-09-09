#!/usr/bin/env bash
# Trade-report row pull (API-only spec, Phase A.2). The server-side
# profit finder needs EVERY fresh row for the sphere's candidate
# stations, not the best-8 per commodity the legacy query keeps.
# PRE-REGISTERED (doctrine 4): candidates + rows for 100 ly / 48 h is
# <= 0.5 s at Deciat and <= 1.5 s at Wyrd (the densest origin we serve).
# If Wyrd is over, the server's default max_stations is 1,000 nearest
# (the local finder caps at 2,500).
#
#   docs/benches/knobs/trade-rows-pull.sh [DATABASE_URL] > docs/benches/trade-rows-pull-YYYY-MM-DD.csv
#
# Each origin runs the candidate query alone, then candidates+rows as one
# COPY to /dev/null (the serialisation the server pays), warm (second of
# two runs is recorded). Timing is wall-clock around psql on the same
# host; the local docker round trip is sub-millisecond.
set -uo pipefail
DB="${1:-postgres://edda:edda@127.0.0.1:55432/edda_dev}"
RADIUS="${RADIUS:-100}"
AGE_H="${AGE_H:-48}"
# CAP: keep the nearest CAP candidates before loading rows (the finder's
# max_stations, 2,500 locally); empty = uncapped.
CAP="${CAP:-}"

ms() { local s; s=$(date +%s%N); "$@" >/dev/null 2>&1; echo $(( ($(date +%s%N) - s) / 1000000 )); }

candidates_sql() {
  cat <<SQL
WITH o AS (SELECT x, y, z FROM systems WHERE lower(name) = lower('$1') LIMIT 1)
SELECT st.id FROM stations st JOIN systems sy ON sy.address = st.system_address, o
WHERE sy.x BETWEEN o.x-$RADIUS AND o.x+$RADIUS AND sy.y BETWEEN o.y-$RADIUS AND o.y+$RADIUS
  AND sy.z BETWEEN o.z-$RADIUS AND o.z+$RADIUS
  AND (sy.x-o.x)^2 + (sy.y-o.y)^2 + (sy.z-o.z)^2 <= $RADIUS*$RADIUS
  AND st.has_market AND NOT COALESCE(st.is_carrier, false)
$( [ -n "$CAP" ] && echo "ORDER BY (sy.x-o.x)^2 + (sy.y-o.y)^2 + (sy.z-o.z)^2 LIMIT $CAP" )
SQL
}
rows_body() {
  cat <<SQL
WITH cand AS ($(candidates_sql "$1"))
SELECT m.station_id, m.commodity_symbol, m.buy_price, m.sell_price, m.demand, m.supply,
       EXTRACT(EPOCH FROM now() - m.observed_at) / 3600.0
FROM market m JOIN cand ON cand.id = m.station_id
WHERE m.observed_at > now() - make_interval(hours => $AGE_H)
SQL
}

echo "origin,radius_ly,max_age_h,cap,candidate_stations,fresh_rows,candidates_ms,candidates_plus_rows_ms"
for origin in Sol Deciat Wyrd; do
  n_cand=$(psql "$DB" -Atc "SELECT count(*) FROM ($(candidates_sql "$origin")) c")
  n_rows=$(psql "$DB" -Atc "SELECT count(*) FROM ($(rows_body "$origin")) r")
  c_ms=0; r_ms=0
  for _ in 1 2; do c_ms=$(ms psql "$DB" -Atc "$(candidates_sql "$origin")"); done
  for _ in 1 2; do r_ms=$(ms psql "$DB" -Atc "COPY ($(rows_body "$origin")) TO STDOUT"); done
  echo "$origin,$RADIUS,$AGE_H,${CAP:-none},$n_cand,$n_rows,$c_ms,$r_ms"
done
