"""Audit a day's manual material trades against the planner.

Reconstructs the inventory before the day's trades from the live store
(current counts, trades undone in reverse), replays the planner (bottom
first, the same knobs as the tab) from that inventory for each trader type
touched, and compares: units received per unit given, value kept
(G1-equivalents: a grade-g unit is worth 6^(g-1)), cross-group spend, and
chains (something received and later paid the same day).

    python docs/benches/knobs/material_trade_audit.py 2026-10-04 [--floor 0.5] [--source-min 0.9]
"""
import csv, glob, json, os, sqlite3, sys
from collections import defaultdict

day = sys.argv[1] if len(sys.argv) > 1 else None
floor_f = float(sys.argv[sys.argv.index("--floor") + 1]) if "--floor" in sys.argv else 0.5
source_min = float(sys.argv[sys.argv.index("--source-min") + 1]) if "--source-min" in sys.argv else 0.9
J = os.path.expanduser(r"~/Saved Games/Frontier Developments/Elite Dangerous")
STORE = os.path.expanduser(r"~/AppData/Local/edda/edda.sqlite3")
CAP = {1: 300, 2: 250, 3: 200, 4: 150, 5: 100}

table = {}
with open("crates/ed-journal/data/material.csv", encoding="utf-8") as f:
    for r in csv.DictReader(f):
        if r["category"] != "None":
            table[r["symbol"].lower()] = dict(kind=r["type"].lower(), group=r["category"], grade=int(r["rarity"]), name=r["name"].strip())

def ratio(s, d):
    a, b = table[s], table[d]
    if a["kind"] != b["kind"]:
        return None
    dd = a["grade"] - b["grade"]
    if a["group"] == b["group"]:
        return None if dd == 0 else ((1, 3 ** dd) if dd > 0 else (6 ** -dd, 1))
    return (6, 1) if dd == 0 else ((2, 3 ** (dd - 1)) if dd > 0 else (6 ** (-dd + 1), 1))

value = lambda sym, n: n * 6 ** (table[sym]["grade"] - 1)

trades = []
for p in sorted(glob.glob(os.path.join(J, "Journal.*.log"))):
    for line in open(p, encoding="utf-8", errors="replace"):
        if '"MaterialTrade"' in line:
            v = json.loads(line)
            if day is None or v["timestamp"].startswith(day):
                trades.append(v)
if not trades:
    sys.exit(f"no MaterialTrade on {day}")

con = sqlite3.connect(f"file:{STORE}?mode=ro", uri=True)
have = {s.lower(): n for s, n in con.execute("SELECT symbol, count FROM materials")}
before = dict(have)
for v in reversed(trades):
    before[v["Paid"]["Material"].lower()] = before.get(v["Paid"]["Material"].lower(), 0) + v["Paid"]["Quantity"]
    before[v["Received"]["Material"].lower()] = before.get(v["Received"]["Material"].lower(), 0) - v["Received"]["Quantity"]

def plan(inv, kind, order="bottom"):
    items = sorted([s for s, t in table.items() if t["kind"] == kind], key=lambda s: (table[s]["group"], table[s]["grade"], s))
    h = {s: inv.get(s, 0) for s in items}
    src = sorted([s for s in items if h[s] >= -(-CAP[table[s]["grade"]] * source_min // 1)], key=lambda s: -table[s]["grade"])
    floors = {s: int(CAP[table[s]["grade"]] * floor_f) for s in src}
    out = []
    def trade(s, d, direction):
        if d in floors:
            return
        r = ratio(s, d)
        if not r:
            return
        g, rc = r
        n = min((h[s] - floors[s]) // g, (CAP[table[d]["grade"]] - h[d]) // rc)
        if n <= 0:
            return
        h[s] -= n * g; h[d] += n * rc
        out.append((s, n * g, d, n * rc, direction))
    below = lambda gr: list(range(1, gr)) if order == "bottom" else list(range(gr - 1, 0, -1))
    for s in src:
        for gr in below(table[s]["grade"]):
            for d in items:
                if table[d]["group"] == table[s]["group"] and table[d]["grade"] == gr:
                    trade(s, d, "down")
    for s in src:
        for gr in [table[s]["grade"]] + below(table[s]["grade"]):
            for d in items:
                if table[d]["group"] != table[s]["group"] and table[d]["grade"] == gr:
                    trade(s, d, "across")
    return out

print(f"# {len(trades)} trades on {day}; knobs: near-full >= {source_min:.0%}, floor {floor_f:.0%}")
received_then_paid = set()
seen_recv = set()
by_kind = defaultdict(list)
for v in trades:
    s, d = v["Paid"]["Material"].lower(), v["Received"]["Material"].lower()
    if s in seen_recv:
        received_then_paid.add(s)
    seen_recv.add(d)
    by_kind[v["TraderType"]].append(v)

for kind, tv in by_kind.items():
    given = sum(v["Paid"]["Quantity"] for v in tv)
    recv = sum(v["Received"]["Quantity"] for v in tv)
    vg = sum(value(v["Paid"]["Material"].lower(), v["Paid"]["Quantity"]) for v in tv)
    vr = sum(value(v["Received"]["Material"].lower(), v["Received"]["Quantity"]) for v in tv)
    cross = [v for v in tv if table[v["Paid"]["Material"].lower()]["group"] != table[v["Received"]["Material"].lower()]["group"]]
    print(f"\n== {kind}: {len(tv)} trades, gave {given} units, received {recv} ({recv / given:.1f} per unit given); value kept {vr / vg:.1%} of the G1-equivalents spent; {len(cross)} across-group trades")
    srcs = sorted({v["Paid"]["Material"].lower() for v in tv}, key=lambda s: -table[s]["grade"])
    for s in srcs:
        spent = sum(v["Paid"]["Quantity"] for v in tv if v["Paid"]["Material"].lower() == s)
        print(f"   {table[s]['name']:<28} G{table[s]['grade']} {before.get(s, 0):>3} -> {have.get(s, 0):>3} ({have.get(s, 0) / CAP[table[s]['grade']]:.0%} of cap left), spent {spent}")
    for v in tv:
        s, d = v["Paid"]["Material"].lower(), v["Received"]["Material"].lower()
        r = ratio(s, d)
        tag = "down" if table[s]["grade"] > table[d]["grade"] else ("across" if table[s]["grade"] == table[d]["grade"] else "up")
        tag += "" if table[s]["group"] == table[d]["group"] else "/x-group"
        print(f"     {v['timestamp'][11:16]} {v['Paid']['Quantity']:>4} {table[s]['name']:<28} -> {v['Received']['Quantity']:>4} {table[d]['name']:<30} {r[0]}:{r[1]:<4} {tag}")
    # The planner from the same starting point, spending to the same floor the commander actually reached.
    for label, fl in (("tab default floor", floor_f), ("the floor you actually went to", min(have.get(s, 0) / CAP[table[s]["grade"]] for s in srcs))):
        saved = floor_f
        floor_f = fl
        p = plan(before, kind)
        floor_f = saved
        pg = sum(t[1] for t in p); pr = sum(t[3] for t in p)
        pvg = sum(value(t[0], t[1]) for t in p); pvr = sum(value(t[2], t[3]) for t in p)
        print(f"   planner (bottom first, {label} {fl:.0%}): {len(p)} trades, give {pg}, receive {pr} ({(pr / pg) if pg else 0:.1f} per unit), value kept {(pvr / pvg) if pvg else 0:.1%}, {sum(1 for t in p if t[4] == 'across')} across")
if received_then_paid:
    print("\nchains (received, then paid the same day):", ", ".join(table[s]["name"] for s in received_then_paid))
else:
    print("\nno chains: every trade went straight from a source to a target")
