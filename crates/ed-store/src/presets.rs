//! Pre-engineered variants the commander's data has shown: read off a
//! real Engineering block (a fitted module, an imported build) and kept,
//! so a saved plan that names one still resolves after a restart. The
//! preset itself is stored as the JSON `ed_ships::Preset` writes; this
//! crate does not know the type.

use anyhow::Result;
use rusqlite::Connection;

/// Keep a preset seen for the first time; a known id is left as it was.
pub fn remember(conn: &Connection, id: &str, item: &str, preset_json: &str, source: &str, ts: &str) -> Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO presets_seen (id, item, preset, source, first_seen) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![id, item, preset_json, source, ts],
    )?;
    Ok(n > 0)
}

/// Every preset seen, as (id, JSON), oldest first.
pub fn all(conn: &Connection) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare("SELECT id, preset FROM presets_seen ORDER BY first_seen, id")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_preset_seen_is_kept_once() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        assert!(remember(&conn, "seen:x:bp:1", "x", "{}", "import: EDSY", "2026-09-27T00:00:00Z").unwrap());
        assert!(!remember(&conn, "seen:x:bp:1", "x", "{\"other\":1}", "loadout", "2026-09-28T00:00:00Z").unwrap(), "the first sighting stands");
        assert_eq!(all(&conn).unwrap(), vec![("seen:x:bp:1".to_string(), "{}".to_string())]);
    }
}
