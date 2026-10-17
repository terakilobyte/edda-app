#!/usr/bin/env python
"""variant_first_found.py <dir-with-*.err> [--csv out.csv]

Per variant family, how often it produced a route and how fast, from the
`variant outcome:` lines a debug matrix run prints (ED_PLOT_DEBUG=1 in
edda_matrix.sh). The order of families by median time-to-route is the
input to KIND_ORDER in long_range.rs (planner unification R1, Task 7):
the box runs a long plot on three threads, so the three quickest families
start first.
"""
import glob
import os
import re
import statistics
import sys

LINE = re.compile(r"variant outcome: wave x(\d+) #(\d+) (.+?) (route|none) (\d+) ms, coarse (\d+) expansions, stall (\d+) ly")


def main(argv):
    folder = argv[1]
    out_csv = argv[3] if len(argv) > 3 and argv[2] == "--csv" else None
    by_kind = {}
    routes = 0
    for path in sorted(glob.glob(os.path.join(folder, "*.err"))):
        routes += 1
        seen_first = False
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                m = LINE.search(line)
                if not m:
                    continue
                wave, idx, kind, outcome, ms, exps, stall = m.groups()
                k = by_kind.setdefault(kind, {"runs": 0, "routes": 0, "ms": [], "first": 0})
                k["runs"] += 1
                if outcome == "route":
                    k["routes"] += 1
                    k["ms"].append(int(ms))
                    if not seen_first:
                        k["first"] += 1
                        seen_first = True
    rows = []
    for kind, k in by_kind.items():
        med = statistics.median(k["ms"]) if k["ms"] else None
        rows.append((kind, k["runs"], k["routes"], k["first"], med))
    rows.sort(key=lambda r: (r[4] is None, r[4] if r[4] is not None else 0, -r[2]))
    print(f"{routes} route files")
    print("| family | runs | routes | first to answer | median ms to route |")
    print("|---|---|---|---|---|")
    for kind, runs, found, first, med in rows:
        print(f"| {kind} | {runs} | {found} | {first} | {'-' if med is None else int(med)} |")
    print()
    print("KIND_ORDER by median ms to route:", [r[0] for r in rows])
    if out_csv:
        with open(out_csv, "w", encoding="utf-8", newline="\n") as f:
            f.write("family,runs,routes,first_to_answer,median_ms_to_route\n")
            for kind, runs, found, first, med in rows:
                f.write(f"{kind},{runs},{found},{first},{'' if med is None else int(med)}\n")


if __name__ == "__main__":
    main(sys.argv)
