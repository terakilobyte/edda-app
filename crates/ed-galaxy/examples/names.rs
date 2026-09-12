//! Prefix search over the index's byname table (the app's autocomplete).
//!     names <index_dir> <prefix> [limit]

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(
        args.next()
            .expect("usage: names <index_dir> <prefix> [limit]"),
    );
    let prefix = args.next().expect("prefix");
    let limit: usize = args.next().and_then(|v| v.parse().ok()).unwrap_or(20);
    let g = ed_galaxy::Galaxy::open(&dir)?;
    for idx in g.complete(&prefix, limit) {
        let r = g.record(idx);
        let p = r.pos();
        let from_sol = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
        use ed_galaxy::StarClassCode as _;
        let class = ed_galaxy::StarClass::from_code(r.class);
        println!(
            "{:<40} [{:>9.1} {:>9.1} {:>9.1}]  {:>8.0} ly from Sol  {class:?}",
            g.name(&r),
            p[0],
            p[1],
            p[2],
            from_sol
        );
    }
    Ok(())
}
