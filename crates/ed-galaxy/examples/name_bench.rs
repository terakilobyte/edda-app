//! Name-lookup triage bench: what does `Galaxy::find` actually cost,
//! cold and warm, on a real index?  `name_bench <index_dir> [count]`
//!
//! Cold here means "first lookups this process, mmap pages faulted in on
//! demand" -- run right after a reboot (or a cache purge) for the true
//! worst case; on a warm OS cache the cold column understates faults.

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("usage: name_bench <index_dir> [count]"));
    let count: usize = args.next().and_then(|c| c.parse().ok()).unwrap_or(200);
    let g = ed_galaxy::Galaxy::open(&dir)?;
    eprintln!("index: {} systems", g.count);

    // Harvest names by striding the record space -- this touches records
    // and names pages we will look up again, so lookups after a harvest
    // are NOT fully cold; the first `find` of each name still walks a
    // byname path of ~27 untouched pages.
    let mut names = Vec::with_capacity(count);
    let stride = (g.count / count).max(1);
    for i in 0..count {
        let idx = (i * stride) as u32;
        names.push(g.name(&g.record(idx)).to_string());
    }
    // Deterministic shuffle so lookups do not follow harvest order.
    let mut s = 0x2545f4914f6cdd1du64;
    for i in (1..names.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        names.swap(i, (s as usize) % (i + 1));
    }

    for pass in ["cold", "warm"] {
        let started = Instant::now();
        let mut worst = std::time::Duration::ZERO;
        let mut found = 0usize;
        for name in &names {
            let one = Instant::now();
            found += g.find(name).is_some() as usize;
            worst = worst.max(one.elapsed());
        }
        let total = started.elapsed();
        println!(
            "{pass}: {found}/{} found, avg {:.1} us, worst {:.1} us, total {:.1} ms",
            names.len(),
            total.as_secs_f64() * 1e6 / names.len() as f64,
            worst.as_secs_f64() * 1e6,
            total.as_secs_f64() * 1e3
        );
    }
    Ok(())
}
