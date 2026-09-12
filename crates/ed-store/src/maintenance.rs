//! Keeping the journal database from growing without bound.
//!
//! Every journal line is stored raw in `events` and never deleted: the raw
//! log is the source of truth every derived table is rebuilt from, and the
//! fits (fuel curve, FSD integrity) get better with history. Six weeks of
//! play measured ~1 MB a day, a third of it event types nothing ever reads
//! back once the live callouts have seen them. Those are pruned here.
//!
//! Pruning cannot happen at ingest: the watcher announces callouts by
//! reading the rows it just stored (`session::events_after`), and watched
//! `FSSSignalDiscovered` signals are among them. So rows are kept for a
//! grace window and deleted after it.

use anyhow::Result;
use rusqlite::Connection;

/// Event types stored for the live pass only. Nothing derives from them,
/// no query reads them back, and re-ingesting the journal restores them.
/// Adding a type here requires checking `derive::DERIVED_FROM`, every
/// `FROM events WHERE event` query and `callouts::from_event` -- the test
/// below pins the first of those.
pub const NOISE_EVENTS: &[&str] = &["FSSSignalDiscovered", "Music", "ReceiveText", "ShipLocker"];

/// Hours a noise row is kept after its timestamp, so the live pass has
/// long enough to announce it even across a restart.
pub const NOISE_GRACE_HOURS: i64 = 24;

/// Delete noise rows with a timestamp before `cutoff_ts` (ISO 8601, as the
/// journal writes it). Returns the number of rows removed. Freed pages are
/// reused by SQLite; `VACUUM` (the app's command) shrinks the file.
pub fn prune_noise(conn: &Connection, cutoff_ts: &str) -> Result<u64> {
    let placeholders = NOISE_EVENTS
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "DELETE FROM events WHERE event IN ({placeholders}) AND ts < ?{}",
        NOISE_EVENTS.len() + 1
    );
    let mut params: Vec<&dyn rusqlite::ToSql> = NOISE_EVENTS
        .iter()
        .map(|e| e as &dyn rusqlite::ToSql)
        .collect();
    params.push(&cutoff_ts);
    Ok(conn.execute(&sql, params.as_slice())? as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        conn
    }

    fn put(conn: &Connection, offset: i64, ts: &str, event: &str) {
        conn.execute(
            "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', ?1, ?2, ?3, ?4)",
            rusqlite::params![offset, ts, event, format!(r#"{{"timestamp":"{ts}","event":"{event}"}}"#)],
        )
        .unwrap();
    }

    #[test]
    fn prunes_only_old_noise() {
        let conn = store();
        put(&conn, 1, "2026-08-01T10:00:00Z", "FSSSignalDiscovered"); // old noise
        put(&conn, 2, "2026-08-01T10:00:01Z", "Music"); // old noise
        put(&conn, 3, "2026-08-01T10:00:02Z", "FSDJump"); // old signal: kept
        put(&conn, 4, "2026-08-30T10:00:00Z", "FSSSignalDiscovered"); // recent noise: kept
        put(&conn, 5, "2026-08-01T10:00:03Z", "ShipTargeted"); // read by merit capture: kept
        let deleted = prune_noise(&conn, "2026-08-29T10:00:00Z").unwrap();
        assert_eq!(deleted, 2);
        let left: Vec<String> = conn
            .prepare("SELECT event FROM events ORDER BY offset")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(left, ["FSDJump", "FSSSignalDiscovered", "ShipTargeted"]);
        assert_eq!(
            prune_noise(&conn, "2026-08-29T10:00:00Z").unwrap(),
            0,
            "idempotent"
        );
    }

    #[test]
    fn noise_is_never_something_a_derived_table_needs() {
        for e in NOISE_EVENTS {
            assert!(
                !crate::derive::DERIVED_FROM.contains(e),
                "{e} feeds a derived table; it cannot be pruned"
            );
        }
    }
}
