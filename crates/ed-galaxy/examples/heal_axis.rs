//! Rebuild a highway sub-index with lost axis-slab classes healed from
//! a Spansh-live backfill (ROUTING-NEXT item 11): the dump left the
//! records class-Unknown, so `learn_class` fills exactly those gaps and
//! the subset keep-filter reads the override-aware class.
//!
//!     heal_axis <index_dir> <axis-heal.json> <out_dir>
//!
//! axis-heal.json: [[id64, "N"|"D"], ...]. Run sidecars on <out_dir>
//! afterwards for agg/alt/graph.

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(
        args.next()
            .expect("usage: heal_axis <index> <heal.json> <out_dir>"),
    );
    let heal = std::path::PathBuf::from(args.next().expect("heal.json"));
    let out = std::path::PathBuf::from(args.next().expect("out_dir"));
    let g = ed_galaxy::Galaxy::open(&dir)?;
    let pairs: Vec<(u64, String)> = serde_json::from_str(&std::fs::read_to_string(&heal)?)?;
    let mut learned = 0usize;
    for (id64, letter) in &pairs {
        let class = match letter.as_str() {
            "N" => ed_galaxy::StarClass::Neutron,
            "D" => ed_galaxy::StarClass::WhiteDwarf,
            other => anyhow::bail!("unknown class letter {other:?}"),
        };
        g.learn_class(*id64, class);
        learned += 1;
    }
    eprintln!(
        "{learned} classes learned; building healed sub-index at {}",
        out.display()
    );
    let t = std::time::Instant::now();
    let stats =
        ed_galaxy::import::subset_cells(&g, &out, ed_galaxy::long_range::NEUTRON_CELL_LY, |r| {
            ed_galaxy::long_range::highway_star(g.class(r))
        })?;
    eprintln!(
        "{} highway stars in {:.1}s (was 3,853,782 with the holes)",
        stats.systems,
        t.elapsed().as_secs_f64()
    );
    Ok(())
}
