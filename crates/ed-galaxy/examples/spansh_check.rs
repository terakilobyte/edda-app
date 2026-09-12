//! Check a Spansh route's waypoints against our boost sub-index.
//!
//!     spansh_check <index_dir> <waypoints.csv>
//!
//! For every waypoint flagged has_neutron, find the nearest star in the
//! sub-index (searching the waypoint's cell and its 26 neighbours) and
//! report the distance. Near-zero means the chain star is in our data;
//! anything else is a hole. Prints a per-waypoint line and a summary.

use ed_galaxy::Galaxy;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().map(String::as_str).unwrap_or(".data/galaxy"));
    let csv = args
        .get(1)
        .map(String::as_str)
        .unwrap_or("spansh-waypoints.csv");
    let sub = Galaxy::open(&ed_galaxy::long_range::neutron_dir(dir))?;
    let cell = sub.cell_ly;
    eprintln!("{} stars, {} ly cells", sub.count, cell);

    let mut flagged = 0u64;
    for i in 0..sub.count as u32 {
        if sub.flags(i) & ed_galaxy::format::FLAG_SCOOP_NEARBY != 0 {
            flagged += 1;
        }
    }
    eprintln!(
        "FLAG_SCOOP_NEARBY: {flagged} of {} sub-index stars ({:.1}%)",
        sub.count,
        flagged as f64 / sub.count as f64 * 100.0
    );

    let mut in_index = 0u32;
    let mut missing = 0u32;
    let mut checked = 0u32;
    let mut near_scoop = 0u32;
    println!(
        "{:<34} {:>9} {:>9} {:>10}",
        "waypoint", "cellstars", "nearest", "verdict"
    );
    for line in std::fs::read_to_string(csv)?.lines().skip(1) {
        let f: Vec<&str> = line.split(',').collect();
        if f.len() < 6 || f[5] != "1" {
            continue; // only has_neutron waypoints
        }
        let p: [f32; 3] = [f[2].parse()?, f[3].parse()?, f[4].parse()?];
        let q = |v: f32| (v / cell).floor() as i32;
        let (cx, cy, cz) = (q(p[0]), q(p[1]), q(p[2]));
        let mut nearest = f32::MAX;
        let mut nearest_idx = None;
        let mut cell_stars = 0u32;
        let mut cell_scoop = 0u32;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some((s0, n)) = sub.cell_range(cx + dx, cy + dy, cz + dz) {
                        if dx == 0 && dy == 0 && dz == 0 {
                            cell_stars = n;
                        }
                        for i in s0..s0 + n {
                            let sp = sub.pos_of(i);
                            let d2 = (sp[0] - p[0]).powi(2)
                                + (sp[1] - p[1]).powi(2)
                                + (sp[2] - p[2]).powi(2);
                            if d2 < nearest {
                                nearest = d2;
                                nearest_idx = Some(i);
                            }
                            if dx == 0
                                && dy == 0
                                && dz == 0
                                && sub.flags(i) & ed_galaxy::format::FLAG_SCOOP_NEARBY != 0
                            {
                                cell_scoop += 1;
                            }
                        }
                    }
                }
            }
        }
        let nearest = if nearest == f32::MAX {
            -1.0
        } else {
            nearest.sqrt()
        };
        checked += 1;
        let verdict = if (0.0..0.5).contains(&nearest) {
            in_index += 1;
            "in-index"
        } else {
            missing += 1;
            "MISSING"
        };
        let scoop =
            nearest_idx.is_some_and(|i| sub.flags(i) & ed_galaxy::format::FLAG_SCOOP_NEARBY != 0);
        near_scoop += u32::from(scoop);
        println!(
            "{:<34} {:>9} {:>7} {:>9.1} {:>10} {}",
            f[0],
            cell_stars,
            cell_scoop,
            nearest,
            verdict,
            if scoop { "scoop" } else { "-" }
        );
    }
    println!("\n{checked} neutron waypoints: {in_index} in our sub-index, {missing} missing; {near_scoop} nearest-stars carry FLAG_SCOOP_NEARBY");
    Ok(())
}
