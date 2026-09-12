//! (Re)build a sub-index's derived sidecars -- the agg250.bin prefix
//! aggregate and the alt250.bin landmark table -- for a directory that
//! already exists (a fresh subset build writes them itself).
//!
//!     sidecars <subindex_dir>

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: sidecars <subindex_dir>"),
    );
    let sub = ed_galaxy::Galaxy::open(&dir)?;
    eprintln!("{} stars, {} ly cells", sub.count, sub.cell_ly);
    let t = Instant::now();
    let agg = ed_galaxy::agg::build(&sub)?;
    agg.write(&dir.join(ed_galaxy::agg::AGG_FILE))?;
    eprintln!("agg250.bin in {:.1}s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let graph = ed_galaxy::cgraph::build(&sub)?;
    graph.write(&dir.join(ed_galaxy::cgraph::GRAPH_FILE))?;
    eprintln!(
        "graph250.bin in {:.1}s ({} cells, {} edges)",
        t.elapsed().as_secs_f64(),
        graph.leaf_count(),
        graph.edge_count()
    );
    // ALT rides the graph's metric (gap edges included), so it builds last.
    let t = Instant::now();
    let alt = ed_galaxy::alt::build(&sub, &graph)?;
    alt.write(&dir.join(ed_galaxy::alt::ALT_FILE))?;
    eprintln!(
        "alt250.bin in {:.1}s ({} landmarks)",
        t.elapsed().as_secs_f64(),
        alt.landmarks().len()
    );
    Ok(())
}
