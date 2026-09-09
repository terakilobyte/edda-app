#!/usr/bin/env python3
"""Refuel-detour analysis of two `bench --hops` dumps (item 21 follow-up).

Hypothesis (user): the ascending route falls off the highway because
its refuel stops are DETOURS (off-chain scoopables reached by short
plain hops) while the descending route's stops are PITSTOPS (on-chain).

From each dump: refuel events = fuel_after rises vs the previous hop.
Around each stop, measure the local hop pattern: the stop hop's own
distance, whether it and its neighbours are boosted, and the plain-hop
lightyears spent within +-2 hops of a refuel vs elsewhere.

Usage: python fuel_probe.py fwd_hops.txt rev_hops.txt
"""
import re
import sys

HOP = re.compile(r"^(.*?)\s{2,}(\S+)\s+([\d.]+) ly(\s+BOOST)?(?:\s+fuel ([\d.]+))?", re.M)


def parse(path):
    hops = []
    for m in HOP.finditer(open(path, encoding="utf-8", errors="replace").read()):
        hops.append(dict(name=m.group(1).strip(), cls=m.group(2), d=float(m.group(3)),
                         boost=bool(m.group(4)), fuel=float(m.group(5)) if m.group(5) else None))
    return hops


def analyse(tag, hops):
    refuels = []
    prev_fuel = None
    for i, h in enumerate(hops):
        if h["fuel"] is not None and prev_fuel is not None and h["fuel"] > prev_fuel + 0.5:
            refuels.append(i)
        if h["fuel"] is not None:
            prev_fuel = h["fuel"]
    near = set()
    for i in refuels:
        near.update(range(max(i - 2, 0), min(i + 3, len(hops))))
    plain_near = sum(h["d"] for i, h in enumerate(hops) if i in near and not h["boost"])
    plain_far = sum(h["d"] for i, h in enumerate(hops) if i not in near and not h["boost"])
    boosted = sum(1 for h in hops if h["boost"])
    stop_d = [hops[i]["d"] for i in refuels]
    stop_boosted = sum(1 for i in refuels if hops[i]["boost"])
    total = sum(h["d"] for h in hops)
    print(f"{tag}: {len(hops)} hops / {total:.0f} ly, {len(refuels)} refuels, {boosted} boosted")
    print(f"  refuel-hop distance: mean {sum(stop_d)/len(stop_d):.0f} ly, "
          f"boosted arrivals {stop_boosted}/{len(refuels)} ({100*stop_boosted/len(refuels):.0f}%)")
    print(f"  plain ly within +-2 hops of a refuel: {plain_near:.0f} ({100*plain_near/total:.1f}% of route)")
    print(f"  plain ly elsewhere:                   {plain_far:.0f} ({100*plain_far/total:.1f}% of route)")
    return plain_near / total


def main():
    a = analyse("forward (ascending)", parse(sys.argv[1]))
    b = analyse("reverse (descending)", parse(sys.argv[2]))
    print(f"\nrefuel-adjacent plain fraction: ascending {100*a:.1f}% vs descending {100*b:.1f}%")
    print("verdict hint: large gap => refuel detours are the chain-fall mechanism")


if __name__ == "__main__":
    main()
