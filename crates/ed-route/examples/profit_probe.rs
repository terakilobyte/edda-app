//! Run the real profit finder against a galaxy database copy and show
//! what it returns — the diagnostic for "the finder skips routes I can
//! see on Inara" (2026-09-06, palladium). Usage:
//!
//!   cargo run -p ed-route --example profit_probe -- <galaxy.sqlite3> \
//!       "<origin system>" <x> <y> <z> [radius_ly]
//!
//! Prints the top legs and the exclusion counters for: defaults,
//! defaults+carriers, and a wide-radius pass — the variants that bound
//! where a missing route went.

use ed_route::profit::{find, Constraints, SearchControl};
use ed_route::request::{plan, LoadoutShip, ProfitRequest};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let db = args.next().expect("galaxy db path");
    let system = args.next().expect("origin system name");
    let x: f64 = args.next().expect("x").parse()?;
    let y: f64 = args.next().expect("y").parse()?;
    let z: f64 = args.next().expect("z").parse()?;
    let radius: f64 = args.next().map(|r| r.parse()).transpose()?.unwrap_or(40.0);

    let conn = rusqlite::Connection::open_in_memory()?;
    ed_store::schema::attach_galaxy(&conn, Some(std::path::Path::new(&db)))?;

    // The maintainer's Panther Clipper Mk II as the journal reports it.
    let live = LoadoutShip {
        hull: Some("panthermkii".into()),
        cargo_capacity: Some(1008),
        max_jump_range: Some(37.568104),
        unladen_mass: Some(1790.899902),
        fuel_main: None,
    };

    for (label, tweak) in [
        // The Trade page's exact defaults: radius 100, max age 48 h,
        // carriers and prohibited off (trade.svelte.js:15-18).
        (
            "UI defaults (rings 5)",
            Box::new(|_: &mut Constraints| {}) as Box<dyn Fn(&mut Constraints)>,
        ),
        ("rings OFF", Box::new(|c: &mut Constraints| c.max_stops = 0)),
        ("rings 3", Box::new(|c: &mut Constraints| c.max_stops = 3)),
        (
            "carriers ON",
            Box::new(|c: &mut Constraints| c.include_carriers = true),
        ),
    ] {
        let req = ProfitRequest {
            system: Some(system.clone()),
            radius_ly: Some(radius),
            max_age_hours: Some(48.0),
            ..Default::default()
        };
        let planned = plan(&req, &live, None).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let mut constraints = planned.constraints;
        tweak(&mut constraints);
        let started = std::time::Instant::now();
        let report = find(
            &conn,
            &system,
            (x, y, z),
            None,
            &planned.ship,
            &constraints,
            10,
            &SearchControl::none(),
        )?;
        println!(
            "== {label} (radius {:.0}, pad {:?}, carriers {}) — {} legs in {:.1}s ==",
            constraints.radius_ly,
            constraints.min_pad,
            constraints.include_carriers,
            report.legs.len(),
            started.elapsed().as_secs_f64(),
        );
        for leg in report.legs.iter().take(10) {
            println!(
                "  {:>12} {:>7}/t  {:<28} arr {:>6.0} ls -> {:<18} arr {:>6.0} ls  {} t | out {:>5.1} min, loop {:>5.1} min | {:>6.0}M/h one-way {:>6.0}M/h repeat",
                leg.commodity,
                leg.profit_per_ton,
                leg.from.station,
                leg.from.arrival_ls.unwrap_or(f64::NAN),
                leg.to.station,
                leg.to.arrival_ls.unwrap_or(f64::NAN),
                leg.tons,
                leg.duration.seconds / 60.0,
                leg.return_duration.seconds / 60.0,
                leg.profit_per_hour / 1e6,
                leg.profit_per_hour_repeat / 1e6,
            );
        }
        println!("  excluded: {}", serde_json::to_string(&report.excluded)?);
    }
    Ok(())
}
