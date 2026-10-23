"""Hull masses from EDSY for the Build Planner, keyed like the Coriolis rows.

The maintainer, 2026-10-23, on the six hulls the 4.4.2.0 patch lightened
while EDCD's coriolis-data sat at September: "then maybe we should use
edsy hull masses". Measured first (docs/benches/2026-10-23-hull-mass-
coriolis-vs-edsy.csv): the two sources agreed on 42 of 48 hulls and
differed on exactly the six the patch named, so EDSY's mass is the
planner's for every hull. Speed, boost and shields stay Coriolis's (they
differ by a unit or two on ten hulls for reasons older than the patch).

Rows are matched by Frontier id (Coriolis edID = EDSY fdid), then by name
(four Coriolis ids are not Frontier's, and one name differs); a hull that
matches neither is an error, not a skip. The output keys are the Coriolis
file keys the catalog iterates, so the loader needs no name mapping.

    node docs/benches/knobs/eddb_to_json.js eddb.js eddb.json
    python docs/benches/knobs/edsy_hull_masses.py eddb.json \
        crates/ed-ships/data/hulls/edsy_masses.json [--csv]

--csv prints the per-hull comparison (mass, speed/boost, shields) for the
bench record instead of only writing the file.
"""
import glob
import json
import sys

SHIP_DIRS = ("crates/ed-ships/data/coriolis/ships", "crates/ed-ships/data/hulls")


def main(eddb_path, out_path, csv):
    edsy = json.load(open(eddb_path, encoding="utf-8"))["ship"]
    by_fdid = {str(v["fdid"]): v for v in edsy.values()}
    by_name = {v["name"].lower(): v for v in edsy.values()}
    masses = {}
    rows = []
    for d in SHIP_DIRS:
        for f in sorted(glob.glob(f"{d}/*.json")):
            j = json.load(open(f, encoding="utf-8"))
            key = next(iter(j))
            s = j[key]
            p = s["properties"]
            # By Frontier id first; four Coriolis rows (Caspian, Corsair,
            # Kestrel, Python Mk II) carry an id EDSY and EDCD do not use, and
            # Coriolis's "Viper" is EDSY's "Viper Mk III": id, then name.
            v = by_fdid.get(str(s.get("edID"))) or by_name.get(p["name"].lower())
            if v is None:
                sys.exit(f"{f}: edID {s.get('edID')} / name {p['name']!r} has no EDSY row")
            masses[key] = v["mass"]
            rows.append((key, p["name"], p["hullMass"], v["mass"], p.get("speed"), p.get("boost"),
                         v.get("topspd"), v.get("bstspd"), p.get("baseShieldStrength"), v.get("shields")))
    out = {"_source": f"EDSY eddb.js via knobs/edsy_hull_masses.py; keys are the Coriolis row keys; {len(masses)} hulls"}
    out.update(sorted(masses.items()))
    json.dump(out, open(out_path, "w", encoding="utf-8", newline="\n"), indent=2)
    open(out_path, "a", encoding="utf-8", newline="\n").write("\n")
    if csv:
        print("hull,name,coriolis_mass,edsy_mass,coriolis_speed_boost,edsy_speed_boost,coriolis_shields,edsy_shields,differs")
        for k, n, cm, em, cs, cb, es, eb, csh, esh in rows:
            d = " ".join(x for x, t in (("mass", cm != em), ("speed", (cs, cb) != (es, eb)), ("shields", csh != esh)) if t)
            print(f"{k},{n},{cm},{em},{cs}/{cb},{es}/{eb},{csh},{esh},{d or 'no'}")
    print(f"wrote {out_path}: {len(masses)} hulls, {sum(1 for r in rows if r[2] != r[3])} masses differ from Coriolis", file=sys.stderr)


if __name__ == "__main__":
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2], "--csv" in sys.argv)
