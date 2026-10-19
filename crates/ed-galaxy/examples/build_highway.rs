//! Build the highway (neutron) sub-index for an installed routing index,
//! exactly as the server and the client do after an install or an overlay
//! apply: `<index_dir>/boost250` from the main-star classes.
//!
//!     build_highway <index_dir>
//!
//! The bench's instrument for measuring on the production index (the
//! 2026-10-19 data-gap work: the bench index carried none of the NavRoute-
//! taught classes the box knows).
use ed_galaxy::Galaxy;
use std::path::Path;
use std::time::Instant;
fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("usage: build_highway <index_dir>");
    let dir = Path::new(&dir);
    let g = Galaxy::open(dir)?;
    let out = ed_galaxy::long_range::neutron_dir(dir);
    if Galaxy::exists(&out) {
        eprintln!("{} already exists", out.display());
        return Ok(());
    }
    let staging = dir.join(".boost250-staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    let t = Instant::now();
    ed_galaxy::import::subset_cells_cancellable(&g, &staging, ed_galaxy::long_range::NEUTRON_CELL_LY, |r| ed_galaxy::long_range::highway_star(g.class(r)), &|| false)?;
    std::fs::rename(&staging, &out)?;
    let sub = Galaxy::open(&out)?;
    eprintln!("{} of {} stars into {} in {:.0} s", sub.count, g.count, out.display(), t.elapsed().as_secs_f64());
    Ok(())
}
