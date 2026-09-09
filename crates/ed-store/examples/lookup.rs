//! Exercise the lookup queries against a real galaxy database.
//!
//!     cargo run -p ed-store --example lookup --release -- <db> <system>

use anyhow::Result;
use ed_store::lookup::{self, PadSize};
use rusqlite::Connection;
use std::time::Instant;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let db = args.next().unwrap_or_else(|| ".data/galaxy.sqlite3".into());
    let target = args.next().unwrap_or_else(|| "Sol".into());

    let conn = Connection::open(&db)?;

    let t = Instant::now();
    let Some(sys) = lookup::system(&conn, &target)? else {
        println!("no system named {target:?} in the database yet");
        return Ok(());
    };
    println!(
        "─ {} ─ ({:.1}ms)",
        sys.name,
        t.elapsed().as_secs_f64() * 1000.0
    );
    println!(
        "  allegiance   {}",
        sys.allegiance.as_deref().unwrap_or("-")
    );
    println!("  security     {}", sys.security.as_deref().unwrap_or("-"));
    println!("  population   {}", sys.population.unwrap_or(0));
    println!(
        "  power        {} / {} [{}]",
        sys.controlling_power.as_deref().unwrap_or("none"),
        sys.power_state.as_deref().unwrap_or("-"),
        sys.power_provenance
            .map(|p| format!("{p:?}"))
            .unwrap_or_else(|| "-".into())
    );
    println!("  stations     {}", sys.station_count);

    let t = Instant::now();
    let sts = lookup::stations_in_system(&conn, &sys.name)?;
    println!(
        "\n─ stations ─ ({:.1}ms)",
        t.elapsed().as_secs_f64() * 1000.0
    );
    for s in sts.iter().take(10) {
        println!(
            "  {:<28} {:<22} {:>8} ls  pad {:<7} {}{}{}{}",
            s.name.as_deref().unwrap_or("?"),
            s.kind.as_deref().unwrap_or("?"),
            s.distance_to_arrival
                .map(|d| format!("{d:.0}"))
                .unwrap_or("?".into()),
            s.max_pad.map(|p| format!("{p:?}")).unwrap_or("?".into()),
            if s.has_market { "market " } else { "" },
            if s.has_outfitting { "outfit " } else { "" },
            if s.has_shipyard { "shipyard " } else { "" },
            if s.is_carrier { "[carrier]" } else { "" },
        );
    }

    let Some(coords) = sys.coords else {
        println!(
            "\n(no coordinates for {}, skipping spatial queries)",
            sys.name
        );
        return Ok(());
    };

    let t = Instant::now();
    let near = lookup::systems_within(&conn, coords, 25.0, 8)?;
    println!(
        "\n─ systems within 25 ly ─ ({:.1}ms)",
        t.elapsed().as_secs_f64() * 1000.0
    );
    for n in &near {
        println!(
            "  {:>6.2} ly  {:<28} {}",
            n.distance_ly,
            n.name,
            n.controlling_power.as_deref().unwrap_or("")
        );
    }

    let t = Instant::now();
    let yards = lookup::nearest_with_service(
        &conn,
        coords,
        "shipyard",
        Some(PadSize::Large),
        60.0,
        false,
        8,
    )?;
    println!(
        "\n─ nearest large-pad shipyards, no carriers ─ ({:.1}ms)",
        t.elapsed().as_secs_f64() * 1000.0
    );
    for r in &yards {
        println!(
            "  {:>6.2} ly  {:<28} {}",
            r.distance_ly,
            r.station.name.as_deref().unwrap_or("?"),
            r.station.system_name.as_deref().unwrap_or("?")
        );
    }

    Ok(())
}
