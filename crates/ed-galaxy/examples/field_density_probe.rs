//! Item 24 probe (a): cost and shape of a FULL-index field-density
//! oracle, plus the density truth along specific corridors.
//!
//!     field_density_probe <main_index_dir> [--voxel LY] \
//!         [--line x0,y0,z0 x1,y1,z1 [label]]...
//!
//! Streams every occupied 50-ly cell entry of the MAIN index once into
//! a coarse voxel count table (default 250 ly), reports what that
//! costs (time, voxels, bytes) — the oracle the density-aware crossing
//! pricing would consult at cgraph build time. Then, per --line,
//! prints the density profile along the segment: per-1,000-ly bucket,
//! the mean and p10 stars-per-voxel within one voxel of the line, and
//! the equivalent expected stars per 78-ly-radius sphere (the plain
//! jump's reach — below ~1 the "plain crossing" priced by gap edges
//! is fiction there). Finally the global voxel-count percentiles of
//! the disc, the agnostic normalization the pricing formula keys on
//! (no slab bounds, no named geography — just measured density).

use std::collections::HashMap;
use std::time::Instant;

use ed_galaxy::format::Galaxy;
use ed_galaxy::StarClassCode as _;

fn parse3(s: &str) -> [f32; 3] {
    let v: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse().expect("x,y,z"))
        .collect();
    [v[0], v[1], v[2]]
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(
        args.first()
            .expect("usage: field_density_probe <main_index_dir> ..."),
    );
    let mut voxel_ly = 250.0f32;
    let mut save: Option<std::path::PathBuf> = None;
    let mut scoopable_only = false;
    let mut lines: Vec<([f32; 3], [f32; 3], String)> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--voxel" => {
                voxel_ly = args[i + 1].parse()?;
                i += 2;
            }
            // Item 24d: dump the voxel table (vx,vy,vz,count CSV) so the
            // field census report can join expansion positions against it.
            "--save" => {
                save = Some(std::path::PathBuf::from(&args[i + 1]));
                i += 2;
            }
            // Item 33: count only SCOOPABLE stars (KGBFOAM main star or a
            // scoop-nearby companion) — the reserve floor's currency. Walks
            // records instead of cell counts, so the build pass is slower.
            "--scoopable" => {
                scoopable_only = true;
                i += 1;
            }
            "--line" => {
                let a = parse3(&args[i + 1]);
                let b = parse3(&args[i + 2]);
                let label = if args.len() > i + 3 && !args[i + 3].starts_with("--") {
                    i += 1;
                    args[i + 2].clone()
                } else {
                    format!("line{}", lines.len())
                };
                lines.push((a, b, label));
                i += 3;
            }
            other => anyhow::bail!("unknown arg {other}"),
        }
    }

    let g = Galaxy::open(&dir)?;
    let cell_ly = g.cell_ly;
    eprintln!("main index: {} stars, {cell_ly} ly cells", g.count);

    // The oracle build: one pass over the occupied cell entries via the
    // public box walk, covering the whole disc.
    let t = Instant::now();
    let mut voxels: HashMap<(i32, i32, i32), u32> = HashMap::new();
    let scale = voxel_ly / cell_ly; // cells per voxel side
    let lo = ed_galaxy::format::cell_of_with([-45_000.0, -20_000.0, -25_000.0], cell_ly);
    let hi = ed_galaxy::format::cell_of_with([45_000.0, 20_000.0, 70_000.0], cell_ly);
    let mut occupied = 0usize;
    g.for_each_cell_in_box(lo, hi, |cx, cy, cz, start, count| {
        occupied += 1;
        let k = (
            (cx as f32 / scale).floor() as i32,
            (cy as f32 / scale).floor() as i32,
            (cz as f32 / scale).floor() as i32,
        );
        let n = if scoopable_only {
            let mut n = 0u32;
            for r in start..start + count {
                let class = {
                    let code = g.class_code(r);
                    if code == ed_galaxy::StarClass::Unknown.code() {
                        g.class(&g.record(r))
                    } else {
                        ed_galaxy::StarClass::from_code(code)
                    }
                };
                if class.scoopable() || g.flags(r) & ed_galaxy::format::FLAG_SCOOP_NEARBY != 0 {
                    n += 1;
                }
            }
            n
        } else {
            count
        };
        *voxels.entry(k).or_insert(0) += n;
        std::ops::ControlFlow::Continue(())
    });
    eprintln!("{occupied} occupied cells in the disc box");
    let built = t.elapsed();
    let bytes = voxels.len() * (12 + 4);
    eprintln!(
        "oracle: {} occupied {voxel_ly} ly voxels in {:.2} s (~{:.1} MB as a flat table)",
        voxels.len(),
        built.as_secs_f64(),
        bytes as f64 / 1e6
    );

    if let Some(path) = &save {
        use std::io::Write as _;
        let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
        writeln!(f, "# voxel_ly {voxel_ly}")?;
        for ((vx, vy, vz), count) in &voxels {
            writeln!(f, "{vx},{vy},{vz},{count}")?;
        }
        eprintln!("voxel table saved to {}", path.display());
    }

    // Global normalization: voxel-count percentiles (occupied only).
    let mut counts: Vec<u32> = voxels.values().copied().collect();
    counts.sort_unstable();
    let pct = |p: f64| counts[((counts.len() - 1) as f64 * p) as usize];
    eprintln!(
        "occupied-voxel stars: p10 {} p25 {} p50 {} p75 {} p90 {} p99 {}",
        pct(0.10),
        pct(0.25),
        pct(0.50),
        pct(0.75),
        pct(0.90),
        pct(0.99)
    );

    // Per-line profiles. Sample the segment every half voxel; each
    // sample reads the 3x3x3 voxel block around it (what a build-time
    // pricing query would do), so the profile numbers ARE the oracle's
    // answers, not a finer truth the oracle cannot see.
    let vol_sphere78 = 4.0 / 3.0 * std::f32::consts::PI * 78.0f32.powi(3);
    let vol_voxel = voxel_ly.powi(3);
    for (a, b, label) in &lines {
        let d = ed_galaxy::format::dist(*a, *b);
        let steps = (d / (voxel_ly * 0.5)).ceil() as usize;
        println!("\n== {label}: {:.0} ly ==", d);
        println!(
            "{:>10} {:>12} {:>12} {:>16}",
            "at_ly", "mean/voxel", "min/voxel", "stars_per_78ly"
        );
        let bucket_ly = 1_000.0f32;
        let mut bucket: Vec<f32> = Vec::new();
        let mut bucket_lo = 0.0f32;
        let flush = |lo: f32, samples: &mut Vec<f32>| {
            if samples.is_empty() {
                return;
            }
            let mean = samples.iter().sum::<f32>() / samples.len() as f32;
            let min = samples.iter().copied().fold(f32::MAX, f32::min);
            let per78 = mean / vol_voxel * vol_sphere78;
            println!("{:>10.0} {:>12.1} {:>12.1} {:>16.2}", lo, mean, min, per78);
            samples.clear();
        };
        for s in 0..=steps {
            let f = s as f32 / steps as f32;
            let at = f * d;
            if at - bucket_lo >= bucket_ly {
                flush(bucket_lo, &mut bucket);
                bucket_lo = (at / bucket_ly).floor() * bucket_ly;
            }
            let p = [
                a[0] + (b[0] - a[0]) * f,
                a[1] + (b[1] - a[1]) * f,
                a[2] + (b[2] - a[2]) * f,
            ];
            let k = (
                (p[0] / voxel_ly).floor() as i32,
                (p[1] / voxel_ly).floor() as i32,
                (p[2] / voxel_ly).floor() as i32,
            );
            let mut sum = 0u64;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        sum += u64::from(
                            voxels
                                .get(&(k.0 + dx, k.1 + dy, k.2 + dz))
                                .copied()
                                .unwrap_or(0),
                        );
                    }
                }
            }
            bucket.push(sum as f32 / 27.0);
        }
        flush(bucket_lo, &mut bucket);
    }
    Ok(())
}
