//! Stream the Spansh dumps into the galaxy tables.
//!
//!     cargo run -p ed-store --example bootstrap_galaxy --release -- <db> <dump.json.gz>...
//!
//! Idempotent: every write is an upsert keyed on the game's own ids, so a
//! re-run refreshes rather than duplicating. Import the stations dump first
//! and the populated dump second -- the latter carries Powerplay and
//! population, and the merge is written so its values win.

use anyhow::{Context, Result};
use ed_store::galaxy;
use rusqlite::Connection;
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let db = PathBuf::from(
        args.next()
            .context("usage: bootstrap_galaxy <db> <dump>...")?,
    );
    let dumps: Vec<PathBuf> = args.map(PathBuf::from).collect();
    anyhow::ensure!(!dumps.is_empty(), "at least one dump path is required");

    let conn = Connection::open(&db)?;
    ed_store::schema::migrate(&conn)?;
    ed_store::schema::attach_galaxy(&conn, Some(&ed_store::schema::galaxy_path(&db)))?;
    // Bulk load: the journal store's durability settings are tuned for small
    // frequent writes, which is the wrong trade for a one-shot 8.6 GB import.
    conn.execute_batch(
        "PRAGMA synchronous = OFF;
         PRAGMA cache_size = -262144;   -- 256 MB page cache
         PRAGMA temp_store = MEMORY;",
    )?;

    for dump in &dumps {
        println!("importing {}", dump.display());
        let started = Instant::now();
        let mut ticks = 0u64;

        let stats = galaxy::import_dump(&conn, dump, |s, _total| {
            ticks += 1;
            if ticks.is_multiple_of(10) {
                let secs = started.elapsed().as_secs_f64().max(0.001);
                println!(
                    "  {:>9} systems  {:>9} stations  {:>11} market rows  \
                     {:>6.0} sys/s  {:.0}s elapsed",
                    s.systems,
                    s.stations,
                    s.market_rows,
                    s.systems as f64 / secs,
                    secs
                );
            }
        })?;

        println!(
            "  done: {} systems, {} stations, {} market rows, {} outfitting, \
             {} shipyard, {} parse errors, {:.0}s",
            stats.systems,
            stats.stations,
            stats.market_rows,
            stats.outfitting_rows,
            stats.shipyard_rows,
            stats.parse_errors,
            started.elapsed().as_secs_f64()
        );
    }

    println!("\noptimising (ANALYZE)...");
    conn.execute_batch("PRAGMA optimize; ANALYZE;")?;

    for (label, sql) in [
        ("systems", "SELECT COUNT(*) FROM sys_systems"),
        (
            "  with powerplay",
            "SELECT COUNT(*) FROM sys_systems WHERE controlling_power IS NOT NULL",
        ),
        (
            "  populated",
            "SELECT COUNT(*) FROM sys_systems WHERE population > 0",
        ),
        ("stations", "SELECT COUNT(*) FROM sys_stations"),
        (
            "  with market",
            "SELECT COUNT(*) FROM sys_stations WHERE has_market = 1",
        ),
        (
            "  fleet carriers",
            "SELECT COUNT(*) FROM sys_stations WHERE is_carrier = 1",
        ),
        ("market rows", "SELECT COUNT(*) FROM sys_market"),
        ("outfitting rows", "SELECT COUNT(*) FROM sys_outfitting"),
        ("shipyard rows", "SELECT COUNT(*) FROM sys_shipyard"),
    ] {
        let n: i64 = conn.query_row(sql, [], |r| r.get(0))?;
        println!("  {label:<18} {n:>12}");
    }

    let size = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
    println!("\n  database on disk   {:.2} GB", size as f64 / 1e9);
    Ok(())
}
