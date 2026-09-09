#!/usr/bin/env python3
"""Score a bench --json line under the seconds model: pipe bench output in.
Usage: bench ... --json | python docs/benches/knobs/score_line.py [t_jump]
"""
import json
import sys

t_jump = float(sys.argv[1]) if len(sys.argv) > 1 else 74.0
d = json.loads([l for l in sys.stdin if l.strip().startswith("{")][-1])
score = d["jumps"] * t_jump + d["est_refuel_s"]
print(
    f"jumps {d['jumps']}  stops {d['refuel_stops']}  tonnes {d['refuel_tonnes']:.0f}  "
    f"refuel_s {d['est_refuel_s']:.0f}  score_s {score:.0f}  wall_ms {d['wall_ms']}  exp {d['expansions']}"
)
