#!/usr/bin/env python3
"""Fit the supercruise time curve from the commander's own journal.

The constants in crates/ed-route/src/cost.rs (SUPERCRUISE_BASE_SECONDS,
the ln coefficient) are ESTIMATED: the obvious journal window
(SupercruiseEntry -> Docked) measures commander idling, not travel
(R^2 ~ 0.001, ledgered). This harness measures the honest window
instead: SupercruiseEntry -> SupercruiseExit, paired to a Docked at a
known station within the grace window, joined to the station's
distance_to_arrival. Focused trade loops (2026-09-05: Talaria 270 ls
<-> Metz 5,394 ls) generate clean points.

Usage:
  python docs/benches/knobs/supercruise_fit.py <edda.sqlite3> <galaxy.sqlite3> [--since ISO]

--since restricts to events at/after the timestamp (e.g. the start of a
focused measuring session), keeping historical faffing out of the fit.
Legs under 30 s are dropped: those are repositioning re-entries beside
the station (a 5 s "approach" of 5,394 ls in the first run), not travel.

Emits the (ls, seconds) points as CSV on stdout (doctrine: bench
records are CSVs) and the least-squares fit of t = a + b*ln(1+ls) with
R^2, next to the current shipped constants (45, 22).
"""
import math
import sqlite3
import sys
from datetime import datetime, timezone


def ts(s):
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc).timestamp()


def main(journal_db, galaxy_db, since=None):
    db = sqlite3.connect(f"file:{journal_db}?mode=ro", uri=True, timeout=30)
    gal = sqlite3.connect(f"file:{galaxy_db}?mode=ro", uri=True, timeout=30)
    rows = db.execute(
        """SELECT file, offset, ts, event,
                  json_extract(raw,'$.StationName'), json_extract(raw,'$.MarketID')
           FROM events
           WHERE event IN ('SupercruiseEntry','SupercruiseExit','Docked','FSDJump','StartJump','FuelScoop')
             AND ts >= COALESCE(?, '')
           ORDER BY file, offset""",
        (since,),
    ).fetchall()

    # Two approach shapes: an in-system hop (SupercruiseEntry -> Exit)
    # and the POST-JUMP arrival, which starts already in supercruise —
    # no entry event, so the window opens at the FSDJump itself (the
    # first harness missed every jump-arrival approach: zero points
    # from a real trade loop). Windows containing a FuelScoop are
    # flagged: scooping time rides inside them.
    points = []
    entry_at = None
    exit_at = None
    scooped = False
    for _, _, t, event, station, market_id in rows:
        if event in ("SupercruiseEntry", "FSDJump"):
            entry_at, exit_at, scooped = ts(t), None, False
        elif event == "StartJump":
            # Charging the next hyperjump: this leg is not an approach.
            entry_at, exit_at, scooped = None, None, False
        elif event == "FuelScoop":
            scooped = True
        elif event == "SupercruiseExit" and entry_at is not None:
            exit_at = ts(t)
        elif event == "Docked" and entry_at is not None and exit_at is not None:
            # Dock within 3 min of the drop = the drop WAS this approach.
            if ts(t) - exit_at <= 180:
                ls = None
                if market_id:
                    row = gal.execute(
                        "SELECT distance_to_arrival FROM sys_stations WHERE id = ?", (market_id,)
                    ).fetchone()
                    ls = row[0] if row else None
                secs = exit_at - entry_at
                if ls is not None and secs >= 30.0:
                    points.append((ls, secs, station or "?", scooped))
            entry_at, exit_at, scooped = None, None, False

    print("distance_ls,seconds,station,scooped")
    for ls, secs, station, scooped in points:
        print(f"{ls:.0f},{secs:.0f},{station},{int(scooped)}")

    # Scoop-free windows are the clean travel measurements; scooped
    # ones are reported but kept out of the fit.
    clean = [p for p in points if not p[3]]
    if len(clean) < 3:
        print(f"# only {len(clean)} scoop-free points of {len(points)}; fly more focused approaches", file=sys.stderr)
        return
    points = clean

    # Least squares on t = a + b*x with x = ln(1+ls).
    xs = [math.log(1.0 + p[0]) for p in points]
    ys = [p[1] for p in points]
    n = len(xs)
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    sxy = sum((x - mx) * (y - my) for x, y in zip(xs, ys))
    b = sxy / sxx if sxx else 0.0
    a = my - b * mx
    ss_res = sum((y - (a + b * x)) ** 2 for x, y in zip(xs, ys))
    ss_tot = sum((y - my) ** 2 for y in ys) or 1e-9
    r2 = 1.0 - ss_res / ss_tot
    print(f"# n={n} fit: t = {a:.1f} + {b:.1f}*ln(1+ls)  R^2={r2:.3f}", file=sys.stderr)
    print("# shipped: t = 45.0 + 22.0*ln(1+ls) (estimated)", file=sys.stderr)
    for ls in (270, 1000, 5394):
        print(
            f"#   {ls:>5} ls: fitted {a + b * math.log(1 + ls):.0f}s vs shipped {45 + 22 * math.log(1 + ls):.0f}s",
            file=sys.stderr,
        )


if __name__ == "__main__":
    since = None
    args = sys.argv[1:]
    if "--since" in args:
        i = args.index("--since")
        since = args[i + 1]
        args = args[:i] + args[i + 2:]
    main(args[0], args[1], since)
