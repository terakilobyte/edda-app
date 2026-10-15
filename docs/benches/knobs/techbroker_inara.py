#!/usr/bin/env python3
"""Diff EDDA's vendored technology-broker recipes against an Inara dump.

Inara blocks server-side fetches (HTTP 503 to curl and to WebFetch), so the
dump is the page text of https://inara.cz/elite/techbroker/ with all three
tabs (HUMAN, GUARDIAN MODULES, GUARDIAN WEAPONS) expanded, copied out of a
browser. The parenthesised numbers after each ingredient are the viewing
commander's own stock and are ignored. Recipe names are matched by a hand
table (Inara's labels are not the game's); ingredient names are compared in
EDDA's namespace (FDevIDs `material.csv`: "... Blueprint Segment", where
Inara prints "Fragment").

    python docs/benches/knobs/techbroker_inara.py <dump.txt> [blueprints.json] > diff.csv

Verdict rows: `missing` (Inara has a recipe EDDA lacks), `extra` (EDDA has
one Inara lacks), `qty` (same recipe, a quantity differs), `ingredient` (an
ingredient present on one side only), `same`. The CSV is the record
(ledger, 2026-10-14: "we seem to be missing some data").
"""
import json
import re
import sys

ALIAS = {
    # Inara label (upper-cased, bracketed class kept) -> EDDA recipe Name
    "SIRIUS AX MISSILE RACK [FIXED LARGE]": "Sirius Modified AX Missile Rack (Large)",
    "SIRIUS AX MISSILE RACK [FIXED MEDIUM]": "Sirius Modified AX Missile Rack (Medium)",
    "AZIMUTH ENHANCED AX MULTI-CANNON [GIMBALLED LARGE]": "Azimuth Modified Enhanced AX Multi-Cannon (Gimballed, Large)",
    "AZIMUTH ENHANCED AX MULTI-CANNON [GIMBALLED MEDIUM]": "Azimuth Modified Enhanced AX Multi-Cannon (Gimballed, Medium)",
    "BOBBLEHEAD": "Thargoid Bobblehead",
    "CORROSION RESISTANT CARGO RACK": "Corrosion Resistant Cargo Rack (2D and 4D)",
    "ENGINEERED DETAILED SURFACE SCANNER": "Engineered Detailed Surface Scanner V1",
    "ENZYME MISSILE RACK [FIXED MEDIUM]": "Enzyme Missile Rack",
    "ENGINEERED FRAME SHIFT DRIVE": "Engineered FSD V1",
    "ENGINEERED FRAME SHIFT DRIVE (SCO) [7A]": "Engineered FSD (SCO) V1 (Class 7)",
    "ENGINEERED FRAME SHIFT DRIVE (SCO) [6A]": "Engineered FSD (SCO) V1 (Class 6)",
    "ENGINEERED FRAME SHIFT DRIVE (SCO) [5A]": "Engineered FSD (SCO) V1 (Class 5)",
    "ENGINEERED FRAME SHIFT DRIVE (SCO) [4A]": "Engineered FSD (SCO) V1 (Class 4)",
    "ENGINEERED FRAME SHIFT DRIVE (SCO) [3A]": "Engineered FSD (SCO) V1 (Class 3)",
    "ENGINEERED FRAME SHIFT DRIVE (SCO) [2A]": "Engineered FSD (SCO) V1 (Class 2)",
    "SIRIUS HEAT SINK LAUNCHER": "Sirius Modified Heat Sink Launcher",
    "META ALLOY HULL REINFORCEMENT": "Meta Alloy Hull Reinforcement",
    "MODIFIED MINING LASER [FIXED SMALL]": "Modified Mining Laser",
    "REMOTE RELEASE FLECHETTE LAUNCHER [FIXED MEDIUM]": "Remote Release Flechette Launcher (Fixed)",
    "REMOTE RELEASE FLECHETTE LAUNCHER [TURRETED MEDIUM]": "Remote Release Flechette Launcher (Turreted)",
    "ENGINEERED SEEKER MISSILE RACK [FIXED MEDIUM]": "Engineered Missile Rack V1",
    "GUARDIAN FSD BOOSTER": "Guardian FSD Booster",
    "GUARDIAN HYBRID POWER DISTRIBUTOR": "Guardian Power Distributor",
    "GUARDIAN POWER PLANT": "Guardian Hybrid Power Plant",
    "XG7 TRIDENT GUARDIAN SHIP LAUNCHED FIGHTER": "Trident (Fighter)",
    "XG8 JAVELIN GUARDIAN SHIP LAUNCHED FIGHTER": "Javelin (Fighter)",
    "XG9 LANCE GUARDIAN SHIP LAUNCHED FIGHTER": "Lance (Fighter)",
}
INGREDIENT = {
    "Guardian Module Blueprint Fragment": "Guardian Module Blueprint Segment",
    "Guardian Weapon Blueprint Fragment": "Guardian Weapon Blueprint Segment",
    "Guardian Vessel Blueprint Fragment": "Guardian Vessel Blueprint Segment",
    "Meta-Alloys": "Meta-alloys",
    "Micro-weave Cooling Hoses": "Micro-Weave Cooling Hoses",
}
MOUNT = {"FIXED": "Fixed", "GIMBALLED": "Gimballed", "TURRETED": "Turreted"}
SIZE = {"SMALL": "Small", "MEDIUM": "Medium", "LARGE": "Large"}


def edda_name(label):
    """Inara's upper-case label -> EDDA's recipe name."""
    bare = re.sub(r"\s*\[\d[A-I]\]$", "", label).strip()
    if bare in ALIAS:
        return ALIAS[bare]
    if label in ALIAS:
        return ALIAS[label]
    m = re.match(r"^(.*?)\s*\[(FIXED|GIMBALLED|TURRETED) (SMALL|MEDIUM|LARGE)\]$", bare)
    if m:
        base = m.group(1).title().replace("Guardian ", "Guardian ")
        base = re.sub(r"^Modified Guardian ", "Modified ", base)
        return f"{base} ({MOUNT[m.group(2)]}, {SIZE[m.group(3)]})"
    return bare.title()


def parse(text):
    items, cur = [], None
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line in ("HUMAN", "GUARDIAN MODULES", "GUARDIAN WEAPONS"):
            continue
        if line in ("Purchasable", "LOCKED", "UNLOCKED"):
            cur = {"status": line, "ingredients": {}}
            items.append(cur)
            continue
        # "20︎ Mechanical Equipment (130)" for a material (an icon glyph
        # after the count), "16Meta-Alloys (0)" for a commodity (no gap).
        # The trailing "(n)" is the viewer's stock; the pinned dump has it stripped.
        m = re.match(r"^(\d+)[︎\s]*(.+?)\s*(?:\(\d+\))?$", line)
        if m and cur is not None and "label" in cur:
            name = INGREDIENT.get(m.group(2), m.group(2))
            cur["ingredients"][name] = int(m.group(1))
        elif cur is not None and "label" not in cur:
            cur["label"] = line
            cur["edda"] = edda_name(line)
    return items


def main():
    dump = open(sys.argv[1], encoding="utf-8").read()
    bp_path = sys.argv[2] if len(sys.argv) > 2 else "crates/ed-engineering/data/blueprints.json"
    bps = json.load(open(bp_path, encoding="utf-8"))
    if isinstance(bps, dict):
        bps = bps.get("blueprints") or list(bps.values())
    edda = {
        b["Name"]: {i["Name"]: i["Size"] for i in b["Ingredients"]}
        for b in bps
        if b.get("Type") in ("Guardian", "Human") and b.get("Grade") is None
    }
    inara = parse(dump)
    rows = []
    seen = set()
    for it in inara:
        name = it["edda"]
        seen.add(name)
        if name not in edda:
            rows.append(("missing", name, it["label"], "", "", json.dumps(it["ingredients"])))
            continue
        ours, theirs = edda[name], it["ingredients"]
        diffs = []
        for ing in sorted(set(ours) | set(theirs)):
            a, b = ours.get(ing), theirs.get(ing)
            if a is None or b is None:
                rows.append(("ingredient", name, it["label"], ing, a if a is not None else "", b if b is not None else ""))
                diffs.append(ing)
            elif a != b:
                rows.append(("qty", name, it["label"], ing, a, b))
                diffs.append(ing)
        if not diffs:
            rows.append(("same", name, it["label"], "", "", ""))
    for name in sorted(set(edda) - seen):
        rows.append(("extra", name, "", "", json.dumps(edda[name]), ""))
    counts = {}
    for r in rows:
        counts[r[0]] = counts.get(r[0], 0) + 1
    print("# techbroker: EDDA blueprints.json vs Inara dump; verdict counts: " + ", ".join(f"{k}={v}" for k, v in sorted(counts.items())))
    print("verdict,edda_name,inara_label,ingredient,edda_qty,inara_qty")
    for r in rows:
        print(",".join(f'"{x}"' if "," in str(x) else str(x) for x in r))


if __name__ == "__main__":
    main()
