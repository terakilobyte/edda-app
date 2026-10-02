#!/usr/bin/env bash
# Weekly publications only. The weekly SYNCS (galaxy_7days from Spansh,
# bodies7days from EDSM) were retired (maintainer, 2026-09-09: "Why are we
# syncing any more than daily? Why do we not trust our own data?"): EDDN
# runs as its own unit and survives a swap, and the nightly galaxy_1day
# carries what EDDN never saw. Break-glass for a missed night is one
# manual line (deploy/README.md, "Break glass").
set -euo pipefail

/usr/local/bin/ed-api publish-community
/usr/local/bin/ed-api publish-stars

# The market freshness index bloats without bound: the board writer
# replaces a station's board by DELETE + INSERT, and a B-tree keyed on
# observed_at never reuses the pages those deletes leave. Measured on
# the box 2026-10-02: market_commodity_fresh_idx at 38 GB on an 11 GB
# heap against 9.7 GB freshly built on the mirror; the database 85 GB
# against the mirror's 49. A concurrent reindex took it to 8.75 GB and
# the database to 56 GB in 3 min 9 s without blocking ingest
# (docs/benches/2026-10-02-market-index-bloat.csv), and the churn is
# about 1 GB a day, so weekly holds it. CONCURRENTLY deadlocks with an
# ANALYZE on the same table (the deploy's post-restart ANALYZE did
# exactly that at 04:39Z); the hydrate ends by 00:25 and this runs at
# 01:00, but a deploy can land any time, so one retry after a pause,
# and any half-built index a failure leaves is dropped first.
reindex_market() {
    psql "$DATABASE_URL" -qAt -c "DROP INDEX CONCURRENTLY IF EXISTS market_commodity_fresh_idx_ccnew" \
        -c "REINDEX INDEX CONCURRENTLY market_commodity_fresh_idx" \
        -c "SELECT 'market_commodity_fresh_idx ' || pg_size_pretty(pg_relation_size('market_commodity_fresh_idx')) || ', database ' || pg_size_pretty(pg_database_size(current_database()))"
}
reindex_market || { sleep 600; reindex_market; }
