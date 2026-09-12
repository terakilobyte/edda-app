//! Run the profit finder against the real galaxy database.
//!
//!     cargo run -p ed-route --example profit --release -- Wongi 200 30 [large]
//!
//! Prints the top legs and round trips with timings, so the solver is judged
//! on 99.8M real market rows rather than the five in the unit tests.

use ed_route::cost::Ship;
use ed_route::profit::{find, Constraints};
use ed_store::lookup::{self, PadSize};
use ed_store::Store;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let system = args.next().unwrap_or_else(|| "Sol".into());
    let cargo: i64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(200);
    let range: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let pad = args.next().and_then(|s| PadSize::parse(&s));
    let max_age: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(72.0);

    // Store::open attaches galaxy.sqlite3 and runs its migrations; a raw
    // Connection::open would see no galaxy tables at all.
    // EDDA_DB overrides, as in the app; release builds do not search for .data/.
    let db = std::env::var_os("EDDA_DB")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(Store::default_db_path);
    let store = Store::open(&db, std::path::Path::new("."))?;
    let conn = store.conn();
    let origin = lookup::system(conn, &system)?
        .and_then(|s| s.coords)
        .ok_or_else(|| anyhow::anyhow!("unknown system {system}"))?;

    let ship = Ship {
        cargo_capacity: cargo,
        jump_range_ly: range,
        laden_range_ly: range,
    };
    let radius: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(40.0);
    // 0 means uncapped, matching the app (request.rs maps 0 to usize::MAX).
    // The raw 0 used to reach Constraints and cap the search at ONE station
    // — the HANDOFF-documented invocation was measuring nothing.
    let cap: usize = match args.next().and_then(|s| s.parse().ok()).unwrap_or(2500) {
        0 => usize::MAX,
        cap => cap,
    };
    let c = Constraints {
        min_pad: pad,
        radius_ly: radius,
        max_age_hours: max_age,
        max_stations: cap,
        ..Default::default()
    };

    let t = std::time::Instant::now();
    let r = find(
        conn,
        &system,
        origin,
        None,
        &ship,
        &c,
        12,
        &ed_route::profit::SearchControl::none(),
    )?;
    let elapsed = t.elapsed();

    println!(
        "{} stations considered in {:.2}s; excluded: {:?}",
        r.stations_considered,
        elapsed.as_secs_f64(),
        r.excluded
    );
    println!("\nTop legs (cr/h):");
    for l in &r.legs {
        println!(
            "{:>9.0}/h  {:>10} profit  {:<28} {} ({}) -> {} ({})  {:.1} ly, {} jumps, {:.0} min, age {:.0}h/{:.0}h",
            l.profit_per_hour,
            l.profit,
            l.commodity,
            l.from.station,
            l.from.system,
            l.to.station,
            l.to.system,
            l.distance_ly,
            l.jumps,
            l.duration.seconds / 60.0,
            l.buy_age_hours,
            l.sell_age_hours
        );
    }
    println!("\nRound trips (cr/h):");
    for t in &r.round_trips {
        println!(
            "{:>9.0}/h  {:>10} per loop  {} <-> {}: {} out, {} back, {:.0} min",
            t.profit_per_hour,
            t.profit,
            t.out.from.station,
            t.out.to.station,
            t.out.commodity,
            t.back.commodity,
            t.duration.seconds / 60.0
        );
    }
    Ok(())
}
