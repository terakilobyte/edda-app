"""Survey a shared journal folder for multi-account structure.
Prints shapes and counts only: commanders are labelled A, B, ... by first appearance; no names, FIDs or systems."""
import sys, os, json, re, collections, datetime as dt

root = sys.argv[1]
files = sorted(f for f in os.listdir(root) if f.startswith("Journal") and f.endswith(".log"))
label = {}          # fid -> letter
rows = []           # per file: (first_ts, last_ts, letter, n_lines, n_commander_events, part, fileheader_ok, events_before_commander)
def lab(fid, name):
    if fid not in label:
        label[fid] = chr(ord("A") + len(label))

    return label[fid]

bad = 0
for f in files:
    p = os.path.join(root, f)
    first = last = None; letter = None; n = 0; ncmd = 0; part = None; fh = False; before = []
    fids = set(); seen_cmd = False
    with open(p, encoding="utf-8", errors="replace") as fh_:
        for line in fh_:
            line = line.strip()
            if not line: continue
            n += 1
            try: j = json.loads(line)
            except Exception: bad += 1; continue
            ts = j.get("timestamp"); ev = j.get("event")
            if ts: first = first or ts; last = ts
            if ev == "Fileheader": fh = True; part = j.get("part")
            elif ev == "Commander":
                ncmd += 1; seen_cmd = True; fids.add(j.get("FID")); letter = lab(j.get("FID"), j.get("Name"))
            elif ev == "LoadGame":
                if j.get("FID"): fids.add(j.get("FID")); letter = letter or lab(j.get("FID"), j.get("Commander"))
            elif not seen_cmd and ev not in ("Fileheader",): before.append(ev)
    rows.append(dict(f=f, first=first, last=last, letter=letter or "?", n=n, ncmd=ncmd, part=part, fh=fh, before=before, nfids=len(fids)))

print(f"files {len(files)}  unparsable lines {bad}")
print(f"distinct FIDs {len(label)} -> labels {sorted(label.values())}")

by = collections.Counter(r["letter"] for r in rows)
print("files per commander:", dict(by))
print("files with 0 Commander events:", sum(1 for r in rows if r["ncmd"] == 0), "| >1:", sum(1 for r in rows if r["ncmd"] > 1), "| files with >1 FID:", sum(1 for r in rows if r["nfids"] > 1))
print("parts:", collections.Counter(r["part"] for r in rows))
print("events before the first Commander event (top):", collections.Counter(e for r in rows for e in r["before"]).most_common(8))
print("lines before first Commander, max:", max((len(r["before"]) for r in rows), default=0))

def t(s): return dt.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ")
# first/last per commander
for L in sorted(by):
    rs = [r for r in rows if r["letter"] == L and r["first"]]
    print(f"  {L}: {len(rs)} files, {min(r['first'] for r in rs)} .. {max(r['last'] for r in rs)}, lines {sum(r['n'] for r in rs)}")

# concurrency: files whose [first,last] overlap with a different commander's file
iv = [(t(r["first"]), t(r["last"]), r["letter"], r["f"]) for r in rows if r["first"] and r["last"]]
iv.sort()
overlaps = []
for i, (a0, a1, la, fa) in enumerate(iv):
    for b0, b1, lb, fb in iv[i+1:]:
        if b0 > a1: break
        if la != lb:
            overlaps.append((max(a0, b0), min(a1, b1), la, lb, fa, fb))
print(f"overlapping sessions of DIFFERENT commanders: {len(overlaps)} file pairs")
tot = sum((e - s).total_seconds() for s, e, *_ in overlaps)
print(f"  total overlapped time {tot/3600:.1f} h; longest {max(((e-s).total_seconds() for s,e,*_ in overlaps), default=0)/3600:.2f} h")
days = sorted({s.date() for s, *_ in overlaps})
print(f"  days with concurrent play: {len(days)}; first {days[0] if days else None}, last {days[-1] if days else None}")
# same-commander overlaps (two instances of one account? should be 0)
same_ov = 0
for i, (a0, a1, la, fa) in enumerate(iv):
    for b0, b1, lb, fb in iv[i+1:]:
        if b0 > a1: break
        if la == lb and b0 < a1: same_ov += 1
print(f"overlapping sessions of the SAME commander: {same_ov}")
# file name timestamp collisions / ordering vs first event
coll = collections.Counter(re.sub(r"\.\d\d\.log$", "", r["f"]) for r in rows)
print("file-name stems used by >1 file:", sum(1 for c in coll.values() if c > 1))
# day-level: how many days has each commander, and how many days both
dA = collections.defaultdict(set)
for r in rows:
    if r["first"]: dA[r["letter"]].add(r["first"][:10])
Ls = sorted(dA)
if len(Ls) >= 2:
    both = set.intersection(*(dA[L] for L in Ls))
    print("days per commander:", {L: len(dA[L]) for L in Ls}, "| days with every commander:", len(both))
# last 30 files: sequence of letters, to show alternation pattern
print("last 40 files by name, commander letters:", "".join(r["letter"] or "?" for r in rows[-40:]))
# status files: mtime vs newest journal
for s in ["Status.json", "Cargo.json", "NavRoute.json", "ShipLocker.json", "edmc-journal-lock.txt"]:
    p = os.path.join(root, s)
    if os.path.exists(p):
        print(f"  {s}: mtime {dt.datetime.utcfromtimestamp(os.path.getmtime(p)).isoformat()}Z size {os.path.getsize(p)}")
print("newest journal last event:", max(r["last"] for r in rows if r["last"]))
