//! The carrier as Frontier last reported it (the app's CAPI fetch), kept
//! so the card survives a restart and always says when it was fetched.
//! One row: a commander owns at most one carrier. Written and read only by
//! the app; the server never sees it.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::Value;

pub fn save(conn: &Connection, callsign: &str, fetched_at: &str, json: &Value) -> Result<()> {
    conn.execute("DELETE FROM carrier_capi", [])?;
    conn.execute(
        "INSERT INTO carrier_capi (callsign, fetched_at, json) VALUES (?1, ?2, ?3)",
        params![callsign, fetched_at, json.to_string()],
    )?;
    Ok(())
}

pub fn load(conn: &Connection) -> Result<Option<Value>> {
    let raw: Option<String> = conn
        .query_row("SELECT json FROM carrier_capi ORDER BY fetched_at DESC LIMIT 1", [], |r| r.get(0))
        .ok();
    Ok(raw.and_then(|s| serde_json::from_str(&s).ok()))
}

pub fn clear(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM carrier_capi", [])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_fetch_is_kept_and_cleared() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        assert_eq!(load(&conn).unwrap(), None);
        save(&conn, "Q6Z-66L", "2026-09-19T03:00:00Z", &serde_json::json!({"callsign": "Q6Z-66L", "hold_t": 4})).unwrap();
        save(&conn, "Q6Z-66L", "2026-09-19T03:20:00Z", &serde_json::json!({"callsign": "Q6Z-66L", "hold_t": 5})).unwrap();
        let v = load(&conn).unwrap().unwrap();
        assert_eq!(v["hold_t"], 5, "one row, the newest");
        let n: i64 = conn.query_row("SELECT count(*) FROM carrier_capi", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        clear(&conn).unwrap();
        assert_eq!(load(&conn).unwrap(), None);
    }
}
