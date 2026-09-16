//! First-sync cost on a real journal folder: what a fresh install does
//! before it can show anything.
//!
//!     cargo run -p ed-store --release --example first_sync -- <journal_dir> <fresh.sqlite3>
//!
//! Prints the ingest and derive phases from `SyncStats`, the database
//! size, and how long the reads the app makes right after a sync take.
//! The maintainer's donated 2019–2026 journal (2026-09-16) is the fixture
//! this was written for; the numbers go in docs/benches.

use anyhow::{Context, Result};
use ed_store::{query, Store};
use std::path::PathBuf;
use std::time::Instant;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let journal_dir = PathBuf::from(args.next().context("usage: first_sync <journal_dir> <db>")?);
    let db = PathBuf::from(args.next().context("usage: first_sync <journal_dir> <db>")?);
    let files = ed_journal::journal::journal_files(&journal_dir)?;
    let bytes: u64 = files.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum();
    println!("journal:  {} ({} files, {:.1} MB)", journal_dir.display(), files.len(), bytes as f64 / 1e6);
    println!("database: {} (fresh: {})", db.display(), !db.exists());

    let t0 = Instant::now();
    let store = Store::open(&db, &journal_dir)?;
    println!("open (schema): {} ms", t0.elapsed().as_millis());

    let t1 = Instant::now();
    let mut last_print = Instant::now();
    let stats = store.sync_with_progress(|name, i, total| {
        if i == total || last_print.elapsed().as_secs() >= 30 {
            last_print = Instant::now();
            println!("  [{i:>4}/{total}] {name} at {:.1} s", t1.elapsed().as_secs_f64());
        }
    })?;
    let sync_s = t1.elapsed().as_secs_f64();

    println!("\n─ sync ───────────────────────────────────");
    println!("  wall             {sync_s:.1} s (stats.elapsed_ms {})", stats.elapsed_ms);
    println!("  files scanned    {}", stats.ingest.files_scanned);
    println!("  files changed    {}", stats.ingest.files_changed);
    println!("  events inserted  {}", stats.ingest.events_inserted);
    println!("  snapshots        {}", stats.ingest.snapshots_updated);
    println!("  events/s         {:.0}", stats.ingest.events_inserted as f64 / sync_s.max(0.001));
    println!("  MB/s             {:.1}", bytes as f64 / 1e6 / sync_s.max(0.001));
    println!("  derive: events read {}, materials {}, cargo {}, engineers {}, powerplay {}, sales {}, merits {}",
        stats.derive.events_read, stats.derive.materials, stats.derive.cargo, stats.derive.engineers,
        stats.derive.powerplay_observations, stats.derive.sales, stats.derive.merit_events);

    let conn = store.conn();
    let total: i64 = store.event_count()?;
    let db_bytes = std::fs::metadata(&db).map(|m| m.len()).unwrap_or(0);
    let wal = std::fs::metadata(db.with_extension("sqlite3-wal")).map(|m| m.len()).unwrap_or(0);
    println!("\n─ store ──────────────────────────────────");
    println!("  events in store  {total}");
    println!("  db size          {:.1} MB (+ wal {:.1} MB)", db_bytes as f64 / 1e6, wal as f64 / 1e6);
    println!("  bytes/event      {:.0}", (db_bytes + wal) as f64 / total.max(1) as f64);
    let mut kinds = conn.prepare("SELECT event, COUNT(*) n FROM events GROUP BY event ORDER BY n DESC LIMIT 12")?;
    for row in kinds.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (k, n) = row?;
        println!("    {n:>9} {k}");
    }

    // A second pass with nothing new: what every launch pays, by phase.
    println!("\n─ no-change sync, by phase ───────────────");
    let t = Instant::now();
    let ing = ed_store::ingest::ingest_all(conn, &journal_dir, |_, _, _| {})?;
    println!("  ingest_all       {} ms ({} files scanned, {} changed)", t.elapsed().as_millis(), ing.files_scanned, ing.files_changed);
    let t = Instant::now();
    let der = ed_store::derive::derive_incremental(conn)?;
    println!("  derive           {} ms ({} events read)", t.elapsed().as_millis(), der.events_read);
    let t = Instant::now();
    let mut seen = std::collections::HashSet::new();
    let obs = ed_store::merit_capture::capture(conn, &mut seen);
    println!("  merit_capture    {} ms ({obs} observations)", t.elapsed().as_millis());

    println!("\n─ reads after sync ───────────────────────");
    let now = "2026-12-31T00:00:00Z";
    let t = Instant::now();
    let loc = query::location(conn)?;
    println!("  location         {} ms → {}", t.elapsed().as_millis(), loc.as_ref().and_then(|l| l.system_name.clone()).unwrap_or_default());
    let t = Instant::now();
    let live = ed_store::missions::active(conn, now)?;
    println!("  missions::active {} ms → {} live", t.elapsed().as_millis(), live.len());
    let t = Instant::now();
    let all = ed_store::missions::missions(conn, "", now)?;
    println!("  missions (all)   {} ms → {}", t.elapsed().as_millis(), all.len());
    let t = Instant::now();
    let key = ed_store::session::last_event_key(conn)?;
    println!("  last_event_key   {} ms → {:?}", t.elapsed().as_millis(), key.map(|k| k.ts));
    let t = Instant::now();
    let nav = query::nav_target(conn)?;
    println!("  nav_target       {} ms → {:?}", t.elapsed().as_millis(), nav.and_then(|n| n.target_system));
    Ok(())
}
