//! Local store for Elite Dangerous journal history.
//!
//! The problem this replaces: `ed_journal::read_all` re-parses six journal
//! files on every command *and* every file-change event. That caps what the
//! app can be -- nothing can ask a question that needs history, because
//! history is never assembled.
//!
//! Instead: backfill every journal file once into SQLite, checkpoint
//! `(file, offset)`, then read only the bytes appended since. Steady-state
//! cost is the size of one journal line.
//!
//! ```no_run
//! # use ed_store::Store;
//! # fn main() -> anyhow::Result<()> {
//! let dir = ed_journal::journal::find_journal_dir(None).unwrap();
//! let store = Store::open(&ed_store::Store::default_db_path(), &dir)?;
//! let stats = store.sync()?;           // first call backfills; later calls tail
//! println!("{} events", store.event_count()?);
//! # Ok(()) }
//! ```

pub mod carrier;
pub mod derive;
pub mod eddn;
pub mod galaxy;
pub mod ingest;
pub mod lookup;
pub mod maintenance;
pub mod market;
pub mod materials;
pub mod merit_capture;
pub mod mining;
pub mod merits;
pub mod missions;
pub mod observe;
#[cfg(feature = "postgres")]
pub mod postgres;
pub mod query;
pub mod route;
pub mod schema;
pub mod session;
pub mod ship_locations;
pub mod sqlite;
pub mod stars;

use anyhow::{Context, Result};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

pub use derive::DeriveStats;
pub use ingest::IngestStats;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncStats {
    pub ingest: IngestStats,
    pub derive: DeriveStats,
    pub elapsed_ms: u64,
}

pub struct Store {
    conn: Connection,
    /// `None` for an in-memory store, which cannot be reopened.
    db_path: Option<PathBuf>,
    journal_dir: PathBuf,
    /// Sale keys already written to the observation journal. Sync runs on
    /// every journal write, so without this a single sale would be recorded
    /// dozens of times over a session.
    observed: std::sync::Mutex<std::collections::HashSet<String>>,
}

/// With more than one writer per file, a lock collision must wait, not
/// error. Generous because the stars product installs in one transaction.
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl Store {
    pub fn open(db_path: &Path, journal_dir: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = Connection::open(db_path)
            .with_context(|| format!("opening database at {}", db_path.display()))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        schema::migrate(&conn)?;
        schema::attach_galaxy(&conn, Some(&schema::galaxy_path(db_path)))?;
        Ok(Store {
            conn,
            db_path: Some(db_path.to_path_buf()),
            journal_dir: journal_dir.to_path_buf(),
            observed: std::sync::Mutex::new(observe::seen_award_keys()),
        })
    }

    /// In-memory store, for tests and for one-shot analysis.
    pub fn open_in_memory(journal_dir: &Path) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::migrate(&conn)?;
        schema::attach_galaxy(&conn, None)?;
        Ok(Store {
            conn,
            db_path: None,
            journal_dir: journal_dir.to_path_buf(),
            observed: Default::default(),
        })
    }

    /// A second writer on the same database files, for a bulk load that
    /// must not hold whatever synchronizes access to the store while it
    /// runs. SQLite serializes the files themselves; the store's own
    /// connection keeps working (the journal watcher keeps ingesting)
    /// while a community hydration streams in through this one.
    pub fn reopen_writer(&self) -> Result<Connection> {
        let db_path = self
            .db_path
            .as_deref()
            .context("an in-memory store has no file to reopen")?;
        let conn = Connection::open(db_path)
            .with_context(|| format!("reopening database at {}", db_path.display()))?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // The schema is already migrated; this connection only needs the
        // same pragmas and the galaxy attach.
        conn.execute_batch("PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;")?;
        schema::attach_galaxy(&conn, Some(&schema::galaxy_path(db_path)))?;
        Ok(conn)
    }

    /// Where the app keeps its database by default.
    ///
    /// Prefers a `.data/` directory in the repo when one exists, walking up
    /// from the working directory. That keeps a development checkout and its
    /// (large) database together, and means `cargo run` and `cargo tauri dev`
    /// find the same file regardless of which crate directory they start in.
    /// An installed build has no such directory and falls back to the
    /// platform's per-user data location.
    pub fn default_db_path() -> PathBuf {
        Self::default_db_path_for(env!("CARGO_PKG_NAME"))
    }

    /// Default database path for an application embedding this store.
    /// The caller supplies its own package name because `CARGO_PKG_NAME`
    /// inside this crate is `ed-store`, not the desktop application's name.
    pub fn default_db_path_for(app_name: &str) -> PathBuf {
        const FILE: &str = "edda.sqlite3";

        // Repository discovery is a development convenience only. A release
        // executable must use per-user app data even if it happens to be
        // launched from a source checkout (critical for clean-install tests).
        if cfg!(debug_assertions) {
            let starts = [
                std::env::current_dir().ok(),
                std::env::current_exe()
                    .ok()
                    .and_then(|e| e.parent().map(PathBuf::from)),
            ];
            for start in starts.into_iter().flatten() {
                for dir in start.ancestors().take(5) {
                    let candidate = dir.join(".data");
                    if candidate.is_dir() {
                        return candidate.join(FILE);
                    }
                }
            }
        }

        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        base.join(app_name).join(FILE)
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn journal_dir(&self) -> &Path {
        &self.journal_dir
    }

    /// Ingest anything new, then rebuild derived state.
    pub fn sync(&self) -> Result<SyncStats> {
        self.sync_with_progress(|_, _, _| {})
    }

    pub fn sync_with_progress(
        &self,
        progress: impl FnMut(&str, usize, usize),
    ) -> Result<SyncStats> {
        let started = std::time::Instant::now();
        let ingest = ingest::ingest_all(&self.conn, &self.journal_dir, progress)?;
        // Derive unconditionally: the whole-file companions (Cargo.json and
        // friends) can change without a single new journal line, so "no new
        // events" does not mean "nothing to re-derive".
        let derive = derive::derive_incremental(&self.conn)?;

        // Capture anything newly measurable while the commander is flying.
        let observed = {
            let mut seen = self.observed.lock().unwrap_or_else(|e| e.into_inner());
            merit_capture::capture(&self.conn, &mut seen)
        };

        let elapsed_ms = started.elapsed().as_millis() as u64;
        if ingest.events_inserted > 0 || observed > 0 {
            // Trace, not info (maintainer, 2026-09-05): this fires on every
            // journal write — several times a minute in flight — and
            // drowned the log ("we don't even want to see them at
            // debug"). RUST_LOG=ed_store=trace brings it back.
            tracing::trace!(
                events = ingest.events_inserted,
                files_changed = ingest.files_changed,
                powerplay = derive.powerplay_observations,
                sales = derive.sales,
                merit_events = derive.merit_events,
                new_observations = observed,
                elapsed_ms,
                "journal synced"
            );
        } else {
            tracing::trace!(elapsed_ms, "journal sync: nothing new");
        }

        Ok(SyncStats {
            ingest,
            derive,
            elapsed_ms,
        })
    }

    pub fn event_count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?)
    }

    /// Event-type histogram, most frequent first. Useful for seeing what a
    /// journal folder actually contains before building against it.
    pub fn event_histogram(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT event, COUNT(*) c FROM events GROUP BY event ORDER BY c DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Two journal lines that exercise the materials replay and a jump.
    const F1: &str = concat!(
        r#"{"timestamp":"2026-08-01T10:00:00Z","event":"Materials","Raw":[{"Name":"iron","Count":10}],"Manufactured":[],"Encoded":[]}"#,
        "\n",
        r#"{"timestamp":"2026-08-01T10:05:00Z","event":"FSDJump","StarSystem":"Deciat","SystemAddress":6681123623626,"ControllingPower":"A. Lavigny-Duval","PowerplayState":"Stronghold","PowerplayStateControlProgress":0.43,"PowerplayStateReinforcement":62721,"PowerplayStateUndermining":46108}"#,
        "\n",
    );

    fn write(dir: &Path, name: &str, body: &str) {
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    fn append(dir: &Path, name: &str, body: &str) {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.join(name))
            .unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    fn store_with(body: &str) -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "Journal.2026-08-01T100000.01.log", body);
        let store = Store::open_in_memory(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn backfills_then_tails_only_new_bytes() {
        let (dir, store) = store_with(F1);
        let first = store.sync().unwrap();
        assert_eq!(first.ingest.events_inserted, 2);

        // Nothing changed: a second sync must not re-insert anything.
        let second = store.sync().unwrap();
        assert_eq!(second.ingest.events_inserted, 0);
        assert_eq!(store.event_count().unwrap(), 2);

        // Append one line: only that line is read.
        append(
            dir.path(),
            "Journal.2026-08-01T100000.01.log",
            "{\"timestamp\":\"2026-08-01T10:06:00Z\",\"event\":\"MaterialCollected\",\"Name\":\"iron\",\"Count\":5}\n",
        );
        let third = store.sync().unwrap();
        assert_eq!(third.ingest.events_inserted, 1);
        assert_eq!(store.event_count().unwrap(), 3);
    }

    #[test]
    fn a_half_written_line_is_not_consumed_until_complete() {
        let (dir, store) = store_with(F1);
        store.sync().unwrap();

        // A tail can catch the game mid-flush. The partial line must not be
        // committed, or the rest of it is lost forever when it lands.
        append(
            dir.path(),
            "Journal.2026-08-01T100000.01.log",
            r#"{"timestamp":"2026-08-01T10:07:00Z","event":"MaterialCol"#,
        );
        assert_eq!(store.sync().unwrap().ingest.events_inserted, 0);
        assert_eq!(store.event_count().unwrap(), 2);

        // Once the line is finished it is picked up whole.
        append(
            dir.path(),
            "Journal.2026-08-01T100000.01.log",
            "lected\",\"Name\":\"iron\",\"Count\":5}\n",
        );
        assert_eq!(store.sync().unwrap().ingest.events_inserted, 1);
        assert_eq!(store.event_count().unwrap(), 3);
    }

    #[test]
    fn a_replaced_shorter_file_is_reread_rather_than_seeked_past() {
        let (dir, store) = store_with(F1);
        store.sync().unwrap();
        assert_eq!(store.event_count().unwrap(), 2);

        // Same name, different (shorter) content: not the file we
        // checkpointed, so the stored offset is meaningless.
        write(
            dir.path(),
            "Journal.2026-08-01T100000.01.log",
            "{\"timestamp\":\"2026-08-01T11:00:00Z\",\"event\":\"Undocked\"}\n",
        );
        let s = store.sync().unwrap();
        assert_eq!(s.ingest.events_inserted, 1);
    }

    #[test]
    fn rollover_to_a_new_session_file_is_picked_up() {
        let (dir, store) = store_with(F1);
        store.sync().unwrap();

        write(
            dir.path(),
            "Journal.2026-08-02T090000.01.log",
            "{\"timestamp\":\"2026-08-02T09:00:00Z\",\"event\":\"Docked\",\"StationName\":\"Ramon City\",\"StarSystem\":\"Paesia\"}\n",
        );
        let s = store.sync().unwrap();
        assert_eq!(s.ingest.events_inserted, 1);

        let (system, docked, station): (String, i64, String) = store
            .conn()
            .query_row(
                "SELECT system_name, docked, station_name FROM location WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(system, "Paesia");
        assert_eq!(docked, 1);
        assert_eq!(station, "Ramon City");
    }

    #[test]
    fn materials_replay_matches_ed_journal_semantics() {
        let (dir, store) = store_with(F1);
        append(
            dir.path(),
            "Journal.2026-08-01T100000.01.log",
            "{\"timestamp\":\"2026-08-01T10:06:00Z\",\"event\":\"MaterialCollected\",\"Name\":\"iron\",\"Count\":5}\n",
        );
        store.sync().unwrap();

        let count: i64 = store
            .conn()
            .query_row("SELECT count FROM materials WHERE symbol='iron'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, 15, "snapshot of 10 plus a delta of 5");
    }

    #[test]
    fn powerplay_fields_are_captured_from_the_jump_event() {
        let (_dir, store) = store_with(F1);
        store.sync().unwrap();

        let (sys, power, state, progress): (String, String, String, f64) = store
            .conn()
            .query_row(
                "SELECT system_name, controlling_power, powerplay_state, control_progress
                 FROM powerplay_observations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(sys, "Deciat");
        assert_eq!(power, "A. Lavigny-Duval");
        assert_eq!(state, "Stronghold");
        assert!((progress - 0.43).abs() < 1e-9);
    }

    /// Snapshot of every derived table, for equivalence checks.
    fn derived_snapshot(store: &Store) -> Vec<(String, String)> {
        let conn = store.conn();
        let mut out = Vec::new();
        for table in [
            "materials",
            "cargo",
            "engineers",
            "loadout",
            "location",
            "nav",
            "powerplay_observations",
            "sales",
            "merit_events",
        ] {
            let mut stmt = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let cols = stmt.column_count();
            let rows = stmt
                .query_map([], move |r| {
                    let mut parts = Vec::new();
                    for i in 0..cols {
                        parts.push(format!("{:?}", r.get::<_, rusqlite::types::Value>(i)?));
                    }
                    Ok(parts.join("|"))
                })
                .unwrap();
            let mut vals: Vec<String> = rows.map(|r| r.unwrap()).collect();
            vals.sort();
            out.push((table.to_string(), vals.join("\n")));
        }
        out
    }

    #[test]
    fn incremental_derive_matches_a_full_rebuild() {
        // The whole point of the watermark is that resuming produces the same
        // state as replaying everything. If it ever doesn't, a live session
        // silently drifts away from the truth.
        let (dir, store) = store_with(F1);
        store.sync().unwrap();

        for line in [
            "{\"timestamp\":\"2026-08-01T10:06:00Z\",\"event\":\"MaterialCollected\",\"Name\":\"iron\",\"Count\":5}\n",
            "{\"timestamp\":\"2026-08-01T10:07:00Z\",\"event\":\"Docked\",\"StationName\":\"Ramon City\",\"StarSystem\":\"Paesia\"}\n",
            "{\"timestamp\":\"2026-08-01T10:08:00Z\",\"event\":\"MarketSell\",\"MarketID\":7,\"Type\":\"gold\",\"Count\":3,\"SellPrice\":100,\"TotalSale\":300,\"AvgPricePaid\":0}\n",
            "{\"timestamp\":\"2026-08-01T10:08:01Z\",\"event\":\"PowerplayMerits\",\"Power\":\"Aisling Duval\",\"MeritsGained\":2,\"TotalMerits\":2}\n",
            "{\"timestamp\":\"2026-08-01T10:09:00Z\",\"event\":\"FSDTarget\",\"Name\":\"Sol\",\"StarClass\":\"G\",\"RemainingJumpsInRoute\":2}\n",
        ] {
            append(dir.path(), "Journal.2026-08-01T100000.01.log", line);
            store.sync().unwrap(); // each one an incremental pass
        }

        let incremental = derived_snapshot(&store);
        derive::derive_all(store.conn()).unwrap(); // full rebuild
        let full = derived_snapshot(&store);

        for ((table, a), (_, b)) in incremental.iter().zip(&full) {
            assert_eq!(a, b, "table {table} diverged between incremental and full");
        }
    }

    #[test]
    fn a_material_spent_to_zero_disappears_on_an_incremental_pass() {
        let (dir, store) = store_with(F1);
        store.sync().unwrap();
        assert_eq!(
            store
                .conn()
                .query_row("SELECT count FROM materials WHERE symbol='iron'", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            10
        );

        // Spend it all. Carrying state forward and upserting only positives
        // would leave iron sitting at 10 -- a wrong number, not a stale one.
        append(
            dir.path(),
            "Journal.2026-08-01T100000.01.log",
            "{\"timestamp\":\"2026-08-01T10:10:00Z\",\"event\":\"MaterialDiscarded\",\"Name\":\"iron\",\"Count\":10}\n",
        );
        store.sync().unwrap();

        let remaining: i64 = store
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM materials WHERE symbol='iron'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "a spent material must not linger");
    }

    #[test]
    fn rederiving_is_idempotent() {
        let (_dir, store) = store_with(F1);
        store.sync().unwrap();
        let a = derive::derive_all(store.conn()).unwrap();
        let b = derive::derive_all(store.conn()).unwrap();
        assert_eq!(a, b);

        let obs: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM powerplay_observations", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(obs, 1, "re-deriving must not duplicate append-only rows");
    }

    #[test]
    fn identical_consecutive_events_are_both_kept() {
        // Two byte-distinct occurrences of the same event are two real
        // events. Deduping on content would silently lose one.
        let line = "{\"timestamp\":\"2026-08-01T10:00:00Z\",\"event\":\"PowerplayMerits\",\"Power\":\"Aisling Duval\",\"MeritsGained\":10,\"TotalMerits\":100}\n";
        let (_dir, store) = store_with(&format!("{line}{line}"));
        store.sync().unwrap();
        assert_eq!(store.event_count().unwrap(), 2);

        let merits: i64 = store
            .conn()
            .query_row("SELECT COUNT(*) FROM merit_events", [], |r| r.get(0))
            .unwrap();
        assert_eq!(merits, 2);
    }
}
