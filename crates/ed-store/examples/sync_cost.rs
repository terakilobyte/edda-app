//! Time one quiet `Store::sync` on a copy of a real store, phase by phase:
//! what EDDA does every 5 s with no new journal line.
//!
//!   cargo run --release -p ed-store --example sync_cost -- <store.sqlite3> <journal dir>
//!
//! 2026-10-03: the dev app read 153 MB and wrote 37 MB per quiet sync.
fn main() -> anyhow::Result<()> {
    let db = std::path::PathBuf::from(std::env::args().nth(1).expect("store path"));
    let journal = std::path::PathBuf::from(std::env::args().nth(2).expect("journal dir"));
    let store = ed_store::Store::open(&db, &journal)?;
    for pass in 1..=3 {
        let t = std::time::Instant::now();
        let ingest = ed_store::ingest::ingest_all(store.conn(), &journal, |_, _, _| {})?;
        let t_ingest = t.elapsed();
        let t = std::time::Instant::now();
        let derive = ed_store::derive::derive_incremental(store.conn())?;
        let t_derive = t.elapsed();
        println!(
            "pass {pass}: ingest {:>6.1} ms (files {}, changed {}, events {}, snapshots {}) | derive {:>7.1} ms (events read {}) ",
            t_ingest.as_secs_f64() * 1e3, ingest.files_scanned, ingest.files_changed, ingest.events_inserted, ingest.snapshots_updated,
            t_derive.as_secs_f64() * 1e3, derive.events_read
        );
    }
    Ok(())
}
