//! Measure the galaxy's highway density curve — the calibration input
//! for the density-adaptive greedy cone (ROUTING-NEXT item 13).
//!
//!     density_probe <index_dir> [--center x,y,z] [--line From To]
//!
//! Radial mode (default): buckets the SUB-INDEX's occupied cells by
//! distance from `--center` (default 25900 ly toward Sagittarius A*,
//! the galactic core) and prints stars-per-cell and occupancy per
//! 1,000 ly band — the density curve the greediness dial is calibrated
//! against, with the arm/inter-arm contrast visible as the spread
//! within a band (p10/p50/p90 of per-cell counts).
//!
//! Line mode: buckets cells within one cell-width of the From→To line
//! by position along it — the density profile a specific plot flies
//! through.

use ed_galaxy::Galaxy;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().map(String::as_str).unwrap_or(".data/galaxy"));
    let g = Galaxy::open(dir)?;
    let sub = Galaxy::open(&ed_galaxy::long_range::neutron_dir(dir))?;
    let cell = sub.cell_ly;

    // Collect every occupied cell's center and star count via the cell
    // walk over the whole occupied bounding volume.
    let mut cells: Vec<([f32; 3], u32)> = Vec::new();
    {
        // Derive per-cell counts by walking records in cell order.
        let mut current: Option<(i64, u32, [f32; 3])> = None;
        let key_of = |p: [f32; 3]| -> i64 {
            let q = |v: f32| (v / cell).floor() as i64;
            (q(p[0]) << 42) ^ (q(p[1]) << 21) ^ q(p[2])
        };
        for i in 0..sub.count as u32 {
            let p = sub.pos_of(i);
            let k = key_of(p);
            match current {
                Some((ck, n, cp)) if ck == k => current = Some((ck, n + 1, cp)),
                Some((_, n, cp)) => {
                    cells.push((cp, n));
                    current = Some((k, 1, p));
                }
                None => current = Some((k, 1, p)),
            }
        }
        if let Some((_, n, cp)) = current {
            cells.push((cp, n));
        }
    }
    eprintln!(
        "{} stars in {} occupied {} ly cells",
        sub.count,
        cells.len(),
        cell
    );

    if let Some(i) = args.iter().position(|a| a == "--line") {
        let from = g
            .find(&args[i + 1])
            .ok_or_else(|| anyhow::anyhow!("unknown from"))?;
        let to = g
            .find(&args[i + 2])
            .ok_or_else(|| anyhow::anyhow!("unknown to"))?;
        let (a, b) = (g.record(from).pos(), g.record(to).pos());
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        let len = len2.sqrt();
        println!(
            "# line {} -> {}: {:.0} ly; bands of 5% with cells within {} ly of the line",
            args[i + 1],
            args[i + 2],
            len,
            cell * 1.5
        );
        let mut bands = [(0u32, 0u32); 20]; // (cells, stars)
        for &(p, n) in &cells {
            let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
            let t = (ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let proj = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
            let d2 = (p[0] - proj[0]).powi(2) + (p[1] - proj[1]).powi(2) + (p[2] - proj[2]).powi(2);
            if d2 <= (cell * 1.5) * (cell * 1.5) {
                let band = ((t * 20.0) as usize).min(19);
                bands[band].0 += 1;
                bands[band].1 += n;
            }
        }
        println!(
            "{:>5} {:>8} {:>8} {:>10}",
            "band", "cells", "stars", "stars/cell"
        );
        for (i, (c, n)) in bands.iter().enumerate() {
            println!(
                "{:>4}% {:>8} {:>8} {:>10.1}",
                i * 5,
                c,
                n,
                if *c > 0 { *n as f32 / *c as f32 } else { 0.0 }
            );
        }
        return Ok(());
    }

    let center: [f32; 3] = args
        .iter()
        .position(|a| a == "--center")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| {
            let p: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            (p.len() == 3).then(|| [p[0], p[1], p[2]])
        })
        .unwrap_or([0.0, 0.0, 25_900.0]);
    println!("# radial density from center {:?}, 1000 ly bands", center);
    let mut bands: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
    for &(p, n) in &cells {
        let r =
            ((p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) + (p[2] - center[2]).powi(2))
                .sqrt();
        bands.entry((r / 1000.0) as u32).or_default().push(n);
    }
    println!(
        "{:>7} {:>8} {:>9} {:>6} {:>6} {:>6}",
        "r (kly)", "cells", "stars", "p10", "p50", "p90"
    );
    for (band, mut counts) in bands {
        counts.sort_unstable();
        let q = |f: f32| counts[((counts.len() - 1) as f32 * f) as usize];
        let total: u64 = counts.iter().map(|&c| c as u64).sum();
        println!(
            "{:>7} {:>8} {:>9} {:>6} {:>6} {:>6}",
            band,
            counts.len(),
            total,
            q(0.1),
            q(0.5),
            q(0.9)
        );
    }
    Ok(())
}
