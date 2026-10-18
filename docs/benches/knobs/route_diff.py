"""route_diff.py <a.route.jsonl> <b.route.jsonl>: where two routes of the same plot part.

Summaries (jumps, boosted, stops marked and tank rises, ly), the shared-star
count, then each route as runs of shared / DIVERGING hops with the span and
boosts of every run. Found the Jongou XM-W -> Byoi CQ-G 401 -> 451 of
2026-10-18 as one 24- vs 73-hop segment after Groek CL-P d5-2.
"""
import json, sys
def load(p):
    r = json.loads(open(p, encoding="utf-8").read().splitlines()[0]); h = r["hops"]
    stops = sum(1 for i in range(1, len(h)) if h[i]["fuel_after"] > h[i-1]["fuel_after"] + 1.0)
    return r, h, stops
def summary(tag, r, h, stops):
    boosted = sum(1 for x in h if x["boosted"])
    print(f"{tag}: {r['jumps']} jumps, {boosted} boosted, {r['refuel_stops']} stops marked / {stops} tank rises, {r['total_ly']:.0f} ly, min_fuel={r.get('min_fuel')}")
a, ha, sa = load(sys.argv[1]); b, hb, sb = load(sys.argv[2])
summary("A", a, ha, sa); summary("B", b, hb, sb)
na = [x["name"] for x in ha]; nb = [x["name"] for x in hb]
common = set(na) & set(nb)
print(f"shared stars: {len(common)} of {len(na)} / {len(nb)}")
# print the segments: runs of shared / unshared along A and B
def runs(names, common):
    out, cur, start = [], None, 0
    for i, n in enumerate(names):
        s = n in common
        if s != cur:
            if cur is not None: out.append((cur, start, i-1))
            cur, start = s, i
    out.append((cur, start, len(names)-1)); return out
for tag, names, h in (("A", na, ha), ("B", nb, hb)):
    print(f"-- {tag} segments (shared?, first..last hop idx, span ly, boosted in span):")
    for s, i, j in runs(names, common):
        span = sum(x["distance_ly"] for x in h[i:j+1]); bo = sum(1 for x in h[i:j+1] if x["boosted"])
        print(f"   {'shared  ' if s else 'DIVERGES'} {i:4}..{j:4} ({j-i+1:3} hops) {span:7.0f} ly, {bo} boosted, {names[i][:22]} .. {names[j][:22]}")
