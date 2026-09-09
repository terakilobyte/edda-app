//! Top-down (x/z) star-density grid of an index, for the galaxy-map
//! visualisation: one pass over the cell array, counts binned into a
//! fixed frame, JSON on stdout.
//!
//!     density <index_dir> [bin_ly] > density.json

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("usage: density <index_dir> [bin_ly]"));
    let bin_ly: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(250.0);
    // Frame around the whole galaxy, Sol at the origin.
    let (x0, z0, x1, z1) = (-45_000.0f32, -20_000.0f32, 45_000.0, 70_000.0);
    let w = ((x1 - x0) / bin_ly).ceil() as usize;
    let h = ((z1 - z0) / bin_ly).ceil() as usize;
    let g = ed_galaxy::Galaxy::open(&dir)?;
    let mut bins = vec![0u32; w * h];
    // The galactic disc is a few thousand ly thick; +-20,000 covers it.
    let lo = ed_galaxy::format::cell_of_with([x0, -20_000.0, z0], g.cell_ly);
    let hi = ed_galaxy::format::cell_of_with([x1, 20_000.0, z1], g.cell_ly);
    g.for_each_cell_in_box(lo, hi, |cx, _cy, cz, _start, count| {
        let x = (cx as f32 + 0.5) * g.cell_ly;
        let z = (cz as f32 + 0.5) * g.cell_ly;
        if x >= x0 && x < x1 && z >= z0 && z < z1 {
            let (bx, bz) = (((x - x0) / bin_ly) as usize, ((z - z0) / bin_ly) as usize);
            bins[bz * w + bx] = bins[bz * w + bx].saturating_add(count);
        }
        std::ops::ControlFlow::Continue(())
    });
    let cells: Vec<String> = bins.iter().map(|c| c.to_string()).collect();
    println!(
        "{{\"w\":{w},\"h\":{h},\"x0\":{x0},\"z0\":{z0},\"bin_ly\":{bin_ly},\"counts\":[{}]}}",
        cells.join(",")
    );
    Ok(())
}
