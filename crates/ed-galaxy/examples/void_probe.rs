//! Walk the straight line between two stars in half-cell steps and print,
//! per step, how many stars the main index holds in that cell and its 26
//! neighbours: the instrument behind honest bridge pricing (R4). A run of
//! empty neighbourhoods wider than the ship's jump is a gap no tank flies.
//!
//!     void_probe <index_dir> <from> <to> [range_ly]
use ed_galaxy::Galaxy;
use std::path::Path;
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: void_probe <index_dir> <from> <to> [range_ly]");
        std::process::exit(2);
    }
    let g = Galaxy::open(Path::new(&args[0]))?;
    let range: f32 = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(77.8);
    let a = g.find(&args[1]).ok_or_else(|| anyhow::anyhow!("unknown {}", args[1]))?;
    let b = g.find(&args[2]).ok_or_else(|| anyhow::anyhow!("unknown {}", args[2]))?;
    let (pa, pb) = (g.pos_of(a), g.pos_of(b));
    let d = ((pb[0] - pa[0]).powi(2) + (pb[1] - pa[1]).powi(2) + (pb[2] - pa[2]).powi(2)).sqrt();
    let cell = g.cell_ly;
    let step = cell / 2.0;
    let n = (d / step).ceil().max(1.0) as usize;
    println!("{} -> {}: {d:.0} ly, {cell} ly cells, {n} steps of {step:.0} ly, range {range}", args[1], args[2]);
    let q = |v: f32| (v / cell).floor() as i32;
    let mut empty_run = 0.0f32;
    let mut worst_run = 0.0f32;
    for i in 0..=n {
        let t = i as f32 / n as f32;
        let p = [pa[0] + t * (pb[0] - pa[0]), pa[1] + t * (pb[1] - pa[1]), pa[2] + t * (pb[2] - pa[2])];
        let (cx, cy, cz) = (q(p[0]), q(p[1]), q(p[2]));
        let mut here = 0u32;
        let mut hood = 0u32;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some((_, k)) = g.cell_range(cx + dx, cy + dy, cz + dz) {
                        hood += k as u32;
                        if dx == 0 && dy == 0 && dz == 0 {
                            here = k as u32;
                        }
                    }
                }
            }
        }
        if hood == 0 {
            empty_run += step;
            worst_run = worst_run.max(empty_run);
        } else {
            empty_run = 0.0;
        }
        println!("  step {i:3} at {:6.0} ly: cell {here:5} stars, neighbourhood {hood:6}", t * d);
    }
    println!("longest empty neighbourhood run {worst_run:.0} ly ({}): {}", if worst_run > range { "WIDER than a jump" } else { "within a jump" }, if worst_run > range { "no tank flies this line" } else { "the line is populated" });
    Ok(())
}
