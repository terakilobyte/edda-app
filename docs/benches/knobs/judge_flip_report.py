#!/usr/bin/env python3
"""Item 29: does a pure fuel judge rank like the seconds judge?

Pairs each cell's eager run under the seconds judge (the eager arm of
prune_gain_P.jsonl) with its --judge fuel run (judge_fuel.jsonl) and
reports flipped winners: cells where the two judges chose different
routes. For each flip, shows what the fuel judge paid in fitted seconds
and what it saved in tonnes — the map of where time is not fuel.

Usage: python judge_flip_report.py
"""
import json
import pathlib

TJ = {"explorer": 74.0, "explorer86": 74.0, "mandalay": 64.0}
HERE = pathlib.Path(__file__).parent


def load(path, want_lean):
    out = {}
    for line in open(path, encoding="utf-8"):
        d = json.loads(line)
        if d["min_fuel"] == want_lean:
            out[(d["from"], d["to"], d["ship"])] = d
    return out


def s(d):
    return d["jumps"] * TJ[d["ship"]] + d["est_refuel_s"]


def main():
    sec = load(HERE / "prune_gain_P.jsonl", False)
    fuel = load(HERE / "judge_fuel.jsonl", False)
    flips, same = [], 0
    for k in sorted(sec):
        if k not in fuel:
            continue
        a, b = sec[k], fuel[k]
        if (a["jumps"], a["refuel_stops"], round(a["total_ly"])) == (b["jumps"], b["refuel_stops"], round(b["total_ly"])):
            same += 1
            continue
        flips.append((k, a, b))
    print(f"{same}/{same + len(flips)} cells identical under both judges; {len(flips)} flips\n")
    if flips:
        print(f"{'cell':<56}{'sec_judge':>16}{'fuel_judge':>16}{'cost_s':>8}{'saved_t':>9}")
        for (f, t, ship), a, b in flips:
            print(
                f"{f[:18]}->{t[:18]} [{ship}]"[:56].ljust(56)
                + f"{a['jumps']}j/{a['refuel_stops']}s/{a['refuel_tonnes']:.0f}t".rjust(16)
                + f"{b['jumps']}j/{b['refuel_stops']}s/{b['refuel_tonnes']:.0f}t".rjust(16)
                + f"{s(b) - s(a):>8.0f}{a['refuel_tonnes'] - b['refuel_tonnes']:>9.0f}"
            )
        cost = sum(s(b) - s(a) for _, a, b in flips)
        saved = sum(a["refuel_tonnes"] - b["refuel_tonnes"] for _, a, b in flips)
        print(f"\nfuel judge total: {cost:+.0f} fitted seconds for {saved:+.0f} tonnes across the flips")


if __name__ == "__main__":
    main()
