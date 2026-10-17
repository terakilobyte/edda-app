#!/usr/bin/env python
"""matrix_rows.py <arm.jsonl> [note]

Print the planner matrix record's CSV rows for one arm of
edda_matrix.sh output (docs/benches/2026-10-17-planner-matrix.csv:
arm,route_from,route_to,straight_ly,jumps,stops,pilot_s,first_found_ms,
wall_s,note). pilot_s is the row's `seconds_v2` when rejudge_seconds.py has
added it, else the bench judge (60 s a jump + 120 s a stop); a lost route
prints empty jumps/stops/pilot_s and its error in the note.
"""
import json
import sys


def main(argv):
    path = argv[1]
    note = argv[2] if len(argv) > 2 else ""
    for line in open(path, encoding="utf-8"):
        if not line.strip():
            continue
        r = json.loads(line)
        if r.get("jumps") is None:
            err = (r.get("error") or "no route").replace(",", ";")[:60]
            print(f"{r['tag']},{r['from']},{r['to']},,,,,,{r['wall_s']},{(note + ' ' if note else '')}{err}")
            continue
        secs = r.get("seconds_v2")
        if secs is None:
            secs = round(r["jumps"] * 60 + (r.get("stops") or 0) * 120)
        ff = r.get("first_found_ms")
        print(f"{r['tag']},{r['from']},{r['to']},{r.get('straight_ly', '')},{r['jumps']},{r.get('stops', '')},{secs},{'' if ff is None else ff},{r['wall_s']},{note}")


if __name__ == "__main__":
    main(sys.argv)
