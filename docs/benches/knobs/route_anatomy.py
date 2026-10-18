"""route_anatomy.py <arm.jsonl> <route_dir> <tag> <ref.jsonl>

Our routes against the external reference's aggregates, per pinned route:
jumps, boosted (neutron) hops, refuel stops, total ly, path stretch
(total / straight). Where the reference has more boosted hops the search
missed neutron chain; where it has fewer stops the fuel model differs;
where its total ly is shorter the path is straighter.
"""
import json, re, sys
arm, rdir, tag, refp = sys.argv[1:5]
def slug(f, t): return re.sub(r"[^A-Za-z0-9_.-]", "_", f"{f}__{t}")
ref = {(r["from"], r["to"]): r for r in map(json.loads, open(refp))}
print(f"{'route':42} {'ours j/boost/stops/kly':>24} {'ref j/neutron/refuel/kly':>26}  {'dj':>4} {'dboost':>6} {'dstops':>6} {'dkly':>6}")
tot = [0, 0, 0, 0.0]
for line in open(arm):
    r = json.loads(line); k = (r["from"], r["to"]); x = ref.get(k)
    try:
        route = json.loads(open(f"{rdir}/{tag}-{slug(*k)}.route.jsonl", encoding="utf-8").read().splitlines()[0])
    except FileNotFoundError:
        continue
    j, b, s, ly = route["jumps"], sum(1 for h in route["hops"] if h["boosted"]), route["refuel_stops"], route["total_ly"]
    if not x: continue
    dj, db, ds, dl = j - x["jumps"], b - x["neutron"], s - x["refuels"], (ly - x["total_ly"]) / 1000
    for i, v in enumerate((dj, db, ds, dl)): tot[i] += v
    flag = "  <-- over" if dj > 0 else ""
    print(f"{k[0][:19]+' -> '+k[1][:19]:42} {j:>5}/{b:>3}/{s:>2}/{ly/1000:5.1f}       {x['jumps']:>5}/{x['neutron']:>3}/{x['refuels']:>2}/{x['total_ly']/1000:5.1f}    {dj:>+4} {db:>+6} {ds:>+6} {dl:>+6.1f}{flag}")
print(f"sum of differences: jumps {tot[0]:+}, boosted {tot[1]:+}, stops {tot[2]:+}, kly {tot[3]:+.1f}")
