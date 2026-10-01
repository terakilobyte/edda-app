#!/usr/bin/env bash
# Mirror the production PostgreSQL into a local WSL database, one for one
# (boss, 2026-09-30: "sync the server db to the wsl db too so that we're
# one for one"). Measured the same day: the compressed dump is ~1.9 GB
# (market 103M rows ≈ 720 MB, outfitting 68M ≈ 170 MB, the rest 954 MB in
# 1m50s), so the dump is a light read on the box; the restore is bound by
# index builds. Afterwards the local `ed-api serve` + `ed-api ingest` run
# against the mirror and the EDDN feed keeps it current.
#
#   bash scripts/mirror-prod-db.sh            # from WSL
#   BOX=root@host DB=edda_mirror bash scripts/mirror-prod-db.sh
set -euo pipefail
BOX=${BOX:-root@77.42.18.140}
DB=${DB:-edda_mirror}
PGURL=${PGURL:-postgres://nathan@127.0.0.1:55432}
EDAPI=${EDAPI:-$HOME/edda-target/release/ed-api}
STAMP=$(date -u +%Y%m%dT%H%M)
REMOTE=/tmp/edda-dump-$STAMP
LOCAL=$HOME/edda-mirror/$STAMP
export PATH=/home/linuxbrew/.linuxbrew/bin:$PATH
log() { echo "$(date -u +%H:%M:%S) $*"; }

log "dump on the box -> $REMOTE (directory format, 2 jobs, zstd)"
ssh -o BatchMode=yes "$BOX" "sudo -u postgres pg_dump -Fd -j 2 --compress=zstd:3 -f $REMOTE edda && du -sh $REMOTE"
log "rsync -> $LOCAL"
mkdir -p "$LOCAL"
rsync -a --info=progress2 -e "ssh -o BatchMode=yes" "$BOX:$REMOTE/" "$LOCAL/" | tail -2
du -sh "$LOCAL"
log "restore into $DB (6 jobs)"
psql "$PGURL/postgres" -q -c "DROP DATABASE IF EXISTS $DB" -c "CREATE DATABASE $DB"
pg_restore -j 6 --no-owner --no-privileges -d "$PGURL/$DB" "$LOCAL" || log "pg_restore reported errors (see above); continuing to verify"
log "verify"
psql "$PGURL/$DB" -At -F ' | ' -c "SELECT (SELECT count(*) FROM stations) AS stations, (SELECT count(*) FROM stations WHERE controlling_faction IS NOT NULL) AS with_faction, (SELECT reltuples::bigint FROM pg_class WHERE relname = 'market') AS market_est, (SELECT count(*) FROM outfitting) AS outfitting, (SELECT name FROM schema_migrations ORDER BY name DESC LIMIT 1) AS last_migration, pg_size_pretty(pg_database_size('$DB')) AS size"
log "migrate with the local binary (anything the working tree has past production)"
DATABASE_URL="$PGURL/$DB" EDDA_API_ARTIFACT_DIR=/tmp/edda-artifacts RUST_LOG=ed_api=info "$EDAPI" migrate 2>&1 | grep -o '"migration":"[^"]*","ms":[0-9]*\|{"migrated.*' || true
log "point the local server and ingester at $DB"
pkill -f "^$EDAPI (serve|ingest)" || true
sleep 1
export DATABASE_URL="$PGURL/$DB" EDDA_API_BIND=127.0.0.1:8787 EDDA_API_ARTIFACT_DIR=/tmp/edda-artifacts RUST_LOG=ed_api=info EDDA_API_EDDN_RELAY=tcp://eddn.edcd.io:9500 EDDA_API_INGEST_BIND=127.0.0.1:8788
mkdir -p /tmp/edda-artifacts
cd /tmp
setsid nohup "$EDAPI" serve > /tmp/edda-serve.log 2>&1 < /dev/null & disown
setsid nohup "$EDAPI" ingest > /tmp/edda-ingest.log 2>&1 < /dev/null & disown
sleep 8
pgrep -fa "^$EDAPI" | cut -c1-80
curl -s http://127.0.0.1:8787/readyz; echo
ssh -o BatchMode=yes "$BOX" "rm -rf $REMOTE"
log "done: local server on $DB; edda_dev kept until the mirror is verified"
