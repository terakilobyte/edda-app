//! What an EDSM `bodies7days.json.gz` dump could teach the star index:
//! every main star in it is looked up by system name and tallied as
//! unknown-in-index (learnable), known-and-agreeing, or known-and-differing.
//!
//!     cargo run -p ed-galaxy --example edsm_bodies --release -- <index_dir> <bodies.json.gz>

use anyhow::Result;
use ed_galaxy::star::StarClassCode as _;
use ed_galaxy::{Galaxy, StarClass};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: edsm_bodies <index_dir> <bodies.json.gz>");
        std::process::exit(2);
    }
    let g = Galaxy::open(Path::new(&args[0]))?;
    let file = std::fs::File::open(&args[1])?;
    let reader: Box<dyn std::io::Read> = if args[1].ends_with(".gz") { Box::new(flate2::read::GzDecoder::new(file)) } else { Box::new(file) };
    let reader = BufReader::with_capacity(1 << 20, reader);

    let (mut bodies, mut stars, mut main_stars, mut parse_errors) = (0u64, 0u64, 0u64, 0u64);
    let (mut not_in_index, mut learnable, mut agree, mut differ, mut edsm_unknown) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut learnable_by_class: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut differ_pairs: BTreeMap<(&'static str, &'static str), u64> = BTreeMap::new();
    let mut updates: BTreeMap<String, u64> = BTreeMap::new();
    let started = std::time::Instant::now();

    for line in reader.lines() {
        let line = line?;
        let t = line.trim().trim_end_matches(',');
        if !t.starts_with('{') {
            continue;
        }
        bodies += 1;
        let v: serde_json::Value = match serde_json::from_str(t) {
            Ok(v) => v,
            Err(_) => {
                parse_errors += 1;
                continue;
            }
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("Star") {
            continue;
        }
        stars += 1;
        if v.get("isMainStar").and_then(|b| b.as_bool()) != Some(true) {
            continue;
        }
        main_stars += 1;
        if let Some(d) = v.get("updateTime").and_then(|s| s.as_str()) {
            *updates.entry(d[..10.min(d.len())].to_string()).or_default() += 1;
        }
        let Some(name) = v.get("systemName").and_then(|s| s.as_str()) else { continue };
        let class = v.get("subType").and_then(|s| s.as_str()).map(StarClass::from_subtype).unwrap_or(StarClass::Unknown);
        if class == StarClass::Unknown {
            edsm_unknown += 1;
            continue;
        }
        let Some(idx) = g.find(name) else {
            not_in_index += 1;
            continue;
        };
        let have = StarClass::from_code(g.class_code(idx));
        if have == StarClass::Unknown {
            learnable += 1;
            *learnable_by_class.entry(class.name()).or_default() += 1;
        } else if have == class {
            agree += 1;
        } else {
            differ += 1;
            *differ_pairs.entry((have.name(), class.name())).or_default() += 1;
        }
        if main_stars % 100_000 == 0 {
            eprintln!("  {main_stars} main stars in {} s", started.elapsed().as_secs());
        }
    }

    println!("bodies {bodies}, stars {stars}, main stars {main_stars}, parse errors {parse_errors}, {} s", started.elapsed().as_secs());
    println!("main stars: not in index {not_in_index}, EDSM class unknown {edsm_unknown}, index unknown -> learnable {learnable}, agree {agree}, differ {differ}");
    println!("learnable by class:");
    for (c, n) in &learnable_by_class {
        println!("  {c:<14} {n}");
    }
    let mut pairs: Vec<_> = differ_pairs.into_iter().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    println!("differences (index -> EDSM), top 12:");
    for ((a, b), n) in pairs.iter().take(12) {
        println!("  {a:<14} -> {b:<14} {n}");
    }
    println!("main-star updates by day:");
    for (d, n) in &updates {
        println!("  {d} {n}");
    }
    Ok(())
}
