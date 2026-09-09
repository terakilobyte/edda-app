#!/usr/bin/env python3
"""Read-only census of fleet-carrier journal events (Item 52 fixtures).

The maintainer bought a carrier on 2026-09-06; everything EDDA knows about
carriers until now came from his SQUADRON's carrier, which writes a
different subset (no CarrierBuy, no owner CarrierStats). This is the
instrument for the difference: counts, first/last timestamps, and ONE
raw sample of each event so the shapes can be pinned as fixtures before
any carrier code is written.

Re-run after each session — the interesting events only appear as he
uses the thing (services added, fuel deposited, cargo transferred, a
jump ordered, a ship parked aboard).

    python scripts/carrier-census.py [path/to/edda.sqlite3]

Read-only by construction (URI mode=ro). The commander's name is
redacted; carrier and market IDs are kept because the fixtures need
them to line up.
"""

from __future__ import annotations

import json
import re
import sqlite3
import sys
from pathlib import Path

DEFAULT_DB = Path(__file__).resolve().parents[1] / ".data" / "edda.sqlite3"

# Everything the Item 52 fixture list asks for, plus the docking itself.
EVENTS = [
    "CarrierBuy",
    "CarrierStats",
    "CarrierDepositFuel",
    "CarrierTradeOrder",
    "CarrierJumpRequest",
    "CarrierJump",
    "CarrierJumpCancelled",
    "CarrierLocation",
    "CarrierCrewServices",
    "CarrierModulePack",
    "CarrierShipPack",
    "CarrierDecommission",
    "CarrierBankTransfer",
    "CarrierFinance",
    "CarrierNameChange",
    "CargoTransfer",
    "StoredShips",
]

REDACT_KEYS = {"Commander", "CommanderName", "Name_Localised"}


def redact(value):
    """Drop the commander's name; keep every id and shape."""
    if isinstance(value, dict):
        return {k: ("<redacted>" if k in REDACT_KEYS else redact(v)) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v) for v in value]
    return value


def main() -> int:
    db = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_DB
    if not db.exists():
        print(f"no database at {db}")
        return 1
    conn = sqlite3.connect(f"file:{db.as_posix()}?mode=ro", uri=True)

    print(f"database: {db}\n")
    print(f"{'event':<24} {'count':>6}  first .. last")
    print("-" * 78)
    present = []
    for event in EVENTS:
        row = conn.execute(
            "SELECT COUNT(*), MIN(ts), MAX(ts) FROM events WHERE event = ?", (event,)
        ).fetchone()
        count, first, last = row
        if count:
            present.append(event)
            print(f"{event:<24} {count:>6}  {first} .. {last}")
        else:
            print(f"{event:<24} {count:>6}  —")

    # Docking at a carrier: the callsign pattern is the giveaway.
    callsign = re.compile(r"^[A-Z0-9]{3}-[A-Z0-9]{3}$")
    docks = []
    for ts, raw in conn.execute(
        "SELECT ts, raw FROM events WHERE event = 'Docked' ORDER BY file, offset"
    ):
        try:
            v = json.loads(raw)
        except json.JSONDecodeError:
            continue
        name = v.get("StationName") or ""
        if callsign.match(name) or v.get("StationType") == "FleetCarrier":
            docks.append((ts, name, v.get("MarketID"), v.get("StarSystem")))
    print(f"\ncarrier dockings: {len(docks)}")
    for ts, name, mid, system in docks[-5:]:
        print(f"  {ts}  {name}  MarketID={mid}  in {system}")

    # FCMaterials.json is a companion file, not an event. OUT OF SCOPE
    # since 2026-09-06 (maintainer: "don't need bartender for now, that's ok"),
    # so an absence here is a DECISION, not an outstanding fixture. Still
    # reported, because if it ever appears we want to know.
    fc = conn.execute(
        "SELECT ts, length(raw) FROM snapshots WHERE name = 'FCMaterials.json'"
    ).fetchone()
    print(f"\nFCMaterials.json snapshot: {'present ' + str(fc) if fc else 'absent'}"
          "  (bartender out of scope; not owed)")

    # Say what is actually still missing, rather than leaving a reader to
    # diff the list above by eye.
    owed = [
        name
        for name in ("CarrierDepositFuel", "CarrierTradeOrder")
        if not conn.execute("SELECT 1 FROM events WHERE event = ? LIMIT 1", (name,)).fetchone()
    ]
    owed.append("StoredShips with a ship parked aboard (verify by hand)")
    print("STILL OUTSTANDING for Item 52 A: " + ", ".join(owed))

    print("\n" + "=" * 78)
    print("ONE RAW SAMPLE OF EACH PRESENT EVENT (commander name redacted)")
    print("=" * 78)
    for event in present:
        row = conn.execute(
            "SELECT ts, raw FROM events WHERE event = ? ORDER BY file DESC, offset DESC LIMIT 1",
            (event,),
        ).fetchone()
        if not row:
            continue
        try:
            v = redact(json.loads(row[1]))
        except json.JSONDecodeError:
            continue
        print(f"\n--- {event} ({row[0]}) ---")
        print(json.dumps(v, indent=2)[:2400])

    conn.close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
