//! Coarse-phase traversal bench: per-cell probing vs the Morton range
//! walk, on a real index. Answers whether BIGMIN walking earns its keep
//! in the gateway/corridor phases, and by how much where space is empty.
//!
//!   morton_bench <index_dir> [--boost <subindex_dir>]
//!
//! The index must be EDGX v3 (the walk falls back to probing on v2, which
//! would bench nothing). Regions: bubble (dense), the Wongi->Colonia
//! corridor (mid), Colonia->Beagle Point (sparse), and a rim void.

use ed_galaxy::StarClassCode as _;
use std::ops::ControlFlow;
use std::time::Instant;

fn cell_box(g: &ed_galaxy::Galaxy, a: [f32; 3], b: [f32; 3], pad: f32) -> ((i32, i32, i32), (i32, i32, i32)) {
    let lo = ed_galaxy::format::cell_of_with([a[0].min(b[0]) - pad, a[1].min(b[1]) - pad, a[2].min(b[2]) - pad], g.cell_ly);
    let hi = ed_galaxy::format::cell_of_with([a[0].max(b[0]) + pad, a[1].max(b[1]) + pad, a[2].max(b[2]) + pad], g.cell_ly);
    (lo, hi)
}

struct Sample {
    cells: u64,
    stars: u64,
    micros: f64,
}

fn probe_all(g: &ed_galaxy::Galaxy, lo: (i32, i32, i32), hi: (i32, i32, i32), reps: u32) -> Sample {
    let mut cells = 0u64;
    let mut stars = 0u64;
    let started = Instant::now();
    for _ in 0..reps {
        cells = 0;
        stars = 0;
        for cx in lo.0..=hi.0 {
            for cy in lo.1..=hi.1 {
                for cz in lo.2..=hi.2 {
                    if let Some((_, count)) = g.cell_range(cx, cy, cz) {
                        cells += 1;
                        stars += count as u64;
                    }
                }
            }
        }
    }
    Sample { cells, stars, micros: started.elapsed().as_secs_f64() * 1e6 / reps as f64 }
}

fn walk_all(g: &ed_galaxy::Galaxy, lo: (i32, i32, i32), hi: (i32, i32, i32), reps: u32) -> Sample {
    let mut cells = 0u64;
    let mut stars = 0u64;
    let started = Instant::now();
    for _ in 0..reps {
        cells = 0;
        stars = 0;
        g.for_each_cell_in_box(lo, hi, |_, _, _, _, count| {
            cells += 1;
            stars += count as u64;
            ControlFlow::Continue(())
        });
    }
    Sample { cells, stars, micros: started.elapsed().as_secs_f64() * 1e6 / reps as f64 }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("usage: morton_bench <index_dir> [--boost <dir>]"));
    let boost = match (args.next().as_deref(), args.next()) {
        (Some("--boost"), Some(p)) => Some(std::path::PathBuf::from(p)),
        _ => None,
    };
    let g = ed_galaxy::Galaxy::open(&dir)?;
    eprintln!("index: {} systems, cell {} ly", g.count, g.cell_ly);

    let position = |name: &str| -> anyhow::Result<[f32; 3]> {
        let idx = g.find(name).ok_or_else(|| anyhow::anyhow!("{name} not in index"))?;
        Ok(g.pos_of(idx))
    };
    let sol = position("Sol")?;
    let wongi = position("Wongi").unwrap_or(sol);
    let colonia = position("Colonia")?;
    let beagle = position("Beagle Point")?;
    let rim = [-4_000.0f32, 800.0, 30_000.0]; // a void well off the arm

    let boost_g = match boost {
        Some(boost_dir) => {
            if !ed_galaxy::Galaxy::exists(&boost_dir) {
                eprintln!("building 250 ly highway sub-index at {}...", boost_dir.display());
                let started = Instant::now();
                ed_galaxy::import::subset_cells(&g, &boost_dir, 250.0, |r| {
                    r.class == ed_galaxy::StarClass::Neutron.code()
                        || r.class == ed_galaxy::StarClass::WhiteDwarf.code()
                })?;
                eprintln!("built in {:.1}s", started.elapsed().as_secs_f64());
            }
            let opened = ed_galaxy::Galaxy::open(&boost_dir)?;
            eprintln!("boost sub-index: {} systems, cell {} ly", opened.count, opened.cell_ly);
            Some((opened, boost_dir))
        }
        None => None,
    };
    let boost_g = boost_g.as_ref();
    let mut indexes: Vec<(&str, &ed_galaxy::Galaxy)> = vec![("full-50ly", &g)];
    if let Some((boost_ref, _)) = boost_g {
        indexes.push(("boost-250ly", boost_ref));
    }

    // Aggregate-oracle arm: "any scoopable / neutron within R of P", the
    // min-fuel search's per-expansion question, asked at points along the
    // corridors -- oracle descent vs the walked cell scan with early exit.
    if let Some((sub, sub_dir)) = boost_g {
        let agg_path = sub_dir.join(ed_galaxy::agg::AGG_FILE);
        let agg = match ed_galaxy::agg::Aggregate::open(&agg_path) {
            Ok(a) => a,
            Err(_) => {
                eprintln!("building {}...", agg_path.display());
                let built = ed_galaxy::agg::build(sub)?;
                built.write(&agg_path)?;
                built
            }
        };
        let scan = |center: [f32; 3], radius: f32, want: u8| -> bool {
            let mut hit = false;
            sub.for_each_within(center, radius, |idx, _| {
                if hit {
                    return;
                }
                let class = ed_galaxy::StarClass::from_code(sub.class_code(idx));
                let flags = ((class == ed_galaxy::StarClass::Neutron) as u8 * ed_galaxy::agg::ANY_NEUTRON) | ((class == ed_galaxy::StarClass::WhiteDwarf) as u8 * ed_galaxy::agg::ANY_WHITE_DWARF) | (((class.scoopable() || sub.flags(idx) & ed_galaxy::format::FLAG_SCOOP_NEARBY != 0) as u8)
                        * ed_galaxy::agg::ANY_SCOOPABLE);
                hit |= flags & want != 0;
            });
            hit
        };
        println!(
            "{:<18} {:>8} {:>10} {:>12} {:>12} {:>8} {:>10} {:>8}",
            "oracle: corridor", "r_ly", "want", "scan_us", "oracle_us", "speedup", "nodes", "hits"
        );
        let corridors: [(&str, [f32; 3], [f32; 3]); 2] =
            [("wongi-colonia", wongi, colonia), ("colonia-beagle", colonia, beagle)];
        for (region, a, b) in corridors {
            for radius in [600.0f32, 1_200.0] {
                for (label, want) in [("scoop", ed_galaxy::agg::ANY_SCOOPABLE), ("neutron", ed_galaxy::agg::ANY_NEUTRON)] {
                    const STEPS: u32 = 200;
                    let at = |i: u32| -> [f32; 3] {
                        let t = i as f32 / (STEPS - 1) as f32;
                        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
                    };
                    let started = Instant::now();
                    let mut scan_hits = 0u32;
                    for i in 0..STEPS {
                        scan_hits += scan(at(i), radius, want) as u32;
                    }
                    let scan_us = started.elapsed().as_secs_f64() * 1e6 / STEPS as f64;
                    let started = Instant::now();
                    let mut oracle_hits = 0u32;
                    let mut nodes = 0u64;
                    for i in 0..STEPS {
                        let (hit, probe) = agg.any_within_probed(sub, at(i), radius, want);
                        oracle_hits += hit as u32;
                        nodes += u64::from(probe.nodes_visited);
                    }
                    let oracle_us = started.elapsed().as_secs_f64() * 1e6 / STEPS as f64;
                    assert_eq!(scan_hits, oracle_hits, "oracle disagrees with the scan in {region}");
                    println!(
                        "{:<18} {:>8.0} {:>10} {:>12.2} {:>12.2} {:>8.2} {:>10.1} {:>7}/{}",
                        region, radius, label, scan_us, oracle_us,
                        scan_us / oracle_us.max(0.001), nodes as f64 / STEPS as f64, oracle_hits, STEPS
                    );
                }
            }
        }
    }

    println!("{:<12} {:<18} {:>7} {:>12} {:>12} {:>12} {:>12} {:>8}", "index", "region", "pad_ly", "box_cells", "occupied", "probe_us", "walk_us", "speedup");
    for (index_name, galaxy) in &indexes {
        let regions: [(&str, [f32; 3], [f32; 3]); 4] = [
            ("bubble", sol, wongi),
            ("wongi-colonia", wongi, colonia),
            ("colonia-beagle", colonia, beagle),
            ("rim-void", rim, rim),
        ];
        for (region, a, b) in regions {
            for pad in [250.0f32, 500.0, 1_000.0] {
                let (lo, hi) = cell_box(galaxy, a, b, pad);
                let volume = (hi.0 - lo.0 + 1) as u64 * (hi.1 - lo.1 + 1) as u64 * (hi.2 - lo.2 + 1) as u64;
                let reps = if volume > 2_000_000 { 1 } else if volume > 20_000 { 5 } else { 50 };
                let p = probe_all(galaxy, lo, hi, reps);
                let w = walk_all(galaxy, lo, hi, reps);
                assert_eq!((p.cells, p.stars), (w.cells, w.stars), "traversals disagree in {region}");
                println!(
                    "{:<12} {:<18} {:>7.0} {:>12} {:>12} {:>12.1} {:>12.1} {:>8.2}",
                    index_name, region, pad, volume, p.cells, p.micros, w.micros,
                    p.micros / w.micros.max(0.001)
                );
            }
        }
    }
    Ok(())
}
