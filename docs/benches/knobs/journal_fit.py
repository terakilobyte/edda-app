#!/usr/bin/env python3
"""Fit the pilot-time coefficients from the commander's own journal
(item 22): seconds per jump from FSDJump cadence, scoop overhead and
seconds-per-tonne from gaps that contain FuelScoop events.

Usage: python docs/benches/knobs/journal_fit.py [journal_dir]
"""
import json
import pathlib
import statistics
import sys
from datetime import datetime

DEFAULT = pathlib.Path.home() / "Saved Games" / "Frontier Developments" / "Elite Dangerous"


def ts(e):
    return datetime.strptime(e["timestamp"], "%Y-%m-%dT%H:%M:%SZ")


def main():
    root = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT
    files = sorted(root.glob("Journal*.log"))
    if not files:
        print(f"no journals under {root}")
        return
    plain_gaps, scoop_gaps = [], []  # scoop_gaps: (gap_s, tonnes)
    ship_plain = {}
    for f in files:
        last_jump, scooped, ship = None, 0.0, None
        for line in open(f, encoding="utf-8", errors="replace"):
            try:
                e = json.loads(line)
            except json.JSONDecodeError:
                continue
            ev = e.get("event")
            if ev in ("LoadGame", "Loadout"):
                ship = e.get("Ship", ship)
            elif ev == "FuelScoop":
                scooped += e.get("Scooped", 0.0)
            elif ev == "FSDJump":
                t = ts(e)
                if last_jump is not None:
                    gap = (t - last_jump).total_seconds()
                    if 20 <= gap <= 600:
                        if scooped > 0.5:
                            scoop_gaps.append((gap, scooped))
                        else:
                            plain_gaps.append(gap)
                            ship_plain.setdefault(ship or "?", []).append(gap)
                last_jump, scooped = t, 0.0
    if not plain_gaps:
        print("no usable jump cadence found")
        return
    t_jump = statistics.median(plain_gaps)
    print(f"{len(files)} journals; {len(plain_gaps)} plain gaps, {len(scoop_gaps)} scooping gaps")
    print(f"t_jump (median plain inter-jump): {t_jump:.0f} s")
    for ship, gaps in sorted(ship_plain.items(), key=lambda kv: -len(kv[1])):
        if len(gaps) >= 20:
            print(f"  {ship}: {statistics.median(gaps):.0f} s over {len(gaps)} jumps")
    if scoop_gaps:
        extras = [(g - t_jump, tn) for g, tn in scoop_gaps if g > t_jump]
        if extras:
            per_tonne = statistics.median(x / tn for x, tn in extras)
            overhead = statistics.median(x - per_tonne * tn for x, tn in extras)
            med_extra = statistics.median(x for x, _ in extras)
            med_tonnes = statistics.median(tn for _, tn in extras)
            print(f"scooping gap extra time: median {med_extra:.0f} s for median {med_tonnes:.1f} t")
            print(f"fit: ~{per_tonne:.1f} s/tonne, stop overhead ~{max(overhead, 0):.0f} s")
            print(f"(flat-model check: current objective charges 120 s/stop, 40 s/jump)")


if __name__ == "__main__":
    main()
