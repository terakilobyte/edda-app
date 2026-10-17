#!/usr/bin/env python
"""planner_before_after.py before.jsonl after.jsonl [--ref ref.jsonl]

The planner matrix gate (planner unification, 2026-10-17): one row per
(from, to) in the before file, the after file's row beside it, a verdict
per route, exit 1 if any route is WORSE. Rows come from
docs/benches/knobs/edda_matrix.sh (fields: from, to, jumps, stops,
total_ly, straight_ly, first_found_ms, wall_s, or error).

Pilot seconds use the bench judge: 60 s per jump + 120 s per stop.
Verdicts: WORSE = more jumps, more stops, a route lost, or the same jumps
at more than +10 % wall; better = fewer jumps or stops or wall under 90 %;
same otherwise. With --ref the external reference's jumps and seconds are
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


def verdict(b, a):
    if a.get("jumps") is None:
        return "WORSE" if b.get("jumps") is not None else "same"
    if b.get("jumps") is None:
        return "better"
    if a["jumps"] > b["jumps"] or (a.get("stops") or 0) > (b.get("stops") or 0):
        return "WORSE"
    if a["wall_s"] > b["wall_s"] * 1.10 and a["jumps"] == b["jumps"]:
        return "WORSE"
    if a["jumps"] < b["jumps"] or (a.get("stops") or 0) < (b.get("stops") or 0) or a["wall_s"] < b["wall_s"] * 0.9:
        return "better"
    return "same"


def main(argv):
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
            f"{fmt(seconds(b))} / {fmt(seconds(a))}",
            f"{fmt((b.get('first_found_ms') or 0) / 1000, 1)} / {fmt((a.get('first_found_ms') or 0) / 1000, 1)}",
            f"{fmt(b.get('wall_s'), 1)} / {fmt(a.get('wall_s'), 1)}",
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
