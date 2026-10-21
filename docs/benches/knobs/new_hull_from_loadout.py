#!/usr/bin/env python3
"""Day one of a new hull: the commander's first `Loadout` becomes the slot
table's entry for it, checked against what Frontier published.

The slot table (`crates/ed-ships/data/ship_slots.json`) is regenerated from
EDSY once EDSY carries the hull (gen_ship_slots.py); on launch day EDSY does
not, and the maintainer is already flying the ship. The journal is the
first source anyway (ground-every-fact rule): the Loadout names every slot
the hull has, with its size in the slot name, and the stock-fit module in
each. This knob derives the entry from that and diffs it against the specs
Frontier printed, so a slot the dev log promised and the Loadout lacks (or
the reverse) is a row, not a surprise.

    python docs/benches/knobs/new_hull_from_loadout.py <Loadout.json> [--expect anacondamkii] [--stock] [--write]

--stock: this Loadout is the ship as sold (written at the shipyard before
any outfitting), so its modules become the table's stock fit; without it
`stock` stays empty rather than recording the fit as flown.

Without --write the entry is printed; with it, the hull is added to
ship_slots.json (replacing an entry with the same symbol) and the Rust
tests pin the rest. `Loadout.json` is one journal line (the `ships.raw`
column of the store holds one per owned ship).

Pre-registered expectation for the Anaconda Mk II (Frontier, October 2026
dev log, published 2026-10-14; ships 2026-10-20): core PP 8 / thrusters 7 /
FSD 8 / life support 5 / distributor 8 / sensors 8; internals 1x S1, 1x S2,
1x S3, 2x S4, 5x S5 (2 military), 1x S6 (cargo only), 2x S7, 1x S8;
hardpoints 2 small, 4 large, 1 huge; max cargo 750. Utility count and the
fuel tank size were not published. The hull symbol is not known until the
Loadout says it: pass --expect with whatever `Ship` the journal writes.
"""
import json
import os
import re
import sys
from collections import Counter

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
SLOTS = os.path.join(ROOT, "crates", "ed-ships", "data", "ship_slots.json")

EXPECTED = {
    "anacondamkii": {  # the symbol EDCD FDevIDs PR #114 published 2026-10-20 (the journal lowercases it)
        "core": {"PowerPlant": 8, "MainEngines": 7, "FrameShiftDrive": 8, "LifeSupport": 5, "PowerDistributor": 8, "Radar": 8},
        "internal": {1: 1, 2: 1, 3: 1, 4: 2, 5: 5, 6: 1, 7: 2, 8: 1},
        "military": 2,
        "cargo_only": {6: 1},
        "hardpoints": {"Small": 2, "Large": 4, "Huge": 1},
        "max_cargo": 750,
        "source": "Frontier October 2026 dev log (2026-10-14): Anaconda Mk II Specs",
    },
}

# The core slots' kinds and the exact-size rule, as the existing table has them.
CORE = {
    "Armour": (["cbh"], False),
    "PowerPlant": (["cpp"], False),
    "MainEngines": (["ct"], False),
    "FrameShiftDrive": (["cfsd", "cfsdo"], False),
    "LifeSupport": (["cls"], True),
    "PowerDistributor": (["cpd"], False),
    "Radar": (["cs"], True),
    "FuelTank": (["cft"], False),
}
HARDPOINT_SIZE = {"Tiny": 0, "Small": 1, "Medium": 2, "Large": 3, "Huge": 4}


def core_size(item):
    """`int_powerplant_size8_class5` -> 8; armour and the like have none."""
    m = re.search(r"_size(\d+)_class", item or "")
    return int(m.group(1)) if m else None


def derive(loadout, stock_fit=False):
    symbol = loadout["Ship"].lower()
    name = loadout.get("Ship_Localised") or loadout["Ship"]
    slots, stock, notes = [], {}, []
    for m in loadout.get("Modules", []):
        slot, item = m["Slot"], (m.get("Item") or "").lower()
        if re.match(r"^(Decal|PaintJob|Bobble|ShipName|ShipID|ShipKit|WeaponColour|EngineColour|VesselVoice|StringLights|Hologram)", slot, re.I):
            continue
        if slot in CORE:
            only, exact = CORE[slot]
            size = core_size(item)
            if size is None and slot != "Armour":
                notes.append(f"{slot}: no size in {item!r}, left 1")
            slots.append({"slot": slot, "group": "core", "size": size if size is not None else 1, "only": only, **({"exact_size": True} if exact else {})})
        elif (hm := re.match(r"^(Tiny|Small|Medium|Large|Huge)Hardpoint\d+$", slot)):
            group = "utility" if hm.group(1) == "Tiny" else "hardpoint"
            slots.append({"slot": slot, "group": group, "size": HARDPOINT_SIZE[hm.group(1)]})
        elif (im := re.match(r"^Slot\d+_Size(\d+)$", slot)):
            slots.append({"slot": slot, "group": "internal", "size": int(im.group(1))})
        elif re.match(r"^Military\d+$", slot):
            size = core_size(item)
            if size is None:
                notes.append(f"{slot}: military slot size unknown from {item!r} (fit a module to learn it)")
            slots.append({"slot": slot, "group": "military", "size": size or 0})
        elif re.match(r"^Cargo\d+$", slot):
            # The cargo-only slot (Panther Clipper Mk II, Anaconda Mk II): racks
            # and fuel tanks only; its size from the fitted rack, else unknown.
            size = core_size(item)
            if size is None:
                notes.append(f"{slot}: cargo slot size unknown from {item!r} (fit a rack to learn it)")
            slots.append({"slot": slot, "group": "internal", "size": size or 0, "only": ["cft", "icr"]})
        elif slot == "PlanetaryApproachSuite":
            # The table keeps it as a size-1 internal that takes only the suite.
            slots.append({"slot": slot, "group": "internal", "size": 1, "only": ["ipas"]})
        elif slot in ("CargoHatch", "DataLinkScanner", "CodexScanner", "DiscoveryScanner", "Fighter", "ColonisationSuite", "ShipCockpit"):
            continue
        else:
            notes.append(f"{slot}: unrecognised slot, skipped ({item!r})")
            continue
        # The table's `stock` is the fit as sold. Only a Loadout written at
        # the shipyard, before any outfitting, is that (--stock says so).
        if item and stock_fit:
            stock[slot] = item
    # Empty internals never appear in a Loadout: the dev log's count is the
    # check for those (reported in the diff, never invented).
    return {"name": name, "slots": slots, "stock": stock}, symbol, notes


def diff(entry, expected):
    rows = []
    sizes = {s["slot"]: s["size"] for s in entry["slots"]}
    for slot, want in expected.get("core", {}).items():
        have = sizes.get(slot)
        rows.append(("core", slot, want, have, "same" if have == want else ("MISSING" if have is None else "DIFFER")))
    have_int = Counter(s["size"] for s in entry["slots"] if s["group"] == "internal")
    for size, want in sorted(expected.get("internal", {}).items()):
        have = have_int.get(size, 0)
        rows.append(("internal", f"size {size}", want, have, "same" if have == want else ("fewer in Loadout: empty slots are not written, check the outfitting screen" if have < want else "DIFFER")))
    mil = sum(1 for s in entry["slots"] if s["group"] == "military")
    rows.append(("military", "count", expected.get("military"), mil, "same" if mil == expected.get("military") else "DIFFER (empty military slots are not written)"))
    have_hp = Counter(re.match(r"^(Small|Medium|Large|Huge)", s["slot"]).group(1) for s in entry["slots"] if s["group"] == "hardpoint")
    for kind, want in expected.get("hardpoints", {}).items():
        have = have_hp.get(kind, 0)
        rows.append(("hardpoint", kind, want, have, "same" if have == want else "DIFFER (empty hardpoints are not written)"))
    util = sum(1 for s in entry["slots"] if s["group"] == "utility")
    rows.append(("utility", "count", "unpublished", util, "from the Loadout"))
    return rows


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    expect = sys.argv[sys.argv.index("--expect") + 1] if "--expect" in sys.argv else None
    write = "--write" in sys.argv
    loadout = json.load(open(args[0], encoding="utf-8"))
    entry, symbol, notes = derive(loadout, stock_fit="--stock" in sys.argv)
    print(f"# {symbol}: {entry['name']}; {len(entry['slots'])} slots from the Loadout of {loadout.get('timestamp')}; UnladenMass {loadout.get('UnladenMass')} CargoCapacity {loadout.get('CargoCapacity')} MaxJumpRange {loadout.get('MaxJumpRange')} FuelCapacity {json.dumps(loadout.get('FuelCapacity'))}")
    for n in notes:
        print(f"# note: {n}")
    exp = EXPECTED.get((expect or symbol).lower())
    if exp:
        print(f"# expected: {exp['source']}")
        print("group,slot,expected,loadout,verdict")
        for r in diff(entry, exp):
            print(",".join(str(x) for x in r))
        cargo = loadout.get("CargoCapacity")
        print(f"cargo,fitted,{exp.get('max_cargo')} max,{cargo},as fitted (max needs every internal a cargo rack)")
    else:
        print("# no pre-registered expectation for this hull; the entry stands on the Loadout alone")
    if write:
        table = json.load(open(SLOTS, encoding="utf-8"))
        table[symbol] = entry
        with open(SLOTS, "w", encoding="utf-8", newline="\n") as f:
            json.dump(table, f, indent=1, ensure_ascii=False, sort_keys=True)
            f.write("\n")
        print(f"# written: {symbol} into {os.path.relpath(SLOTS, ROOT)}", file=sys.stderr)
    else:
        print("# entry (not written; pass --write):", file=sys.stderr)
        print(json.dumps(entry, ensure_ascii=False), file=sys.stderr)


if __name__ == "__main__":
    main()
