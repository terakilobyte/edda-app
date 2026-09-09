#!/usr/bin/env python3
"""Grade one sweep CSV against another. Joins on (from,to,ship),
reports per-cell deltas in jumps/refuels/wall and the flat public
seconds (60 s/jump + 120 s/stop — the shipped unfitted judge), then
the better/worse/same tally and nets. Cells missing from either side
(no route, skips) are listed, never silently dropped.

Usage: python sweep_compare.py <baseline.csv> <candidate.csv>
"""
import csv
import sys


def load(path):
    rows = {}
    for r in csv.DictReader(l for l in open(path, encoding="utf-8") if not l.startswith("#")):
        key = (r["route_from"], r["route_to"], r["ship"])
        rows[key] = r
    return rows


def main():
    base, cand = load(sys.argv[1]), load(sys.argv[2])
    flat = lambda r: int(r["jumps"]) * 60 + int(r["refuels"]) * 120
    n_b = n_w = n_s = 0
    net = net_wall = 0
    print(f"{'route':<58} {'base j/r':>9} {'cand j/r':>9} {'d_flat':>8} {'d_wall':>8}")
    for key in sorted(set(base) | set(cand)):
        b, c = base.get(key), cand.get(key)
        name = f"{key[0][:20]}->{key[1][:20]} {key[2]}"
        if not b or not c or not b["jumps"] or not c["jumps"]:
            note = (c or b or {}).get("note", "")
            missing = "only-baseline" if not c else "only-candidate" if not b else ""
            if (b and not b["jumps"]) and (c and not c["jumps"]):
                print(f"{name:<58} {'—':>9} {'—':>9} {'both no-route':>17}  {note}")
            else:
                print(f"{name:<58} !! {missing or 'one side routeless'}: {note}")
            continue
        d = flat(c) - flat(b)
        dw = int(c["wall_ms"]) - int(b["wall_ms"])
        net += d
        net_wall += dw
        n_b += d < 0
        n_w += d > 0
        n_s += d == 0
        mark = "" if d == 0 else ("  WIN" if d < 0 else "  LOSS")
        print(f"{name:<58} {b['jumps']}/{b['refuels']:>3} {c['jumps']}/{c['refuels']:>3} {d:>+8} {dw:>+8}{mark}")
    print()
    print(f"cells: {n_b} better / {n_w} worse / {n_s} same (flat seconds)")
    print(f"net: {net:+} flat s, wall {net_wall:+} ms")


if __name__ == "__main__":
    main()
