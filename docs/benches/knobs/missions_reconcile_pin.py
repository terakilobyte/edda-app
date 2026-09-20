"""Measure, before building (CLAUDE.md), the two mission questions the
ODEliteTracker review raised (docs/2026-09-20-odelitetracker-mission-gap-review.md):

1. Startup reconciliation: at each startup `Missions` event, how many
   missions EDDA would still hold live (accepted, no terminal event, not
   expired) that the game's own list does NOT carry as Active — dead
   missions that would linger on the HUD until Expiry. Also how many the
   game lists as Failed that EDDA has no MissionFailed for.
2. Docking with hand-ins ready: how often a `Docked` lands at a station
   that is the hand-in of a mission the game has redirected (a
   do-then-return kind) but not yet completed, and how many per dock —
   sizing the "ready to hand in here" line.

Reads the commander's own store (edda.sqlite3, table events). Prints CSV.
Usage: python docs/benches/knobs/missions_reconcile_pin.py [edda.sqlite3] > docs/benches/<date>-missions-reconcile-pin.csv
"""
import json
import os
import sqlite3
import sys

REROUTE_ONLY = {"delivery", "courier", "collect", "altruism", "altruismcredits", "passengervip", "passengerbulk", "smuggle"}


def kind_of(name):
    n = (name or "").removeprefix("Mission_").lower()
    return n.split("_")[0]


def main():
    db = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.environ.get("LOCALAPPDATA", ""), "edda", "edda.sqlite3")
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    rows = con.execute(
        "SELECT ts, event, raw FROM events WHERE event IN ('Missions','MissionAccepted','MissionCompleted','MissionFailed','MissionAbandoned','MissionRedirected','Docked') ORDER BY ts, file, offset"
    ).fetchall()
    live = {}          # id -> dict(name, expiry, kind, hand_in, ready)
    startup_rows = []
    dock_rows = []
    docks = 0
    docks_with_ready = 0
    ready_per_dock = []
    stale_total = 0
    failed_unseen_total = 0
    for ts, event, raw in rows:
        v = json.loads(raw)
        if event == "MissionAccepted":
            live[v["MissionID"]] = {"name": v.get("Name"), "expiry": v.get("Expiry"), "kind": kind_of(v.get("Name")), "hand_in": None, "ready": False}
        elif event in ("MissionCompleted", "MissionFailed", "MissionAbandoned"):
            live.pop(v.get("MissionID"), None)
        elif event == "MissionRedirected":
            m = live.get(v.get("MissionID"))
            if m:
                m["hand_in"] = (v.get("NewDestinationSystem"), v.get("NewDestinationStation"))
                if m["kind"] not in REROUTE_ONLY:
                    m["ready"] = True
        elif event == "Docked":
            docks += 1
            here = (v.get("StarSystem"), v.get("StationName"))
            n = sum(1 for m in live.values() if m["ready"] and m["hand_in"] == here)
            if n:
                docks_with_ready += 1
                ready_per_dock.append(n)
                dock_rows.append((ts, here[1], here[0], n))
        elif event == "Missions":
            game_active = {a["MissionID"] for a in v.get("Active", [])}
            game_failed = {a["MissionID"] for a in v.get("Failed", [])}
            game_complete = {a["MissionID"] for a in v.get("Complete", [])}
            ours = {mid for mid, m in live.items() if not (m["expiry"] and m["expiry"] < ts)}
            stale = ours - game_active - game_failed - game_complete
            failed_unseen = game_failed & set(live)
            stale_total += len(stale)
            failed_unseen_total += len(failed_unseen)
            startup_rows.append((ts, len(ours), len(game_active), len(stale), len(failed_unseen), len(game_complete & set(live))))
            # ODET's guard: a fresh operation start (nothing to reconcile) is when the game lists none.
            # Reconcile as the game says: what it does not list is gone.
            for mid in list(live):
                if mid not in game_active and mid not in game_complete and mid not in game_failed:
                    live.pop(mid)
    print("# Pin: startup Missions reconciliation and docks with hand-ins ready, on the maintainer's store.")
    print(f"# startups={len(startup_rows)} stale_live_total={stale_total} failed_unseen_total={failed_unseen_total} docks={docks} docks_with_ready={docks_with_ready} max_ready_per_dock={max(ready_per_dock) if ready_per_dock else 0} mean_ready_per_dock={(sum(ready_per_dock)/len(ready_per_dock)) if ready_per_dock else 0:.2f}")
    print("kind,ts,ours_live,game_active,stale_ours_not_in_game,game_failed_we_missed,game_complete_still_ours_or_station,system,ready_here")
    for r in startup_rows:
        if r[3] or r[4] or r[5]:
            print("startup," + ",".join(str(x) for x in r) + ",,")
    for ts, st, sy, n in dock_rows:
        print(f"dock,{ts},,,,,,{st},{sy},{n}")


if __name__ == "__main__":
    main()
