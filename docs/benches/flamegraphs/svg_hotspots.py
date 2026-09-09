#!/usr/bin/env python3
"""Cross-era hotspot table from a flamegraph run folder.

Sums inclusive sample percentages per curated function bucket from each
era--cell SVG (inferno title format: "name (N samples, P.PP%)"), so the
same buckets can be compared across eras. Buckets are matched on the
deepest frame name, and each bucket takes the MAX percentage among its
matching frames (the top-most instance dominates; summing instances
would double-count recursion/call sites).

Usage: python svg_hotspots.py <run_dir>
"""
import pathlib
import re
import sys

BUCKETS = {
    "coarse scan (for_each_within/cell walk)": r"for_each_within|for_each_cell_in_box|morton",
    "goal field build (Dijkstra)": r"goal_field",
    "cell graph / min_pair": r"cgraph|min_pair",
    "coarse relax + heap": r"plan_long_with|BinaryHeap|sift",
    "bidi": r"plan_long_bidi",
    "leg refine (exact planner)": r"refine_waypoints|router::plan",
    "fuel model": r"fuel::",
    "dist / math": r"format::dist",
    # NOT bare "alloc::" -- alloc::boxed::...::call_once is the thread
    # trampoline, an ancestor of everything (measured 98-100%).
    "alloc": r"RawVec|__rust_alloc|alloc::alloc::|HeapAlloc|malloc",
    "hashmap": r"hashbrown|HashMap",
}
TITLE = re.compile(r"<title>([^<]+) \(([\d,]+) samples?, ([\d.]+)%\)</title>")


def main():
    run = pathlib.Path(sys.argv[1])
    svgs = sorted(run.glob("*.svg"))
    cells = sorted({s.stem.split("--")[1] for s in svgs})
    eras = sorted({s.stem.split("--")[0] for s in svgs})
    for cell in cells:
        print(f"\n== {cell} ==")
        print(f"{'bucket':<42}" + "".join(f"{e:>22}" for e in eras))
        rows = {b: {} for b in BUCKETS}
        for era in eras:
            p = run / f"{era}--{cell}.svg"
            if not p.exists():
                continue
            best = {b: 0.0 for b in BUCKETS}
            for m in TITLE.finditer(p.read_text(encoding="utf-8", errors="replace")):
                name, pct = m.group(1), float(m.group(3))
                for b, pat in BUCKETS.items():
                    if re.search(pat, name):
                        best[b] = max(best[b], pct)
            for b in BUCKETS:
                rows[b][era] = best[b]
        for b in BUCKETS:
            print(f"{b:<42}" + "".join(f"{rows[b].get(e, 0.0):>21.1f}%" for e in eras))


if __name__ == "__main__":
    main()
