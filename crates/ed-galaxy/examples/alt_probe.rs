//! Where does the ALT bound actually exceed the straight line?
//!     alt_probe <index_dir> <from> <to>
//! Samples points along the from->to line and prints euclid vs bound to
//! the destination's cell.

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(args.next().expect("usage: alt_probe <index> <from> <to>"));
    let from = args.next().expect("from");
    let to = args.next().expect("to");
    let g = ed_galaxy::Galaxy::open(&dir)?;
    let sub = ed_galaxy::Galaxy::open(&ed_galaxy::long_range::neutron_dir(&dir))?;
    let alt = sub
        .alt()
        .ok_or_else(|| anyhow::anyhow!("no alt250.bin beside the sub-index"))?;
    let a = g.pos_of(
        g.find(&from)
            .ok_or_else(|| anyhow::anyhow!("unknown {from}"))?,
    );
    let b = g.pos_of(g.find(&to).ok_or_else(|| anyhow::anyhow!("unknown {to}"))?);
    let goal_cell = sub.cell_index_of_pos(b).ok_or_else(|| {
        anyhow::anyhow!("{to} is not in an occupied highway cell; probing the nearest instead")
    })?;
    println!(
        "{:>6} {:>12} {:>12} {:>8}",
        "t%", "euclid_ly", "alt_ly", "binds"
    );
    let mut binding = 0u32;
    for i in 0..=20 {
        let t = i as f32 / 20.0;
        let p = [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ];
        let euclid = ed_galaxy::format::dist(p, b);
        let (bound, note) = match sub.cell_index_of_pos(p) {
            Some(cell) => (alt.lower_bound_ly(cell, goal_cell), ""),
            None => (0.0, " (off-highway cell)"),
        };
        let binds = bound > euclid;
        binding += binds as u32;
        println!(
            "{:>5}% {:>12.0} {:>12.0} {:>8}{}",
            i * 5,
            euclid,
            bound,
            if binds { "YES" } else { "" },
            note
        );
    }
    println!("bound exceeds euclid at {binding}/21 sample points");
    Ok(())
}
