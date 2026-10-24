//! Backfill once, then tail.
//!
//! The journal folder holds two very different kinds of file and they are
//! handled differently (see `docs/PLAN.md` §4.3):
//!
//! * `Journal.*.log` -- append-only, rolls over to a new file each game
//!   session. Read by byte offset, so steady-state cost is "bytes written
//!   since the last event", not "re-parse six files".
//! * `Status.json`, `Cargo.json`, ... -- rewritten in full on every change.
//!   Re-read whole and upserted as a snapshot.
//!
//! Nothing here writes to a derived table. Ingest's only job is to get rows
//! into `events` (and `snapshots`) exactly once; interpretation happens in
//! [`crate::derive`].

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Whole-file JSON companions the game rewrites in place.
pub const COMPANION_FILES: &[&str] = &[
    "Status.json",
    "Cargo.json",
    "Market.json",
    "NavRoute.json",
    "Outfitting.json",
    "Shipyard.json",
    "ShipLocker.json",
    "Backpack.json",
    "ModulesInfo.json",
];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IngestStats {
    pub files_scanned: usize,
    pub files_changed: usize,
    pub events_inserted: u64,
    pub snapshots_updated: usize,
}

/// `Journal.*.log` names in the folder, oldest first. NOT a plain string
/// sort: the game's two name formats do not interleave chronologically
/// (see `ed_journal::journal_file`), which is why every query that means
/// "in time order" orders by `ts` and only then by `(file, offset)`.
pub fn journal_file_names(dir: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("reading journal dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("Journal.") && n.ends_with(".log"))
        .collect();
    names.sort_by_key(|n| ed_journal::journal_file::sort_key(n));
    Ok(names)
}

fn checkpoint(conn: &Connection, name: &str) -> Result<Option<(u64, u64)>> {
    let row = conn
        .query_row(
            "SELECT size, offset FROM journal_files WHERE name = ?1",
            [name],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();
    Ok(row.map(|(s, o)| (s as u64, o as u64)))
}

/// The archive name for a truncated-and-reused journal file's previous
/// content: a letter inserted before the "log" extension, so
/// "Journal.X.01.log" becomes "Journal.X.01.a.log" — which sorts BEFORE
/// the original ("a" < "l"), keeping (file, offset) replay chronological,
/// and can never appear on disk. The first free letter wins, so a name
/// reused twice archives as .a. then .b.
fn archive_name(conn: &Connection, name: &str) -> Result<String> {
    let stem = name.strip_suffix("log").unwrap_or(name);
    for letter in 'a'..='z' {
        let candidate = format!("{stem}{letter}.log");
        let taken: bool = conn
            .query_row("SELECT 1 FROM events WHERE file = ?1 LIMIT 1", [&candidate], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if !taken {
            return Ok(candidate);
        }
    }
    anyhow::bail!("journal {name} has been truncated 26 times; refusing to archive further")
}

/// Ingest whatever is new in one journal file. Returns rows inserted.
///
/// Three failure modes this deliberately handles, because each one silently
/// corrupts state otherwise:
///
/// * **Truncation / replacement** -- if the file is now smaller than our
///   stored offset it isn't the file we checkpointed: the old content's
///   rows are archived under a synthetic name (they are real history —
///   Elite's 4.4.1.1 relaunch reused a live journal's filename) and the
///   new content is read from byte zero.
/// * **Partial lines** -- a tail can catch a line mid-flush. Only bytes up
///   to the final newline are consumed; the remainder is left for next time.
/// * **Re-reads** -- `(file, offset)` is the primary key and inserts are
///   `OR IGNORE`, so re-ingesting a file is a no-op rather than a duplicate.
pub fn ingest_file_for(conn: &Connection, dir: &Path, name: &str, expect_fid: Option<&str>) -> Result<u64> {
    let path = dir.join(name);
    let size = std::fs::metadata(&path)
        .with_context(|| format!("stat {}", path.display()))?
        .len();

    let mut start = checkpoint(conn, name)?.map(|(_, o)| o).unwrap_or(0);
    // Smaller than our checkpoint means this is not the file we recorded.
    // Its existing rows are stale, and because `(file, offset)` is the key,
    // leaving them in place would make `INSERT OR IGNORE` silently discard
    // the replacement content at those same offsets.
    let replaced = size < start;
    if replaced {
        start = 0;
    }
    if size == start {
        return Ok(0);
    }

    let mut f = std::fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((size - start) as usize);
    f.read_to_end(&mut buf)?;

    let Some(last_nl) = buf.iter().rposition(|b| *b == b'\n') else {
        // Nothing but a partial line so far -- leave the checkpoint alone.
        return Ok(0);
    };
    let consumable = &buf[..=last_nl];

    let tx = conn.unchecked_transaction()?;
    if replaced {
        // The old incarnation's rows are REAL HISTORY, not garbage: Elite's
        // 4.4.1.1 patch relaunch reused a journal filename for a brand-new
        // session (field case 2026-09-05), and the delete that lived here
        // erased a whole trading day — sales and merits included — the
        // moment the truncation was noticed. Archive instead: re-key the
        // old rows to a name that sorts BEFORE the original (chronological
        // replay by (file, offset) stays correct) and never collides with
        // the re-read. A same-named file shrinking because the OLD content
        // was invalid has never been observed; losing real history has.
        let archived = archive_name(&tx, name)?;
        for sql in [
            "UPDATE events SET file = ?2 WHERE file = ?1",
            "UPDATE powerplay_observations SET file = ?2 WHERE file = ?1",
            "UPDATE sales SET file = ?2 WHERE file = ?1",
            "UPDATE merit_events SET file = ?2 WHERE file = ?1",
            "UPDATE combat_kills SET file = ?2 WHERE file = ?1",
            "UPDATE combat_incidents SET file = ?2 WHERE file = ?1",
        ] {
            tx.execute(sql, params![name, archived])?;
        }
        tracing::warn!(
            file = name,
            archived = %archived,
            "journal file was truncated and reused (a patch relaunch?); previous content archived"
        );
    }

    let mut inserted = 0u64;
    let mut last_ts: Option<String> = None;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO events
                 (file, offset, ts, event, system_address, market_id, raw)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )?;

        let mut pos = 0usize;
        for line in consumable.split_inclusive(|b| *b == b'\n') {
            let abs = start + pos as u64;
            pos += line.len();

            let Ok(text) = std::str::from_utf8(line) else {
                continue;
            };
            let text = text.trim().trim_start_matches('\u{feff}');
            if text.is_empty() {
                continue;
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
                continue;
            };
            let Some(event) = v.get("event").and_then(|x| x.as_str()) else {
                continue;
            };
            // A file belongs to one commander. A later line naming another
            // FID has never been seen (0 of 1,396 files, 2026-10-24); if a
            // game build ever does it, the rest of the file is refused with
            // one warning, never blended into this store.
            if let Some(expect) = expect_fid {
                if matches!(event, "Commander" | "LoadGame") {
                    if let Some(fid) = v.get("FID").and_then(|f| f.as_str()) {
                        if fid != expect {
                            tracing::warn!(file = name, offset = abs, found = fid, expected = expect, "journal line names another commander; the rest of this file is refused");
                            break;
                        }
                    }
                }
            }
            let ts = v
                .get("timestamp")
                .and_then(|x| x.as_str())
                .unwrap_or_default();

            inserted += stmt.execute(params![
                name,
                abs as i64,
                ts,
                event,
                v.get("SystemAddress").and_then(|x| x.as_i64()),
                v.get("MarketID").and_then(|x| x.as_i64()),
                text,
            ])? as u64;

            if !ts.is_empty() {
                last_ts = Some(ts.to_string());
            }
        }
    }

    let consumed = start + consumable.len() as u64;
    tx.execute(
        "INSERT INTO journal_files (name, size, offset, last_ts, updated_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(name) DO UPDATE SET
             size = excluded.size,
             offset = excluded.offset,
             last_ts = COALESCE(excluded.last_ts, journal_files.last_ts),
             updated_at = excluded.updated_at",
        params![name, size as i64, consumed as i64, last_ts],
    )?;
    tx.commit()?;

    Ok(inserted)
}

/// Tail one journal file into the store, every line (the single-store path).
pub fn ingest_file(conn: &Connection, dir: &Path, name: &str) -> Result<u64> {
    ingest_file_for(conn, dir, name, None)
}

/// Ingest the given journal files only (no companions). The registry hands
/// each store the names it owns; `expect_fid` is that store's commander.
pub fn ingest_files(
    conn: &Connection,
    dir: &Path,
    names: &[String],
    expect_fid: Option<&str>,
    mut progress: impl FnMut(&str, usize, usize),
) -> Result<IngestStats> {
    let total = names.len();
    let mut stats = IngestStats { files_scanned: total, ..Default::default() };
    for (i, name) in names.iter().enumerate() {
        progress(name, i + 1, total);
        let n = ingest_file_for(conn, dir, name, expect_fid)?;
        if n > 0 {
            stats.files_changed += 1;
            stats.events_inserted += n;
        }
    }
    Ok(stats)
}

/// The snapshot a store holds when a companion file could not be
/// attributed to it: every `json_extract` on it is NULL, so the fuel
/// gauge, flags and nav target show nothing rather than another
/// commander's values.
pub const UNKNOWN_SNAPSHOT: &str = "{}";

/// Upsert one companion snapshot.
pub fn write_snapshot(conn: &Connection, name: &str, ts: Option<&str>, mtime: Option<i64>, raw: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO snapshots (name, ts, mtime, raw) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(name) DO UPDATE SET ts = excluded.ts, mtime = excluded.mtime, raw = excluded.raw",
        params![name, ts, mtime, raw],
    )?;
    Ok(())
}

/// This store does not know whose `name` is right now.
pub fn mark_unknown(conn: &Connection, name: &str) -> Result<()> {
    write_snapshot(conn, name, None, None, UNKNOWN_SNAPSHOT)
}

/// Re-read the whole-file JSON companions and upsert them.
pub fn ingest_companions(conn: &Connection, dir: &Path) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut updated = 0usize;
    for name in COMPANION_FILES {
        let path = dir.join(name);
        let Ok(md) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        // Unchanged since the stored copy: nothing to read, write or count.
        // Until 2026-10-03 every quiet sync re-read and re-wrote all nine
        // files and reported nine "updated" snapshots, which made the
        // watcher announce a journal change every 5 s with the game idle —
        // and every panel re-derived everything (153 MB read, 37 MB written
        // per sync, measured on the maintainer's machine).
        let stored: Option<Option<i64>> = tx
            .query_row("SELECT mtime FROM snapshots WHERE name = ?1", [name], |r| r.get(0))
            .optional()?;
        if let (Some(Some(stored)), Some(now)) = (stored, mtime) {
            if stored == now {
                continue;
            }
        }

        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let raw = raw.trim().trim_start_matches('\u{feff}').to_string();
        if raw.is_empty() {
            continue;
        }
        // The game can be mid-write; a half-written file is skipped rather
        // than stored, so a reader never sees a truncated snapshot.
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let ts = v.get("timestamp").and_then(|x| x.as_str());

        write_snapshot(&tx, name, ts, mtime, &raw)?;
        updated += 1;
    }
    tx.commit()?;
    Ok(updated)
}

/// Ingest every journal file plus the companions.
///
/// `progress` is called as `(file_name, index, total)` before each file.
/// Backfill is ~60k events on a year-old journal folder and grows with
/// playtime, so a first run needs to be able to say what it is doing rather
/// than looking like a hang.
pub fn ingest_all(
    conn: &Connection,
    dir: &Path,
    progress: impl FnMut(&str, usize, usize),
) -> Result<IngestStats> {
    let names = journal_file_names(dir)?;
    let mut stats = ingest_files(conn, dir, &names, None, progress)?;
    stats.snapshots_updated = ingest_companions(conn, dir)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {

    /// The game renamed its journals in late 2022: `Journal.YYMMDDHHMMSS.NN.log`
    /// became `Journal.YYYY-MM-DDTHHMMSS.NN.log`. As plain strings the 2021
    /// and 2022 files sort AFTER every 2023+ file, so a commander who played
    /// through the change had their oldest history replayed LAST and read as
    /// "latest": a 2022 ship, still in flight (tester, 2026-09-15).
    #[test]
    fn journal_files_list_chronologically_across_both_name_formats() {
        let dir = tempfile::tempdir().unwrap();
        for name in [
            "Journal.2026-09-15T090000.01.log",
            "Journal.221231235959.01.log",
            "Journal.2024-01-01T120000.01.log",
            "Journal.200101000000.01.log",
            "Journal.210615080000.02.log",
            "Journal.210615080000.01.log",
        ] {
            std::fs::File::create(dir.path().join(name)).unwrap();
        }
        let names = journal_file_names(dir.path()).unwrap();
        assert_eq!(
            names,
            vec![
                "Journal.200101000000.01.log",
                "Journal.210615080000.01.log",
                "Journal.210615080000.02.log",
                "Journal.221231235959.01.log",
                "Journal.2024-01-01T120000.01.log",
                "Journal.2026-09-15T090000.01.log",
            ]
        );
    }
    use super::*;
    use std::io::Write;

    /// A truncated-and-reused journal file (Elite's 4.4.1.1 relaunch,
    /// field case 2026-09-05) is ARCHIVED, never deleted: the old
    /// incarnation's rows survive under a name that sorts before the
    /// original, the new content reads from byte zero, and history that
    /// exists only in the old incarnation — the Loadout that says which
    /// ship the commander flies — remains queryable. The delete this
    /// replaced erased a whole trading day.
    #[test]
    fn truncated_journal_file_archives_history_instead_of_deleting_it() {
        let dir = tempfile::tempdir().unwrap();
        let name = "Journal.2026-08-01T100000.01.log";
        let write = |body: &str| {
            let mut f = std::fs::File::create(dir.path().join(name)).unwrap();
            f.write_all(body.as_bytes()).unwrap();
        };
        write(concat!(
            r#"{"timestamp":"2026-08-01T10:00:00Z","event":"Loadout","Ship":"panthermkii","ShipID":28}"#,
            "\n",
            r#"{"timestamp":"2026-08-01T10:01:00Z","event":"Bounty","Target":"cobramkiii","TotalReward":1000,"VictimFaction":"Pirates"}"#,
            "\n",
        ));
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        ingest_file(&conn, dir.path(), name).unwrap();
        crate::derive::derive_incremental(&conn).unwrap();
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM combat_kills"), 1);

        // Same name, shorter content, NO fresh Loadout — the patch-relaunch
        // shape: a new session of nothing but chatter.
        write(concat!(
            r#"{"timestamp":"2026-08-01T11:00:00Z","event":"Undocked"}"#,
            "\n",
            r#"{"timestamp":"2026-08-01T11:00:05Z","event":"Music","MusicTrack":"Exploration"}"#,
            "\n",
        ));
        ingest_file(&conn, dir.path(), name).unwrap();
        let archived = "Journal.2026-08-01T100000.01.a.log";
        assert!(archived < name, "archive sorts before the original for chronological replay");
        assert_eq!(
            count("SELECT COUNT(*) FROM events WHERE file = 'Journal.2026-08-01T100000.01.a.log'"),
            2,
            "old incarnation archived, not deleted"
        );
        assert_eq!(
            count("SELECT COUNT(*) FROM events WHERE file = 'Journal.2026-08-01T100000.01.log'"),
            2,
            "new incarnation read from byte zero"
        );
        assert_eq!(count("SELECT COUNT(*) FROM combat_kills"), 1, "the bounty survives");
        let last_ship: String = conn
            .query_row(
                "SELECT json_extract(raw, '$.Ship') FROM events WHERE event = 'Loadout' ORDER BY ts DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(last_ship, "panthermkii", "the ship the commander flies is not forgotten");

        // A second reuse of the same name archives as .b. — no collision.
        write("{\"timestamp\":\"2026-08-01T12:00:00Z\",\"event\":\"Shutdown\"}\n");
        ingest_file(&conn, dir.path(), name).unwrap();
        assert_eq!(
            count("SELECT COUNT(*) FROM events WHERE file LIKE '%.b.log'"),
            2,
            "second truncation archives the previous incarnation under the next letter"
        );
    }

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    #[test]
    fn a_line_naming_a_second_fid_is_refused_from_that_line_on() {
        let dir = tempfile::tempdir().unwrap();
        let name = "Journal.2026-10-17T180648.01.log";
        std::fs::write(dir.path().join(name), concat!(
            r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#, "\n",
            r#"{"timestamp":"2026-10-17T18:06:56Z","event":"LoadGame","FID":"F0000001","Commander":"Alpha","Ship":"Mandalay","ShipID":31}"#, "\n",
            r#"{"timestamp":"2026-10-17T18:30:00Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#, "\n",
            r#"{"timestamp":"2026-10-17T18:30:01Z","event":"LoadGame","FID":"F0000002","Commander":"Beta","Ship":"Type9","ShipID":4}"#, "\n",
        )).unwrap();
        let conn = mem();
        let n = ingest_file_for(&conn, dir.path(), name, Some("F0000001")).unwrap();
        assert_eq!(n, 2, "the two lines before the foreign FID land");
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM events WHERE json_extract(raw,'$.FID') = 'F0000002'"), 0);
        // The checkpoint moved past the refused lines: a second pass reads nothing.
        assert_eq!(ingest_file_for(&conn, dir.path(), name, Some("F0000001")).unwrap(), 0);
    }

    #[test]
    fn without_an_expected_fid_every_line_lands_as_today() {
        let dir = tempfile::tempdir().unwrap();
        let name = "Journal.2026-10-17T180648.01.log";
        std::fs::write(dir.path().join(name), concat!(
            r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#, "\n",
            r#"{"timestamp":"2026-10-17T18:30:00Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#, "\n",
        )).unwrap();
        let conn = mem();
        assert_eq!(ingest_file(&conn, dir.path(), name).unwrap(), 2);
    }

    #[test]
    fn ingest_files_reads_only_the_names_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let a = "Journal.2026-10-17T180648.01.log";
        let b = "Journal.2026-10-17T181000.01.log";
        std::fs::write(dir.path().join(a), concat!(r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#, "\n")).unwrap();
        std::fs::write(dir.path().join(b), concat!(r#"{"timestamp":"2026-10-17T18:10:00Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#, "\n")).unwrap();
        let conn = mem();
        let stats = ingest_files(&conn, dir.path(), &[a.to_string()], Some("F0000001"), |_, _, _| {}).unwrap();
        assert_eq!((stats.files_scanned, stats.files_changed, stats.events_inserted, stats.snapshots_updated), (1, 1, 1, 0));
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(count("SELECT COUNT(*) FROM events WHERE file = 'Journal.2026-10-17T181000.01.log'"), 0);
    }

    #[test]
    fn a_snapshot_can_be_written_and_marked_unknown() {
        let conn = mem();
        write_snapshot(&conn, "Status.json", Some("2026-10-17T18:06:55Z"), Some(1), r#"{"Fuel":{"FuelMain":12.5}}"#).unwrap();
        let fuel: Option<f64> = conn.query_row("SELECT json_extract(raw,'$.Fuel.FuelMain') FROM snapshots WHERE name='Status.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(fuel, Some(12.5));
        mark_unknown(&conn, "Status.json").unwrap();
        let fuel: Option<f64> = conn.query_row("SELECT json_extract(raw,'$.Fuel.FuelMain') FROM snapshots WHERE name='Status.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(fuel, None, "an unknown snapshot reads as no value, never as the other commander's");
        let raw: String = conn.query_row("SELECT raw FROM snapshots WHERE name='Status.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(raw, UNKNOWN_SNAPSHOT);
        // Marking a name that was never written is fine and leaves a row.
        mark_unknown(&conn, "Cargo.json").unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM snapshots WHERE name='Cargo.json'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }
}
