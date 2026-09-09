#!/usr/bin/env bash
# Daily enrichment (maintainer, 2026-09-08: "why daily 7 days? why not daily
# dailies?"): Spansh's last-24-hours dump, hydrated BEFORE the routing
# reconcile so the systems, stations, pads and services it carries reach
# the routing product and the profit finder the same day, then the daily
# market window. This is the only scheduled sync (maintainer, 2026-09-09); a
# missed night is healed by hand, not by a standing weekly (README,
# "Break glass"). Measured basis: galaxy_7days (3.1 GB) hydrated in 16-18
# min; galaxy_1day is ~1.35 GB and publishes at 05:05Z, so the 00:05Z
# run takes the previous day's file - a day's lag, not a week's.
set -euo pipefail
cd /var/lib/edda/dumps

curl -fsSL -o galaxy_1day.json.gz https://downloads.spansh.co.uk/galaxy_1day.json.gz
/usr/local/bin/ed-api hydrate --spansh galaxy_1day.json.gz
rm -f galaxy_1day.json.gz

# Routing first (cheap when nothing changed), then the daily window.
/usr/local/bin/ed-api reconcile-routing
/usr/local/bin/ed-api publish-market-daily
