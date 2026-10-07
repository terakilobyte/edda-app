#!/usr/bin/env bash
# Planner-pool A/B (2026-10-07, Statler's finding on the box: one long plot
# starves every other plot because all plots fan out over one rayon pool).
# Runs three scenarios against one ed-api and prints CSV rows:
#   (a) Sol -> Beagle Point alone
#   (b) Sol -> Deciat started 15 s into a Wongi -> Beagle Point plot (Alioth is the warm-up, so it would be a cache hit)
#   (c) two crossings at once (HIP 90112 and Sagittarius A* to Beagle Point)
# Each row: arm, scenario, request, http, client_wall_s, elapsed_ms (the
# server's own planning time from the answer), jumps. Every crossing has
# its own origin so the server's route cache never answers for the planner.
#
#   route_pool_ab.sh <arm-label> <api-base-url>
# e.g. route_pool_ab.sh main-shared-6 http://127.0.0.1:8787
set -u
ARM="${1:?arm label}"; API="${2:?api base url}"
TMP="${TMPDIR:-/tmp}/route_pool_ab.$$"; mkdir -p "$TMP"
plot() { # name from to range -> prints "http,wall,elapsed_ms,jumps"
  local name="$1" from="$2" to="$3" range="$4"
  local out="$TMP/$name.json"
  local w; w=$(curl -s -m 400 -o "$out" -w "%{http_code},%{time_total}" -H "content-type: application/json" \
    -d "{\"from\":\"$from\",\"to\":\"$to\",\"range_ly\":$range,\"supercharge\":true}" "$API/v1/route")
  local el jumps
  el=$(grep -o '"elapsed_ms":[0-9]*' "$out" | head -1 | cut -d: -f2)
  jumps=$(grep -o '"jumps":[0-9]*' "$out" | head -1 | cut -d: -f2)
  echo "$w,${el:-},${jumps:-}"
}
row() { echo "$ARM,$1,$2,$3"; }
# Warm the index (names, mmap) with a short plot nobody times.
plot warm Sol Alioth 30 >/dev/null
# (a) alone
row a_alone "Sol->Beagle Point" "$(plot a Sol 'Beagle Point' 72.2)"
sleep 2
# (b) a short plot 15 s into a crossing
plot b_long Wongi 'Beagle Point' 72.2 > "$TMP/b_long.txt" &
LONG=$!
sleep 15
row b_short_during_long "Sol->Deciat" "$(plot b_short Sol Deciat 30)"
wait $LONG
row b_long_with_short "Wongi->Beagle Point" "$(cat "$TMP/b_long.txt")"
sleep 2
# (c) two crossings at once (a second start, so the cache cannot answer the first)
plot c1 'HIP 90112' 'Beagle Point' 72.2 > "$TMP/c1.txt" &
P1=$!
plot c2 'Sagittarius A*' 'Beagle Point' 72.2 > "$TMP/c2.txt" &
P2=$!
wait $P1; wait $P2
row c_two_at_once_1 "HIP 90112->Beagle Point" "$(cat "$TMP/c1.txt")"
row c_two_at_once_2 "Sagittarius A*->Beagle Point" "$(cat "$TMP/c2.txt")"
rm -rf "$TMP"
