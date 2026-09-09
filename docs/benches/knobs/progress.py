#!/usr/bin/env python3
"""Progress check for a running knob study: python docs/benches/knobs/progress.py [expected_trials]"""
import json
import pathlib
import sys
from datetime import datetime

expected = int(sys.argv[1]) if len(sys.argv) > 1 else 81
trials = []
for line in open(pathlib.Path(__file__).parent / "trials.jsonl", encoding="utf-8"):
    d = json.loads(line)
    if d.get("study") == "v4":  # current-study marker; adjust per space change
        trials.append(d)
n = len(trials)
if n < 2:
    print(f"{n} trials logged; too early for a pace")
    sys.exit(0)
t0 = datetime.strptime(trials[1]["at"], "%Y-%m-%dT%H:%M:%S")
t1 = datetime.strptime(trials[-1]["at"], "%Y-%m-%dT%H:%M:%S")
pace = (t1 - t0).total_seconds() / max(n - 2, 1)
left = (expected - n) * pace / 60
base = trials[0]["total_s"]
best_s, best_n = min((t["total_s"], t["trial"]) for t in trials)
pruned = sum(1 for t in trials if t.get("pruned"))
print(f"{n - 1} of {expected} done; ~{max(left, 0):.0f} min remaining (pace {pace:.0f} s/trial)")
print(f"baseline {base:.0f}; best #{best_n}: {best_s:.0f} ({best_s - base:+.0f}); pruned {pruned}")
