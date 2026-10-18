//! The feasibility search alone: a flyable witness route for one pair, or
//! "no route", with its cost (planner unification R3, 2026-10-18).
//!
//!     witness <index_dir> <from> <to> [--inject premium N] [--no-boost] [--max-ms T]
//! ED_WITNESS_TRACE=1 prints the frontier every 20,000 expansions.
//!
//! Ship: the maintainer's Caspian Explorer (128 t, 6.8 t a jump, 77.81 ly
//! max with a 10.5 ly booster, x6 neutron / x3 white dwarf SCO Mk II).
use ed_galaxy::fuel::{BoostProfile, FuelModel};
use ed_galaxy::router::RouteRequest;
use ed_galaxy::Galaxy;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: witness <index_dir> <from> <to> [--inject premium N] [--no-boost]");
        std::process::exit(2);
    }
    let g = Galaxy::open(std::path::Path::new(&args[1]))?;
    let from = g.find(&args[2]).ok_or_else(|| anyhow::anyhow!("unknown system {}", args[2]))?;
    let to = g.find(&args[3]).ok_or_else(|| anyhow::anyhow!("unknown system {}", args[3]))?;
    let mut injection = None;
    let mut supercharge = true;
    let mut max_ms: u64 = 0;
    let mut i = 4;
    while i < args.len() {
        match args[i].as_str() {
            "--inject" => {
                let n: u32 = args.get(i + 2).and_then(|v| v.parse().ok()).unwrap_or(22);
                injection = Some((2.0f32, "premium", n));
                i += 3;
            }
            "--no-boost" => {
                supercharge = false;
                i += 1;
            }
            "--max-ms" => {
                max_ms = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(60_000);
                i += 2;
            }
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    let model = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
    let req = RouteRequest {
        from,
        to,
        range_ly: model.range_at(model.capacity),
        supercharge,
        boost: BoostProfile { neutron: 6.0, white_dwarf: 3.0 },
        fuel: Some(model),
        start_fuel: 128.0,
        injection,
        ..Default::default()
    };
    let straight = {
        let (a, b) = (g.pos_of(from), g.pos_of(to));
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let t = std::time::Instant::now();
    let cancel = || max_ms > 0 && t.elapsed().as_millis() as u64 > max_ms;
    match ed_galaxy::feasible::find_witness(&g, &req, &cancel) {
        Ok(w) => {
            let boosted = w.stars.windows(2).filter(|p| matches!(g.class(&g.record(p[0])), ed_galaxy::StarClass::Neutron | ed_galaxy::StarClass::WhiteDwarf)).count();
            let total: f32 = w.stars.windows(2).map(|p| { let (a, b) = (g.pos_of(p[0]), g.pos_of(p[1])); ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() }).sum();
            println!("{{\"from\":\"{}\",\"to\":\"{}\",\"straight_ly\":{:.0},\"hops\":{},\"boosted_departures\":{},\"injections\":{},\"total_ly\":{:.0},\"expansions\":{},\"ms\":{}}}", args[2], args[3], straight, w.stars.len() - 1, boosted, w.injections.len(), total, w.expansions, t.elapsed().as_millis());
        }
        Err(e) => {
            println!("{{\"from\":\"{}\",\"to\":\"{}\",\"straight_ly\":{:.0},\"error\":\"{e:?}\",\"ms\":{}}}", args[2], args[3], straight, t.elapsed().as_millis());
        }
    }
    Ok(())
}
