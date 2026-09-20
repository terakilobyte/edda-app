"""Pin: the vendored ship data's slot arrays against real journal Loadouts.

For every ship in the commander's store (edda.sqlite3, table `ships`), the
journal names each slot with its size (`Slot03_Size6`, `Military01`,
`MediumHardpoint2`, `TinyHardpoint1`). The vendored ship JSON lists
`slots.internal`, `slots.hardpoints` and `slots.standard` as ordered arrays
with no names. A swap picker needs the two to agree, slot by slot: internal
sizes in slot-number order (military slots in their place), hardpoints per
size, core max classes at or above what is fitted.

Usage: python docs/benches/knobs/slot_order_pin.py [path/to/edda.sqlite3] > docs/benches/<date>-slot-order-pin.csv
"""
import glob
import json
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
SHIPS = os.path.join(ROOT, "crates", "ed-ships", "data", "coriolis", "ships", "*.json")
NAMES = os.path.join(ROOT, "crates", "ed-journal", "data", "shipyard.csv")

CORE = ["PowerPlant", "MainEngines", "FrameShiftDrive", "LifeSupport", "PowerDistributor", "Radar", "FuelTank"]
HP = {"Tiny": 0, "Small": 1, "Medium": 2, "Large": 3, "Huge": 4}


def coriolis():
    out = {}
    for f in glob.glob(SHIPS):
        d = json.load(open(f, encoding="utf-8"))
        for _, ship in d.items():
            out[ship["properties"]["name"].lower()] = ship["slots"]
    return out


def display_names():
    import csv
    m = {}
    for r in csv.DictReader(open(NAMES, encoding="utf-8")):
        m[r["symbol"].lower()] = re.sub(r"Mk(?=[IVX])", "Mk ", r["name"])
    return m


def journal_slots(raw):
    v = json.loads(raw)
    internal, military, hard, core = [], [], {}, {}
    for m in v.get("Modules", []):
        slot = m["Slot"]
        item = m["Item"].lower()
        if (g := re.match(r"Slot(\d+)_Size(\d+)$", slot)):
            internal.append((int(g.group(1)), int(g.group(2))))
        elif (g := re.match(r"Military(\d+)$", slot)):
            military.append(int(g.group(1)))
        elif (g := re.match(r"(Tiny|Small|Medium|Large|Huge)Hardpoint(\d+)$", slot)):
            # Only FITTED hardpoints appear in a Loadout, so the journal's count
            # per size is a lower bound on the ship's mounts.
            hard[HP[g.group(1)]] = hard.get(HP[g.group(1)], 0) + 1
        elif slot in CORE:
            g = re.search(r"size(\d+)_class", item)
            core[slot] = int(g.group(1)) if g else None
    # Likewise only fitted internals appear: compare by slot NUMBER, not by run.
    return dict(internal), len(military), hard, core


def main():
    db = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.environ.get("LOCALAPPDATA", ""), "edda", "edda.sqlite3")
    cor = coriolis()
    names = display_names()
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    print("ship,journal_internal,coriolis_internal,internal_agree,journal_military,coriolis_military,journal_hardpoints,coriolis_hardpoints,hardpoints_fit,core_within_max,verdict")
    for symbol, raw in con.execute("SELECT ship, raw FROM ships ORDER BY ship_id"):
        name = names.get((symbol or "").lower(), symbol)
        slots = cor.get((name or "").lower())
        if slots is None:
            print(f"{name},,,,,,,,,,no vendored data")
            continue
        j_int, j_mil, j_hard, j_core = journal_slots(raw)
        # Coriolis: internal array in slot order; military entries are objects with name Military
        c_int = [s if isinstance(s, int) else None for s in slots["internal"] if isinstance(s, int) or s.get("name") == "Military"]
        c_int_plain = [s for s in slots["internal"] if isinstance(s, int)]
        c_mil = sum(1 for s in slots["internal"] if isinstance(s, dict) and s.get("name") == "Military")
        c_hard = {}
        for s in slots["hardpoints"]:
            s = s if isinstance(s, int) else s["class"]
            c_hard[s] = c_hard.get(s, 0) + 1
        # The journal numbers plain internals SlotNN in the vendored order; military slots are apart.
        internal_agree = all(n - 1 < len(c_int_plain) and c_int_plain[n - 1] == size for n, size in j_int.items())
        hard_agree = all(c_hard.get(k, 0) >= n for k, n in j_hard.items())
        core_ok = all(j_core.get(k) is None or j_core[k] <= slots["standard"][i] for i, k in enumerate(CORE))
        verdict = "agree" if internal_agree and hard_agree and j_mil == c_mil and core_ok else "DIFFER"
        print(f"{name},{'/'.join(f"{n}:{s}" for n, s in sorted(j_int.items()))},{'/'.join(map(str, c_int_plain))},{internal_agree},{j_mil},{c_mil},{json.dumps(j_hard).replace(',', ';')},{json.dumps(c_hard).replace(',', ';')},{hard_agree},{core_ok},{verdict}")


if __name__ == "__main__":
    main()
