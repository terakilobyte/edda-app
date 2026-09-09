#!/usr/bin/env python3
"""Read-only census of StoredShips rows in a live EDDA database.

Item 53 ("Where's my ship?") measure-first step: before any
ship_locations table is built, find out whether the journal actually
carries the fields the Journal Manual v38 documents, and -- the real
risk -- how STALE the newest snapshot is. StoredShips is written when
you visit a shipyard, so every answer the feature could give inherits
that age. A feature that answers "your Anaconda is in Deciat, as of
eleven weeks ago" is a different feature from the one proposed.

Read-only by construction (URI mode=ro): never writes to the maintainer's
live dev database. Re-run after the carrier purchase to see the
carrier-parked shape (Item 52 fixture list).

    python scripts/storedships-census.py [path/to/edda.sqlite3]
"""

from __future__ import annotations

import json
import sqlite3
import sys
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path

DEFAULT_DB = Path(__file__).resolve().parents[1] / ".data" / "edda.sqlite3"

# The fields Item 53's design leans on, per Journal Manual v38.
TOP_LEVEL = ["MarketID", "StationName", "StarSystem"]
REMOTE_FIELDS = ["StarSystem", "InTransit", "ShipMarketID", "TransferPrice", "TransferType"]
HERE_FIELDS = ["ShipID", "ShipType", "Name", "Value", "Hot"]


def parse_ts(ts: str) -> datetime | None:
    try:
        return datetime.fromisoformat(ts.replace("Z", "+00:00"))
    except (ValueError, AttributeError):
        return None


def age_days(ts: str, now: datetime) -> float | None:
    t = parse_ts(ts)
    return None if t is None else (now - t).total_seconds() / 86400.0


def main() -> int:
    db = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_DB
    if not db.exists():
        print(f"no database at {db}")
        return 1
    conn = sqlite3.connect(f"file:{db.as_posix()}?mode=ro", uri=True)
    rows = conn.execute(
        "SELECT ts, raw FROM events WHERE event = 'StoredShips' ORDER BY file, offset"
    ).fetchall()
    now = datetime.now(timezone.utc)

    print(f"database: {db}")
    print(f"StoredShips rows: {len(rows)}")
    if not rows:
        print("nothing to measure -- the commander has not visited a shipyard since ingest began.")
        return 0

    print(f"first: {rows[0][0]}  ({age_days(rows[0][0], now):.1f} days ago)")
    print(f"last:  {rows[-1][0]}  ({age_days(rows[-1][0], now):.1f} days ago)")
    print()

    # THE staleness question: how long between shipyard visits? That gap
    # distribution IS the freshness the feature can promise.
    stamps = [parse_ts(r[0]) for r in rows]
    gaps = [
        (b - a).total_seconds() / 86400.0
        for a, b in zip(stamps, stamps[1:])
        if a and b
    ]
    if gaps:
        gaps_sorted = sorted(gaps)
        mid = gaps_sorted[len(gaps_sorted) // 2]
        print("gap between snapshots (days): "
              f"min {gaps_sorted[0]:.2f}  median {mid:.2f}  max {gaps_sorted[-1]:.2f}")
        print()

    # Field presence across every row: does this game build write what
    # the Manual documents?
    top_present: Counter[str] = Counter()
    remote_present: Counter[str] = Counter()
    here_present: Counter[str] = Counter()
    remote_total = 0
    here_total = 0
    for _, raw in rows:
        try:
            v = json.loads(raw)
        except json.JSONDecodeError:
            continue
        for f in TOP_LEVEL:
            if f in v:
                top_present[f] += 1
        for ship in v.get("ShipsHere") or []:
            here_total += 1
            for f in HERE_FIELDS:
                if f in ship:
                    here_present[f] += 1
        for ship in v.get("ShipsRemote") or []:
            remote_total += 1
            for f in REMOTE_FIELDS:
                if f in ship:
                    remote_present[f] += 1

    print(f"top-level fields (of {len(rows)} rows):")
    for f in TOP_LEVEL:
        print(f"  {f:<16} {top_present[f]:>4}")
    print(f"ShipsHere entries: {here_total}")
    for f in HERE_FIELDS:
        print(f"  {f:<16} {here_present[f]:>4}")
    print(f"ShipsRemote entries: {remote_total}")
    for f in REMOTE_FIELDS:
        print(f"  {f:<16} {remote_present[f]:>4}")
    print()

    # The newest snapshot is what the feature would actually answer from.
    latest = json.loads(rows[-1][1])
    here = latest.get("ShipsHere") or []
    remote = latest.get("ShipsRemote") or []
    located = [s for s in remote if s.get("StarSystem")]
    in_transit = [s for s in remote if s.get("InTransit")]
    print("newest snapshot:")
    print(f"  as of        {rows[-1][0]} ({age_days(rows[-1][0], now):.1f} days ago)")
    print(f"  here         {len(here)} (at {latest.get('StationName')!r} in {latest.get('StarSystem')!r})")
    print(f"  remote       {len(remote)}  -- {len(located)} with a system, {len(in_transit)} in transit")
    print(f"  named ships  {sum(1 for s in here + remote if s.get('Name'))} of {len(here) + len(remote)}")
    systems = Counter(s.get("StarSystem") for s in located)
    print(f"  distinct systems holding ships: {len(systems)}")
    carrier_ids = {s.get("ShipMarketID") for s in remote if s.get("ShipMarketID")}
    print(f"  distinct ShipMarketIDs: {len(carrier_ids)}")

    # Does the answer set match what the Ships tab already lists?
    owned = conn.execute(
        "SELECT COUNT(DISTINCT json_extract(raw, '$.ShipID')) FROM events WHERE event = 'Loadout'"
    ).fetchone()[0]
    print(f"  ships with a Loadout on record: {owned}")

    # Disambiguation rungs (maintainer-ruled order): how often would rung 3
    # (type alone) actually be ambiguous on this commander's fleet?
    types = Counter(s.get("ShipType") for s in here + remote if s.get("ShipType"))
    dupes = {t: n for t, n in types.items() if n > 1}
    print(f"  hull types owned more than once: {len(dupes)}"
          + (f" -- {dupes}" if dupes else ""))
    names = Counter(
        (s.get("Name") or "").strip().lower() for s in here + remote if (s.get("Name") or "").strip()
    )
    name_dupes = {n: c for n, c in names.items() if c > 1}
    print(f"  duplicate ship NAMES: {len(name_dupes)}"
          + (f" -- {list(name_dupes)}" if name_dupes else ""))
    conn.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
