//! Backfill the real journal folder and report what landed.
//!
//!     cargo run -p ed-store --example backfill --release
//!
//! Safe to re-run: ingest is keyed on (file, offset), so a second run only
//! picks up bytes written since the first.

use anyhow::{Context, Result};
use ed_store::{query, Store};

fn main() -> Result<()> {
    let journal_dir = ed_journal::journal::find_journal_dir(None)
        .context("could not locate the Elite Dangerous journal folder")?;
    println!("journal:  {}", journal_dir.display());

    let db = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(Store::default_db_path);
    println!("database: {}\n", db.display());

    let store = Store::open(&db, &journal_dir)?;

    let mut last = 0usize;
    let stats = store.sync_with_progress(|name, i, total| {
        if i == total || i - last >= 10 {
            last = i;
            println!("  [{i:>3}/{total}] {name}");
        }
    })?;

    println!("\n─ ingest ─────────────────────────────────");
    println!("  files scanned    {}", stats.ingest.files_scanned);
    println!("  files changed    {}", stats.ingest.files_changed);
    println!("  events inserted  {}", stats.ingest.events_inserted);
    println!("  snapshots        {}", stats.ingest.snapshots_updated);
    println!("  total events     {}", store.event_count()?);
    println!("  elapsed          {} ms", stats.elapsed_ms);

    println!("\n─ derived ────────────────────────────────");
    println!("  events read      {}", stats.derive.events_read);
    println!("  materials        {}", stats.derive.materials);
    println!("  cargo            {}", stats.derive.cargo);
    println!("  engineers        {}", stats.derive.engineers);
    println!("  powerplay obs    {}", stats.derive.powerplay_observations);
    println!("  sales            {}", stats.derive.sales);
    println!("  merit events     {}", stats.derive.merit_events);

    let conn = store.conn();

    if let Some(loc) = query::location(conn)? {
        println!("\n─ location ───────────────────────────────");
        println!(
            "  {} {}",
            loc.system_name.as_deref().unwrap_or("?"),
            if loc.docked {
                format!("(docked at {})", loc.station_name.as_deref().unwrap_or("?"))
            } else {
                "(in space)".to_string()
            }
        );
        if let Some(pp) = loc
            .system_name
            .as_deref()
            .and_then(|s| query::powerplay_for_system(conn, s).ok().flatten())
        {
            println!(
                "  power: {} / {} ({:.1}% control)",
                pp.controlling_power.as_deref().unwrap_or("none"),
                pp.powerplay_state.as_deref().unwrap_or("?"),
                pp.control_progress.unwrap_or(0.0) * 100.0
            );
        }
    }

    if let Some(nav) = query::nav_target(conn)? {
        println!("\n─ next jump ──────────────────────────────");
        println!(
            "  {} ({}) — {} jumps left — {}",
            nav.target_system.as_deref().unwrap_or("?"),
            nav.star_class.as_deref().unwrap_or("?"),
            nav.remaining_jumps
                .map(|n| n.to_string())
                .unwrap_or("?".into()),
            match nav.scoopable {
                Some(true) => "scoopable",
                Some(false) => "NOT scoopable",
                None => "unknown",
            }
        );
    }

    let engs = query::engineers(conn)?;
    let unlocked: Vec<_> = engs.iter().filter(|e| e.is_unlocked()).collect();
    println!("\n─ engineers ──────────────────────────────");
    println!("  {} known, {} unlocked", engs.len(), unlocked.len());
    for e in &unlocked {
        println!("    {} (rank {})", e.name, e.rank.unwrap_or(0));
    }

    let dupes = query::duplicate_sales(conn)?;
    println!("\n─ duplicate sales ────────────────────────");
    if dupes.is_empty() {
        println!("  none — every sale row is distinct");
    } else {
        for (ts, commodity, count, n) in dupes.iter().take(10) {
            println!("  {n}x  {ts}  {commodity} x{count}");
        }
    }

    let joined = query::sales_with_merits(conn, 5)?;
    let earning: Vec<_> = joined.iter().filter(|s| s.merits > 0).collect();
    println!("\n─ sales joined to merits ─────────────────");
    println!("  {} sales, {} earned merits", joined.len(), earning.len());
    println!(
        "  {:<20} {:>6} {:>14} {:>8} {:>4} {:>10}",
        "commodity", "tons", "profit", "merits", "evts", "cr/merit"
    );
    for s in earning.iter().rev().take(15) {
        let profit = s.profit.or(s.total_sale).unwrap_or(0);
        println!(
            "  {:<20} {:>6} {:>14} {:>8} {:>4} {:>10.1}",
            s.commodity,
            s.count,
            profit,
            s.merits,
            s.merit_events,
            profit as f64 / s.merits as f64
        );
    }

    print_merit_model(conn)?;

    println!("\n─ top event types ────────────────────────");
    for (event, n) in store.event_histogram()?.into_iter().take(12) {
        println!("  {n:>7}  {event}");
    }

    Ok(())
}

// Appended: Phase 1 merit calibration report.
fn print_merit_model(conn: &rusqlite::Connection) -> Result<()> {
    let model = ed_store::merits::calibrate(conn, 5)?;
    println!("\n─ merit calibration ──────────────────────");
    for s in &model.stations {
        let k = match s.k() {
            Some(k) => format!(
                "K = {k:.2}  (±{:.3}%)",
                s.precision().unwrap_or(0.0) * 100.0
            ),
            None => "NO SINGLE K — observations contradict".to_string(),
        };
        println!(
            "  {:<18} {:<28} {:<12} n={:<3} {}",
            s.station.as_deref().unwrap_or("?"),
            s.system.as_deref().unwrap_or("?"),
            s.powerplay_state.as_deref().unwrap_or("-"),
            s.samples,
            k
        );
    }
    Ok(())
}
