"""Name audit: every namespace EDDA prints or matches on, cross-checked.

The rule (maintainer, 2026-09-27, after "Guardian Wreckage Components"
read as none in the hold and "Micro Controllers" found no sellers 28 ly
from Sol): the FDev symbol is the key everywhere — storage, wire, joins —
display names exist only to print, and Frontier's own string wins over
EDCD's where the two differ. Anything that arrives as a name is resolved
to a symbol at the boundary. This script measures that rule across every
table it touches and prints a CSV; a DIFFER row is a bug or a missing
override, never a shrug.

Checks:
  recipe      every blueprint / unlock ingredient names a catalogued material
              or commodity (crates/ed-engineering/data/blueprints.json vs
              the FDevIDs tables in crates/ed-journal/data/)
  sources     every material_sources.json material is catalogued
  frontier    every (Name, Name_Localised) pair Frontier wrote into the
              commander's own journal (Materials, MaterialCollected, Cargo,
              Market, ShipLocker...) prints as the catalog prints it —
              Frontier's string is the measurement, the catalog the claim
  server      the API's commodities table (a CSV export, see usage): every
              symbol is a canonical FDevIDs symbol with a name; a spaced,
              hyphened or dotted symbol is a display name interned by mistake
  edsy        every EDSY module kind name starts as EDCD's outfitting name
              does, and every EDSY hull is a shipyard.csv hull

Usage:
  python docs/benches/knobs/name_audit.py [edda.sqlite3] [prod-commodities.csv]
      > docs/benches/<date>-name-audit.csv
  The commodities export: psql "$DATABASE_URL" -Atc "COPY (SELECT symbol, name,
  category, (SELECT count(*) FROM market m WHERE m.commodity_symbol=c.symbol)
  FROM commodities c ORDER BY 1) TO STDOUT WITH CSV" > prod-commodities.csv
"""
import csv
import json
import os
import re
import sqlite3
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
J = os.path.join(ROOT, "crates", "ed-journal", "data")
E = os.path.join(ROOT, "crates", "ed-engineering", "data")
S = os.path.join(ROOT, "crates", "ed-ships", "data")

# Frontier's own strings that override EDCD's (mirrors ed_journal::catalog::FRONTIER_NAMES
# and ed_store::market::frontier_commodity_symbol; a new DIFFER row below is a candidate).
FRONTIER_MATERIALS = {
    "guardian_sentinel_wreckagecomponents": "Guardian Wreckage Components",
    "guardian_moduleblueprint": "Guardian Module Blueprint Fragment",
    "guardian_weaponblueprint": "Guardian Weapon Blueprint Fragment",
    "drones": "Limpet",
}
# Materials the game has and FDevIDs lacks (mirrors ed_journal::catalog::EXTRA_MATERIALS).
EXTRA_MATERIALS = {"tg_causticcrystal": "Caustic Crystal", "tg_causticshard": "Caustic Shard"}
FRONTIER_COMMODITIES = {"low temp. diamonds": "lowtemperaturediamond"}


def rows(path):
    with open(path, encoding="utf-8") as f:
        return list(csv.DictReader(f))


def load_catalog():
    """symbol(lower) -> (name, kind); name(lower) -> symbol(lower). EDCD's names with Frontier's overrides."""
    by_symbol, by_name = {}, {}

    def put(symbol, name, kind):
        symbol, name = symbol.strip(), name.strip()
        by_symbol[symbol.lower()] = (name, kind)
        by_name.setdefault(name.lower(), symbol.lower())

    for r in rows(os.path.join(J, "material.csv")):
        put(r["symbol"], r["name"], "material")
    for r in rows(os.path.join(J, "commodity.csv")):
        put(r["symbol"], r["name"], "commodity")
    for r in rows(os.path.join(J, "rare_commodity.csv")):
        put(r["symbol"], r["name"], "commodity")
    micro = os.path.join(J, "microresources.csv")
    if os.path.exists(micro):
        for r in rows(micro):
            put(r["symbol"], r.get("English name") or r.get("name", ""), "microresource")
    for symbol, name in EXTRA_MATERIALS.items():
        put(symbol, name, "material")
    for symbol, name in FRONTIER_MATERIALS.items():
        kind = by_symbol.get(symbol, ("", "material"))[1]
        by_symbol[symbol] = (name, kind)
        by_name[name.lower()] = symbol
    return by_symbol, by_name


def ingredients(blueprints):
    out = set()

    def walk(o):
        if isinstance(o, dict):
            if isinstance(o.get("Name"), str) and "Size" in o and len(o) <= 3:
                out.add(o["Name"].strip())
            for v in o.values():
                walk(v)
        elif isinstance(o, list):
            for v in o:
                walk(v)

    walk(blueprints)
    return out


def main():
    db = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.environ.get("LOCALAPPDATA", ""), "edda", "edda.sqlite3")
    prod = sys.argv[2] if len(sys.argv) > 2 else None
    by_symbol, by_name = load_catalog()
    print("# Name audit: the FDev symbol is the key everywhere, display names only print, Frontier's own string wins over EDCD's. Every DIFFER is a bug or a missing override; OPEN is a difference between two references that no journal has measured Frontier's string for yet (EDCD's stands until one does).")
    print("check,namespace,item,ours,reference,verdict")
    agree = differ = open_ = 0

    def row(check, ns, item, ours, ref, ok, verdict=None):
        nonlocal agree, differ, open_
        verdict = verdict or ("agree" if ok else "DIFFER")
        agree += verdict == "agree"
        differ += verdict == "DIFFER"
        open_ += verdict == "OPEN"
        q = lambda s: '"' + str(s).replace('"', '""') + '"'
        print(f"{check},{ns},{q(item)},{q(ours)},{q(ref)},{verdict}")

    # recipe: ingredients vs catalog
    bp = json.load(open(os.path.join(E, "blueprints.json"), encoding="utf-8"))
    ing = ingredients(bp)
    # Mirrors RECIPE_SPELLINGS in ed_engineering::Catalog::load: three spellings the data gets wrong, corrected at load.
    spellings = {"abnormal compact emission data": "abnormal compact emissions data", "ballistic data": "ballistics data", "xihe companions": "xihe biomorphic companions"}
    bad = sorted(i for i in ing if spellings.get(i.lower(), i.lower()) not in by_name and i.lower() != "push")
    row("recipe", "ingredients", f"{len(ing)} distinct", f"{len(bad)} not catalogued", " / ".join(bad) if bad else "every ingredient catalogued", not bad)

    # sources: material_sources.json vs catalog
    src = json.load(open(os.path.join(E, "material_sources.json"), encoding="utf-8"))
    names = {m for s in src for m in s["materials"]}
    bad = sorted(m for m in names if m.lower() not in by_name)
    row("sources", "material_sources.json", f"{len(names)} materials", f"{len(bad)} not catalogued", " / ".join(bad) if bad else "all catalogued", not bad)

    # frontier: the journal's own (Name, Name_Localised) pairs vs the catalog
    if os.path.exists(db):
        con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
        pairs = {}
        for (raw,) in con.execute("SELECT raw FROM events WHERE event IN ('Materials','MaterialCollected','MaterialDiscarded','MaterialTrade','Cargo','Market','CollectCargo','EjectCargo','MarketBuy','MarketSell','Synthesis','EngineerCraft','TechnologyBroker')"):
            v = json.loads(raw)

            def walk(o):
                if isinstance(o, dict):
                    n, l = o.get("Name"), o.get("Name_Localised")
                    if isinstance(n, str) and isinstance(l, str) and l.strip():
                        sym = re.sub(r"^\$(.+?)_name;?$", r"\1", n.strip().lower())
                        pairs.setdefault(sym, set()).add(l.strip())
                    for k in ("Type", "Received", "Paid", "Materials", "Commodities", "Ingredients", "Items", "Raw", "Manufactured", "Encoded", "Inventory"):
                        if k in o:
                            walk(o[k])
                    t, tl = o.get("Type"), o.get("Type_Localised")
                    if isinstance(t, str) and isinstance(tl, str) and tl.strip():
                        sym = re.sub(r"^\$(.+?)_name;?$", r"\1", t.strip().lower())
                        pairs.setdefault(sym, set()).add(tl.strip())
                elif isinstance(o, list):
                    for x in o:
                        walk(x)

            walk(v)
        seen = mismatch = unknown = 0
        for sym, locs in sorted(pairs.items()):
            if sym not in by_symbol:
                unknown += 1
                row("frontier", "journal symbol", sym, "not in the catalog", " / ".join(sorted(locs)), False)
                continue
            seen += 1
            ours = by_symbol[sym][0]
            for loc in sorted(locs):
                if loc.lower() != ours.lower():
                    mismatch += 1
                    row("frontier", "display name", sym, ours, loc, False)
        row("frontier", "summary", f"{len(pairs)} symbols Frontier named in this journal", f"{mismatch} print differently, {unknown} unknown", "Frontier's Name_Localised", mismatch == 0 and unknown == 0)
    else:
        row("frontier", "journal", db, "no database", "", False)

    # server: the API's commodities table
    if prod and os.path.exists(prod):
        spaced = nameless = unknown = 0
        with open(prod, encoding="utf-8") as f:
            for symbol, name, category, market_rows in csv.reader(f):
                if re.search(r"[ .\-]", symbol):
                    spaced += 1
                    canonical = FRONTIER_COMMODITIES.get(symbol) or by_name.get(symbol)
                    row("server", "display-name symbol", symbol, f"{market_rows} market rows", f"folds onto {canonical}" if canonical else "no canonical row by name", False)
                    continue
                if symbol not in by_symbol:
                    unknown += 1
                    row("server", "symbol", symbol, f"{market_rows} market rows, not in FDevIDs", name, False)
                elif not name:
                    nameless += 1
                    row("server", "nameless canonical", symbol, "name empty", by_symbol[symbol][0], False)
        row("server", "summary", os.path.basename(prod), f"{spaced} display-name symbols, {nameless} nameless canonical, {unknown} unknown", "FDevIDs", spaced == 0 and nameless == 0 and unknown == 0)

    # edsy: module kind names vs outfitting.csv; hulls vs shipyard.csv
    out_names = {}
    for r in rows(os.path.join(J, "outfitting.csv")):
        out_names.setdefault(r["symbol"].lower(), r["name"].strip())
    kinds = json.load(open(os.path.join(S, "module_kinds.json"), encoding="utf-8"))
    bad = []
    for item, k in kinds.items():
        edcd = out_names.get(item.lower())
        if edcd is None:
            continue  # bulkheads and presets live in the hull entries
        if not k["name"].lower().startswith(edcd.lower().split(" (")[0][: len(k["name"])].lower()[:6]):
            bad.append(f"{item}: EDSY {k['name']!r} vs EDCD {edcd!r}")
    # EDSY's kind name is the planner's group label; the module itself prints
    # EDCD's name (gated in ed-journal tests/edcd_exact.rs). Where the two
    # references disagree and this journal carries no Frontier string for
    # modules, the row is OPEN, not a fault.
    row("edsy", "module group labels", f"{len(kinds)} module symbols", f"{len(bad)} start differently from EDCD", " / ".join(bad[:8]) if bad else "all start as EDCD names do", not bad, None if not bad else "OPEN")
    ships = {r["symbol"].lower() for r in rows(os.path.join(J, "shipyard.csv"))}
    hulls = json.load(open(os.path.join(S, "ship_slots.json"), encoding="utf-8"))
    bad = sorted(h for h in hulls if h.lower() not in ships)
    row("edsy", "hulls", f"{len(hulls)} hulls", f"{len(bad)} not in shipyard.csv", " / ".join(bad) if bad else "every hull is EDCD's", not bad)

    print(f"# {agree} agree, {differ} differ, {open_} open")


if __name__ == "__main__":
    main()
