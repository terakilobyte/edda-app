#!/usr/bin/env python3
"""Item 23 residual: where do two directions of the same corridor take
different lanes? Parses two `bench --hops` dumps (with @[x y z]
positions), flips the second onto the first's direction, and per 5%
band along the corridor axis prints each route's mean lateral offset
from the straight line, the lateral separation between the two lanes,
jumps, boosted share, and plain-ly share. The band where separation
jumps is where the ascending frontier leaves the descending chain.

Usage: python lane_diff.py fwd_hops.txt rev_hops.txt [band_pct]
"""
import math
import re
import sys

HOP = re.compile(r"^(.{1,60}?)\s{2,}(\S{1,4})\s+([\d.]+) ly (BOOST )?.*@\[(-?\d+) (-?\d+) (-?\d+)\]", re.M)


def parse(path, flip):
    hops = []
    for m in HOP.finditer(open(path, encoding="utf-8", errors="replace").read()):
        boost = bool(m.group(4))
        pos = (float(m.group(5)), float(m.group(6)), float(m.group(7)))
        hops.append((pos, boost, float(m.group(3))))
    if flip:
        hops = hops[::-1]
    return hops


def main():
    fwd = parse(sys.argv[1], False)
    rev = parse(sys.argv[2], True)
    band = float(sys.argv[3]) if len(sys.argv) > 3 else 5.0
    a, b = fwd[0][0], fwd[-1][0]
    ab = [b[i] - a[i] for i in range(3)]
    L = math.sqrt(sum(v * v for v in ab))
    u = [v / L for v in ab]

    def project(hops):
        out = []
        for pos, boost, d in hops:
            w = [pos[i] - a[i] for i in range(3)]
            t = sum(w[i] * u[i] for i in range(3))
            lat = [w[i] - t * u[i] for i in range(3)]
            out.append((t / L, lat, boost, d))
        return out

    F, R = project(fwd), project(rev)
    n = int(100 / band)
    print(f"corridor {L:.0f} ly; fwd {len(fwd)} hops, rev {len(rev)} hops (flipped)")
    print(f"{'band':>6} {'fwd_lat':>8} {'rev_lat':>8} {'sep':>8} {'fwd_j':>6} {'rev_j':>6} {'fwd_boost%':>10} {'rev_boost%':>10} {'fwd_plain_ly':>12} {'rev_plain_ly':>12}")
    for k in range(n):
        lo, hi = k / n, (k + 1) / n
        fs = [h for h in F if lo <= h[0] < hi]
        rs = [h for h in R if lo <= h[0] < hi]
        if not fs and not rs:
            continue

        def mean_lat(hs):
            if not hs:
                return None
            m = [sum(h[1][i] for h in hs) / len(hs) for i in range(3)]
            return m

        fm, rm = mean_lat(fs), mean_lat(rs)
        mag = lambda v: math.sqrt(sum(x * x for x in v)) if v else float("nan")
        sep = mag([fm[i] - rm[i] for i in range(3)]) if fm and rm else float("nan")
        bp = lambda hs: 100 * sum(h[2] for h in hs) / len(hs) if hs else float("nan")
        plain = lambda hs: sum(h[3] for h in hs if not h[2])
        print(
            f"{int(lo*100):>5}% {mag(fm):>8.0f} {mag(rm):>8.0f} {sep:>8.0f} {len(fs):>6} {len(rs):>6} {bp(fs):>10.0f} {bp(rs):>10.0f} {plain(fs):>12.0f} {plain(rs):>12.0f}"
        )


if __name__ == "__main__":
    main()
