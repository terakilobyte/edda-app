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
