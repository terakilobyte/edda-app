#!/usr/bin/env python3
"""Item 24d: join the planner's expansion census against the field oracle.

    field_census_report.py <voxels.csv> <census.csv> [...]

<voxels.csv> comes from `field_density_probe <index> --save voxels.csv`
(one `vx,vy,vz,count` row per occupied 250-ly voxel, `# voxel_ly N`
header). Each <census.csv> comes from `bench ... --census census.csv`
(one `route,ship,phase,x,y,z` row per traced expansion; append-safe
across runs).

For every (route, ship) the report prints, per phase, how the search's
expansions distribute over field-density bins -- the occupied-voxel
quartile bands of the WHOLE disc (the agnostic normalization from the
24a probe), plus a dense-tax summary: the share of expansions landing
in the top band vs the share of the route's straight line that band
covers. A ratio well above 1 in every phase is the user's 24d claim --
high-density regions tax all phases, not just the coarse opening.
"""

import sys
from collections import defaultdict


def load_voxels(path):
    voxel_ly = 250.0
    table = {}
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line.startswith("#"):
                if "voxel_ly" in line:
                    voxel_ly = float(line.split()[-1])
                continue
            if not line:
                continue
            vx, vy, vz, count = line.split(",")
            table[(int(vx), int(vy), int(vz))] = int(count)
    return voxel_ly, table


def main():
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    voxel_ly, table = load_voxels(sys.argv[1])
    counts = sorted(table.values())
    q = lambda p: counts[int((len(counts) - 1) * p)]
    bands = [q(0.25), q(0.50), q(0.75)]  # occupied-voxel quartiles
    names = [f"q1(<={bands[0]})", f"q2(<={bands[1]})", f"q3(<={bands[2]})", f"q4(>{bands[2]})"]

    def band_of(count):
        for i, b in enumerate(bands):
            if count <= b:
                return i
        return 3

    def voxel_of(x, y, z):
        return (int(x // voxel_ly), int(y // voxel_ly), int(z // voxel_ly))

    # (route, ship) -> phase -> [band counts], plus positions for the
    # straight-line coverage baseline.
    hist = defaultdict(lambda: defaultdict(lambda: [0, 0, 0, 0]))
    ends = {}
    for path in sys.argv[2:]:
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#"):
                    continue
                route, ship, phase, x, y, z = line.split(",")
                x, y, z = float(x), float(y), float(z)
                count = table.get(voxel_of(x, y, z), 0)
                hist[(route, ship)][phase][band_of(count)] += 1
                lo, hi = ends.setdefault((route, ship), ([x, y, z], [x, y, z]))
                # Track the two extreme trace points as the corridor ends
                # (first and farthest-from-first seen).
                d2 = sum((a - b) ** 2 for a, b in zip([x, y, z], lo))
                if d2 > sum((a - b) ** 2 for a, b in zip(hi, lo)):
                    ends[(route, ship)] = (lo, [x, y, z])

    for (route, ship), phases in sorted(hist.items()):
        print(f"\n== {route} [{ship}] ==")
        # Straight-line band coverage: sample the corridor every half voxel.
        lo, hi = ends[(route, ship)]
        d = sum((a - b) ** 2 for a, b in zip(lo, hi)) ** 0.5
        steps = max(2, int(d / (voxel_ly / 2)))
        line_bands = [0, 0, 0, 0]
        for s in range(steps + 1):
            f = s / steps
            p = [lo[i] + (hi[i] - lo[i]) * f for i in range(3)]
            line_bands[band_of(table.get(voxel_of(*p), 0))] += 1
        line_total = sum(line_bands)
        print(f"{'phase':>8} {'exps':>9} " + " ".join(f"{n:>12}" for n in names) + f" {'dense-tax':>10}")
        line_share = line_bands[3] / line_total if line_total else 0.0
        for phase, bins in sorted(phases.items()):
            total = sum(bins)
            shares = [b / total for b in bins]
            tax = shares[3] / line_share if line_share > 0 else float("nan")
            print(
                f"{phase:>8} {total:>9} "
                + " ".join(f"{s:>11.1%}" for s in shares)
                + f" {tax:>9.2f}x"
            )
        print(
            f"{'line':>8} {line_total:>9} "
            + " ".join(f"{b / line_total:>11.1%}" for b in line_bands)
            + f" {'1.00x':>10}"
        )


if __name__ == "__main__":
    main()
