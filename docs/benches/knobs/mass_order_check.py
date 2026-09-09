#!/usr/bin/env python3
"""Item 47: verify the tie-break premise against a real Spansh dump.

    mass_order_check.py <galaxy dump .json.gz>

The reconcile's duplicate-main-star tie-break keeps the FIRST listed
mainStar body (matching ed-galaxy import.rs). The premise: dump body
order lists the most massive co-main star first, so first-listed ==
the game's arrival star. This script measures that premise: for every
system with two or more mainStar Star bodies, compare solarMasses in
listed order and count violations (first listed not the heaviest).
"""

import gzip
import json
import sys

path = sys.argv[1]
systems = 0
multi_main = 0
with_masses = 0
first_is_heaviest = 0
ties = 0
violations = 0
examples = []

with gzip.open(path, "rt", encoding="utf-8", errors="replace") as f:
    for line in f:
        line = line.strip().rstrip(",")
        if not line.startswith("{"):
            continue
        try:
            system = json.loads(line)
        except json.JSONDecodeError:
            continue
        systems += 1
        mains = [
            body
            for body in system.get("bodies", [])
            if body.get("mainStar") and body.get("type") == "Star"
        ]
        if len(mains) < 2:
            continue
        multi_main += 1
        masses = [body.get("solarMasses") for body in mains]
        if any(mass is None for mass in masses):
            continue
        with_masses += 1
        top = max(masses)
        if masses[0] == top:
            first_is_heaviest += 1
            if masses.count(top) > 1:
                ties += 1
        else:
            violations += 1
            if len(examples) < 10:
                examples.append(
                    {
                        "name": system.get("name"),
                        "id64": system.get("id64"),
                        "order": [
                            (body.get("subType"), body.get("solarMasses"))
                            for body in mains
                        ],
                    }
                )

print(f"systems scanned:            {systems:,}")
print(f"multi-mainStar systems:     {multi_main:,}")
print(f"  with masses on all mains: {with_masses:,}")
print(f"  first listed is heaviest: {first_is_heaviest:,}")
print(f"    (of which exact ties):  {ties:,}")
print(f"  VIOLATIONS:               {violations:,}")
for example in examples:
    print(f"  violation: {example}")
