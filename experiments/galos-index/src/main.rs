//! Our router over their index. See Cargo.toml for the ask.
//!
//! `build` turns every record of an ed-galaxy star index into a
//! `galos_index::System` and raises the galos octree over them, then writes
//! it out the way their builder does. The one trick: their payload's `id64`
//! field carries OUR record index, so a point found in one of their cells
//! is looked up in our memory-mapped records for its class and flags with
//! no translation table between the two. Magnitude and temperature come
//! from their own class-letter fallback, the path two thirds of their
//! systems take.
//!
//! `route` runs the same plan twice: once with our 50 ly grid answering
//! the neighbour question, once with a descent of their octree answering
//! it, and reports jumps, expansions and wall time for each. The plan is
//! otherwise identical, so a difference in jumps is a bug in the adapter,
//! and a difference in time is the structure.

use anyhow::{anyhow, bail, Context, Result};
use ed_galaxy::format::Galaxy;
use ed_galaxy::router::{self, Control, Near, RouteRequest};
use ed_galaxy::StarClassCode as _;
use galos_index::geometry::CellId;
use galos_index::source::{FsSource, Source};
use galos_index::tree::{BuildParams, Snapshot, System};
use galos_index::walk::Index;
use galos_index::Point;
use galos_photometry::ClassLight;
use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

mod theirs;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("info") if args.len() >= 2 => info(Path::new(&args[1]), &args[2..]),
        // build <edda> <out> [--within CX CY CZ R] [--for-map]
        //   --within: only the systems within R ly of the centre, for a
        //   build that fits the machine; a bitmap of the records taken is
        //   written beside the tree so the grid side of a comparison can
        //   be held to the same subset.
        //   --for-map: a directory their map can open. Payload id64 is the
        //   real address (not our record index), and the names chunks and
        //   the supercharge table are written beside the tree.
        Some("build") if args.len() >= 3 => {
            let mut within = None;
            let mut for_map = false;
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--within" if i + 4 < args.len() => {
                        let c: Vec<f32> = args[i + 1..i + 4].iter().map(|s| s.parse()).collect::<std::result::Result<_, _>>()?;
                        within = Some(([c[0], c[1], c[2]], args[i + 4].parse::<f32>()?));
                        i += 5;
                    }
                    "--for-map" => {
                        for_map = true;
                        i += 1;
                    }
                    other => bail!("unknown build option {other}"),
                }
            }
            build(Path::new(&args[1]), Path::new(&args[2]), within, for_map)
        }
        Some("route") if args.len() >= 6 => {
            let reps = args.get(6).map(|s| s.parse()).transpose()?.unwrap_or(3);
            let g = Galaxy::open(Path::new(&args[1]))?;
            let oct = Octree::load(Path::new(&args[2]))?;
            let range: f32 = args[5].parse()?;
            println!("route_from,route_to,range_ly,structure,jumps,boosted,expansions,best_ms,median_ms");
            compare(&g, &oct, &args[3], &args[4], range, reps)
        }
        Some("pins") if args.len() >= 5 => {
            let reps = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(3);
            let g = Galaxy::open(Path::new(&args[1]))?;
            let oct = Octree::load(Path::new(&args[2]))?;
            let range: f32 = args[4].parse()?;
            println!("route_from,route_to,range_ly,structure,jumps,boosted,expansions,best_ms,median_ms");
            let csv = std::fs::read_to_string(&args[3])?;
            let mut seen = std::collections::HashSet::new();
            for line in csv.lines().filter(|l| !l.starts_with('#') && !l.starts_with("route_from")) {
                let mut it = line.split(',');
                let (Some(from), Some(to)) = (it.next(), it.next()) else { continue };
                if !seen.insert((from.to_string(), to.to_string())) {
                    continue;
                }
                if let Err(e) = compare(&g, &oct, from, to, range, reps) {
                    eprintln!("{from} -> {to}: {e}");
                }
            }
            Ok(())
        }
        // theirs <edda-index> FROM TO RANGE [reps]: their map's router over
        // every position in our index, all three of their modes, against
        // our planner on the same pair. No octree involved.
        Some("theirs") if args.len() >= 5 => {
            let reps = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(3);
            let range: f32 = args[4].parse()?;
            theirs_vs_ours(Path::new(&args[1]), &args[2], &args[3], range, reps)
        }
        // theirs-raw <stars.bin> FROM_IDX TO_IDX RANGE [reps] [mode]: their
        // router over the records alone — the one 5.8 GB file, no names,
        // no grid — for a machine that has the memory but not the index.
        // Record indices come from `info` on a full index. mode = quick |
        // direct | shortest | all (default all).
        Some("theirs-raw") if args.len() >= 5 => {
            let reps = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(3);
            let mode = args.get(6).map(String::as_str).unwrap_or("all").to_string();
            theirs_raw(Path::new(&args[1]), args[2].parse()?, args[3].parse()?, args[4].parse()?, reps, &mode)
        }
        // ours <edda-index> FROM TO RANGE [reps]: our planner alone, same
        // request shape as the comparison rows.
        Some("ours") if args.len() >= 5 => {
            let reps = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(3);
            let range: f32 = args[4].parse()?;
            let g = Galaxy::open(Path::new(&args[1]))?;
            println!("route_from,route_to,range_ly,router,mode,jumps,expansions,best_ms,median_ms");
            ours_only(&g, &args[2], &args[3], range, reps)
        }
        _ => bail!("usage: build <edda-index> <out> [--within CX CY CZ R]  |  route <edda-index> <galos-dir> FROM TO RANGE [reps]  |  pins <edda-index> <galos-dir> <pins.csv> RANGE [reps]  |  theirs <edda-index> FROM TO RANGE [reps]  |  info <edda-index> [--within CX CY CZ R]"),
    }
}

// ---------------------------------------------------------------- build --

fn info(edda: &Path, rest: &[String]) -> Result<()> {
    let g = Galaxy::open(edda)?;
    let n = g.count;
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for idx in 0..n as u32 {
        let p = g.pos_of(idx);
        for a in 0..3 {
            lo[a] = lo[a].min(p[a]);
            hi[a] = hi[a].max(p[a]);
        }
    }
    println!("records: {n}");
    println!("bounds: x {:.0}..{:.0}  y {:.0}..{:.0}  z {:.0}..{:.0}", lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]);
    for k in 0..5 {
        let idx = (n as u64 * k / 5) as u32;
        let r = g.record(idx);
        println!("sample {idx}: {} at {:?}", g.name(&r), r.pos());
    }
    let extra: Vec<&str> = if rest.first().map(String::as_str) == Some("--within") { vec![] } else { rest.iter().map(String::as_str).collect() };
    for name in ["Sol", "Colonia", "Beagle Point", "Wongi", "Sagittarius A*"].into_iter().chain(extra) {
        println!("{name}: {}", g.find(name).map(|i| format!("idx {i} at {:?}", g.pos_of(i))).unwrap_or_else(|| "absent".into()));
    }
    // info <edda> --within CX CY CZ R: how many records a subset build would take.
    if rest.len() == 5 && rest[0] == "--within" {
        let c: Vec<f32> = rest[1..4].iter().map(|s| s.parse()).collect::<std::result::Result<_, _>>()?;
        let r: f32 = rest[4].parse()?;
        let r2 = r * r;
        let mut count = 0u64;
        for idx in 0..n as u32 {
            let p = g.pos_of(idx);
            let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
            if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= r2 {
                count += 1;
            }
        }
        println!("within {r} ly of {:?}: {count} records ({:.1}%)", c, count as f64 * 100.0 / n as f64);
    }
    Ok(())
}

/// One bit per record: which of our records a subset build took. Written by
/// `build --within`, read by the grid side of a comparison so both
/// structures route over the same systems.
struct Subset {
    bits: Vec<u64>,
}

impl Subset {
    const FILE: &'static str = "subset.bits";
    fn all(n: usize) -> Subset {
        Subset { bits: vec![u64::MAX; n.div_ceil(64)] }
    }
    fn has(&self, idx: u32) -> bool {
        self.bits.get(idx as usize / 64).map(|w| w >> (idx % 64) & 1 == 1).unwrap_or(false)
    }
    fn set(&mut self, idx: u32, on: bool) {
        let w = &mut self.bits[idx as usize / 64];
        if on { *w |= 1 << (idx % 64) } else { *w &= !(1 << (idx % 64)) }
    }
    fn write(&self, dir: &Path) -> Result<()> {
        let bytes: Vec<u8> = self.bits.iter().flat_map(|w| w.to_le_bytes()).collect();
        Ok(std::fs::write(dir.join(Self::FILE), bytes)?)
    }
    fn read(dir: &Path) -> Result<Option<Subset>> {
        let path = dir.join(Self::FILE);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(path)?;
        Ok(Some(Subset { bits: bytes.chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect() }))
    }
}

/// Our grid, held to the subset: the same records the octree was built
/// over, one bit test per neighbour.
struct GridSubset<'a> {
    g: &'a Galaxy,
    subset: &'a Subset,
}

impl Near for GridSubset<'_> {
    fn for_each_within_toward(&self, pos: [f32; 3], radius: f32, goal: Option<([f32; 3], f32)>, f: &mut dyn FnMut(u32, f32)) {
        let subset = self.subset;
        Galaxy::for_each_within_toward(self.g, pos, radius, goal, |idx, d| {
            if subset.has(idx) {
                f(idx, d)
            }
        })
    }
}

fn build(edda: &Path, out: &Path, within: Option<([f32; 3], f32)>, for_map: bool) -> Result<()> {
    use galos_index::meta::{NameEntry, SystemBoost};
    let t0 = Instant::now();
    let g = Galaxy::open(edda).with_context(|| format!("opening {}", edda.display()))?;
    let n = g.count;
    eprintln!("records: {n} ({:.1} s to map)", t0.elapsed().as_secs_f64());

    // One System per record. Position widened to f64; brightness and heat
    // from their class-letter table; nothing else is known to our index.
    let t1 = Instant::now();
    let mut systems: Vec<System> = Vec::with_capacity(n);
    let mut subset = within.map(|_| Subset { bits: vec![0; n.div_ceil(64)] });
    let mut names: Vec<NameEntry> = Vec::new();
    let mut boosts: Vec<SystemBoost> = Vec::new();
    for idx in 0..n as u32 {
        let p = g.pos_of(idx);
        if let Some((c, r)) = within {
            let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
            if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > r * r {
                continue;
            }
            subset.as_mut().unwrap().set(idx, true);
        }
        let class = ed_galaxy::StarClass::from_code(g.class_code(idx));
        let light = ClassLight::of(class.letter());
        let rec = g.record(idx);
        if for_map {
            names.push(NameEntry { address: rec.id64 as i64, name: g.name(&rec).to_string(), position: p });
            let boost = match class {
                ed_galaxy::StarClass::Neutron => Some(galos_index::meta::Boost::Neutron),
                ed_galaxy::StarClass::WhiteDwarf => Some(galos_index::meta::Boost::WhiteDwarf),
                _ => None,
            };
            if let Some(boost) = boost {
                boosts.push(SystemBoost { address: rec.id64 as i64, boost });
            }
        }
        systems.push(System {
            id64: if for_map { rec.id64 } else { idx as u64 },
            position: [p[0] as f64, p[1] as f64, p[2] as f64],
            absolute_magnitude: light.absolute_magnitude.0,
            temperature: light.temperature.0,
            age_bucket: 0,
            updated_at: 0,
        });
    }
    eprintln!("converted: {:.1} s, {} B/system in the input slice", t1.elapsed().as_secs_f64(), std::mem::size_of::<System>());
    report_rss("after convert");

    let t2 = Instant::now();
    let snap = Snapshot::build(&systems, &BuildParams::default());
    let cells = snap.index.len();
    let leaves = snap.index.cells().filter(|c| c.is_leaf()).count();
    let points = snap.point_count();
    eprintln!("built: {:.1} s, {cells} cells ({leaves} leaves), {points} points", t2.elapsed().as_secs_f64());
    report_rss("after build");
    drop(systems);

    let t3 = Instant::now();
    std::fs::create_dir_all(out)?;
    snap.write(out).with_context(|| format!("writing {}", out.display()))?;
    if let Some(s) = &subset {
        s.write(out)?;
    }
    if for_map {
        // Their sidecars: the names table (address-ordered chunks, the
        // search index and their router's graph) and the supercharge
        // table. Populated, reaches, factions and bodies are not written;
        // the map warns and draws uncoloured, which is the honest state of
        // what our star index knows.
        let t = Instant::now();
        boosts.sort_by_key(|b| b.address);
        galos_index::source::write_meta(&galos_index::source::boosts_path(out), &boosts)?;
        let count = names.len();
        let mut table = galos_index::names::NameTable::from_entries(std::mem::take(&mut names));
        let chunks = table.publish(out)?;
        eprintln!("sidecars: {count} names in {chunks} chunks, {} jet cones, {:.1} s", boosts.len(), t.elapsed().as_secs_f64());
    }
    let (files, bytes) = dir_size(out)?;
    eprintln!("written: {:.1} s, {files} files, {bytes} B ({:.2} GB), {:.1} B/system", t3.elapsed().as_secs_f64(), bytes as f64 / 1e9, bytes as f64 / points as f64);
    let (efiles, ebytes) = dir_size(edda)?;
    eprintln!("ours: {efiles} files, {ebytes} B ({:.2} GB), {:.1} B/system over all {n}", ebytes as f64 / 1e9, ebytes as f64 / n as f64);
    println!("systems,cells,leaves,build_s,write_s,galos_bytes,edda_bytes");
    println!("{points},{cells},{leaves},{:.1},{:.1},{bytes},{ebytes}", t2.elapsed().as_secs_f64(), t3.elapsed().as_secs_f64());
    Ok(())
}

fn dir_size(dir: &Path) -> Result<(u64, u64)> {
    let mut files = 0;
    let mut bytes = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            let (f, b) = dir_size(&entry.path())?;
            files += f;
            bytes += b;
        } else {
            files += 1;
            bytes += meta.len();
        }
    }
    Ok((files, bytes))
}

/// Peak resident set, where the OS says it (Linux). On macOS run under
/// `/usr/bin/time -l` and read "maximum resident set size".
fn report_rss(when: &str) {
    if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
        for line in s.lines() {
            if line.starts_with("VmHWM") || line.starts_with("VmRSS") {
                eprintln!("{when}: {}", line.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
    }
}

// ---------------------------------------------------------------- route --

/// Their index, resident: the cell tree and every cell's payload, the way
/// their map holds the cells it draws. Reading a payload file per
/// expansion would measure the disk, not the structure.
struct Octree {
    index: Index,
    payloads: HashMap<CellId, Vec<Point>>,
    /// Which of our records the tree was built over, when it was a subset.
    subset: Option<Subset>,
}

impl Octree {
    fn load(dir: &Path) -> Result<Octree> {
        let t0 = Instant::now();
        let src = FsSource::new(dir);
        let index = pollster::block_on(src.index()).with_context(|| format!("reading {}", dir.display()))?;
        let mut payloads = HashMap::with_capacity(index.len());
        let mut points = 0usize;
        for cell in index.cells() {
            let p = pollster::block_on(src.payload(cell.id))?;
            points += p.len();
            payloads.insert(cell.id, p);
        }
        let subset = Subset::read(dir)?;
        eprintln!("galos index: {} cells, {points} points resident, {:.1} s to load{}", index.len(), t0.elapsed().as_secs_f64(), if subset.is_some() { ", a subset" } else { "" });
        Ok(Octree { index, payloads, subset })
    }
}

impl Near for Octree {
    fn for_each_within_toward(&self, pos: [f32; 3], radius: f32, goal: Option<([f32; 3], f32)>, f: &mut dyn FnMut(u32, f32)) {
        let center = [pos[0] as f64, pos[1] as f64, pos[2] as f64];
        let r = radius as f64;
        let r2 = radius * radius;
        let goal = goal.map(|(gp, m)| ([gp[0] as f64, gp[1] as f64, gp[2] as f64], m as f64));
        // Descend from the root. Every cell owns a slice of its own (the
        // brightest few hundred of an internal cell, up to 4,096 in a
        // leaf), so every cell on the way down is scanned, not just the
        // leaves; a cell whose box the sphere never touches is dropped
        // with its whole subtree, and so is one that cannot hold a system
        // closer than `max_goal` to the goal.
        let mut stack = vec![CellId::ROOT];
        while let Some(id) = stack.pop() {
            let Some(cell) = self.index.get(id) else { continue };
            let bounds = id.bounds();
            if bounds.distance_to(center) > r {
                continue;
            }
            if let Some((gp, max_goal)) = goal {
                if bounds.distance_to(gp) > max_goal {
                    continue;
                }
            }
            if let Some(points) = self.payloads.get(&id) {
                for p in points {
                    let dx = p.pos[0] as f32 - pos[0];
                    let dy = p.pos[1] as f32 - pos[1];
                    let dz = p.pos[2] as f32 - pos[2];
                    let d2 = dx * dx + dy * dy + dz * dz;
                    if d2 <= r2 {
                        f(p.id64 as u32, d2.sqrt());
                    }
                }
            }
            if !cell.is_leaf() {
                let ids = id.children();
                for octant in 0..8u8 {
                    if cell.has_child(octant) {
                        stack.push(ids[octant as usize]);
                    }
                }
            }
        }
    }
}

fn compare(g: &Galaxy, oct: &Octree, from: &str, to: &str, range: f32, reps: usize) -> Result<()> {
    let from_idx = g.find(from).ok_or_else(|| anyhow!("unknown system {from}"))?;
    let to_idx = g.find(to).ok_or_else(|| anyhow!("unknown system {to}"))?;
    let req = RouteRequest { from: from_idx, to: to_idx, range_ly: range, ..Default::default() };
    let ctl = Control::none();

    let mut ours = None;
    let mut theirs = None;
    let mut ours_ms = Vec::new();
    let mut theirs_ms = Vec::new();
    for _ in 0..reps.max(1) {
        let t = Instant::now();
        let r = match &oct.subset {
            Some(subset) => {
                if !subset.has(from_idx) || !subset.has(to_idx) {
                    bail!("{from} or {to} is outside the subset the octree was built over");
                }
                router::plan_with(g, &GridSubset { g, subset }, &req, &ctl)?
            }
            None => router::plan(g, &req, &ctl)?,
        };
        ours_ms.push(t.elapsed().as_secs_f64() * 1e3);
        ours = Some(r);
        let t = Instant::now();
        let r = router::plan_with(g, oct, &req, &ctl)?;
        theirs_ms.push(t.elapsed().as_secs_f64() * 1e3);
        theirs = Some(r);
    }
    let (a, b) = (ours.unwrap(), theirs.unwrap());
    let same = a.jumps == b.jumps && a.hops.iter().zip(&b.hops).all(|(x, y)| x.idx == y.idx);
    for (label, r, ms) in [("edda_grid", &a, &mut ours_ms), ("galos_octree", &b, &mut theirs_ms)] {
        ms.sort_by(|x, y| x.total_cmp(y));
        println!("{from},{to},{range},{label},{},{},{},{:.1},{:.1}", r.jumps, r.boosted_jumps, r.expansions, ms[0], ms[ms.len() / 2]);
    }
    if !same {
        eprintln!("DIFFER: {from} -> {to}: grid {} jumps, octree {} jumps", a.jumps, b.jumps);
    }
    Ok(())
}

// --------------------------------------------------------------- theirs --

/// Their router (theirs.rs, the map's graph.rs unchanged) over every
/// position we hold, in Quick, Direct and Shortest, with their Standard
/// drive; then our planner on the same pair at weight 1.3 (our default)
/// and 1.0 (exact), supercharge on, no fuel. Same records, same range.
fn theirs_vs_ours(edda: &Path, from: &str, to: &str, range: f32, reps: usize) -> Result<()> {
    use theirs::{Boosts, Drive, JumpGraph, Places, Routing};
    let g = Galaxy::open(edda)?;
    let n = g.count;
    let from_idx = g.find(from).ok_or_else(|| anyhow!("unknown system {from}"))?;
    let to_idx = g.find(to).ok_or_else(|| anyhow!("unknown system {to}"))?;
    let (from_addr, to_addr) = (g.record(from_idx).id64 as i64, g.record(to_idx).id64 as i64);

    // Their graph, straight from our records: address and position for
    // the places, the jet-cone table from our star class.
    let t0 = Instant::now();
    let places = Places::build((0..n as u32).map(|idx| (g.record(idx).id64 as i64, g.pos_of(idx))));
    let mut boosts = HashMap::new();
    for idx in 0..n as u32 {
        let boost = match ed_galaxy::StarClass::from_code(g.class_code(idx)) {
            ed_galaxy::StarClass::Neutron => Some(galos_index::meta::Boost::Neutron),
            ed_galaxy::StarClass::WhiteDwarf => Some(galos_index::meta::Boost::WhiteDwarf),
            _ => None,
        };
        if let Some(b) = boost {
            boosts.insert(g.record(idx).id64 as i64, b);
        }
    }
    let graph = JumpGraph::new(places, Boosts::from_map(boosts));
    eprintln!("their graph: {n} places, built in {:.1} s", t0.elapsed().as_secs_f64());
    report_rss("after their graph");

    println!("route_from,route_to,range_ly,router,mode,jumps,expansions,best_ms,median_ms");
    for (label, how) in [("quick", Routing::Quick), ("direct", Routing::Direct), ("shortest", Routing::Shortest)] {
        let mut ms = Vec::new();
        let mut found = None;
        for _ in 0..reps.max(1) {
            let t = Instant::now();
            found = theirs::route(&graph, from_addr, to_addr, range as f64, how, Drive::Standard);
            ms.push(t.elapsed().as_secs_f64() * 1e3);
        }
        ms.sort_by(|x, y| x.total_cmp(y));
        match found {
            Some(f) => println!("{from},{to},{range},galos,{label},{},{},{:.1},{:.1}", f.jumps, f.expansions, ms[0], ms[ms.len() / 2]),
            None => println!("{from},{to},{range},galos,{label},none,,{:.1},{:.1}", ms[0], ms[ms.len() / 2]),
        }
    }
    drop(graph);
    ours_only(&g, from, to, range, reps)
}

/// Their router over stars.bin alone. Records are 29 bytes from a 32-byte
/// header: x y z as f32, class, flags, companion, name_len u16, id64 u64,
/// name_off u64 (ed_galaxy::format, version 2+). The name files are not
/// needed: the caller passes record indices.
fn theirs_raw(stars: &Path, from_idx: u32, to_idx: u32, range: f32, reps: usize, mode: &str) -> Result<()> {
    use theirs::{Boosts, Drive, JumpGraph, Places, Routing};
    let f = std::fs::File::open(stars).with_context(|| format!("opening {}", stars.display()))?;
    // SAFETY: written once, never modified in place (ed_galaxy::format).
    let map = unsafe { memmap2::Mmap::map(&f)? };
    anyhow::ensure!(&map[0..4] == b"EDGX", "not a galaxy index");
    let version = u32::from_le_bytes(map[4..8].try_into().unwrap());
    let n = u64::from_le_bytes(map[8..16].try_into().unwrap()) as usize;
    const HEADER: usize = 32;
    // v1 records are 32 bytes with the class in its own byte; v2+ are 29
    // with the class in the low nibble of byte 12 (StarRecord::read_v1 /
    // read_from). id64 sits at 16..24 in both.
    let rec_len = if version == 1 { 32 } else { 29 };
    anyhow::ensure!(map.len() >= HEADER + n * rec_len, "truncated");
    let rec = |idx: u32| -> ([f32; 3], u8, i64) {
        let o = HEADER + idx as usize * rec_len;
        let b = &map[o..o + rec_len];
        let f = |i: usize| f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let class = if version == 1 { b[12] } else { b[12] & 0x0f };
        ([f(0), f(4), f(8)], class, u64::from_le_bytes(b[16..24].try_into().unwrap()) as i64)
    };
    eprintln!("records: {n}");
    let (_, _, from_addr) = rec(from_idx);
    let (_, _, to_addr) = rec(to_idx);

    let t0 = Instant::now();
    let places = Places::build((0..n as u32).map(|idx| {
        let (p, _, addr) = rec(idx);
        (addr, p)
    }));
    let mut boosts = HashMap::new();
    for idx in 0..n as u32 {
        let (_, class, addr) = rec(idx);
        let boost = match ed_galaxy::StarClass::from_code(class) {
            ed_galaxy::StarClass::Neutron => Some(galos_index::meta::Boost::Neutron),
            ed_galaxy::StarClass::WhiteDwarf => Some(galos_index::meta::Boost::WhiteDwarf),
            _ => None,
        };
        if let Some(b) = boost {
            boosts.insert(addr, b);
        }
    }
    eprintln!("jet cones: {}", boosts.len());
    let graph = JumpGraph::new(places, Boosts::from_map(boosts));
    eprintln!("their graph: {n} places, built in {:.1} s", t0.elapsed().as_secs_f64());
    report_rss("after their graph");

    println!("route_from,route_to,range_ly,router,mode,jumps,expansions,best_ms,median_ms");
    for (label, how) in [("quick", Routing::Quick), ("direct", Routing::Direct), ("shortest", Routing::Shortest)] {
        if mode != "all" && mode != label {
            continue;
        }
        let mut ms = Vec::new();
        let mut found = None;
        for _ in 0..reps.max(1) {
            let t = Instant::now();
            found = theirs::route(&graph, from_addr, to_addr, range as f64, how, Drive::Standard);
            ms.push(t.elapsed().as_secs_f64() * 1e3);
        }
        ms.sort_by(|x, y| x.total_cmp(y));
        match found {
            Some(f) => println!("idx{from_idx},idx{to_idx},{range},galos,{label},{},{},{:.1},{:.1}", f.jumps, f.expansions, ms[0], ms[ms.len() / 2]),
            None => println!("idx{from_idx},idx{to_idx},{range},galos,{label},none,,{:.1},{:.1}", ms[0], ms[ms.len() / 2]),
        }
    }
    Ok(())
}

/// Our planner on the same pair, at our default weight and exact.
fn ours_only(g: &Galaxy, from: &str, to: &str, range: f32, reps: usize) -> Result<()> {
    let from_idx = g.find(from).ok_or_else(|| anyhow!("unknown system {from}"))?;
    let to_idx = g.find(to).ok_or_else(|| anyhow!("unknown system {to}"))?;
    let ctl = Control::none();
    // OURS_MODES=w1.3,w1.0,thorough selects which to run (default all
    // three). `thorough` is weight 1.0 with the admissible boost
    // heuristic: the mode that can find a neutron-highway route, and the
    // one comparable to their Direct.
    let wanted = std::env::var("OURS_MODES").unwrap_or_else(|_| "w1.3,w1.0,thorough".into());
    for (label, weight, thorough) in [("w1.3", 1.3f32, false), ("w1.0", 1.0, false), ("thorough", 1.0, true)] {
        if !wanted.split(',').any(|m| m == label) {
            continue;
        }
        let req = RouteRequest { from: from_idx, to: to_idx, range_ly: range, weight, thorough, ..Default::default() };
        let mut ms = Vec::new();
        let mut r = None;
        for _ in 0..reps.max(1) {
            let t = Instant::now();
            r = Some(router::plan(g, &req, &ctl));
            ms.push(t.elapsed().as_secs_f64() * 1e3);
        }
        ms.sort_by(|x, y| x.total_cmp(y));
        match r.unwrap() {
            Ok(r) => println!("{from},{to},{range},edda,{label},{},{},{:.1},{:.1}", r.jumps, r.expansions, ms[0], ms[ms.len() / 2]),
            Err(e) => println!("{from},{to},{range},edda,{label},none ({e}),,{:.1},{:.1}", ms[0], ms[ms.len() / 2]),
        }
    }
    Ok(())
}
