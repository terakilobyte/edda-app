#!/usr/bin/env python3
"""Measure the trade leg model's time constants against the commander's
own journal (maintainer, 2026-09-09: "time args should be optional to the api
with sensible defaults if omitted" - and doctrine: measure before the
client sends anything).

The server's ed_route::cost::Timing defaults, per leg:
  jump 18 s, undock 60 s, docking 60 s, market 30 s, supercruise base 45 s.

Phases, from EDDA's own event store (read-only):
  jump         FSDJump -> FSDJump with nothing but transit between
  undock       Undocked -> first FSDJump (no dock between)
  docking      DockingGranted -> Docked
  supercruise  arrival FSDJump (or SupercruiseEntry) -> SupercruiseExit,
               with the following Docked's DistFromStarLS, bucketed and
               fitted (base = intercept, slope = s per ls)
  market       Docked -> Undocked with a MarketBuy/MarketSell between
  end_to_end   arrival FSDJump -> Docked, for reference

Usage: python docs/benches/knobs/trade_timing_profile.py [edda.sqlite3] [--days N] [--csv out.csv]
"""
import json
import os
import sqlite3
import statistics
import sys
from datetime import datetime, timedelta, timezone

DEFAULT_DB = os.environ.get("EDDA_DB", os.path.join(os.environ.get("LOCALAPPDATA", ""), "EDDA", "edda.sqlite3"))
CONSTANTS = {"jump": 18.0, "undock": 60.0, "docking": 60.0, "market": 30.0, "supercruise_base": 45.0}
BOUNDS = {"jump": (5, 300), "undock": (10, 600), "docking": (5, 600), "supercruise": (5, 3600), "market": (20, 1800), "end_to_end": (10, 3600)}
TRANSIT_ONLY = {"FSDJump", "FuelScoop", "StartJump", "FSSSignalDiscovered", "FSSDiscoveryScan", "Scan", "NavRoute", "NavRouteClear", "ReceiveText", "Music", "ShipTargeted", "SupercruiseEntry", "JetConeBoost", "FSSAllBodiesFound", "MultiSellExplorationData", "CodexEntry", "ReservoirReplenished", "Friends", "Shutdown", "Fileheader", "Commander", "LoadGame", "Loadout", "Materials", "Rank", "Progress", "Statistics", "Location", "Powerplay", "Reputation", "EngineerProgress", "SquadronStartup", "Missions", "Cargo", "Status"}


def ts(s):
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=timezone.utc)


def stats(name, xs):
    lo, hi = BOUNDS[name]
    xs = [x for x in xs if lo <= x <= hi]
    if not xs:
        return {"n": 0}
    xs.sort()
    q = statistics.quantiles(xs, n=4) if len(xs) >= 4 else [xs[0], xs[len(xs) // 2], xs[-1]]
    return {"n": len(xs), "median": statistics.median(xs), "p25": q[0], "p75": q[2], "mean": statistics.fmean(xs)}


def main():
    args = sys.argv[1:]
    db = DEFAULT_DB
    days = None
    csv_out = None
    i = 0
    while i < len(args):
        if args[i] == "--days":
            days = int(args[i + 1]); i += 2
        elif args[i] == "--csv":
            csv_out = args[i + 1]; i += 2
        else:
            db = args[i]; i += 1
    conn = sqlite3.connect(f"file:{db}?immutable=1", uri=True)
    since = (datetime.now(timezone.utc) - timedelta(days=days)).strftime("%Y-%m-%dT%H:%M:%SZ") if days else "0000"
    rows = conn.execute(
        "SELECT ts, event, raw FROM events WHERE ts >= ? AND event IN ('FSDJump','Undocked','Docked','DockingGranted','SupercruiseEntry','SupercruiseExit','MarketBuy','MarketSell','Location') ORDER BY ts, offset",
        (since,),
    ).fetchall()
    # For the "transit only" jump gap we need every event in between; a
    # second pass over all events flags gaps with non-transit activity.
    all_events = conn.execute("SELECT ts, event FROM events WHERE ts >= ? ORDER BY ts, offset", (since,)).fetchall()
    gaps = {k: [] for k in ("jump", "undock", "docking", "supercruise", "market", "end_to_end")}
    sc_pairs = []  # (seconds, ls)

    # jump gaps: walk all events, reset on anything non-transit
    last_jump = None
    for t, ev in all_events:
        if ev == "FSDJump":
            if last_jump is not None:
                gaps["jump"].append((ts(t) - last_jump).total_seconds())
            last_jump = ts(t)
        elif ev not in TRANSIT_ONLY:
            last_jump = None

    # phase pairs
    undocked_at = None
    granted_at = None
    sc_start = None
    arrival_at = None
    docked_at = None
    traded = False
    for t, ev, raw in rows:
        t = ts(t)
        if ev == "Undocked":
            undocked_at = t
            if docked_at is not None and traded:
                gaps["market"].append((t - docked_at).total_seconds())
            docked_at = None
            traded = False
            arrival_at = None
        elif ev == "FSDJump":
            if undocked_at is not None:
                gaps["undock"].append((t - undocked_at).total_seconds())
                undocked_at = None
            arrival_at = t
            sc_start = t
        elif ev == "SupercruiseEntry":
            sc_start = t
        elif ev == "SupercruiseExit":
            if sc_start is not None:
                pending_sc = (t - sc_start).total_seconds()
                sc_start = None
            else:
                pending_sc = None
            last_exit = (t, pending_sc)
        elif ev == "DockingGranted":
            granted_at = t
        elif ev == "Docked":
            docked_at = t
            traded = False
            if granted_at is not None:
                gaps["docking"].append((t - granted_at).total_seconds())
                granted_at = None
            v = json.loads(raw)
            ls = v.get("DistFromStarLS")
            try:
                if last_exit[1] is not None and ls is not None and (t - last_exit[0]).total_seconds() < 900:
                    sc = last_exit[1]
                    gaps["supercruise"].append(sc)
                    sc_pairs.append((sc, float(ls)))
            except NameError:
                pass
            if arrival_at is not None and (t - arrival_at).total_seconds() < 3600:
                gaps["end_to_end"].append((t - arrival_at).total_seconds())
            arrival_at = None
        elif ev in ("MarketBuy", "MarketSell"):
            traded = True

    out = {k: stats(k, v) for k, v in gaps.items()}
    # supercruise fit: seconds = base + slope * ls (least squares on bounded pairs)
    pairs = [(s, ls) for s, ls in sc_pairs if BOUNDS["supercruise"][0] <= s <= BOUNDS["supercruise"][1] and 0 < ls < 200000]
    fit = None
    if len(pairs) >= 10:
        n = len(pairs)
        mx = sum(ls for _, ls in pairs) / n
        my = sum(s for s, _ in pairs) / n
        sxx = sum((ls - mx) ** 2 for _, ls in pairs)
        sxy = sum((ls - mx) * (s - my) for s, ls in pairs)
        slope = sxy / sxx if sxx else 0.0
        fit = {"n": n, "base": my - slope * mx, "slope_s_per_ls": slope}
    buckets = {}
    for s, ls in pairs:
        b = "<100" if ls < 100 else "<500" if ls < 500 else "<2000" if ls < 2000 else ">=2000"
        buckets.setdefault(b, []).append(s)

    label = f"last {days} days" if days else "all journals"
    print(f"trade timing profile - {label} - {db}")
    print(f"{'phase':12} {'n':>5} {'median':>8} {'p25':>7} {'p75':>7} {'const':>7} {'ratio':>6}")
    lines = ["phase,n,median_s,p25_s,p75_s,mean_s,constant_s,median_over_constant"]
    for k in ("jump", "undock", "docking", "market", "supercruise", "end_to_end"):
        st = out[k]
        c = CONSTANTS.get(k if k != "supercruise" else "supercruise_base")
        if st["n"] == 0:
            print(f"{k:12} {0:>5}")
            lines.append(f"{k},0,,,,,{c or ''},")
            continue
        ratio = (st["median"] / c) if c else None
        print(f"{k:12} {st['n']:>5} {st['median']:>8.1f} {st['p25']:>7.1f} {st['p75']:>7.1f} {c if c is not None else '':>7} {f'{ratio:.2f}' if ratio else '':>6}")
        lines.append(f"{k},{st['n']},{st['median']:.1f},{st['p25']:.1f},{st['p75']:.1f},{st['mean']:.1f},{c if c is not None else ''},{f'{ratio:.2f}' if ratio else ''}")
    if fit:
        print(f"supercruise fit: base {fit['base']:.1f} s + {fit['slope_s_per_ls']*1000:.2f} s per 1,000 ls (n={fit['n']})")
        lines.append(f"supercruise_fit_base,{fit['n']},{fit['base']:.1f},,,,45,{fit['base']/45:.2f}")
        lines.append(f"supercruise_fit_slope_s_per_kls,{fit['n']},{fit['slope_s_per_ls']*1000:.2f},,,,,")
    for b in ("<100", "<500", "<2000", ">=2000"):
        if b in buckets:
            xs = buckets[b]
            print(f"  supercruise ls {b:>6}: n={len(xs):>3} median {statistics.median(xs):.0f} s")
            lines.append(f"supercruise_ls_{b.replace('<','lt').replace('>=','ge')},{len(xs)},{statistics.median(xs):.1f},,,,,")
    if csv_out:
        with open(csv_out, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
        print("wrote", csv_out)


if __name__ == "__main__":
    main()
