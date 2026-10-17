//! Build `scoop250.bin` for an existing highway sub-index from its full
//! index, and print the distance histogram (planner unification R2).
//!
//!     scoop_side <full_index_dir> <subindex_dir>
use ed_galaxy::Galaxy;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (full_dir, sub_dir) = (args.first().expect("usage: scoop_side <full_index_dir> <subindex_dir>"), args.get(1).expect("usage: scoop_side <full_index_dir> <subindex_dir>"));
    let full = Galaxy::open(std::path::Path::new(full_dir))?;
    let sub = Galaxy::open(std::path::Path::new(sub_dir))?;
    let started = std::time::Instant::now();
    let bytes = ed_galaxy::import::nearest_scoop_bytes(&full, &sub);
    let secs = started.elapsed().as_secs_f64();
    let path = std::path::Path::new(sub_dir).join(ed_galaxy::scoop_side::SCOOP_SIDE_FILE);
    ed_galaxy::scoop_side::write(&path, &bytes)?;
    let n = bytes.len() as f64;
    let within = |ly: u8| bytes.iter().filter(|&&b| b <= ly).count() as f64 / n * 100.0;
    let none = bytes.iter().filter(|&&b| b == ed_galaxy::scoop_side::NONE).count() as f64 / n * 100.0;
    let mut sorted = bytes.clone();
    sorted.sort_unstable();
    println!("records,{},secs,{secs:.1},within_20_pct,{:.1},within_40_pct,{:.1},within_60_pct,{:.1},within_80_pct,{:.1},within_100_pct,{:.1},none_pct,{none:.1},p50,{},p90,{},file,{}",
        bytes.len(), within(20), within(40), within(60), within(80), within(100), sorted[sorted.len() / 2], sorted[sorted.len() * 9 / 10], path.display());
    Ok(())
}
