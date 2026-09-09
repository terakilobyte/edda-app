#!/usr/bin/env python3
"""Are we already a Spansh peer? (ledger item 49 follow-up, maintainer ask)

    eddn_coverage_check.py <galaxy_7days.json.gz> <overlap_start_iso>

For every station in the dump whose market updateTime falls inside our
own EDDN listening window, check whether our Postgres heard a board for
that station in the same window. Coverage ~100% means the dump teaches
us nothing we didn't hear ourselves for the overlap — the dump
dependency reduces to pre-watermark history + curation.

Prints: dump stations updated in window, how many we also heard (and
within-1h), how many we missed, plus 10 example misses.

Needs: DATABASE_URL. Run in WSL where the dump and Postgres live.
"""

import gzip
import json
import os
import subprocess
import sys
from datetime import datetime, timezone

dump_path, start_iso = sys.argv[1], sys.argv[2]
start = datetime.fromisoformat(start_iso).replace(tzinfo=timezone.utc)

# Our observations: station id -> epoch of market_observed_at.
ours = {}
sql = "SELECT id, EXTRACT(EPOCH FROM market_observed_at)::BIGINT FROM stations WHERE market_observed_at >= '%s'" % start_iso
out = subprocess.run(
    ["psql", os.environ["DATABASE_URL"], "-tA", "-c", sql],
    capture_output=True, text=True, check=True,
).stdout
for line in out.splitlines():
    if "|" in line:
        sid, epoch = line.split("|")
        ours[int(sid)] = int(epoch)

dump_updated = 0
heard = 0
heard_close = 0
missed = 0
examples = []

with gzip.open(dump_path, "rt", encoding="utf-8", errors="replace") as f:
    for line in f:
        line = line.strip().rstrip(",")
        if not line.startswith("{"):
            continue
        try:
            system = json.loads(line)
        except json.JSONDecodeError:
            continue
        for station in system.get("stations", []):
            market = station.get("market")
            mid = station.get("id")
            if not market or mid is None:
                continue
            update = market.get("updateTime")
            if not update:
                continue
            try:
                when = datetime.fromisoformat(update.replace("+00", "+00:00"))
            except ValueError:
                continue
            if when.tzinfo is None:
                when = when.replace(tzinfo=timezone.utc)
            if when < start:
                continue
            dump_updated += 1
            epoch = int(when.timestamp())
            heard_at = ours.get(mid)
            if heard_at is None:
                missed += 1
                if len(examples) < 10:
                    examples.append((station.get("name"), system.get("name"), update))
            else:
                heard += 1
                if abs(heard_at - epoch) <= 3600:
                    heard_close += 1

print(f"overlap window start:            {start_iso}")
print(f"dump stations updated in window: {dump_updated:,}")
print(f"  we also heard the station:     {heard:,}")
print(f"    (same board within 1h):      {heard_close:,}")
print(f"  we MISSED:                     {missed:,}")
if dump_updated:
    print(f"coverage: {heard / dump_updated * 100:.1f}%")
for name, system, when in examples:
    print(f"  missed: {name} @ {system} ({when})")
