#!/usr/bin/env python3
"""Item 25c: compare the min-fuel toggle's lean arm across two studies —
B = full toggle (prune_gain.jsonl) vs C = toggle minus the scan gate
(prune_gain_C.jsonl, run under ED_MINFUEL_SCAN=0). Also sanity-checks
that the eager arms of both studies byte-match (the env override must
be inert in default mode).

Usage: python prune_decompose_report.py
"""
import json
import pathlib
import sys

TJ = {"explorer": 74.0, "explorer86": 74.0, "mandalay": 64.0}
HERE = pathlib.Path(__file__).parent


def load(path):
    eager, lean = {}, {}
    for line in open(path, encoding="utf-8"):
        d = json.loads(line)
        key = (d["from"], d["to"], d["ship"])
        (lean if d["min_fuel"] else eager)[key] = d
    return eager, lean


def score(d):
    return d["jumps"] * TJ[d["ship"]] + d["est_refuel_s"]


def main():
    eb, lb = load(HERE / "prune_gain.jsonl")
    ec, lc = load(HERE / "prune_gain_C.jsonl")
    drift = sum(
        1 for k in eb
        if k in ec and (eb[k]["jumps"], eb[k]["refuel_stops"]) != (ec[k]["jumps"], ec[k]["refuel_stops"])
    )
    print(f"eager-arm drift between studies: {drift} cells (volatile-cell flicker expected on a couple)")
    print(f"{'cell':<58}{'B_s':>9}{'C_s':>9}{'C-B':>8}{'dB_j':>6}{'dB_st':>6}")
    rows = []
    tb = tc = 0.0
    for k in sorted(lb):
        if k not in lc:
            continue
        b, c = lb[k], lc[k]
        sb, sc = score(b), score(c)
        tb += sb
        tc += sc
        rows.append((sc - sb, f"{k[0][:20]}->{k[1][:20]} [{k[2]}]", sb, sc,
                     c["jumps"] - b["jumps"], c["refuel_stops"] - b["refuel_stops"]))
    for d, label, sb, sc, dj, ds in sorted(rows):
        if abs(d) >= 1:
            print(f"{label:<58}{sb:>9.0f}{sc:>9.0f}{d:>8.0f}{dj:>6}{ds:>6}")
    same = sum(1 for d, *_ in rows if abs(d) < 1)
    print(f"\n{same}/{len(rows)} lean cells identical-scored; totals B {tb:.0f} s vs C {tc:.0f} s"
          f" -> C {'saves' if tc < tb else 'costs'} {abs(tb-tc):.0f} s ({100*(tb-tc)/tb:+.2f}%) across the matrix")


if __name__ == "__main__":
    main()
