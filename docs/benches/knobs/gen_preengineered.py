"""Generate crates/ed-ships/data/preengineered.json — every pre-engineered
module a technology broker sells (bought again with every unit, never
unlocked) or a community goal handed out — from sources, not memory
(maintainer, 2026-09-27: "stop making assumptions. You need to ground
everything with a search and confirmation from a reliable source").

Sources, in the order they win:
  1. The commander's own journal: a preset already in the file whose
     modifiers were read from a real Loadout (source "journal") keeps them.
  2. EDCD/coriolis-data (vendored at crates/ed-ships/data/coriolis/): every
     module row carrying `preEngineered` — its blueprints, grade,
     experimental, availability, and the stat ratios against the base row
     of the same symbol (the ratios the physics multiplies our base table
     by, so Coriolis's own ratio is the consistent one).
  3. The community wiki's module pages (fetched 2026-09-27 via the
     MediaWiki API; URLs in WIKI below): ratios only where Coriolis carries
     none, and the purchase rule ("must be unlocked repeatedly ... a
     purchase in exchange for materials, aka a Barter, not an unlock").
  4. The recipe names: crates/ed-engineering/data/blueprints.json, pinned
     against the wiki by tech_broker_pin.py (2026-09-27: every per-unit
     recipe there agrees with its module page; the broker page's own
     Purchases table lists the plain unlock costs and is the one that is
     wrong).

Where two sources disagree and no third decides, the CSV row says OPEN
and the Coriolis ratio is used (it is relative to the base table we use).

Usage:
  python docs/benches/knobs/gen_preengineered.py > docs/benches/<date>-preengineered-pin.csv
"""
import glob
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
CORIOLIS = os.path.join(ROOT, "crates", "ed-ships", "data", "coriolis", "modules")
BLUEPRINTS = os.path.join(ROOT, "crates", "ed-engineering", "data", "blueprints.json")
OUTFITTING = os.path.join(ROOT, "crates", "ed-journal", "data", "outfitting.csv")
OUT = os.path.join(ROOT, "crates", "ed-ships", "data", "preengineered.json")

# Coriolis stat key -> the journal's Modifiers label (ratio on the base).
LABELS = {
    "mass": "Mass", "power": "PowerDraw", "integrity": "Integrity", "range": "MaximumRange", "damage": "Damage",
    "reload": "ReloadTime", "clip": "AmmoClipSize", "ammo": "AmmoMaximum", "distdraw": "DistributorDraw",
    "thermload": "ThermalLoad", "shotspeed": "ShotSpeed", "falloff": "DamageFalloffRange", "piercing": "ArmourPiercing",
    "optmass": "FSDOptimalMass", "maxfuel": "MaxFuelPerJump", "boot": "BootTime",
}
INVERSE = {"fireint": "RateOfFire"}  # a fire interval ratio is the inverse rate-of-fire ratio

# The broker's recipe for a variant, by (symbol, blueprints). Every name is
# checked against blueprints.json below; a missing one is reported, never invented.
RECIPES = {
    ("hpt_guardian_shardcannon_fixed_small", ("Weapon_LongRange", "Weapon_Focused")): "Modified Shard Cannon (Fixed, Small)",
    ("hpt_guardian_shardcannon_fixed_medium", ("Weapon_LongRange", "Weapon_Focused")): "Modified Shard Cannon (Fixed, Medium)",
    ("hpt_guardian_gausscannon_fixed_small", ("Weapon_HighCapacity", "Weapon_RapidFire")): "Modified Gauss Cannon (Fixed, Small)",
    ("hpt_guardian_gausscannon_fixed_medium", ("Weapon_HighCapacity", "Weapon_RapidFire")): "Modified Gauss Cannon (Fixed, Medium)",
    ("hpt_guardian_plasmalauncher_fixed_small", ("Weapon_Overcharged", "Weapon_Focused")): "Modified Plasma Charger (Fixed, Small)",
    ("hpt_guardian_plasmalauncher_fixed_medium", ("Weapon_Overcharged", "Weapon_Focused")): "Modified Plasma Charger (Fixed, Medium)",
    ("hpt_heatsinklauncher_turret_tiny", ("Misc_HeatSinkCapacity",)): "Sirius Modified Heat Sink Launcher",
    ("hpt_atdumbfiremissile_fixed_medium", ("Weapon_HighCapacity", "Weapon_RapidFire")): "Sirius Modified AX Missile Rack (Medium)",
    ("hpt_atdumbfiremissile_fixed_large", ("Weapon_HighCapacity", "Weapon_RapidFire")): "Sirius Modified AX Missile Rack (Large)",
    ("hpt_mininglaser_fixed_small", ("Weapon_LongRange",)): "Modified Mining Laser",
    ("hpt_basicmissilerack_fixed_medium", ("Weapon_HighCapacity", "Weapon_LightWeight")): "Engineered Missile Rack V1",
    ("int_detailedsurfacescanner_tiny", ("Sensor_Expanded",)): "Engineered Detailed Surface Scanner V1",
    ("int_hyperdrive_size5_class5", ("FSD_LongRange", "FSD_FastBoot")): "Engineered FSD V1",
    ("int_hyperdrive_overcharge_size2_class5", ("FSD_LongRange", "FSD_FastBoot")): "Engineered FSD (SCO) V1 (Class 2)",
    ("int_hyperdrive_overcharge_size3_class5", ("FSD_LongRange", "FSD_FastBoot")): "Engineered FSD (SCO) V1 (Class 3)",
    ("int_hyperdrive_overcharge_size4_class5", ("FSD_LongRange", "FSD_FastBoot")): "Engineered FSD (SCO) V1 (Class 4)",
    ("int_hyperdrive_overcharge_size5_class5", ("FSD_LongRange", "FSD_FastBoot")): "Engineered FSD (SCO) V1 (Class 5)",
    ("int_hyperdrive_overcharge_size6_class5", ("FSD_LongRange", "FSD_FastBoot")): "Engineered FSD (SCO) V1 (Class 6)",
    ("int_hyperdrive_overcharge_size7_class5", ("FSD_LongRange", "FSD_FastBoot")): "Engineered FSD (SCO) V1 (Class 7)",
}

# Ratios from the wiki's module pages, used only where Coriolis carries none
# for the label. Each entry names its page.
WIKI = {
    ("hpt_guardian_shardcannon_fixed_small", ("Weapon_LongRange", "Weapon_Focused")): ("https://elite-dangerous.fandom.com/wiki/Guardian_Shard_Cannon#Modified_Shard_Cannon", {
        "Mass": 1.5, "PowerDraw": 2.3, "DistributorDraw": 5.277, "ThermalLoad": 16.5, "ArmourPiercing": 2.02, "MaximumRange": 1.765, "ShotSpeed": 5.558, "DamageFalloffRange": 0.882}),
    ("hpt_guardian_shardcannon_fixed_medium", ("Weapon_LongRange", "Weapon_Focused")): ("https://elite-dangerous.fandom.com/wiki/Guardian_Shard_Cannon#Modified_Shard_Cannon", {
        "Mass": 1.5, "PowerDraw": 2.3, "DistributorDraw": 5.277, "ThermalLoad": 16.5, "ArmourPiercing": 2.02, "MaximumRange": 1.765, "ShotSpeed": 5.558, "DamageFalloffRange": 0.882}),
    ("hpt_atdumbfiremissile_fixed_medium", ("Weapon_HighCapacity", "Weapon_RapidFire")): ("https://elite-dangerous.fandom.com/wiki/AX_Missile_Rack#Sirius_AX_Missile_Racks", {
        "AmmoClipSize": 2.0, "AmmoMaximum": 2.0, "RateOfFire": 1.429, "ReloadTime": 0.55, "DistributorDraw": 0.8, "Damage": 0.97, "Mass": 1.7, "PowerDraw": 1.2}),
    ("hpt_atdumbfiremissile_fixed_large", ("Weapon_HighCapacity", "Weapon_RapidFire")): ("https://elite-dangerous.fandom.com/wiki/AX_Missile_Rack#Sirius_AX_Missile_Racks", {
        "AmmoClipSize": 2.0, "AmmoMaximum": 2.0, "RateOfFire": 1.429, "ReloadTime": 0.55, "DistributorDraw": 0.8, "Damage": 0.97, "Mass": 1.7, "PowerDraw": 1.2}),
    ("hpt_mininglaser_fixed_small", ("Weapon_LongRange",)): ("https://elite-dangerous.fandom.com/wiki/Mining_Laser#Modified_Mining_Laser", {
        "Integrity": 0.5, "Damage": 0.95, "PowerDraw": 0.5, "DistributorDraw": 0.5, "ThermalLoad": 0.51, "MaximumRange": 5.0, "DamageFalloffRange": 8.33}),
    ("hpt_heatsinklauncher_turret_tiny", ("Misc_HeatSinkCapacity",)): ("https://elite-dangerous.fandom.com/wiki/Heatsink_Launcher#Sirius_Heatsink_Launcher", {
        "AmmoMaximum": 5 / 3, "ReloadTime": 1.75}),
}


def edcd_names():
    """symbol -> (name with class and rating as ed_journal::modules::item_name prints it, the mount word)."""
    import csv
    out = {}
    with open(OUTFITTING, encoding="utf-8") as f:
        for r in csv.DictReader(f):
            mount = {"Fixed": " (fixed)", "Gimballed": " (gimballed)", "Turreted": " (turreted)"}.get(r["mount"], "")
            name = r["name"].strip() if not r["class"] else f"{r['name'].strip()} {r['class']}{r['rating']}{mount}"
            out.setdefault(r["symbol"].lower(), (name, r["mount"]))
    return out


def coriolis_rows():
    rows = []
    for f in sorted(glob.glob(os.path.join(CORIOLIS, "**", "*.json"), recursive=True)):
        d = json.load(open(f, encoding="utf-8"))
        items = d if isinstance(d, list) else (d.get(next(iter(d))) if isinstance(d, dict) else [])
        if not isinstance(items, list):
            continue
        base = {}
        for m in items:
            if isinstance(m, dict) and "symbol" in m and not m.get("preEngineered"):
                base.setdefault(m["symbol"].lower(), m)
        for m in items:
            if isinstance(m, dict) and m.get("preEngineered"):
                b = base.get(m["symbol"].lower(), {})
                ratios = {}
                for k, v in m.items():
                    if isinstance(v, (int, float)) and isinstance(b.get(k), (int, float)) and b[k] and abs(v / b[k] - 1) > 1e-6:
                        if k in LABELS:
                            ratios[LABELS[k]] = round(v / b[k], 6)
                        elif k in INVERSE:
                            ratios[INVERSE[k]] = round(b[k] / v, 6)
                rows.append((os.path.relpath(f, ROOT).replace(os.sep, "/"), m, ratios))
    return rows


def main():
    recipes = {}
    for b in json.load(open(BLUEPRINTS, encoding="utf-8")):
        if b.get("Type") in ("Guardian", "Human") and b.get("Grade") is None:
            recipes[b["Name"]] = b
    existing = {}
    if os.path.exists(OUT):
        for p in json.load(open(OUT, encoding="utf-8")):
            existing[(p["item"].lower(), p["blueprint"].lower(), p["level"])] = p
    names = edcd_names()
    print("# Pin: crates/ed-ships/data/preengineered.json generated from EDCD/coriolis-data preEngineered rows; recipes from blueprints.json (pinned to the wiki by tech_broker_pin.py); modifiers from the journal, else Coriolis, else the wiki. OPEN: the two references disagree on a figure and no third decides; Coriolis's ratio is used because it is relative to the base table the physics uses.")
    print("symbol,blueprints,name,recipe,modifiers,verdict")
    out = []
    for path, m, ratios in coriolis_rows():
        symbol = m["symbol"].lower()
        pe = m["preEngineered"]
        blueprints = tuple(b for b in pe.get("blueprints", []) if b)
        grade = int(pe.get("grade") or 1)
        experimental = next((e for e in pe.get("experimentalEffects", []) if e), None)
        cg = str(pe.get("availability", "")).upper() == "CG"
        key = (symbol, blueprints)
        recipe_name = RECIPES.get(key)
        recipe = recipes.get(recipe_name) if recipe_name else None
        broker = "community goal" if cg else (recipe["Type"].lower() if recipe else "unknown")
        modifiers = dict(ratios)
        sources = [f"coriolis-data {path}"]
        wiki = WIKI.get(key)
        open_ = []
        if wiki:
            url, w = wiki
            for label, r in w.items():
                if label in modifiers:
                    if abs(modifiers[label] - r) > 0.05 * max(abs(r), 1e-9):
                        open_.append(f"{label}: coriolis {modifiers[label]:.4g} vs wiki {r:.4g}")
                else:
                    modifiers[label] = round(r, 6)
            sources.append(f"wiki {url}")
        prev = existing.get((symbol, (blueprints[0] if blueprints else "").lower(), grade))
        pid = prev["id"] if prev and prev.get("id", "").startswith("sco_v1_") else f"pe:{symbol}:{'+'.join(b.lower() for b in blueprints)}:{grade}"
        if prev and prev.get("modifiers") and "journal" in str(prev.get("source", "journal")):
            modifiers = dict(prev["modifiers"])
            sources.insert(0, prev.get("source") or "journal: a real Loadout")
        plain, _mount = names.get(symbol, (symbol, ""))
        # Short: the recipe's name with the module's class and rating in
        # place of its "(Fixed, Medium)", else Coriolis's label; the
        # engineering in words goes in `description`.
        label = recipe_name or m.get("name", "pre-engineered")
        cr = plain.split(" ")[-2] if plain.endswith(")") and len(plain.split(" ")) > 2 else plain.split(" ")[-1]
        short = re.sub(r"\s*\((Fixed|Gimballed|Turreted)(, \w+)?\)", lambda mm: f" {cr} ({mm.group(1).lower()})", label) if recipe_name else label
        desc = pe.get("description", "").replace("This module has been pre-engineered with ", "").rstrip(".")
        preset = {
            "id": pid,
            "item": symbol,
            "name": f"{short} · pre-engineered {plain}",
            "description": desc or None,
            "broker": broker,
            "blueprint": blueprints[0] if blueprints else "",
            "blueprints": list(blueprints),
            "level": grade,
            "quality": 1.0,
            "experimental": experimental,
            "modifiers": modifiers,
            "unlock": recipe_name if recipe else None,
            "per_unit": not cg,
            "source": "; ".join(sources) + ("; OPEN: " + " / ".join(open_) if open_ else ""),
        }
        out.append(preset)
        verdict = "OPEN" if open_ else ("agree" if (recipe or cg) else "DIFFER")
        note = "community goal, no broker recipe" if cg else (f"recipe {recipe_name}" if recipe else f"no recipe in blueprints.json for {recipe_name or 'this variant'}")
        print(f'{symbol},"{"+".join(blueprints)} G{grade}","{preset["name"]}","{note}","{", ".join(f"{k} {v:.4g}" for k, v in sorted(modifiers.items())) or "none in any source"}",{verdict}')
        if open_:
            print(f'{symbol},"{"+".join(blueprints)} G{grade}","","","{"; ".join(open_)}",OPEN')
    for k, name in RECIPES.items():
        if name not in recipes:
            print(f'{k[0]},"{"+".join(k[1])}","","recipe {name} named here is not in blueprints.json","",DIFFER')
    out.sort(key=lambda p: (p["item"], p["id"]))
    with open(OUT, "w", encoding="utf-8", newline="\n") as f:
        json.dump(out, f, indent=1, ensure_ascii=False)
        f.write("\n")
    print(f"# {len(out)} presets written", file=sys.stderr)


if __name__ == "__main__":
    main()
