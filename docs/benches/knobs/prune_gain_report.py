#!/usr/bin/env python3
"""Eager-vs-lean time-in-seat report from a prune_gain.sh jsonl.

Pairs each cell's default and --min-fuel runs, scores both on the fitted
seconds model (t_jump per ship + bench's est_refuel_s, which already
includes 36 s/stop + tonnes/scoop-rate), and prints per-cell deltas plus
per-ship and overall totals. The tonnage column is the fuel-carry
feedback the user named: every eager top-up buys weight, weight buys
burn, burn buys the next top-up.

Usage: python prune_gain_report.py [prune_gain.jsonl]
"""
import json
import pathlib
import sys

TJ = {"explorer": 74.0, "explorer86": 74.0, "mandalay": 64.0}


def score(d):
    return d["jumps"] * TJ[d["ship"]] + d["est_refuel_s"]


def main():
    path = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else pathlib.Path(__file__).parent / "prune_gain.jsonl")
    cells = {}
    for line in open(path, encoding="utf-8"):
        d = json.loads(line)
        key = (d["from"], d["to"], d["ship"])
        cells.setdefault(key, {})["lean" if d["min_fuel"] else "eager"] = d
    print(f"{'cell':<58}{'eager_s':>9}{'lean_s':>9}{'saved_s':>9}{'%':>6}{'d_stops':>8}{'d_tonnes':>9}{'d_jumps':>8}")
    totals = {}
    rows = []
    for (f, t, ship), modes in sorted(cells.items()):
        if "eager" not in modes or "lean" not in modes:
            print(f"{f}->{t} [{ship}]: INCOMPLETE PAIR")
            continue
        e, l = modes["eager"], modes["lean"]
        es, ls = score(e), score(l)
        rows.append((es - ls, f"{f[:20]}->{t[:20]} [{ship}]", es, ls,
                     l["refuel_stops"] - e["refuel_stops"],
                     l["refuel_tonnes"] - e["refuel_tonnes"],
                     l["jumps"] - e["jumps"]))
        tot = totals.setdefault(ship, [0.0, 0.0])
        tot[0] += es
        tot[1] += ls
    for saved, label, es, ls, ds, dt, dj in sorted(rows, reverse=True):
        print(f"{label:<58}{es:>9.0f}{ls:>9.0f}{saved:>9.0f}{100*saved/es:>5.1f}%{ds:>8}{dt:>9.0f}{dj:>8}")
    print()
    ge, gl = 0.0, 0.0
    for ship, (es, ls) in sorted(totals.items()):
        ge += es
        gl += ls
        print(f"{ship:<12} eager {es:>9.0f} s   lean {ls:>9.0f} s   saved {es-ls:>8.0f} s ({100*(es-ls)/es:.1f}%)")
    print(f"{'ALL':<12} eager {ge:>9.0f} s   lean {gl:>9.0f} s   saved {ge-gl:>8.0f} s ({100*(ge-gl)/ge:.1f}%)"
          f"   = {(ge-gl)/60:.0f} pilot-minutes across the matrix")


if __name__ == "__main__":
    main()
