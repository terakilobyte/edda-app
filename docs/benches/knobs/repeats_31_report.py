#!/usr/bin/env python3
"""Item 31 repeats gate: medians over 3 reps per (mover cell, headroom
rung). A rung delta is REAL where the medians differ and the reps
agree; it's re-roll noise where reps straddle. Verdict feeds the
h2-default / h0-optimistic decision.

Usage: python repeats_31_report.py
"""
import json
import pathlib
import statistics

TJ = {"explorer": 74.0, "explorer86": 74.0, "mandalay": 64.0}
HERE = pathlib.Path(__file__).parent


def main():
    cells = {}
    for line in open(HERE / "repeats_31.jsonl", encoding="utf-8"):
        d = json.loads(line)
        k = (d["from"], d["to"], d["ship"], d["min_fuel"])
        cells.setdefault(k, {}).setdefault(d["headroom"], []).append(
            (d["jumps"] * TJ[d["ship"]] + d["est_refuel_s"], d["jumps"], d["refuel_stops"])
        )
    print(f"{'cell':<50}{'h10 med(spread)':>20}{'h2 med(spread)':>20}{'h0 med(spread)':>20}{'h0-h10':>8}")
    real_gain = noise = 0
    tot = {10: 0.0, 2: 0.0, 0: 0.0}
    for k in sorted(cells):
        row = {}
        for h in (10, 2, 0):
            reps = cells[k].get(h, [])
            scores = sorted(s for s, _, _ in reps)
            med = statistics.median(scores) if scores else float("nan")
            spread = (scores[-1] - scores[0]) if scores else float("nan")
            row[h] = (med, spread, reps)
            tot[h] += med
        d = row[0][0] - row[10][0]
        overlap = abs(d) <= max(row[0][1], row[10][1])
        tag = "noise" if overlap else ("REAL" if d < 0 else "REAL-LOSS")
        if overlap:
            noise += 1
        elif d < 0:
            real_gain += 1
        label = f"{k[0][:14]}->{k[1][:14]} [{k[2]}{' MF' if k[3] else ''}]"
        cellfmt = lambda h: f"{row[h][0]:.0f} ({row[h][1]:.0f})".rjust(20)
        print(f"{label:<50}{cellfmt(10)}{cellfmt(2)}{cellfmt(0)}{d:>8.0f} {tag}")
    print(f"\nmedian totals: h10 {tot[10]:.0f}  h2 {tot[2]:.0f} ({tot[10]-tot[2]:+.0f})  h0 {tot[0]:.0f} ({tot[10]-tot[0]:+.0f})")
    print(f"{real_gain} real gains, {noise} noise cells (of {len(cells)})")


if __name__ == "__main__":
    main()
