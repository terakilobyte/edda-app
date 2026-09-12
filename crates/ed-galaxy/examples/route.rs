//! Plot a route across the star index.
//!
//!     cargo run -p ed-galaxy --example route --release -- .data/galaxy "Wongi" "Colonia" 37.6 [--no-boost] [--dry 3]

use ed_galaxy::router::{plan, Control, RouteRequest};
use ed_galaxy::Galaxy;
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!(
            "usage: route <index_dir> <from> <to> <range_ly> [--no-boost] [--dry N] [--weight W]"
        );
        std::process::exit(2);
    }
    let g = Galaxy::open(Path::new(&args[0]))?;
    let from = g
        .find(&args[1])
        .ok_or_else(|| anyhow::anyhow!("unknown system {}", args[1]))?;
    let to = g
        .find(&args[2])
        .ok_or_else(|| anyhow::anyhow!("unknown system {}", args[2]))?;
    let mut req = RouteRequest {
        from,
        to,
        range_ly: args[3].parse()?,
        ..Default::default()
    };
    let mut i = 4;
    while i < args.len() {
        match args[i].as_str() {
            "--no-boost" => req.supercharge = false,
            "--dry" => {
                req.max_dry_jumps = args[i + 1].parse()?;
                i += 1;
            }
            "--weight" => {
                req.weight = args[i + 1].parse()?;
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    eprintln!(
        "{} systems in index; planning {} -> {} at {} ly",
        g.count, args[1], args[2], req.range_ly
    );
    let progress = |n: u64, rem: f32| eprintln!("  {n} expansions, best remaining {rem:.0} ly");
    let ctl = Control {
        cancelled: &|| false,
        progress: &progress,
        stage: &|_, _, _| {},
        found: &|_| {},
        trace: &|_, _, _| {},
    };
    let r = plan(&g, &req, &ctl)?;
    println!(
        "{} jumps, {:.1} ly flown ({:.1} ly straight), {} boosted, {} expansions, {} ms",
        r.jumps, r.total_ly, r.straight_ly, r.boosted_jumps, r.expansions, r.elapsed_ms
    );
    for h in &r.hops {
        println!(
            "{:>4}  {:<32} {:>3} {:>6.1} ly {}{}",
            h.idx,
            h.name,
            h.class.letter(),
            h.distance_ly,
            if h.boosted { "BOOST " } else { "" },
            if h.scoopable { "" } else { "dry" }
        );
    }
    Ok(())
}
