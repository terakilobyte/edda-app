# Module rows the Coriolis data does not carry yet

Same shape and group codes as `../coriolis/modules/**` (the catalog reads
both); shaped by hand from the sources named below until EDCD's
coriolis-data carries them, then deleted here and vendored upstream.

- `extra.json`, 2026-10-20:
  - `Int_LargeCargoRack_Size6_Class1` (crl): the 6D Mk II Cargo Rack, 96 t,
    587,400 Cr, id 129051593 -- EDCD FDevIDs PR #114 and EDSY 4.24.2.2; fitted
    in the maintainer's Anaconda Mk II Cargo01 (CargoCapacity 96, 2026-10-21).
  - `Int_MkIILargeBuggyBay_Size4_Class3` (pv): the Mk II Large Planetary
    Vehicle Hangar 4F, 15 t, 0.8 MW, id 129046059 -- EDSY 4.24.2.2 (EDCD row
    from the Rhino update, 2026-09-05); fitted in the same Loadout.
