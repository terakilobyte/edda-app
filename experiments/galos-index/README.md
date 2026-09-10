# Our router over galos's octree

Spike, 2026-09-09. Maintainer: "build their index fully using our data,
see the disk size of it, and spin an experiment where we use our router
on their index", and: "I am super skeptical of that 0.08 seconds when
there are 200M systems". Nothing here merges; the numbers go in
`docs/benches/` either way.

galos: https://github.com/nixpulvis/galos (MIT, Nathan Lilienthal),
pinned at a8fc2fc, the commit the comparison page was written against.

## Pre-registered, before the full run

1. **Jumps identical** on every route, grid and octree, because the plan
   is the same code asking the same neighbour question of the same
   records. A difference is a bug in the adapter, not a result.
2. **Octree plot time within 2× of the grid** on the pinned routes. Our
   grid answers a range query by binary-searching a sorted cell table and
   scanning contiguous 50 ly slices; their tree is a descent from the
   root through every cell whose box the sphere touches, each cell owning
   a slice of its own (the brightest 512 of an internal cell, up to 4,096
   in a leaf), so the root's slice is scanned on every expansion.
3. **Their directory larger than ours.** Cells alone: 39 bytes a system
   in their payload against 29 in our record. With a names table (which
   this spike does not write; their map holds ~45 B/system of names
   resident) they land near 84 B/system against our 56 with names and
   the by-name index.
4. **Build memory is the risk at 199 M.** Their builder takes the whole
   galaxy as one slice of 64-byte `System`s and raises the tree in
   memory. Measured on the 3.8 M fixture first; the full build only runs
   on the PC if the per-system figure fits under 24 GB (WSL cap) or 32 GB
   (native).
5. **Their own router's 0.08 s** (Sol → Colonia at 50 ly, Quick mode,
   2.4 M systems) is to be re-measured over the same 199 M-system names
   table, once its graph code is lifted out of the bevy map. Expectation:
   the graph build alone (their `Places` arrays plus buckets) is several
   GB and tens of seconds, and Quick mode slows with the density of the
   bubble crossing, not the total count; the corridor to Colonia is
   sparse in both datasets.

## Running

    export CARGO_TARGET_DIR=<somewhere with room>
    cargo build --release
    B=$CARGO_TARGET_DIR/release/galos-index-experiment

    # our index -> their octree (records only; no names table)
    /usr/bin/time -l $B build <edda-index-dir> <out-dir>      # macOS
    /usr/bin/time -v $B build <edda-index-dir> <out-dir>      # Linux

    # one route, both structures, N repetitions (best and median ms)
    $B route <edda-index-dir> <out-dir> Sol Colonia 50 5

    # every distinct pair in a pins CSV at one range
    $B pins <edda-index-dir> <out-dir> ../../docs/benches/2026-09-02-sweep14-item20-pins.csv 50 5

`build` prints a one-line CSV on stdout (systems, cells, leaves, build_s,
write_s, galos_bytes, edda_bytes) and the narrative on stderr. `route`
and `pins` print CSV rows on stdout and `DIFFER:` on stderr if jumps or
hops ever disagree.
