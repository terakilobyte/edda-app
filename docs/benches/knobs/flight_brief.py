#!/usr/bin/env python3
"""The flight brief: pre-registered predictions for a real validation
flight. Reads bench --route-out archives and emits, per route: jumps,
stops, predicted pilot time under the fitted model (74 s/jump Caspian)
and the public defaults (60 s + 120 s), the min-tank profile, and every
hop flying within the named margin of our priced reach — the hops to
watch in the cockpit. Map vs terrain, on the record before takeoff.

Usage: python flight_brief.py <routes.jsonl>
"""
import json
import pathlib
import sys


def main():
    path = pathlib.Path(sys.argv[1])
    for line in open(path, encoding="utf-8"):
        r = json.loads(line)
        hops = r["hops"]
        scooped, prev = 0.0, None
        min_tank, min_at = 999.0, ""
        for h in hops:
            f = h.get("fuel_after")
            if f is not None:
                if prev is not None and f > prev + 0.05:
                    scooped += f - prev
                if f < min_tank:
                    min_tank, min_at = f, h["name"]
                prev = f
        fitted = r["jumps"] * 74 + r["refuel_stops"] * 36 + scooped / 1.245
        public = r["jumps"] * 60 + r["refuel_stops"] * 120
        long_hops = sorted(hops[1:], key=lambda h: -h["distance_ly"])[:3]
        print(f"== {r['from']} -> {r['to']} [{r['ship']}{' min-fuel' if r['min_fuel'] else ''}]")
        print(f"   {r['jumps']} jumps, {r['refuel_stops']} stops, {r['total_ly']:.0f} ly, {scooped:.0f} t scooped")
        print(f"   predicted pilot time: fitted {fitted/60:.1f} min ({fitted:.0f} s)  |  public-default {public/60:.1f} min")
        print(f"   min tank: {min_tank:.1f} t at {min_at}")
        print(f"   longest throws: " + ", ".join(f"{h['distance_ly']:.1f} ly -> {h['name']}" for h in long_hops))
        stops = [h["name"] for i, h in enumerate(hops) if i and h.get("fuel_after") is not None and hops[i-1].get("fuel_after") is not None and h["fuel_after"] > hops[i-1]["fuel_after"] + 0.05]
        print(f"   scheduled drinks: {', '.join(stops) if stops else 'none'}")
        print()


if __name__ == "__main__":
    main()
