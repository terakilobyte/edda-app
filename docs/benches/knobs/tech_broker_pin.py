"""Pin the technology-broker recipes in crates/ed-engineering/data/blueprints.json
against the community wiki's Technology Broker page, and record which items
are permanent UNLOCKS and which are PURCHASES — pre-engineered modules that
cost the resources again with every unit (the page's own words: "one
payment of the requested resources will immediately grant one
pre-Engineered module ... More of these modules can be obtained only by
paying more resources each time"). The maintainer's rule of 2026-09-27:
ground it with a source, never assume.

Source: https://elite-dangerous.fandom.com/wiki/Technology_Broker (wikitext
via the MediaWiki API; the HTML is paywalled to bots). Fetched with
--fetch, or read from a saved file.

Usage:
  python docs/benches/knobs/tech_broker_pin.py --fetch > docs/benches/<date>-tech-broker-pin.csv
  python docs/benches/knobs/tech_broker_pin.py <broker.wikitext> > ...
The JSON form (--json <out>) writes what gen_preengineered.py reads: every
purchase item with its broker and recipe name.
"""
import json
import os
import re
import sys
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))))
BLUEPRINTS = os.path.join(ROOT, "crates", "ed-engineering", "data", "blueprints.json")
API = "https://elite-dangerous.fandom.com/api.php?action=parse&page=Technology_Broker&prop=wikitext&format=json"
CLASS_WORD = {"0": "Tiny", "1": "Small", "2": "Medium", "3": "Large", "4": "Huge"}


def fetch():
    req = urllib.request.Request(API, headers={"User-Agent": "Mozilla/5.0 (EDDA tech_broker_pin)"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.loads(r.read().decode("utf-8"))["parse"]["wikitext"]["*"]


def strip_links(s):
    s = re.sub(r"\[\[(?:[^|\]]*\|)?([^\]]*)\]\]", r"\1", s)
    s = re.sub(r"<[^>]+>", " ", s)
    return re.sub(r"\s+", " ", s).strip()


def parse(wikitext):
    """[(broker, kind, item, [(qty, resource)])] from the page's tables."""
    out = []
    broker = kind = None
    for line in wikitext.splitlines():
        h = re.match(r"^(={2,4})\s*(.+?)\s*\1\s*$", line)
        if h:
            title = h.group(2).strip()
            if len(h.group(1)) == 3:
                broker = title
            elif len(h.group(1)) == 4:
                kind = title
            continue
        if line.startswith("|-") or line.startswith("{|") or line.startswith("|}") or line.startswith("!"):
            continue
        if not (line.startswith("|") and broker and kind in ("Unlocks", "Purchases")):
            continue
        # A resources row: "|16 [[Meta-Alloys]]<br />26 [[Iron]]..."; an item row: "|[[Shock Cannon]] (Fixed, Class 1)".
        if re.match(r"^\|\s*\d+\s*\[\[", line):
            if out and out[-1][3] == []:
                out[-1][3] = [(int(q), strip_links(name)) for q, name in re.findall(r"(\d+)\s*\[\[([^\]]+)\]\]", line)]
            continue
        if "[[" in line:
            out.append([broker, kind, strip_links(line[1:]), []])
    return [tuple(o) for o in out if o[3]]


def recipe_name(item):
    """The wiki's item label as blueprints.json spells the recipe: (Fixed, Class 2) -> (Fixed, Medium)."""
    s = re.sub(r"\(([^)]*?)Class (\d)\)", lambda m: f"({m.group(1)}{CLASS_WORD.get(m.group(2), m.group(2))})", item)
    return s.strip()


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    json_out = sys.argv[sys.argv.index("--json") + 1] if "--json" in sys.argv else None
    text = fetch() if "--fetch" in sys.argv else open(args[0], encoding="utf-8").read()
    items = parse(text)
    recipes = {}
    for b in json.load(open(BLUEPRINTS, encoding="utf-8")):
        if b.get("Type") in ("Guardian", "Human") and b.get("Grade") is None:
            recipes[b["Name"].lower()] = b
    print("# Pin: technology-broker recipes in blueprints.json against the community wiki's Technology Broker page (fetched via the MediaWiki API). kind=Purchases means a pre-engineered module bought each time, not an unlock. A DIFFER is a recipe the data spells or counts differently from the wiki, or an item one side lacks.")
    print("broker,kind,item,ours,reference,verdict")
    agree = differ = 0
    seen = set()
    purchases = []
    for broker, kind, item, res in items:
        name = recipe_name(item)
        r = recipes.get(name.lower())
        want = sorted((n.lower(), q) for q, n in res)
        if r is None:
            differ += 1
            print(f'{broker},{kind},"{item}","no recipe named {name}","{"; ".join(f"{q} {n}" for q, n in res)}",DIFFER')
            continue
        seen.add(name.lower())
        ours = sorted((i["Name"].lower(), i["Size"]) for i in r["Ingredients"])
        ok = ours == want
        agree += ok
        differ += not ok
        print(f'{broker},{kind},"{item}","{"; ".join(f"{q} {n}" for n, q in ours)}","{"; ".join(f"{q} {n}" for n, q in want)}",{"agree" if ok else "DIFFER"}')
        if kind == "Purchases":
            purchases.append({"broker": broker.lower(), "item": item, "recipe": r["Name"]})
    for name, r in sorted(recipes.items()):
        if name not in seen:
            differ += 1
            print(f'{r["Type"]},?,"{r["Name"]}","in blueprints.json","not on the wiki page",DIFFER')
    print(f"# {agree} agree, {differ} differ")
    if json_out:
        with open(json_out, "w", encoding="utf-8", newline="\n") as f:
            json.dump({"source": API, "purchases": purchases}, f, indent=1)
            f.write("\n")


if __name__ == "__main__":
    main()
