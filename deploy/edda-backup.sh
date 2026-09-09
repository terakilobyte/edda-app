#!/usr/bin/env bash
# Nightly dump of the tables that are NOT rebuildable from public dumps:
# accumulated station identity, prohibited lists, services, and the
# ingest watermark. Everything else on the box regenerates (ledger,
# 2026-09-04). Fourteen dumps kept.
set -euo pipefail
out="/var/lib/edda/backups/irreplaceables-$(date -u +%Y%m%d).sql.gz"
pg_dump "$DATABASE_URL" \
    --table=stations --table=station_prohibited --table=station_services \
    --table=eddn_ingestion --data-only |
    gzip > "$out"
# Size floor: the first-ever run silently "succeeded" with a 50 KB
# pre-restore snapshot (audit H3). A real dump of these tables is tens
# of MB; below the floor is a failure, loudly.
[ "$(stat -c%s "$out")" -gt 10000000 ] || { echo "backup suspiciously small: $(stat -c%s "$out") bytes" >&2; exit 1; }
# ls exits 2 on zero matches and pipefail would kill the script on the
# very first night; the subshell-or-true keeps rotation honest.
(ls -1t /var/lib/edda/backups/irreplaceables-*.sql.gz 2>/dev/null || true) | tail -n +15 | xargs -r rm --
