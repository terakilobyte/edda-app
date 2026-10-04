"""Diff EDDA's vendored engineer grades against the wiki's Engineers overview
table (https://elite-dangerous.fandom.com/wiki/Engineers), transcribed
below as the boss pasted it on 2026-10-04. Run from the repo root:

    python docs/benches/knobs/engineer_table_vs_wiki_overview.py

Result 2026-10-04: 25 engineers both sides, ONE difference — Lori Jameson,
Kill Warrant Scanner G3 in ours, absent from the overview table. Her own
wiki article lists "Kill Warrant Scanner (Grade 3)", so the overview is
the one that omits it; ours stands (it also matches the 2026-09-19 audit
against Inara + the wiki, docs/benches/2026-09-19-engineer-grade-audit.csv).
Refresh WIKI when the page changes and rerun.
"""
import json, re

WIKI = {
 "Tod 'The Blaster' McQuinn": ["Multi-cannon (G5)","Rail Gun (G5)","Fragment Cannon (G3)","Cannon (G2)"],
 "Selene Jean": ["Armour (G5)","Hull Reinforcement Package (G5)"],
 "Didi Vatermann": ["Shield Booster (G5)","Shield Generator (G3)"],
 "Bill Turner": ["Sensors (G5)","Plasma Accelerator (G5)","Detailed Surface Scanner (G5)","AFMU (G3)","Frame Shift Wake Scanner (G3)","Fuel Scoop (G3)","Kill Warrant Scanner (G3)","Life Support (G3)","Manifest Scanner (G3)","Refinery (G3)"],
 "Broo Tarquin": ["Beam Laser (G5)","Burst Laser (G5)","Pulse Laser (G5)"],
 "Liz Ryder": ["Missile Rack (G5)","Seeker Missile Rack (G5)","Torpedo Pylon (G5)","Mine Launcher (G3)","Hull Reinforcement Package (G1)","Armour (G1)"],
 "Hera Tani": ["Detailed Surface Scanner (G5)","Power Plant (G5)","Power Distributor (G3)","Sensors (G3)"],
 "Tiana Fortune": ["Manifest Scanner (G5)","Collector Limpet Controller (G5)","Frame Shift Wake Scanner (G5)","Fuel Transfer Limpet Controller (G5)","Hatch Breaker Limpet Controller (G5)","Kill Warrant Scanner (G5)","Prospector Limpet Controller (G5)","Sensors (G5)","Detailed Surface Scanner (G3)","Frame Shift Drive Interdictor (G3)"],
 "Felicity Farseer": ["Frame Shift Drive (G5)","Detailed Surface Scanner (G3)","Sensors (G3)","Thrusters (G3)","Power Plant (G1)","Frame Shift Drive Interdictor (G1)","Shield Booster (G1)"],
 "Colonel Bris Dekker": ["Frame Shift Drive Interdictor (G4)","Frame Shift Drive (G3)"],
 "Juri Ishmaak": ["Detailed Surface Scanner (G5)","Mine Launcher (G5)","Sensors (G5)","Frame Shift Wake Scanner (G3)","Kill Warrant Scanner (G3)","Manifest Scanner (G3)","Missile Rack (G3)","Seeker Missile Rack (G3)","Torpedo Pylon (G3)"],
 "The Sarge": ["Cannon (G5)","Collector Limpet Controller (G5)","Fuel Transfer Limpet Controller (G5)","Hatch Breaker Limpet Controller (G5)","Prospector Limpet Controller (G5)","Rail Gun (G3)"],
 "Elvira Martuuk": ["Frame Shift Drive (G5)","Shield Generator (G3)","Thrusters (G2)","Shield Cell Bank (G1)"],
 "The Dweller": ["Power Distributor (G5)","Pulse Lasers (G4)","Beam Lasers (G3)","Burst Lasers (G3)"],
 "Lei Cheung": ["Shield Generator (G5)","Detailed Surface Scanner (G5)","Sensors (G5)","Shield Booster (G3)"],
 "Marco Qwent": ["Power Plant (G4)","Power Distributor (G3)"],
 "Professor Palin": ["Thrusters (G5)","Frame Shift Drive (G3)"],
 "Zacariah Nemo": ["Fragment Cannon (G5)","Multi-cannon (G3)","Plasma Accelerator (G2)"],
 "Lori Jameson": ["Detailed Surface Scanner (G5)","Sensors (G5)","AFMU (G4)","Fuel Scoop (G4)","Life Support (G4)","Refinery (G4)","Frame Shift Wake Scanner (G3)","Manifest Scanner (G3)","Shield Cell Bank (G3)"],
 "Ram Tah": ["Chaff Launcher (G5)","ECM (G5)","Heat Sink Launcher (G5)","Point Defence (G5)","Collector Limpet Controller (G4)","Fuel Transfer Limpet Controller (G4)","Prospector Limpet Controller (G4)","Hatch Breaker Limpet Controller (G3)"],
 "Etienne Dorn": ["Detailed Surface Scanner (G5)","Frame Shift Wake Scanner (G5)","Kill Warrant Scanner (G5)","Life Support (G5)","Manifest Scanner (G5)","Plasma Accelerator (G5)","Power Distributor (G5)","Power Plant (G5)","Sensors (G5)","Rail Gun (G5)"],
 "Marsha Hicks": ["Cannon (G5)","Collector Limpet Controller (G5)","Fragment Cannon (G5)","Fuel Scoop (G5)","Fuel Transfer Limpet Controller (G5)","Hatch Breaker Limpet Controller (G5)","Multi-cannon (G5)","Prospector Limpet Controller (G5)","Refinery (G5)"],
 "Mel Brandon": ["Beam Laser (G5)","Burst Laser (G5)","Pulse Laser (G5)","Shield Generator (G5)","Thrusters (G5)","Shield Booster (G5)","Frame Shift Drive (G5)","Frame Shift Drive Interdictor (G5)","Shield Cell Bank (G4)"],
 "Petra Olmanova": ["Armour (G5)","AFMU (G5)","Chaff Launcher (G5)","ECM (G5)","Heat Sink Launcher (G5)","Hull Reinforcement Package (G5)","Mine Launcher (G5)","Missile Rack (G5)","Point Defence (G5)","Seeker Missile Rack (G5)","Torpedo Pylon (G5)"],
 "Chloe Sedesi": ["Thrusters (G5)","Frame Shift Drive (G3)"],
}
# the wiki's spellings onto ours (engineer_grades.json "note": seeker and dumbfire racks are one type,
# DSS is Surface Scanner, FS Wake Scanner is Wake Scanner)
MAP = {"AFMU": "Auto Field-Maintenance Unit", "Detailed Surface Scanner": "Surface Scanner", "Frame Shift Wake Scanner": "Wake Scanner",
       "ECM": "Electronic Countermeasure", "Pulse Lasers": "Pulse Laser", "Beam Lasers": "Beam Laser", "Burst Lasers": "Burst Laser",
       "Seeker Missile Rack": "Missile Rack"}
ours = json.load(open("crates/ed-engineering/data/engineer_grades.json", encoding="utf-8"))["engineers"]
wiki = {}
for eng, mods in WIKI.items():
    d = {}
    for m in mods:
        name, g = re.match(r"(.*) \(G(\d)\)", m).groups()
        name = MAP.get(name, name); g = int(g)
        d[name] = max(d.get(name, 0), g)
    wiki[eng] = d
diffs = 0
for eng in sorted(set(wiki) | set(ours)):
    w, o = wiki.get(eng), ours.get(eng)
    if w is None: print(f"ONLY OURS: {eng} -> {o}"); diffs += 1; continue
    if o is None: print(f"ONLY WIKI: {eng} -> {w}"); diffs += 1; continue
    for m in sorted(set(w) | set(o)):
        if w.get(m) != o.get(m):
            print(f"{eng:<28} {m:<32} wiki G{w.get(m, '-')}  ours G{o.get(m, '-')}"); diffs += 1
print(f"\n{len(wiki)} engineers on the wiki, {len(ours)} in ours, {diffs} differences")
