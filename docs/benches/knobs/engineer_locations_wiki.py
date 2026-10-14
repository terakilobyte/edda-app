#!/usr/bin/env python3
"""Ground the vendored engineer locations against the wiki's Engineers
table (api.php, never scraped HTML): prints every engineer whose base or
system differs, and every wiki name the vendored file lacks.

    python docs/benches/knobs/engineer_locations_wiki.py

House rule (2026-09-27): journal > vendored EDCD data > wiki via api.php;
the journal carries no engineer locations and EDCD has none, so the wiki
is the source for this fact, pinned here. 2026-10-14: the five ship
engineers the unlock guide lacked were added from this table. The wiki
names not vendored are the Odyssey suit/weapon engineers, which the ship
build planner never sends anyone to.
"""
import json
import pathlib
import re
import sys
import urllib.parse
import urllib.request

sys.stdout.reconfigure(encoding="utf-8")

ROOT = pathlib.Path(__file__).resolve().parents[3]
VENDORED = ROOT / "crates/ed-engineering/data/engineer_unlocks.json"
ALIAS = {"Tod 'The Blaster' McQuinn": 'Tod "The Blaster" McQuinn'}


def wikitext(title):
    q = urllib.parse.urlencode({"action": "query", "prop": "revisions", "rvprop": "content", "rvslots": "main", "format": "json", "titles": title, "redirects": 1})
    req = urllib.request.Request("https://elite-dangerous.fandom.com/api.php?" + q, headers={"User-Agent": "EDDA-facts-check/1.0 (engineer_locations_wiki.py)"})
    page = next(iter(json.load(urllib.request.urlopen(req, timeout=30))["query"]["pages"].values()))
    return page["revisions"][0]["slots"]["main"]["*"]


def plain(cell):
    return re.sub(r"\[\[([^\]|]+)(?:\|[^\]]+)?\]\]", r"\1", cell).strip()


def table_rows(text):
    """name -> {base, system}. The tables differ in column order, so the
    base is taken by shape: the one plain-text cell (no link, no bullet
    list, no grade), and the system is the first linked page after the
    name that is not a region."""
    out = {}
    regions = {"Core Systems", "Colonia Region", "Witch Head Nebula"}
    for row in text.split("|-"):
        m = re.search(r"\[\[([^\]|]+)(?:\|[^\]]+)?\]\]", row)
        if not m:
            continue
        name = m.group(1)
        cells = [c.strip() for c in re.split(r"\n\|", row) if c.strip()]
        links = re.findall(r"\[\[([^\]|]+)(?:\|[^\]]+)?\]\]", row)
        systems = [l for l in links[1:] if l not in regions]
        base = next((c for c in cells[1:] if "[[" not in c and not c.startswith("*") and not re.search(r"\bG[1-5]\b", c) and 0 < len(c) < 40), None)
        if not systems or base is None:
            continue
        out[name] = {"base": plain(base), "system": systems[0]}
    return out


def main():
    wiki = table_rows(wikitext("Engineers"))
    ours = {e["name"]: e for e in json.load(open(VENDORED, encoding="utf-8"))["engineers"]}
    diffs = 0
    for name, e in ours.items():
        w = wiki.get(name) or wiki.get(ALIAS.get(name, ""))
        if not w:
            print(f"not in the wiki table: {name}")
            continue
        # The system is the fact the tour stands on; the base cell's
        # position differs between the wiki's tables, so a base that does
        # not match is reported, not counted.
        if w["system"] != e["system"]:
            diffs += 1
            print(f"DIFF {name}: vendored system {e['system']} vs wiki {w['system']}")
        elif w["base"] != e["base"]:
            print(f"base cell read as {w['base']!r} for {name} (vendored {e['base']!r}, system {e['system']} agrees)")
    aliased = set(ALIAS.values())
    missing = [n for n in wiki if n not in ours and n not in aliased]
    print(f"{len(ours)} vendored, {len(wiki)} wiki rows, {diffs} differences")
    print(f"wiki names not vendored ({len(missing)}): {', '.join(missing)}")


if __name__ == "__main__":
    main()
