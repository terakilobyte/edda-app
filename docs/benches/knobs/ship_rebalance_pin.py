#!/usr/bin/env python3
"""Baseline (and later: diff) for the ship rebalance Frontier announced in
the October 2026 dev log (published 2026-10-14, lands 2026-10-20 with the
Anaconda Mk II): Asp Scout, Federal Dropship, Imperial Clipper, Type-6,
Type-7, Viper Mk IV, Sidewinder, Eagle, Caspian, Mandalay.

What EDDA holds for a hull comes from two tables: the vendored Coriolis
ship rows (`crates/ed-ships/data/coriolis/ships/*.json`: hull mass, speed,
boost, base shield and armour, agility, and the stock slot sizes) and the
slot table (`crates/ed-ships/data/ship_slots.json`: every slot by journal
name). Both go stale the day the patch lands. This knob prints the figures
the dev log says will move, per ship, so the pre-patch row is on record;
rerun after the vendored data is refreshed and diff the two CSVs.

    python docs/benches/knobs/ship_rebalance_pin.py > docs/benches/<date>-ship-rebalance.csv

The "announced" column is the dev log's wording, pre-registered: a rerun
whose rows do not move where the log said they would means the vendored
data has not caught up, not that Frontier changed nothing.
"""
import glob
import json
import os
from collections import Counter

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
CORIOLIS = os.path.join(ROOT, "crates", "ed-ships", "data", "coriolis", "ships")
SLOTS = os.path.join(ROOT, "crates", "ed-ships", "data", "ship_slots.json")

# coriolis file stem, slot-table symbol, the dev log's announced changes
SHIPS = [
    ("asp_scout", "asp_scout", "additional utility slot; improved jump range; increased speed; additional maneuverability"),
    ("federal_dropship", "federation_dropship", "additional size 6 internal; improved jump range; increased speed"),
    ("imperial_clipper", "empire_trader", "additional size 7 internal; improved jump range; increased shield capacity"),
    ("type_6_transporter", "type6", "additional size 6 internal; increased shield capacity"),
    ("type_7_transport", "type7", "additional size 7 internal; improved jump range; greater thermal"),
    ("viper_mk_iv", "viper_mkiv", "increased shield capacity; increased speed; additional maneuverability"),
    ("sidewinder", "sidewinder", "increased shield capacity; additional maneuverability"),
    ("eagle", "eagle", "increased shield capacity"),
    ("explorer_nx", "explorer_nx", "improved jump range"),
    ("mandalay", "mandalay", "improved jump range"),
]
PROPS = ["hullMass", "speed", "boost", "baseShieldStrength", "baseArmour", "heatCapacity", "pitch", "roll", "yaw", "masslock"]


def main():
    table = json.load(open(SLOTS, encoding="utf-8"))
    print("# Ship rebalance baseline from EDDA's vendored tables (Coriolis ship rows + ship_slots.json); the dev log of 2026-10-14 announced the changes in the last column, landing 2026-10-20. Rerun after refreshing the vendored data and diff.")
    print("ship,symbol," + ",".join(PROPS) + ",stock_fsd_class,internals_by_size,military,hardpoints,utilities,slot_table_internals,slot_table_utilities,announced")
    for stem, symbol, announced in SHIPS:
        path = os.path.join(CORIOLIS, f"{stem}.json")
        if not os.path.exists(path):
            print(f"{stem},{symbol}," + "," * (len(PROPS) + 7) + f"MISSING coriolis file,{announced}")
            continue
        d = json.load(open(path, encoding="utf-8"))
        ship = d[next(iter(d))]
        p = ship.get("properties", {})
        slots = ship.get("slots", {})
        internals = Counter(s if isinstance(s, int) else s.get("class") for s in slots.get("internal", []))
        hard = slots.get("hardpoints", [])
        hp = sum(1 for h in hard if h > 0)
        util = sum(1 for h in hard if h == 0)
        standard = slots.get("standard", [])
        fsd = standard[2] if len(standard) > 2 else ""
        mil = sum(1 for s in slots.get("internal", []) if isinstance(s, dict) and "mil" in json.dumps(s.get("eligible", {})).lower())
        hull = table.get(symbol) or {}
        t_int = Counter(s["size"] for s in hull.get("slots", []) if s["group"] == "internal")
        t_util = sum(1 for s in hull.get("slots", []) if s["group"] == "utility")
        fmt = lambda c: " ".join(f"{k}x{v}" for k, v in sorted(c.items(), reverse=True) if k is not None)
        row = [p.get("name", stem), symbol] + [str(p.get(k, "")) for k in PROPS] + [str(fsd), fmt(internals), str(mil), str(hp), str(util), fmt(t_int) if hull else "NOT IN SLOT TABLE", str(t_util) if hull else "", announced]
        print(",".join(f'"{x}"' if "," in x or " " in x else x for x in row))


if __name__ == "__main__":
    main()
