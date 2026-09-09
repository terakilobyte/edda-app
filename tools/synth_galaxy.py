#!/usr/bin/env python3
"""Deterministic synthetic corridor galaxy for routing benchmarks.

A 24,000 ly corridor along +x, class mix matched to the real index
(~1.7% neutron, ~1.4% white dwarf, ~55% scoopable), a quarter of
neutrons with a scoopable companion inside 1,500 ls. Two named
endpoints: SynthStart (0,0,0) and SynthGoal (22000,0,0), both G class.
"""
import json, random, sys

N = int(sys.argv[1]) if len(sys.argv) > 1 else 2_500_000
# --desort: opt-in axis-stripe desert (the desORt's controlled analog):
# a 300 ly-wide dead stripe RUNNING ALONG the corridor at z ~ 0 for
# x 4k-20k, 99% culled -- the straight line lies inside it, so the
# planner must ride the shoulder, like real routes near the axes.
# Opt-in so the pinned synthetic stays byte-stable.
DESORT = "--desort" in sys.argv
rng = random.Random(0xEDDA)
CLASSES = [  # (subType, weight)
    ("Neutron Star", 0.0175), ("White Dwarf (DA) Star", 0.014),
    ("K (Yellow-Orange) Star", 0.12), ("G (White-Yellow) Star", 0.08),
    ("F (White) Star", 0.04), ("B (Blue-White) Star", 0.01),
    ("A (Blue-White) Star", 0.02), ("O (Blue-White) Star", 0.002),
    ("M (Red dwarf) Star", 0.28), ("L (Brown dwarf) Star", 0.14),
    ("T (Brown dwarf) Star", 0.12), ("Y (Brown dwarf) Star", 0.08),
]
rest = 1.0 - sum(w for _, w in CLASSES)
CLASSES.append(("T Tauri Star", rest))
cum, acc = [], 0.0
for sub, w in CLASSES:
    acc += w; cum.append((acc, sub))

def pick():
    r = rng.random()
    for c, sub in cum:
        if r <= c: return sub
    return cum[-1][1]

def emit(name, id64, x, y, z, sub, companion):
    bodies = [{"type": "Star", "subType": sub, "mainStar": True, "distanceToArrival": 0}]
    if companion:
        bodies.append({"type": "Star", "subType": "G (White-Yellow) Star",
                       "distanceToArrival": rng.uniform(100, 1400)})
    print(json.dumps({"name": name, "id64": id64,
        "coords": {"x": round(x,3), "y": round(y,3), "z": round(z,3)},
        "bodies": bodies}, separators=(",", ":")))

emit("SynthStart", 1, 0.0, 0.0, 0.0, "G (White-Yellow) Star", False)
emit("SynthGoal", 2, 22000.0, 0.0, 0.0, "G (White-Yellow) Star", False)
emitted = 0
i = 0
while emitted < N:
    i += 1
    x = rng.uniform(-500.0, 23500.0)
    y = rng.uniform(-2500.0, 2500.0)
    z = rng.uniform(-500.0, 500.0)
    # The wall: a dead zone across the corridor at x 11k-14k, passable
    # only through a narrow lane at y > 1600 — the straight-line
    # heuristic points straight into it.
    in_wall = 11000.0 <= x <= 14000.0 and y < 1600.0
    if in_wall and rng.random() < 0.995:
        continue
    if DESORT and abs(z) <= 150.0 and 4000.0 <= x <= 20000.0 and rng.random() < 0.99:
        continue
    sub = pick()
    # Rim effect: highway density thins with x, so the far half stalls.
    if sub.startswith(("Neutron", "White Dwarf")) and rng.random() < (x / 30000.0):
        sub = "M (Red dwarf) Star"
    companion = sub.startswith(("Neutron", "White Dwarf")) and rng.random() < 0.25
    emit(f"Synth {i//10000:03d}-Z d{i%10000}", 1000 + i, x, y, z, sub, companion)
    emitted += 1
