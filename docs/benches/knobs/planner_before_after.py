#!/usr/bin/env python
"""planner_before_after.py before.jsonl after.jsonl [--ref ref.jsonl] [--judge seconds]

The planner matrix gate (planner unification, 2026-10-17): one row per
(from, to) in the before file, the after file's row beside it, a verdict
per route, exit 1 if any route is WORSE. Rows come from
docs/benches/knobs/edda_matrix.sh (fields: from, to, jumps, stops,
total_ly, straight_ly, first_found_ms, wall_s, or error).

Pilot seconds use the bench judge: 60 s per jump + 120 s per stop. With
`--judge seconds` the rows' `seconds_v2` (rejudge_seconds.py: plain 53,
boosted 73, passing scoop 6, stop 120 + tonnes) is the figure compared and
the verdict is on seconds, stops and wall; jumps are shown, not judged (a
route with more plain jumps and fewer boosted ones can be the faster one).
Verdicts: WORSE = more jumps, more stops, a route lost, or the same jumps
at more than +10 % AND +1 s wall; better = fewer jumps or stops, or wall
under 90 % and more than 1 s faster; same otherwise. Runs must not share
the machine with compiles (the r0 arm did, and three sub-second routes
moved by tenths). With --ref the external reference's jumps and seconds are
shown for the eye; they are not part of the verdict.
"""
import json
import sys

T_JUMP, T_STOP = 60.0, 120.0


def load(path):
    rows = {}
    for line in open(path, encoding="utf-8"):
        if not line.strip():
            continue
        r = json.loads(line)
        rows[(r["from"], r["to"])] = r
    return rows


def seconds(r):
    if r.get("jumps") is None:
        return None
    return r["jumps"] * T_JUMP + (r.get("stops") or 0) * T_STOP


def fmt(v, nd=0):
    if v is None:
        return "-"
    return f"{v:.{nd}f}" if isinstance(v, float) else str(v)


def wall(r, other):
    # A same-day re-measurement, when a row carries one, is what the wall
    # verdict compares (the planner's wave policy makes some routes bimodal
    # in time: 18 s or 77 s for the same binary and route, 2026-10-17);
    # the original figure stays in the row.
    if "wall_s_sameday" in r or "wall_s_sameday" in other:
        return r.get("wall_s_sameday", r["wall_s"])
    return r["wall_s"]


JUDGE = "jumps"


def verdict(b, a):
    if a.get("jumps") is None:
        return "WORSE" if b.get("jumps") is not None else "same"
    if b.get("jumps") is None:
        return "better"
    if JUDGE == "seconds" and "seconds_v2" in a and "seconds_v2" in b:
        sa, sb = a["seconds_v2"], b["seconds_v2"]
        if sa > sb * 1.002 or (a.get("stops") or 0) > (b.get("stops") or 0):
            return "WORSE"
        wa, wb = wall(a, b), wall(b, a)
        if wa > wb * 1.10 and wa > wb + 1.0 and sa >= sb:
            return "WORSE"
        if sa < sb or (a.get("stops") or 0) < (b.get("stops") or 0) or (wa < wb * 0.9 and wa + 1.0 < wb):
            return "better"
        return "same"
    if a["jumps"] > b["jumps"] or (a.get("stops") or 0) > (b.get("stops") or 0):
        return "WORSE"
    # Wall time has a noise floor: single runs under a few seconds move by
    # tenths with page cache and scheduling; a verdict needs +10 % AND +1 s.
    wa, wb = wall(a, b), wall(b, a)
    if wa > wb * 1.10 and wa > wb + 1.0 and a["jumps"] == b["jumps"]:
        return "WORSE"
    if a["jumps"] < b["jumps"] or (a.get("stops") or 0) < (b.get("stops") or 0) or (wa < wb * 0.9 and wa + 1.0 < wb):
        return "better"
    return "same"


def main(argv):
    global JUDGE
    argv = list(argv)
    if "--judge" in argv:
        i = argv.index("--judge")
        JUDGE = argv[i + 1]
        del argv[i:i + 2]
    before, after = load(argv[1]), load(argv[2])
    ref = load(argv[4]) if len(argv) > 4 and argv[3] == "--ref" else {}
    head = ["route", "straight ly", "jumps b/a", "stops b/a", "pilot s b/a", "first s b/a", "wall s b/a"]
    if ref:
        head.append("ref jumps / s")
    head.append("verdict")
    print("| " + " | ".join(head) + " |")
    print("|" + "---|" * len(head))
    counts = {"better": 0, "same": 0, "WORSE": 0}
    for key in sorted(before):
        b, a = before[key], after.get(key, {})
        v = verdict(b, a)
        counts[v] += 1
        cells = [
            f"{key[0]} -> {key[1]}",
            fmt(b.get("straight_ly")),
            f"{fmt(b.get('jumps'))} / {fmt(a.get('jumps'))}",
            f"{fmt(b.get('stops'))} / {fmt(a.get('stops'))}",
            (f"{fmt(b.get('seconds_v2'))} / {fmt(a.get('seconds_v2'))}" if JUDGE == "seconds" else f"{fmt(seconds(b))} / {fmt(seconds(a))}"),
            f"{fmt((b.get('first_found_ms') or 0) / 1000, 1)} / {fmt((a.get('first_found_ms') or 0) / 1000, 1)}",
            f"{fmt(wall(b, a), 1)} / {fmt(wall(a, b), 1)}" + (" (same-day)" if "wall_s_sameday" in b or "wall_s_sameday" in a else ""),
        ]
        if ref:
            r = ref.get(key, {})
            cells.append(f"{fmt(r.get('jumps'))} / {fmt(r.get('ref_s'), 0)}")
        cells.append(v)
        print("| " + " | ".join(cells) + " |")
    print()
    print(f"better {counts['better']} · same {counts['same']} · WORSE {counts['WORSE']} of {len(before)}")
    return 1 if counts["WORSE"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
