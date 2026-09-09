#!/usr/bin/env python3
"""Item 31: the fuel-headroom ladder — 10 t (shipped) / 6.8 (one-jump
semantic) / 2 / 0 (Spansh parity). Per-cell fitted seconds and jumps
across rungs, per-mode totals, and strand detection (cells missing at a
rung that exist at the baseline = the search failed there — the
load-bearing alarm).

Usage: python headroom_report.py
"""
import json
import pathlib

TJ = {"explorer": 74.0, "explorer86": 74.0, "mandalay": 64.0}
HERE = pathlib.Path(__file__).parent
RUNGS = [("10", "prune_gain_P.jsonl"), ("6.8", "prune_gain_h6.8.jsonl"),
         ("2", "prune_gain_h2.jsonl"), ("0", "prune_gain_h0.jsonl")]


def load(name):
    out = {}
    for line in open(HERE / name, encoding="utf-8"):
        d = json.loads(line)
        out[(d["from"], d["to"], d["ship"], d["min_fuel"])] = d
    return out


def s(d):
    return d["jumps"] * TJ[d["ship"]] + d["est_refuel_s"]


def main():
    data = {r: load(f) for r, f in RUNGS}
    base = data["10"]
    for rung in data:
        missing = [k for k in base if k not in data[rung]]
        if missing:
            print(f"STRAND ALARM at headroom {rung}: {len(missing)} cells missing: {missing[:4]}")
    print(f"{'cell':<52}" + "".join(f"{'h' + r:>14}" for r, _ in RUNGS))
    tot = {r: [0.0, 0] for r, _ in RUNGS}
    interesting = []
    for k in sorted(base):
        row = []
        for r, _ in RUNGS:
            d = data[r].get(k)
            row.append(d)
            if d:
                tot[r][0] += s(d)
                tot[r][1] += d["jumps"]
        if row[0] and row[-1] and (row[0]["jumps"] != row[-1]["jumps"] or abs(s(row[0]) - s(row[-1])) > 30):
            interesting.append((s(row[0]) - s(row[-1]), k, row))
    for delta, k, row in sorted(interesting, reverse=True):
        label = f"{k[0][:16]}->{k[1][:16]} [{k[2]}{' MF' if k[3] else ''}]"
        cells = "".join(
            f"{d['jumps']}j/{s(d):.0f}".rjust(14) if d else f"{'—':>14}" for d in row
        )
        print(f"{label:<52}{cells}")
    print()
    for r, _ in RUNGS:
        sec, j = tot[r]
        print(f"headroom {r:>4}: total {sec:>9.0f} s, {j} jumps"
              + (f"   ({tot['10'][0] - sec:+.0f} s, {tot['10'][1] - j:+d} j vs shipped)" if r != "10" else "   (shipped baseline)"))


if __name__ == "__main__":
    main()
