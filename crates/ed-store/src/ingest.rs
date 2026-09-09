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

/// `Journal.*.log` names in the folder, oldest first. The timestamped names
/// sort chronologically as plain strings, which is what makes ordering by
/// `(file, offset)` a chronological replay.
pub fn journal_file_names(dir: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .with_context(|| format!("reading journal dir {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("Journal.") && n.ends_with(".log"))
        .collect();
    names.sort();
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
pub fn ingest_file(conn: &Connection, dir: &Path, name: &str) -> Result<u64> {
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

        tx.execute(
            "INSERT INTO snapshots (name, ts, mtime, raw) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(name) DO UPDATE SET
                 ts = excluded.ts, mtime = excluded.mtime, raw = excluded.raw",
            params![name, ts, mtime, raw],
        )?;
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
    mut progress: impl FnMut(&str, usize, usize),
) -> Result<IngestStats> {
    let names = journal_file_names(dir)?;
    let total = names.len();
    let mut stats = IngestStats {
        files_scanned: total,
        ..Default::default()
    };

    for (i, name) in names.iter().enumerate() {
        progress(name, i + 1, total);
        let n = ingest_file(conn, dir, name)?;
        if n > 0 {
            stats.files_changed += 1;
            stats.events_inserted += n;
        }
    }

    stats.snapshots_updated = ingest_companions(conn, dir)?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
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
}
