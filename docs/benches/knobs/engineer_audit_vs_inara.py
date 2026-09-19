"""Compare EDDA's vendored blueprint data (per engineer, per module type, max grade)
against Inara's Engineers directory (fetched 2026-09-19)."""
import json, collections, sys

INARA = """
Bill Turner: Detailed Surface Scanner 5, Plasma Accelerator 5, Sensors 5, Cargo Rack 4, Auto Field-Maintenance Unit 3, Frame Shift Wake Scanner 3, Fuel Scoop 3, Kill Warrant Scanner 3, Life Support 3, Manifest Scanner 3, Refinery 3
Broo Tarquin: Beam Laser 5, Burst Laser 5, Pulse Laser 5
Chloe Sedesi: Thrusters 5, Frame Shift Drive 3
Colonel Bris Dekker: Frame Shift Drive Interdictor 4, Frame Shift Drive 3
Didi Vatermann: Shield Booster 5, Shield Generator 3
Elvira Martuuk: Frame Shift Drive 5, Shield Generator 3, Thrusters 2, Shield Cell Bank 1
Etienne Dorn: Detailed Surface Scanner 5, Frame Shift Wake Scanner 5, Kill Warrant Scanner 5, Life Support 5, Manifest Scanner 5, Plasma Accelerator 5, Power Distributor 5, Power Plant 5, Rail Gun 5, Sensors 5
Felicity Farseer: Frame Shift Drive 5, Detailed Surface Scanner 3, Sensors 3, Thrusters 3, Frame Shift Drive Interdictor 1, Power Plant 1, Shield Booster 1
Hera Tani: Detailed Surface Scanner 5, Power Plant 5, Power Distributor 3, Sensors 3
Juri Ishmaak: Abrasion Blaster 5, Detailed Surface Scanner 5, Enzyme Missile Rack 5, Mine Launcher 5, Mining Laser 5, Sensors 5, Frame Shift Wake Scanner 3, Kill Warrant Scanner 3, Manifest Scanner 3, Missile Rack 3, Seeker Missile Rack 3, Torpedo Pylon 3
Lei Cheung: Detailed Surface Scanner 5, Sensors 5, Shield Generator 5, Shield Booster 3
Liz Ryder: Missile Rack 5, Seeker Missile Rack 5, Torpedo Pylon 5, Abrasion Blaster 3, Enzyme Missile Rack 3, Mine Launcher 3, Mining Laser 3, Armour 1, Hull Reinforcement Package 1
Lori Jameson: Detailed Surface Scanner 5, Sensors 5, Auto Field-Maintenance Unit 4, Fuel Scoop 4, Life Support 4, Refinery 4, Frame Shift Wake Scanner 3, Kill Warrant Scanner 3, Manifest Scanner 3, Shield Cell Bank 3
Marco Qwent: Power Plant 4, Power Distributor 3
Marsha Hicks: Cannon 5, Collector Limpet Controller 5, Fragment Cannon 5, Fuel Scoop 5, Fuel Transfer Limpet Controller 5, Hatch Breaker Limpet Controller 5, Multi-cannon 5, Prospector Limpet Controller 5, Refinery 5
Mel Brandon: Beam Laser 5, Burst Laser 5, Frame Shift Drive 5, Frame Shift Drive Interdictor 5, Pulse Laser 5, Shield Booster 5, Shield Generator 5, Thrusters 5, Shield Cell Bank 4
Petra Olmanova: Armour 5, Auto Field-Maintenance Unit 5, Chaff Launcher 5, Electronic Countermeasure 5, Heat Sink Launcher 5, Hull Reinforcement Package 5, Mine Launcher 5, Missile Rack 5, Module Reinforcement Package 5, Point Defence 5, Seeker Missile Rack 5, Torpedo Pylon 5
Professor Palin: Thrusters 5, Frame Shift Drive 3
Ram Tah: Chaff Launcher 5, Collector Limpet Controller 5, Electronic Countermeasure 5, Fuel Transfer Limpet Controller 5, Heat Sink Launcher 5, Point Defence 5, Prospector Limpet Controller 5, Cargo Rack 4, Hatch Breaker Limpet Controller 3, Guardian Gauss Cannon 1, Guardian Plasma Charger 1, Guardian Shard Cannon 1
Selene Jean: Armour 5, Hull Reinforcement Package 5, Module Reinforcement Package 5
The Dweller: Power Distributor 5, Pulse Laser 4, Beam Laser 3, Burst Laser 3
The Sarge: Cannon 5, Collector Limpet Controller 5, Fuel Transfer Limpet Controller 5, Hatch Breaker Limpet Controller 5, Prospector Limpet Controller 5, Rail Gun 3
Tiana Fortune: Collector Limpet Controller 5, Frame Shift Wake Scanner 5, Fuel Transfer Limpet Controller 5, Hatch Breaker Limpet Controller 5, Kill Warrant Scanner 5, Manifest Scanner 5, Prospector Limpet Controller 5, Sensors 5, Cargo Rack 4, Detailed Surface Scanner 3, Frame Shift Drive Interdictor 3
Tod "The Blaster" McQuinn: Multi-cannon 5, Rail Gun 5, Fragment Cannon 3, Cannon 2
Zacariah Nemo: Fragment Cannon 5, Multi-cannon 3, Plasma Accelerator 2
"""

# Inara's names -> EDEngineer's Type names.
ALIAS = {
    "Detailed Surface Scanner": "Surface Scanner",
    "Frame Shift Wake Scanner": "Wake Scanner",
    "Tod \"The Blaster\" McQuinn": "Tod McQuinn",
}

def inara_table():
    t = {}
    for line in INARA.strip().splitlines():
        eng, rest = line.split(":", 1)
        eng = ALIAS.get(eng.strip(), eng.strip())
        for part in rest.split(","):
            part = part.strip()
            name, grade = part.rsplit(" ", 1)
            t[(ALIAS.get(name, name), eng)] = int(grade)
    return t

def ours_table(path):
    b = json.load(open(path, encoding="utf-8"))
    mx = collections.defaultdict(int)
    for x in b:
        if "Grade" not in x:
            continue
        for e in x["Engineers"]:
            if e.startswith("@"):
                continue
            mx[(x["Type"], e)] = max(mx[(x["Type"], e)], x["Grade"])
    return dict(mx)

ours = ours_table(sys.argv[1])
ref = inara_table()
ship_engs = {e for (_, e) in ref}
ours_ship = {k: v for k, v in ours.items() if k[1] in ship_engs}
print("engineers: inara", len(ship_engs), "| ours (all)", len({e for (_, e) in ours}), "| ours ship-engineer names not on inara:",
      sorted({e for (_, e) in ours} - ship_engs))
print("\nINARA has, EDDA lacks (engineer cannot be offered at all):")
for k in sorted(set(ref) - set(ours_ship)):
    print(f"  {k[1]:24s} {k[0]:34s} G{ref[k]}")
print("\nEDDA has, INARA lacks (we may offer an engineer who does not do it):")
for k in sorted(set(ours_ship) - set(ref)):
    print(f"  {k[1]:24s} {k[0]:34s} G{ours_ship[k]}")
print("\nmax-grade DISAGREEMENTS (edda vs inara):")
for k in sorted(set(ref) & set(ours_ship)):
    if ref[k] != ours_ship[k]:
        print(f"  {k[1]:24s} {k[0]:34s} edda G{ours_ship[k]}  inara G{ref[k]}")
print("\nagreements:", sum(1 for k in set(ref) & set(ours_ship) if ref[k] == ours_ship[k]), "of", len(ref))
