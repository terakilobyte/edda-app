"""Second pass: what the two commanders do, together and apart. Counts and shapes only."""
import sys, os, json, collections, datetime as dt
root = sys.argv[1]
files = sorted(f for f in os.listdir(root) if f.startswith("Journal") and f.endswith(".log"))
def t(s): return dt.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ")
label = {}
def lab(fid):
    if fid not in label: label[fid] = chr(ord("A") + len(label))
    return label[fid]
sess = []   # (first, last, letter, file, events list [(ts, ev, j)])
for f in files:
    evs = []; letter = "?"
    for line in open(os.path.join(root, f), encoding="utf-8", errors="replace"):
        try: j = json.loads(line)
        except Exception: continue
        if "timestamp" not in j: continue
        if j.get("event") == "Commander": letter = lab(j.get("FID"))
        evs.append((j["timestamp"], j.get("event"), j))
    if evs: sess.append((t(evs[0][0]), t(evs[-1][0]), letter, f, evs))
sess.sort(key=lambda s: s[0])

# per-commander profile
prof = {}
for first, last, L, f, evs in sess:
    p = prof.setdefault(L, dict(ships=collections.Counter(), carrier=0, odyssey=0, horizons=0, events=collections.Counter(), hours=0.0, newcmdr=0, systems=set(), wing=0, multicrew=0))
    p["hours"] += (last - first).total_seconds() / 3600
    for ts, ev, j in evs:
        p["events"][ev] += 1
        if ev == "LoadGame":
            p["ships"][j.get("Ship")] += 1; p["odyssey"] += bool(j.get("Odyssey")); p["horizons"] += bool(j.get("Horizons"))
        elif ev == "CarrierStats": p["carrier"] += 1
        elif ev == "NewCommander": p["newcmdr"] += 1
        elif ev in ("FSDJump", "Location", "CarrierJump"): p["systems"].add(j.get("SystemAddress"))
        elif ev in ("WingJoin", "WingAdd", "WingInvite"): p["wing"] += 1
        elif ev.startswith("Crew") or ev in ("JoinACrew", "QuitACrew", "KickCrewMember"): p["multicrew"] += 1
for L in sorted(prof):
    p = prof[L]
    print(f"{L}: {p['hours']:.0f} h in game, ships at LoadGame {dict(p['ships'].most_common(5))}, Odyssey {p['odyssey']}/{sum(p['ships'].values())}, CarrierStats {p['carrier']}, NewCommander {p['newcmdr']}, distinct systems {len(p['systems'])}, wing events {p['wing']}, crew events {p['multicrew']}")
    print("   top events:", p["events"].most_common(12))

# concurrency detail
pairs = []
for i, a in enumerate(sess):
    for b in sess[i+1:]:
        if b[0] > a[1]: break
        if a[2] != b[2] and a[2] != "?" and b[2] != "?":
            pairs.append((a, b, max(a[0], b[0]), min(a[1], b[1])))
print(f"\nconcurrent pairs (both commanders): {len(pairs)}")
same_sys = 0; same_sys_pairs = 0; within1s = 0; total_evs_in_overlap = 0; wing_in_overlap = 0
for a, b, s, e in pairs:
    ea = [(t(ts), ev, j) for ts, ev, j in a[4] if s <= t(ts) <= e]
    eb = [(t(ts), ev, j) for ts, ev, j in b[4] if s <= t(ts) <= e]
    total_evs_in_overlap += len(ea) + len(eb)
    sa = {j.get("SystemAddress") for _, ev, j in ea if ev in ("FSDJump", "Location", "CarrierJump", "Docked", "SupercruiseExit")} - {None}
    sb = {j.get("SystemAddress") for _, ev, j in eb if ev in ("FSDJump", "Location", "CarrierJump", "Docked", "SupercruiseExit")} - {None}
    if sa & sb: same_sys_pairs += 1
    wing_in_overlap += sum(1 for _, ev, _ in ea + eb if ev in ("WingJoin", "WingAdd", "WingInvite"))
    ta = sorted(x[0] for x in ea); tb = sorted(x[0] for x in eb)
    import bisect
    for x in ta:
        k = bisect.bisect_left(tb, x - dt.timedelta(seconds=1))
        if k < len(tb) and tb[k] <= x + dt.timedelta(seconds=1): within1s += 1
print(f"  pairs where both were in a common system during the overlap: {same_sys_pairs}")
print(f"  events inside overlaps: {total_evs_in_overlap}; A-events with a B-event within 1 s: {within1s}; wing events inside overlaps: {wing_in_overlap}")
# the "same commander overlapping" cases
so = []
for i, a in enumerate(sess):
    for b in sess[i+1:]:
        if b[0] > a[1]: break
        if a[2] == b[2] and a[2] != "?": so.append((a, b))
for a, b in so:
    print(f"  same-commander overlap {a[2]}: file1 last event {a[4][-1][1]} at {a[1].time()}, file2 first {b[4][0][1]} at {b[0].time()}, overlap {(a[1]-b[0]).total_seconds():.0f} s, file1 lines {len(a[4])}")
# Commander events per file > 1: what sits between them (relog pattern)
between = collections.Counter(); gaps = []
for first, last, L, f, evs in sess:
    idx = [i for i, (_, ev, _) in enumerate(evs) if ev == "Commander"]
    for i0, i1 in zip(idx, idx[1:]):
        gaps.append((t(evs[i1][0]) - t(evs[i0][0])).total_seconds())
        for _, ev, _ in evs[i0+1:i1]:
            if ev in ("Shutdown", "Music", "LoadGame", "Location", "Died", "Resurrect"): between[ev] += 1
print("\nrelogs inside one file:", len(gaps), "median gap min", sorted(gaps)[len(gaps)//2]/60 if gaps else None, "| markers between:", dict(between))
# Fileheader -> Commander distance and whether LoadGame always follows Commander
dist = []; lg_after = 0; cmd_total = 0
for first, last, L, f, evs in sess:
    for i, (_, ev, _) in enumerate(evs):
        if ev == "Commander":
            cmd_total += 1
            nxt = [e for _, e, _ in evs[i+1:i+8]]
            if "LoadGame" in nxt: lg_after += 1
print(f"Commander events {cmd_total}, LoadGame within the next 7 lines {lg_after}")
# Friends events: do they name the other account? count only
fr = collections.Counter()
for first, last, L, f, evs in sess:
    for _, ev, j in evs:
        if ev == "Friends": fr[(L, j.get("Status"))] += 1
print("Friends events by (commander, status):", dict(fr))
