# Hull rows the Coriolis data does not carry yet

Same shape as `../coriolis/ships/*.json` (the catalog reads both), shaped by
hand from the sources named below until EDCD's coriolis-data carries the hull;
when it does, delete the file here and vendor the upstream row (the Coriolis
directory itself is never edited by hand).

- `anacondamkii.json` -- Anaconda Mk II, 2026-10-20. Hull figures from EDSY
  4.24.2.2 (mass, speed, boost, shields, armour, hardness, agility, prices,
  reserve fuel); slots from Frontier's October 2026 dev log, agreed by EDSY;
  bulkhead ids and names from EDCD FDevIDs PR #114, masses from EDSY. The
  cross-reference: `docs/benches/2026-10-20-anaconda-mkii-edcd-crossref.csv`.
  Heat capacity and masslock were unpublished and are left out. The
  maintainer's Loadout pins mass and fuel when he flies it.
- `edsy_masses.json` -- hull mass for every hull from EDSY, keyed by the
  Coriolis row key; `Catalog::load` takes it over the row's `hullMass`.
  Generated, never hand-edited: `python docs/benches/knobs/edsy_hull_masses.py
  <eddb.json> crates/ed-ships/data/hulls/edsy_masses.json`. Why (the
  maintainer, 2026-10-23): the 4.4.2.0 patch lightened six hulls and
  coriolis-data had not moved; measured first, the two sources agreed on the
  other 42 (`docs/benches/2026-10-23-hull-mass-coriolis-vs-edsy.csv`).
  Speed, boost and shields stay Coriolis's.
