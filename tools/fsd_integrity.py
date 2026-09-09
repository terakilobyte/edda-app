"""FSD integrity loss per supercharge, from the commander's journal in edda.sqlite3.

Walk every event in journal order. Each Loadout is a health observation of
the fitted FSD; between two consecutive observations of the same drive on
the same ship (with no repair in between) count the supercharged FSDJumps,
by the class of the star boosted FROM (the previous jump's arrival star),
and the hull/heat damage events. Then fit loss per boost per drive.
"""
import os
import json, sqlite3, statistics, sys
from collections import defaultdict

DB = os.environ.get("EDDA_DB", os.path.join(".data", "edda.sqlite3"))
con = sqlite3.connect(DB)
rows = con.execute(
    "SELECT file, offset, ts, event, raw FROM events WHERE event IN "
    "('Loadout','FSDJump','AfmuRepairs','Repair','RepairAll','HullDamage','HeatDamage','ShipyardSwap','Location','CarrierJump','FSDTarget','Scan') "
    "ORDER BY file, offset"
).fetchall()

def fsd_of(loadout):
    for m in loadout.get("Modules", []):
        if m.get("Slot") == "FrameShiftDrive":
            return m.get("Item", "").lower(), m.get("Health")
    return None, None

def star_kind(cls):
    if not cls: return "unknown"
    c = cls.upper()
    if c == "N": return "neutron"
    if c.startswith("D"): return "white_dwarf"
    return "other"

# star class of every system the journal has jumped into: the boost source
# for a jump is the system it leaves, whatever event started the session.
# FSDJump carries no star class; FSDTarget (the class of the system being
# targeted) and Scan (a main star at 0 ls) do.
star_of = {}
for _f, _o, _t, _e, _raw in rows:
    if _e not in ("FSDTarget", "Scan"): continue
    try: _v = json.loads(_raw)
    except Exception: continue
    if _e == "FSDTarget" and _v.get("StarClass"):
        star_of[_v.get("Name")] = _v["StarClass"]
    elif _e == "Scan" and _v.get("StarType") and _v.get("DistanceFromArrivalLS", 1) == 0:
        star_of[_v.get("StarSystem")] = _v["StarType"]

# state
ship_id = None
here = None            # current system name
max_range = {}         # ship_id -> MaxJumpRange from its last Loadout
inferred = 0
last_obs = {}          # (ship_id, item) -> dict(health, ts, boosts:{kind:[dist]}, damage, repaired)
arrival_class = None   # star class of the system we are in (boost source for the next jump)
windows = []           # finished windows

def close_window(key, new_health, ts, obs_ts):
    w = last_obs.get(key)
    if w is None or new_health is None: return
    n = sum(len(v) for v in w["boosts"].values())
    windows.append(dict(ship=key[0], item=key[1], h0=w["health"], h1=new_health, t0=w["ts"], t1=ts,
                        boosts=dict(w["boosts"]), n=n, damage=w["damage"], repaired=w["repaired"], plain=w["plain"]))

for file, offset, ts, event, raw in rows:
    try: v = json.loads(raw)
    except Exception: continue
    if event in ("Location", "CarrierJump"):
        here = v.get("StarSystem", here)
    elif event == "ShipyardSwap":
        ship_id = v.get("ShipID", ship_id); arrival_class = None
    elif event == "Loadout":
        ship_id = v.get("ShipID", ship_id)
        if v.get("MaxJumpRange"): max_range[ship_id] = v["MaxJumpRange"]
        item, health = fsd_of(v)
        if item is None or health is None: continue
        key = (ship_id, item)
        close_window(key, health, ts, ts)
        last_obs[key] = dict(health=health, ts=ts, boosts=defaultdict(list), damage=0, repaired=False, plain=0)
    elif event == "FSDJump":
        boosted = v.get("BoostUsed") is not None
        dist = v.get("JumpDist", 0.0)
        # A supercharged jump the journal did not flag: farther than the
        # drive's unboosted maximum (with a little slack for fuel/cargo).
        if not boosted and max_range.get(ship_id) and dist > max_range[ship_id] * 1.15:
            boosted = True; inferred += 1
        source = star_kind(star_of.get(here))
        # A boost the journal did not flag, from a star that cannot boost,
        # is a range mis-estimate, not a boost.
        if boosted and v.get("BoostUsed") is None and source not in ("neutron", "white_dwarf"):
            boosted = False; inferred -= 1
        for key, w in last_obs.items():
            if key[0] != ship_id: continue
            if boosted: w["boosts"][source].append(dist)
            else: w["plain"] += 1
        here = v.get("StarSystem", here)
    elif event == "RepairAll":
        # Every module back to full: a fresh, known observation for this ship's drive.
        for key, w in list(last_obs.items()):
            if key[0] == ship_id:
                close_window(key, None, ts, ts)
                last_obs[key] = dict(health=1.0, ts=ts, boosts=defaultdict(list), damage=0, repaired=False, plain=0)
    elif event == "AfmuRepairs":
        if "hyperdrive" in str(v.get("Module", "")).lower() and v.get("Health") is not None:
            for key, w in list(last_obs.items()):
                if key[0] == ship_id:
                    close_window(key, None, ts, ts)
                    last_obs[key] = dict(health=v["Health"], ts=ts, boosts=defaultdict(list), damage=0, repaired=False, plain=0)
    elif event == "Repair":
        items = [str(i) for i in (v.get("Items") or [v.get("Item")]) if i]
        if any(i != "Wear" for i in items):
            for key, w in last_obs.items():
                if key[0] == ship_id: w["repaired"] = True
    elif event in ("HullDamage", "HeatDamage"):
        for key, w in last_obs.items():
            if key[0] == ship_id: w["damage"] += 1

# ── report ──
print(f"windows between FSD health observations: {len(windows)}; boosts inferred from distance (unflagged): {inferred}")
clean = [w for w in windows if not w["repaired"]]
print(f"  without a repair in between: {len(clean)}; with boosts: {sum(1 for w in clean if w['n'] > 0)}")
by_drive = defaultdict(list)
for w in clean:
    by_drive[w["item"]].append(w)

def short(item):
    size = item.split("size")[1][0] if "size" in item else "?"
    cls = item.split("class")[1][0] if "class" in item else "?"
    rating = {"5": "A", "4": "B", "3": "C", "2": "D", "1": "E"}.get(cls, "?")
    return f"{size}{rating}{' SCO' if 'overcharge' in item else ''}{' MkII' if 'mkii' in item or '_v2' in item else ''}  ({item})"

print()
print(f"{'drive':<52} {'windows':>7} {'boosts':>6} {'plain':>6} {'sum dH':>8} {'per boost':>10} {'spread (per window)':>22}")
for item, ws in sorted(by_drive.items()):
    nb = sum(w["n"] for w in ws); npl = sum(w["plain"] for w in ws)
    dh = sum(w["h0"] - w["h1"] for w in ws)
    per = [ (w["h0"] - w["h1"]) / w["n"] for w in ws if w["n"] > 0 ]
    spread = f"{min(per)*100:.2f}..{max(per)*100:.2f} % (median {statistics.median(per)*100:.2f})" if per else "-"
    print(f"{short(item):<52} {len(ws):>7} {nb:>6} {npl:>6} {dh*100:>7.2f}% {(dh/nb*100 if nb else 0):>9.3f}% {spread:>22}")

print()
print("windows with boosts (drive, h0 -> h1, boosts by source, plain jumps, damage events):")
for w in sorted(clean, key=lambda w: (w["item"], w["t0"])):
    if w["n"] == 0 and (w["h0"] - w["h1"]) == 0: continue
    src = ", ".join(f"{k} x{len(v)} ({min(v):.0f}-{max(v):.0f} ly)" for k, v in w["boosts"].items()) or "-"
    print(f"  {short(w['item']).split('  ')[0]:<10} {w['h0']*100:6.2f} -> {w['h1']*100:6.2f}  dH {(w['h0']-w['h1'])*100:5.2f}%  boosts {w['n']:>2} [{src}]  plain {w['plain']:>3}  dmg {w['damage']}  {w['t0'][:16]}..{w['t1'][:16]}")

# plain-jump control: windows with no boosts but health change
ctrl = [w for w in clean if w["n"] == 0 and w["plain"] > 0]
if ctrl:
    dh = sum(w["h0"] - w["h1"] for w in ctrl); npl = sum(w["plain"] for w in ctrl)
    print(f"\ncontrol: {len(ctrl)} boost-free windows, {npl} plain jumps, total dH {dh*100:.3f}% -> {dh/npl*100:.4f}% per plain jump")

# ── per-source fit over every clean window that has boosts of one source only ──
print()
print("per boost by source (windows whose boosts are all one source; loss split evenly over the window's boosts):")
by_src = defaultdict(list)
for w in clean:
    if w["n"] == 0: continue
    srcs = [k for k, v in w["boosts"].items() if v]
    if len(srcs) != 1: continue
    by_src[(srcs[0], w["item"])].append(((w["h0"] - w["h1"]) / w["n"], w["n"], w["damage"]))
for (src, item), xs in sorted(by_src.items()):
    per = [x[0] for x in xs]; n = sum(x[1] for x in xs)
    print(f"  {src:<12} {short(item).split('  ')[0]:<10} boosts {n:>2} windows {len(xs):>2}  per boost {min(per)*100:.2f}..{max(per)*100:.2f} % median {statistics.median(per)*100:.2f} %  (damage events in those windows: {sum(x[2] for x in xs)})")
allp = [x[0] for xs in by_src.values() for x in xs]
if allp:
    print(f"  all sources, all drives: {len(allp)} windows, median {statistics.median(allp)*100:.2f} %, mean {statistics.mean(allp)*100:.2f} %, range {min(allp)*100:.2f}..{max(allp)*100:.2f} %")
