# Plot traces

`wongi-colonia-explorer.json.gz` — `examples/trace_plot.rs` output for Wongi → Colonia at commit `522b679` (white dwarfs on the highway by default) over the full index `routing/8` (198,978,505 systems, `boost250/` sub-index of 3,853,782 stars), Explorer Mk II with the size 8 SCO Mk II drive (×6 neutron / ×3 white dwarf): 57 jumps, 56 boosted, 5 refuel stops, white-dwarf hop at LAWD 68. Slide 18 of `docs/how-edda-thinks.html` replays it. Regenerate with `cargo run -p ed-galaxy --example trace_plot --release -- <index> Wongi Colonia > trace.json` (UTF-8) and `gzip -9`.
