#!/usr/bin/env python3
"""Item 34 rung 1 verdict: crossing-replan (cr1) vs shipped (cr0), A/B
from crossing_matrix.jsonl. Per (from,to,ship) cell: median-of-reps
jumps/stops/tonnes/wall for both arms, scored two ways — the flat
public judge (60 s/jump + 120 s/stop) and the fitted Caspian judge
(74 s/jump + 36 s/stop + est_refuel_s). Movers (x3 reps) also report
rep spread so a knife-edge re-roll can't masquerade as a win.

Usage: python crossing_matrix_report.py crossing_matrix.jsonl
"""
import json
import statistics
import sys
from collections import defaultdict


def med(rows, k):
    return statistics.median(r[k] for r in rows)


def main():
    cells = defaultdict(lambda: defaultdict(list))
    for line in open(sys.argv[1], encoding="utf-8"):
        r = json.loads(line)
        cells[(r["from"], r["to"], r["ship"])][r["cr"]].append(r)

    def flat(j, s):
        return j * 60 + s * 120

    def fitted(j, s, ref_s):
        return j * 74 + s * 36 + ref_s

    tot = {"flat": 0.0, "fitted": 0.0, "wall": 0.0}
    n_better = n_worse = n_same = 0
    print(f"{'route':<52} {'cr0 j/s':>9} {'cr1 j/s':>9} {'d_flat':>8} {'d_fit':>8} {'d_t':>7} {'d_wall':>8}")
    for key in sorted(cells):
        arms = cells[key]
        if 0 not in arms or 1 not in arms:
            print(f"!! incomplete cell {key}: arms {sorted(arms)}")
            continue
        a, b = arms[0], arms[1]
        j0, s0 = med(a, "jumps"), med(a, "refuel_stops")
        j1, s1 = med(b, "jumps"), med(b, "refuel_stops")
        r0, r1 = med(a, "est_refuel_s"), med(b, "est_refuel_s")
        t0, t1 = med(a, "refuel_tonnes"), med(b, "refuel_tonnes")
        w0, w1 = med(a, "elapsed_ms"), med(b, "elapsed_ms")
        d_flat = flat(j1, s1) - flat(j0, s0)
        d_fit = fitted(j1, s1, r1) - fitted(j0, s0, r0)
        tot["flat"] += d_flat
        tot["fitted"] += d_fit
        tot["wall"] += w1 - w0
        if d_fit < -1:
            n_better += 1
        elif d_fit > 1:
            n_worse += 1
        else:
            n_same += 1
        name = f"{key[0][:18]}->{key[1][:18]} {key[2]}"
        spread = ""
        if len(a) > 1:
            js0 = sorted(r["jumps"] for r in a)
            js1 = sorted(r["jumps"] for r in b)
            spread = f"   reps j cr0={js0} cr1={js1}"
        print(f"{name:<52} {int(j0)}/{int(s0):>2} {'':>3} {int(j1)}/{int(s1):>2} {'':>3} {d_flat:>+8.0f} {d_fit:>+8.0f} {t1-t0:>+7.1f} {w1-w0:>+8.0f}{spread}")
    print()
    print(f"cells: {n_better} better / {n_worse} worse / {n_same} same (fitted seconds)")
    print(f"net: flat {tot['flat']:+.0f} s, fitted {tot['fitted']:+.0f} s, wall {tot['wall']:+.0f} ms")


if __name__ == "__main__":
    main()
