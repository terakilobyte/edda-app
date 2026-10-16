#!/usr/bin/env python3
"""Combat efficiency per ship, measured from the journal (2026-10-16, the
maintainer's brief: "average time from start of combat to kill for each
ship a player has, how long the total combat session lasted").

Reads the app's event log read-only (every journal line is in `events`),
or a JSONL file of raw journal lines, reconstructs sessions and prints
per-ship rows. This is the reference implementation of the definitions in
docs/superpowers/specs/2026-10-16-combat-sessions-design.md; the store's
`combat::derive_sessions` must reproduce it (pinned by a fixture test).

    python docs/benches/knobs/combat_sessions.py [edda.sqlite3 | events.jsonl] [--sessions] [--kills] [--export-jsonl FROM TO OUT]

Definitions (all from the journal; nothing inferred from the build):

* The ship at time t is the last `Loadout` before t (ShipID, Ship,
  ShipName trimmed). Rows group by ShipID; the name shown is the last one.
* Supercruise: SupercruiseEntry/FSDJump/CarrierJump put the commander in
  supercruise; SupercruiseExit/Docked/Undocked/Touchdown/Liftoff/LoadGame
  put them in normal space. Nothing counts in supercruise.
* A session is one normal-space instance: entered at SupercruiseExit,
  Undocked, Liftoff or LoadGame; left at SupercruiseEntry, FSDJump,
  Docked, Shutdown, LoadGame, Died, or a Loadout with another ShipID.
  Kept only if a fight happened: a kill, UnderAttack, HullDamage
  (PlayerPilot, not Fighter), FighterDestroyed or Died. Locks alone never
  make a session, nor do shields dropping alone (power management).
* Times: entered, start (first combat event), end (last), left (closer).
  Session length = left - entered, but idle after the last combat event
  is capped at IDLE_CAP (600 s): a crash or an AFK tail is not combat
  (measured 2026-10-16: a 21 h "session" was an instance never left in
  the journal). Fighting = end - start; throughput is per hour of
  session length.
* A kill is Bounty, FactionKillBond, CapShipBond or PVPKill; target ship
  is Bounty.Target else the last locked ship; reward TotalReward else
  Reward.
* Time to kill (TTK) = kill ts - first lock on that pilot in the session
  (Bounty.PilotName matched to ShipTargeted.PilotName; a bond uses the
  last locked pilot); the lock is consumed by the kill. A kill with no
  lock has no TTK (fighter and turret kills: on the Type-10, 2,209 of
  2,880 kills came with the fighter out) and is counted in kills_untimed.
* Fighter out: LaunchFighter until DockFighter/FighterDestroyed; reset by
  Loadout and by every session closer (a fighter cannot follow the ship
  out of the instance).
* Kill interval = kill ts - previous kill in the session (or start).

Nothing here identifies another commander: PVPKill victims are counted,
never printed; NPC pilot names are only used to match a lock to a kill.
"""
import json
import os
import sqlite3
import statistics
import sys
from collections import defaultdict
from datetime import datetime, timezone

KILLS = {"Bounty", "FactionKillBond", "CapShipBond", "PVPKill"}
COMBAT = KILLS | {"ShipTargeted", "UnderAttack", "HullDamage", "ShieldState", "FighterDestroyed", "Died"}
SC_ON = {"SupercruiseEntry", "FSDJump", "CarrierJump"}
SC_OFF = {"SupercruiseExit", "Docked", "Undocked", "Touchdown", "Liftoff", "LoadGame"}
ENTER = {"SupercruiseExit", "Undocked", "Liftoff", "LoadGame"}
LEAVE = {"SupercruiseEntry", "FSDJump", "Docked", "Shutdown", "LoadGame", "Died"}
FIGHTER = {"LaunchFighter", "DockFighter", "FighterDestroyed"}
QUALIFY = KILLS | {"UnderAttack", "HullDamage", "FighterDestroyed", "Died"}
IDLE_CAP = 600.0
WANT = COMBAT | SC_ON | SC_OFF | ENTER | LEAVE | FIGHTER | {"Loadout"}


def ts(s):
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp()


def iso(t):
    return datetime.fromtimestamp(t, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def load(path):
    if path.endswith(".jsonl"):
        rows = []
        for line in open(path, encoding="utf-8"):
            if line.strip():
                r = json.loads(line)
                if r.get("event") in WANT:
                    rows.append((ts(r["timestamp"]), r["event"], r))
        return rows
    c = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    q = ",".join("?" * len(WANT))
    rows = c.execute(f"SELECT ts, event, raw FROM events WHERE event IN ({q}) ORDER BY ts, file, offset", sorted(WANT)).fetchall()
    return [(ts(t), e, json.loads(r)) for t, e, r in rows]


def export_jsonl(path, start, end, out):
    """Every journal line (all event types) between two timestamps, in replay order."""
    c = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    rows = c.execute("SELECT raw FROM events WHERE ts >= ? AND ts < ? ORDER BY ts, file, offset", (start, end)).fetchall()
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        for (raw,) in rows:
            f.write(json.dumps(json.loads(raw), ensure_ascii=False, separators=(",", ":")) + "\n")
    print(f"# {len(rows)} events {start}..{end} -> {out}", file=sys.stderr)


def sessions(events):
    """Replay: yields kept sessions in order (the last may be open)."""
    ship = (None, "(unknown)", "")
    in_sc = False
    fighter_out = False
    entered = None  # ts the current instance was entered
    out, cur = [], None

    def close(t, by):
        nonlocal cur
        if cur:
            cur["left"] = t
            cur["closed_by"] = by
            if cur["qualifies"]:
                out.append(cur)
        cur = None

    for t, e, r in events:
        if e == "Loadout":
            new = (r.get("ShipID"), (r.get("Ship") or "").lower(), (r.get("ShipName") or "").strip())
            if cur and new[0] != cur["ship"][0]:
                close(t, "loadout")
            ship = new
            fighter_out = False
            continue
        if e in LEAVE and e != "Died":
            close(t, e.lower())
            fighter_out = False
        if e in SC_ON:
            in_sc = True
            entered = None
        if e in SC_OFF:
            in_sc = False
        if e in ENTER:
            entered = t
        if e in FIGHTER:
            fighter_out = e == "LaunchFighter"
        if e not in COMBAT or in_sc:
            continue
        if e == "ShipTargeted" and not r.get("TargetLocked"):
            continue
        if e == "HullDamage" and (not r.get("PlayerPilot") or r.get("Fighter")):
            continue
        if e == "ShieldState" and r.get("ShieldsUp"):
            continue
        if cur is None:
            cur = {"ship": ship, "entered": entered if entered is not None else t, "start": t, "end": t, "left": None, "closed_by": None, "events": [], "qualifies": False}
        cur["end"] = t
        cur["events"].append((t, e, r, fighter_out))
        if e in QUALIFY:
            cur["qualifies"] = True
        if e == "Died":
            close(t, "died")
            fighter_out = False
    if cur and cur["qualifies"]:
        out.append(cur)
    return out


def analyse(s):
    first_lock, last_lock = {}, None
    kills, untimed, prev_kill = [], 0, s["start"]
    under_attack = hull_hits = shields_down = died = fighter_kills = 0
    credits = 0
    for t, e, r, fighter_out in s["events"]:
        if e == "ShipTargeted":
            pilot = r.get("PilotName")
            if pilot:
                first_lock.setdefault(pilot, t)
                last_lock = (pilot, (r.get("Ship") or "").lower())
        elif e in KILLS:
            if e == "Bounty":
                pilot, target = r.get("PilotName"), (r.get("Target") or "").lower()
            else:
                pilot, target = last_lock or (None, None)
            reward = r.get("TotalReward") or r.get("Reward") or 0
            credits += reward
            lock = first_lock.pop(pilot, None) if pilot else None
            if lock is None:
                untimed += 1
            if fighter_out:
                fighter_kills += 1
            kills.append({"ts": t, "kind": e, "target": target or "(unknown)", "ttk": None if lock is None else t - lock, "interval": t - prev_kill, "reward": reward, "fighter_out": fighter_out})
            prev_kill = t
        elif e == "UnderAttack":
            under_attack += 1
        elif e == "HullDamage":
            hull_hits += 1
        elif e == "ShieldState":
            shields_down += 1
        elif e == "Died":
            died += 1
    left = s["end"] if s["left"] is None else min(s["left"], s["end"] + IDLE_CAP)
    length = left - s["entered"]
    return {**s, "secs": length, "fighting": s["end"] - s["start"], "kills": kills, "kills_untimed": untimed, "kills_fighter": fighter_kills, "under_attack": under_attack, "hull_hits": hull_hits, "shields_down": shields_down, "died": died, "credits": credits}


def med(xs):
    xs = [x for x in xs if x is not None]
    return f"{statistics.median(xs):.0f}" if xs else ""


def p90(xs):
    xs = sorted(x for x in xs if x is not None)
    return f"{xs[int(len(xs) * 0.9) - 1]:.0f}" if len(xs) >= 10 else ""


def main():
    if "--export-jsonl" in sys.argv:
        i = sys.argv.index("--export-jsonl")
        path = next((a for a in sys.argv[1:i] if not a.startswith("--")), os.path.join(os.environ.get("LOCALAPPDATA", ""), "edda", "edda.sqlite3"))
        export_jsonl(path, sys.argv[i + 1], sys.argv[i + 2], sys.argv[i + 3])
        return
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    path = args[0] if args else os.path.join(os.environ.get("LOCALAPPDATA", ""), "edda", "edda.sqlite3")
    ss = [analyse(s) for s in sessions(load(path))]
    total_kills = sum(len(s["kills"]) for s in ss)
    print(f"# Combat sessions (one normal-space instance with a fight in it) from the journal event log ({os.path.basename(path)}); definitions in docs/benches/knobs/combat_sessions.py and the 2026-10-16 spec. {len(ss)} sessions, {total_kills} kills.")
    if "--sessions" in sys.argv:
        print("entered,ship,name,secs,fighting_s,closed_by,kills,kills_untimed,kills_fighter,credits,median_ttk_s,median_interval_s,under_attack,hull_hits,shields_down,died")
        for s in ss:
            print(",".join(str(x) for x in [iso(s["entered"]), s["ship"][1], s["ship"][2], f"{s['secs']:.0f}", f"{s['fighting']:.0f}", s["closed_by"] or "open", len(s["kills"]), s["kills_untimed"], s["kills_fighter"], s["credits"], med([k["ttk"] for k in s["kills"]]), med([k["interval"] for k in s["kills"]]), s["under_attack"], s["hull_hits"], s["shields_down"], s["died"]]))
        return
    if "--kills" in sys.argv:
        print("ts,ship,kind,target,ttk_s,interval_s,fighter_out,reward")
        for s in ss:
            for k in s["kills"]:
                ttk = "" if k["ttk"] is None else f"{k['ttk']:.0f}"
                print(",".join(str(x) for x in [iso(k["ts"]), s["ship"][1], k["kind"], k["target"], ttk, f"{k['interval']:.0f}", int(k["fighter_out"]), k["reward"]]))
        return
    by, names = defaultdict(list), {}
    for s in ss:
        key = (s["ship"][0], s["ship"][1])
        by[key].append(s)
        if s["ship"][2]:
            names[key] = s["ship"][2]
    print("ship_id,ship,name,sessions,hours,fighting_hours,kills,kills_timed,kills_fighter,kills_per_hour,median_ttk_s,p90_ttk_s,median_interval_s,credits,credits_per_hour,under_attack,hull_hits,shields_down,died")
    for key, group in sorted(by.items(), key=lambda kv: -sum(len(s["kills"]) for s in kv[1])):
        kills = [k for s in group for k in s["kills"]]
        hours = sum(s["secs"] for s in group) / 3600
        fighting = sum(s["fighting"] for s in group) / 3600
        ttks = [k["ttk"] for k in kills if k["ttk"] is not None]
        credits = sum(s["credits"] for s in group)
        row = [key[0], key[1], names.get(key, ""), len(group), f"{hours:.2f}", f"{fighting:.2f}", len(kills), len(ttks), sum(s["kills_fighter"] for s in group),
               f"{len(kills) / hours:.1f}" if hours else "", med(ttks), p90(ttks), med([k["interval"] for k in kills]), credits, f"{credits / hours:.0f}" if hours else "",
               sum(s["under_attack"] for s in group), sum(s["hull_hits"] for s in group), sum(s["shields_down"] for s in group), sum(s["died"] for s in group)]
        print(",".join(str(x) for x in row))


if __name__ == "__main__":
    main()
