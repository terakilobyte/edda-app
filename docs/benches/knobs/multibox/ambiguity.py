"""Companion-file attribution: Status.json carries no FID. Rule under test: a companion write belongs to
the commander whose journal wrote most recently. Proxy for a companion write: every journal line in a
concurrent window. Ambiguous = the OTHER commander also wrote within W seconds before it.
Also: how often both commanders' last-known cargo tonnage is equal (whether Status.Cargo could break ties)."""
import sqlite3, sys, json, bisect, datetime as dt, collections
c = sqlite3.connect(sys.argv[1])
label = {}; fid_of_file = {}
for f, raw in c.execute("select file, raw from events where event='Commander'"):
    fid = json.loads(raw).get("FID")
    label.setdefault(fid, chr(ord("A") + len(label))); fid_of_file.setdefault(f, label[fid])
def t(s): return dt.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").timestamp()
# per-file windows
win = {}
for f, a, b in c.execute("select file, min(ts), max(ts) from events group by file"):
    if f in fid_of_file: win[f] = (t(a), t(b), fid_of_file[f])
ivs = sorted(win.values())
overl = []
for i, (a0, a1, la) in enumerate(ivs):
    for b0, b1, lb in ivs[i+1:]:
        if b0 > a1: break
        if la != lb: overl.append((max(a0, b0), min(a1, b1)))
overl.sort()
def in_overlap(x):
    k = bisect.bisect_right(overl, (x, float("inf"))) - 1
    return k >= 0 and overl[k][0] <= x <= overl[k][1]
# timestamps per commander, journal lines excluding Music/Friends/ReceiveText (not state) -- and including, both
NOISE = ("Music", "Friends", "ReceiveText", "FSSSignalDiscovered")
ts = collections.defaultdict(list); ts_all = collections.defaultdict(list)
cargo_t = collections.defaultdict(list)  # (ts, tonnage) per commander
for f, s, ev, raw in c.execute("select file, ts, event, raw from events order by ts, file, offset"):
    L = fid_of_file.get(f)
    if not L: continue
    x = t(s)
    ts_all[L].append(x)
    if ev not in NOISE: ts[L].append(x)
    if ev == "Cargo":
        j = json.loads(raw)
        if "Inventory" in j: cargo_t[L].append((x, sum(i.get("Count", 0) for i in j["Inventory"])))
Ls = sorted(ts)
for name, src in (("state lines", ts), ("all lines", ts_all)):
    for W in (1, 3, 5, 10, 30):
        n = amb = 0
        for L in Ls:
            others = sorted(x for M in Ls if M != L for x in src[M])
            for x in src[L]:
                if not in_overlap(x): continue
                n += 1
                k = bisect.bisect_right(others, x)
                if k > 0 and x - others[k-1] <= W: amb += 1
        print(f"{name}: W={W:2}s  lines in concurrent windows {n}, ambiguous {amb} ({100*amb/max(n,1):.1f}%)")
# cargo tonnage tie rate: sample each minute of overlap
tie = tot = 0
for s, e in overl:
    x = s
    while x <= e:
        vals = []
        for L in Ls:
            arr = cargo_t[L]; k = bisect.bisect_right(arr, (x, float("inf"))) - 1
            vals.append(arr[k][1] if k >= 0 else None)
        if None not in vals:
            tot += 1; tie += len(set(vals)) == 1
        x += 60
print(f"minutes of concurrent play sampled {tot}; both commanders' last-known cargo tonnage equal {tie} ({100*tie/max(tot,1):.1f}%)")
print(f"concurrent windows {len(overl)}, total {sum(e-s for s,e in overl)/3600:.1f} h")
