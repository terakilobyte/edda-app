//! Count the main-star classes across an index: how much of the galaxy we
//! route through is class Unknown (no star body with a known type in the
//! dump), and how the known classes split. The instrument behind the data
//! gap of 2026-10-18: 11 of 91, 2 of 60 and 16 of 72 of the external
//! reference's neutron waypoints on three routes were Unknown to us.
//!
//!     class_census <index_dir>
use ed_galaxy::Galaxy;
use std::collections::BTreeMap;
use std::path::Path;
fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().map(String::as_str).unwrap_or(".data/galaxy");
    let g = Galaxy::open(Path::new(dir))?;
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for i in 0..g.count as u32 {
        let r = g.record(i);
        *counts.entry(format!("{:?}", g.class(&r))).or_default() += 1;
    }
    let total = g.count as f64;
    println!("{} systems in {dir}", g.count);
    let mut rows: Vec<(&String, &u64)> = counts.iter().collect();
    rows.sort_by(|a, b| b.1.cmp(a.1));
    for (class, n) in rows {
        println!("{class:<16} {n:>12}  {:6.2} %", *n as f64 / total * 100.0);
    }
    Ok(())
}
