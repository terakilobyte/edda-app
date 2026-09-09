#!/usr/bin/env python3
"""Item 31 flight gate: map vs terrain, from the pilot's own journals.

    flight_report.py <route.jsonl-line-or-file> <Journal...log> [...]

<route> is one bench --route-out line (or a file whose first matching
line is used) — the plan. Journals are the game's own record — the
terrain. The report aligns FSDJump events to the planned hops by
system name and prints, per hop: actual seconds since the previous
jump, actual vs planned fuel burn (the model-error measurement in the
wild), boost class, and adherence; then the summary that decides the
gate: total seat time vs the fitted prediction, mean/max per-hop fuel
model error, off-plan jumps, and scoop stops taken vs planned.
"""

import json
import sys
from datetime import datetime


def parse_ts(s):
    return datetime.strptime(s, "%Y-%m-%dT%H:%M:%SZ")


def main():
    route_path, journals = sys.argv[1], sys.argv[2:]
    route = None
    with open(route_path) as f:
        for line in f:
            line = line.strip()
            if line.startswith("{"):
                route = json.loads(line)
                break
    assert route, "no route line found"
    # The plan source can be the app's own followed route (the
    # active_route store / RouteView shape, {"route": {"hops": ...}})
    # rather than a bench --route-out line — after the maiden flight
    # graded 52/58 OFF-PLAN because the app replotted en route and the
    # archived bench route was stale, the app's store is the truer map.
    if "hops" not in route and isinstance(route.get("route"), dict):
        inner = route["route"]
        route = {
            "from": inner["hops"][0]["name"] if inner.get("hops") else "?",
            "to": inner["hops"][-1]["name"] if inner.get("hops") else "?",
            "ship": inner.get("ship"),
            "min_fuel": True,
            "jumps": inner.get("jumps", max(0, len(inner.get("hops", [])) - 1)),
            "refuel_stops": inner.get("refuel_stops", 0),
            "hops": [
                {"name": h["name"], "distance_ly": h.get("distance_ly", 0.0),
                 "boosted": h.get("boosted", False), "fuel_after": h.get("fuel_after")}
                for h in inner.get("hops", [])
            ],
        }
    hops = route["hops"]
    planned = {h["name"].lower(): h for h in hops}
    order = [h["name"].lower() for h in hops]

    jumps, scoops, starts = [], [], []
    for path in journals:
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                try:
                    o = json.loads(line)
                except json.JSONDecodeError:
                    continue
                if o.get("event") == "FSDJump":
                    jumps.append(o)
                elif o.get("event") == "StartJump" and o.get("JumpType") == "Hyperspace":
                    starts.append(o)
                elif o.get("event") == "FuelScoop":
                    scoops.append(o)
    jumps.sort(key=lambda o: o["timestamp"])

    # Clip to the flight: from the first jump into hop[1] (or later) to
    # the arrival at the final hop.
    names = [j.get("StarSystem", "").lower() for j in jumps]
    try:
        start_i = next(i for i, n in enumerate(names) if n in order[1:])
        end_i = max(i for i, n in enumerate(names) if n == order[-1])
    except (StopIteration, ValueError):
        sys.exit("journals never touch the planned route")
    flight = jumps[start_i : end_i + 1]

    print(f"plan: {route['from']} -> {route['to']} [{route.get('ship','?')}] "
          f"{route['jumps']} j / {route['refuel_stops']} st"
          + (" (min-fuel)" if route.get("min_fuel") else ""))
    print(f"{'hop':>4} {'system':<30} {'dt_s':>6} {'fuel_used':>9} {'planned':>8} {'err_t':>6}  flags")
    prev_t = None
    total_s = 0.0
    errs = []
    on_plan = 0
    plan_burn = {}
    for i in range(1, len(hops)):
        prev = hops[i - 1].get("fuel_after")
        cur = hops[i].get("fuel_after")
        if prev is not None and cur is not None and cur <= prev:
            plan_burn[hops[i]["name"].lower()] = prev - cur
    for j in flight:
        name = j.get("StarSystem", "").lower()
        t = parse_ts(j["timestamp"])
        dt = (t - prev_t).total_seconds() if prev_t else 0.0
        prev_t = t
        total_s += dt
        used = j.get("FuelUsed", 0.0)
        pb = plan_burn.get(name)
        err = (used - pb) if pb is not None else None
        if name in planned:
            on_plan += 1
        if err is not None:
            errs.append(abs(err))
        flags = []
        if name not in planned:
            flags.append("OFF-PLAN")
        if j.get("BoostUsed"):
            flags.append(f"boost x{j['BoostUsed']}")
        print(f"{len([1 for _ in [0]]) and flight.index(j):>4} {j.get('StarSystem','?')[:30]:<30} "
              f"{dt:>6.0f} {used:>9.2f} {pb if pb is not None else float('nan'):>8.2f} "
              f"{err if err is not None else float('nan'):>6.2f}  {' '.join(flags)}")

    tj = 74.0
    fitted = route["jumps"] * tj + route["refuel_stops"] * 36.0
    # Pre-registered contract (ledger bf10b16): total_time =
    # arrival_time - first_jump_time. The first jump's TIME is its
    # initiation — the first Hyperspace StartJump before the first
    # arrival — falling back to the first FSDJump timestamp when the
    # journals carry no StartJump.
    arrival_t = parse_ts(flight[-1]["timestamp"])
    first_arrival_t = parse_ts(flight[0]["timestamp"])
    first_start = [s0 for s0 in starts if parse_ts(s0["timestamp"]) <= first_arrival_t]
    if first_start:
        first_jump_t = parse_ts(max(first_start, key=lambda o: o["timestamp"])["timestamp"])
        basis = "first StartJump"
    else:
        first_jump_t = first_arrival_t
        basis = "first FSDJump (no StartJump in journals)"
    total_contract = (arrival_t - first_jump_t).total_seconds()
    print(f"\nTOTAL ({basis} -> arrival): {total_contract:.0f} s = {total_contract/60:.1f} min"
          f" | pre-registered fitted prediction: {fitted:.0f} s ({fitted/60:.1f} min)")
    print(f"inter-arrival sum (cross-check): {total_s:.0f} s ({total_s/60:.1f} min) over {len(flight)} jumps")
    print(f"adherence: {on_plan}/{len(flight)} jumps on plan"
          f" | fuel model error: mean {sum(errs)/len(errs):.2f} t, max {max(errs):.2f} t over {len(errs)} matched hops"
          if errs else "no matched hops for fuel error")
    print(f"scoop events in window: {sum(1 for s in scoops if flight and parse_ts(flight[0]['timestamp']) <= parse_ts(s['timestamp']) <= parse_ts(flight[-1]['timestamp']))}"
          f" (planned stops: {route['refuel_stops']})")


if __name__ == "__main__":
    main()
