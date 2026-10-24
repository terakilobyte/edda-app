"""What the shipped store does with two accounts in one folder: attribute every
event row to its commander by file, then count how often the 'newest row wins'
readers would change commander. Counts only."""
import sqlite3, sys, json, collections, datetime as dt
db = sys.argv[1]
c = sqlite3.connect(db)
tables = [r[0] for r in c.execute("select name from sqlite_master where type='table' order by 1")]
print("tables:", len(tables), tables)
print("events columns:", [r[1] for r in c.execute("pragma table_info(events)")])
# file -> commander label via the Commander event's FID
label = {}; fid_of_file = {}
for f, raw in c.execute("select file, raw from events where event='Commander'"):
    fid = json.loads(raw).get("FID")
    if fid not in label: label[fid] = chr(ord("A") + len(label))
    fid_of_file.setdefault(f, label[fid])
print("files attributed:", len(fid_of_file), "commanders:", sorted(set(fid_of_file.values())))
n_all = c.execute("select count(*) from events").fetchone()[0]
n_un = c.execute("select count(*) from events where file not in (%s)" % ",".join("?"*len(fid_of_file)), list(fid_of_file)).fetchone()[0]
print(f"events {n_all}, unattributed (menu-only files) {n_un}")

def t(s): return dt.datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ")
# flips: for each 'newest row wins' event type, walk rows in (ts,file,offset) order and count commander changes
types = ["Loadout", "Location", "FSDJump", "Materials", "Cargo", "Missions", "Statistics", "Rank", "Progress", "Powerplay", "CarrierStats", "Docked", "LoadGame", "Commander", "ShipLocker", "Market", "Outfitting", "Shipyard", "EngineerProgress", "Reputation"]
print("\nevent type: rows, commander flips (consecutive rows from different commanders), flips inside the last 90 days")
cut = (t("2026-10-17T20:05:10Z") - dt.timedelta(days=90)).strftime("%Y-%m-%dT%H:%M:%SZ")
for ev in types:
    prev = None; flips = 0; flips90 = 0; rows = 0; per = collections.Counter()
    for ts, f in c.execute("select ts, file from events where event=? order by ts, file, offset", (ev,)):
        L = fid_of_file.get(f)
        if L is None: continue
        rows += 1; per[L] += 1
        if prev and L != prev:
            flips += 1
            if ts >= cut: flips90 += 1
        prev = L
    print(f"  {ev:17} {rows:7} rows {dict(per)}  flips {flips:5}  last-90-days {flips90}")

# fastest flip-flop: Location/Loadout rows from different commanders within 60 s
fast = 0; prev = None
for ts, f in c.execute("select ts, file from events where event in ('Location','FSDJump','Loadout','Cargo','Materials','ShipLocker') order by ts, file, offset"):
    L = fid_of_file.get(f)
    if L is None: continue
    if prev and L != prev[0] and (t(ts) - t(prev[1])).total_seconds() <= 60: fast += 1
    prev = (L, ts)
print(f"\nstate-bearing rows from the OTHER commander within 60 s of the previous one: {fast}")

# missions: derived table rows by commander (via the accepting event's file if the table carries one)
cols = [r[1] for r in c.execute("pragma table_info(missions)")]
print("\nmissions columns:", cols)
if "file" in cols:
    per = collections.Counter(fid_of_file.get(f) for (f,) in c.execute("select file from missions"))
    print("missions by commander:", dict(per))
else:
    # join on the MissionAccepted event by MissionID
    per = collections.Counter()
    ids = {}
    for f, raw in c.execute("select file, raw from events where event='MissionAccepted'"):
        ids[json.loads(raw).get("MissionID")] = fid_of_file.get(f)
    for (mid,) in c.execute("select mission_id from missions") if "mission_id" in cols else []:
        per[ids.get(mid)] += 1
    print("missions by commander (via MissionAccepted):", dict(per))
# carriers: distinct CarrierID per commander
cid = collections.defaultdict(set)
for f, raw in c.execute("select file, raw from events where event='CarrierStats'"):
    cid[fid_of_file.get(f)].add(json.loads(raw).get("CarrierID"))
print("owned carriers per commander (distinct CarrierID in CarrierStats):", {k: len(v) for k, v in cid.items()})
for tbl in ("carrier", "carriers", "carrier_state", "carrier_cargo", "ship_locations", "materials", "snapshots"):
    if tbl in tables:
        n = c.execute(f"select count(*) from {tbl}").fetchone()[0]
        print(f"  {tbl}: {n} rows")
# ship_locations: ships by commander? ShipID is per-commander; collisions?
sid = collections.defaultdict(set)
for f, raw in c.execute("select file, raw from events where event='Loadout'"):
    sid[fid_of_file.get(f)].add(json.loads(raw).get("ShipID"))
if len(sid) >= 2:
    Ls = [k for k in sid if k]
    print("ShipIDs per commander:", {k: len(v) for k, v in sid.items()}, "| ShipIDs shared by both:", len(set.intersection(*(sid[k] for k in Ls))))
print("db size MB:", round(__import__('os').path.getsize(db)/1e6, 1))
