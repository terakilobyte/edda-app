//! Watch a live session: tail the journal, ingest EDDN, log everything.
//!
//!     cargo run -p ed-store --example watch --release --features live-feed
//!
//! Run this in a spare terminal while playing. It does three things:
//!
//! 1. Tails the journal and syncs each change into the store.
//! 2. Subscribes to EDDN and writes market/Powerplay updates into the galaxy
//!    tables.
//! 3. Appends every measurable game fact to `.data/logs/observations.jsonl`.
//!
//! The third is the point. `docs/PLAN.md` §2 leaves the merit constant `K`
//! per-station with an unknown driver, and the only way to resolve it is to
//! accumulate sales across many stations and control states. Each sale you
//! make while this runs is one more data point.
//!
//! Everything also goes to `.data/logs/edda.log.<date>` as JSON lines.

use anyhow::{Context, Result};
use ed_store::{observe, Store};
use notify::{RecursiveMode, Watcher};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let journal_dir = ed_journal::journal::find_journal_dir(None)
        .context("could not locate the Elite Dangerous journal folder")?;
    let db = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(Store::default_db_path);

    let _guard = observe::init(&db, true)?;
    let store = Arc::new(Mutex::new(Store::open(&db, &journal_dir)?));

    println!("journal:      {}", journal_dir.display());
    println!("database:     {}", db.display());
    println!("logs:         {}", observe::log_dir(&db).display());
    println!(
        "observations: {}\n",
        observe::log_dir(&db).join("observations.jsonl").display()
    );

    // Catch up before watching, so a session started earlier is not missed.
    {
        let guard = store.lock().unwrap();
        let stats = guard.sync()?;
        tracing::info!(
            events = stats.ingest.events_inserted,
            total = guard.event_count().unwrap_or(0),
            "initial sync complete"
        );
    }

    // ── Journal watcher ─────────────────────────────────────────────
    {
        let store = store.clone();
        let dir = journal_dir.clone();
        std::thread::spawn(move || {
            let (tx, rx) = channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(e) => {
                    tracing::error!(error = %e, "could not create file watcher");
                    return;
                }
            };
            if let Err(e) = watcher.watch(&dir, RecursiveMode::NonRecursive) {
                tracing::error!(error = %e, dir = %dir.display(), "could not watch journal");
                return;
            }
            tracing::info!(dir = %dir.display(), "watching journal");

            loop {
                match rx.recv() {
                    Ok(_) => {
                        // Coalesce the burst notify fires per write.
                        while rx.recv_timeout(Duration::from_millis(400)).is_ok() {}
                        let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                        if let Err(e) = guard.sync() {
                            tracing::error!(error = %e, "sync failed");
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // ── Periodic heartbeat ──────────────────────────────────────────
    {
        let store = store.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(300));
            let guard = store.lock().unwrap_or_else(|e| e.into_inner());
            let conn = guard.conn();
            let loc = ed_store::query::location(conn).ok().flatten();
            tracing::info!(
                events = guard.event_count().unwrap_or(0),
                system = loc
                    .as_ref()
                    .and_then(|l| l.system_name.as_deref())
                    .unwrap_or("?"),
                docked = loc.as_ref().map(|l| l.docked).unwrap_or(false),
                "heartbeat"
            );
        });
    }

    // ── EDDN feed ───────────────────────────────────────────────────
    tracing::info!(relay = ed_eddn::EDDN_RELAY, "connecting to EDDN");
    let mut last = Instant::now();
    let mut rows = 0u64;

    ed_eddn::live::run(ed_eddn::EDDN_RELAY, |env, stats| {
        {
            let guard = store.lock().unwrap_or_else(|e| e.into_inner());
            match ed_store::eddn::apply(guard.conn(), env) {
                Ok(a) => rows += a.market_rows + a.systems + a.outfitting_rows + a.shipyard_rows,
                Err(e) => tracing::error!(error = %e, schema = env.schema(), "eddn apply failed"),
            }
        }

        // At ~10 messages a second, per-message logging would bury the
        // journal events that actually matter.
        if last.elapsed() >= Duration::from_secs(60) {
            tracing::info!(
                received = stats.received,
                decoded = stats.decoded,
                decode_errors = stats.decode_errors,
                commodity = stats.commodity,
                journal = stats.journal,
                reconnects = stats.reconnects,
                rows_written = rows,
                "eddn"
            );
            rows = 0;
            last = Instant::now();
        }
        true
    })
    .await?;

    Ok(())
}
