#!/usr/bin/env bash
# Elasticity capture (ledgered study, 2026-09-04): poll named station
# boards for one commodity and append a CSV row whenever the board's
# observed_at changes. Ground truth for demand-saturation pairs — the
# market table is newer-wins, so un-snapshotted states evaporate.
#
#   board_watch.sh <out.csv> <commodity_symbol> <hours> <station_id>...
#
# Needs DATABASE_URL. Run in WSL beside the Postgres it watches.
set -u
out="$1"; symbol="$2"; hours="$3"; shift 3
ids=$(IFS=,; echo "$*")
[ -f "$out" ] || echo "captured_at,station_id,station,sell_price,demand,buy_price,supply,observed_at" > "$out"
end=$(( $(date +%s) + hours * 3600 ))
declare -A last
while [ "$(date +%s)" -lt "$end" ]; do
    while IFS='|' read -r sid sname sell demand buy supply obs; do
        [ -z "${sid:-}" ] && continue
        if [ "${last[$sid]:-}" != "$obs" ]; then
            last[$sid]="$obs"
            echo "$(date -u +%FT%TZ),$sid,$sname,$sell,$demand,$buy,$supply,$obs" >> "$out"
        fi
    done < <(psql "$DATABASE_URL" -tA -c "
        SELECT m.station_id, s.name, m.sell_price, m.demand, m.buy_price, m.supply, m.observed_at
        FROM market m JOIN stations s ON s.id = m.station_id
        WHERE m.station_id IN ($ids) AND m.commodity_symbol = '$symbol'")
    sleep 300
done
echo "watch complete: $(wc -l < "$out") lines in $out"
