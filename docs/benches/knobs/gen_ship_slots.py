"""Generate the ship slot table and the module kind table (facts of the game's
outfitting), and pin them against the vendored Coriolis sizes and the
commander's own journal Loadouts.

Sources: the facts are Frontier's (which slots a hull has, their journal
names and sizes, which module types each takes, which modules are bound to
a hull). EDSY (edsy.org) is the reference these facts are compiled from,
the way its exports carry the journal's own slot names; no EDSY code or
file is vendored — point this script at a local copy of its data,
converted to JSON (see docs/benches/knobs/eddb_to_json.js).

Outputs (checked in):
  crates/ed-ships/data/ship_slots.json    per hull: slots with journal name, size, what fits
  crates/ed-ships/data/module_kinds.json  per module symbol: kind, class, hull binding, one-per-ship limit
Pin (stdout, CSV): sizes vs Coriolis; slot names vs the store's Loadouts.

Usage:
  node docs/benches/knobs/eddb_to_json.js <eddb.js> <eddb.json>
  python docs/benches/knobs/gen_ship_slots.py <eddb.json> [edda.sqlite3] > docs/benches/<date>-ship-slots-pin.csv
"""
import csv
import glob
import json
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
OUT_SLOTS = os.path.join(ROOT, "crates", "ed-ships", "data", "ship_slots.json")
OUT_KINDS = os.path.join(ROOT, "crates", "ed-ships", "data", "module_kinds.json")
CORIOLIS = os.path.join(ROOT, "crates", "ed-ships", "data", "coriolis", "ships", "*.json")
SHIPYARD = os.path.join(ROOT, "crates", "ed-journal", "data", "shipyard.csv")

# Journal names for the core slots, in the reference's component order
# (index 0 is the bulkhead).
CORE = ["Armour", "PowerPlant", "MainEngines", "FrameShiftDrive", "LifeSupport", "PowerDistributor", "Radar", "FuelTank"]
HP_WORD = {1: "Small", 2: "Medium", 3: "Large", 4: "Huge"}
# What each slot family takes, by module kind (the reference's group table).
GROUP_KINDS = {
    "hardpoint": "hel hul hc hex hexxm hexxc hexgg hexgp hexgs hexsc hextp hfc hm hmtl hmtm hmr hmc hpa hpl hrg htp".split(),
    "utility": "ucl uec uex uhsl ukws ucs upd upwa usb ufsws".split(),
    "military": "ihrp isrp imahrp imrp iscb".split(),
    "internal": "iafmu icr iclc idlc iex ifh ifa ifsdb ifsdi ifs cft iftlc ihblc ihrp isrp imahrp imrp imlc ipc ipvh iplc inlc ir irlc islc iscb isg iss".split(),
}
CORE_KINDS = [["cbh"], ["cpp"], ["ct"], ["cfsd", "cfsdo"], ["cls"], ["cpd"], ["cs"], ["cft"]]
# Life support and sensors must be exactly the slot's size; everything else may be smaller.
EXACT_SIZE = {"LifeSupport", "Radar"}


def main():
    eddb = json.load(open(sys.argv[1], encoding="utf-8"))
    db = sys.argv[2] if len(sys.argv) > 2 else os.path.join(os.environ.get("LOCALAPPDATA", ""), "edda", "edda.sqlite3")
    ships = eddb["ship"]
    modules = eddb["module"]
    limits = eddb["limit"]
    id_to_fd = {mid: m["fdname"] for mid, m in modules.items() if m.get("fdname")}
    shipid_to_fd = {sid: s["fdname"] for sid, s in ships.items()}

    # ---- module kinds
    kinds = {}
    for mid, m in modules.items():
        fd = m.get("fdname")
        if not fd or m.get("hidden"):
            continue
        k = {"kind": m["mtype"], "class": m.get("class", 0), "rating": m.get("rating", ""), "name": m.get("name", "")}
        if m.get("mount"):
            k["mount"] = {"F": "fixed", "G": "gimballed", "T": "turreted"}.get(m["mount"], m["mount"])
        if m.get("reserved"):
            k["ships"] = sorted(shipid_to_fd[str(s)].lower() for s in m["reserved"] if str(s) in shipid_to_fd)
        if m.get("limit") and m["limit"] in limits:
            k["limit"] = limits[m["limit"]]
        elif m["mtype"] in limits:
            k["limit"] = limits[m["mtype"]]
        if m.get("noundersize"):
            k["exact_size"] = True
        kinds[fd.lower()] = k

    # ---- ship slots
    out = {}
    for sid, s in ships.items():
        slots = s["slots"]
        names = s.get("slotnames", {})
        reserved = s.get("reserved", {})
        entry = {"name": s["name"], "slots": []}

        def at(lst, i):
            return lst[i] if lst and i < len(lst) else None

        def add(group, i, size, default_name):
            name = at(names.get(group), i) or default_name
            only = at(reserved.get(group), i)
            slot = {"slot": name, "group": group, "size": size}
            if only:
                slot["only"] = sorted(only)
            entry["slots"].append(slot)

        # hardpoints, numbered per size word in order of appearance
        counters = {}
        for i, size in enumerate(slots["hardpoint"]):
            word = HP_WORD[size]
            counters[word] = counters.get(word, 0) + 1
            add("hardpoint", i, size, f"{word}Hardpoint{counters[word]}")
        for i, _ in enumerate(slots["utility"]):
            add("utility", i, 0, f"TinyHardpoint{i + 1}")
        for i, size in enumerate(slots["component"]):
            slot = {"slot": CORE[i], "group": "core", "size": size, "only": CORE_KINDS[i]}
            if CORE[i] in EXACT_SIZE:
                slot["exact_size"] = True
            entry["slots"].append(slot)
        for i, size in enumerate(slots["military"]):
            add("military", i, size, f"Military{i + 1:02d}")
        for i, size in enumerate(slots["internal"]):
            add("internal", i, size, f"Slot{i + 1:02d}_Size{size}")
        # Every hull has the Planetary Approach Suite slot (the reference folds it away).
        entry["slots"].append({"slot": "PlanetaryApproachSuite", "group": "internal", "size": 1, "only": ["ipas"]})

        # stock fit, by journal symbol
        stock = {}
        st = s.get("stock", {})
        groups = [("hardpoint", slots["hardpoint"]), ("utility", slots["utility"]), ("component", slots["component"]), ("military", slots["military"]), ("internal", slots["internal"])]
        for group, sizes in groups:
            g = "core" if group == "component" else group
            slot_names = [x["slot"] for x in entry["slots"] if x["group"] == g and x["slot"] != "PlanetaryApproachSuite"]
            for i, mid in enumerate(st.get(group, [])):
                if not mid or i >= len(slot_names):
                    continue
                fd = id_to_fd.get(str(mid)) or (s.get("module", {}).get(str(mid), {}).get("fdname"))
                if fd:
                    stock[slot_names[i]] = fd
        entry["stock"] = stock
        out[s["fdname"].lower()] = entry

    # The Planetary Approach Suite is its own module kind in the journal.
    kinds.setdefault("int_planetapproachsuite", {"kind": "ipas", "class": 1, "rating": "I", "name": "Planetary Approach Suite"})

    json.dump(out, open(OUT_SLOTS, "w", encoding="utf-8", newline="\n"), indent=1, sort_keys=True)
    json.dump(kinds, open(OUT_KINDS, "w", encoding="utf-8", newline="\n"), indent=1, sort_keys=True)

    # ---- pin 1: sizes against the vendored Coriolis ship data
    cor = {}
    for f in glob.glob(CORIOLIS):
        d = json.load(open(f, encoding="utf-8"))
        for _, ship in d.items():
            cor[ship["properties"]["name"].lower()] = ship["slots"]
    print("# Pin: our slot table against the vendored Coriolis sizes (hardpoints, internal, military, core) and against the commander's own Loadouts (every fitted slot named in the table). Coriolis folds the Type-11's limpet/hangar slots and the Panther's cargo slots away; ours keeps them, so those two internal rows DIFFER by design.")
    print("check,ship,ours,reference,verdict")
    for fd, e in sorted(out.items()):
        c = cor.get(e["name"].lower())
        if c is None:
            print(f"coriolis,{e['name']},,,no Coriolis data")
            continue
        ours_hp = sorted(x["size"] for x in e["slots"] if x["group"] == "hardpoint")
        cor_hp = sorted(x if isinstance(x, int) else x["class"] for x in c["hardpoints"] if (x if isinstance(x, int) else x["class"]) > 0)
        ours_int = sorted(x["size"] for x in e["slots"] if x["group"] == "internal" and x["slot"] != "PlanetaryApproachSuite")
        cor_int = sorted(x for x in c["internal"] if isinstance(x, int))
        ours_mil = sorted(x["size"] for x in e["slots"] if x["group"] == "military")
        cor_mil = sorted(x["class"] for x in c["internal"] if isinstance(x, dict) and x.get("name") == "Military")
        ours_core = [x["size"] for x in e["slots"] if x["group"] == "core"][1:]
        for label, a, b in [("hardpoints", ours_hp, cor_hp), ("internal", ours_int, cor_int), ("military", ours_mil, cor_mil), ("core", ours_core, list(c["standard"]))]:
            print(f"{label},{e['name']},{'/'.join(map(str, a))},{'/'.join(map(str, b))},{'agree' if a == b else 'DIFFER'}")

    # ---- pin 2: slot names against the commander's own Loadouts
    if os.path.exists(db):
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        for symbol, raw in con.execute("SELECT ship, raw FROM ships ORDER BY ship_id"):
            e = out.get((symbol or "").lower())
            if e is None:
                print(f"journal,{symbol},,,no slot table")
                continue
            ours = {x["slot"].lower() for x in e["slots"]}
            v = json.loads(raw)
            # Cosmetic and fixed slots are not outfitting: the cockpit, cargo hatch,
            # hologram, scanners, paint and kit.
            journal = {m["Slot"] for m in v.get("Modules", []) if not re.match(r"(?i)(decal|paintjob|shipkit|bobble|weaponcolour|enginecolour|vesselvoice|shipname|shipid|string|datalinkscanner|codexscanner|discoveryscanner|cargohatch|shipcockpit|hologram)", m["Slot"])}
            missing = sorted(j for j in journal if j.lower() not in ours)
            print(f"journal,{e['name']},{len(journal)} fitted slots named,{len(missing)} not in table{(': ' + ' '.join(missing)) if missing else ''},{'agree' if not missing else 'DIFFER'}")


if __name__ == "__main__":
    main()
