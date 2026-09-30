//! Module prices the commander has SEEN, per station: the game's own
//! `Outfitting.json` at every dock, which — unlike the community feed's
//! v2 boards — prices each entry in credits and in merc coins and keys it
//! by FDev id. A pre-engineered merc-coin variant shares the plain
//! module's symbol (Balanced Power Distributor is
//! `int_powerdistributor_size5_class5`, 500 merc coins, 0 credits), so
//! this is the only exact source for "can I buy this here for credits"
//! at the stations the commander has visited (maintainer, 2026-09-29:
//! "if a player doesn't have merc coin we shouldn't show the result").
//! One row per (station, symbol): the cheapest credit price (0 = none),
//! the cheapest merc price (0 = none), the merc entries' ids.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenPrice {
    pub market_id: i64,
    pub symbol: String,
    pub credits: i64,
    pub merc: i64,
    pub variant_ids: Vec<i64>,
    pub ts: String,
}

/// Fold one Outfitting.json board into the table, replacing what was
/// known for that station. Idempotent on the board's timestamp: the
/// snapshot table is rewritten on every companion-file change, and
/// Status.json changes several times a second.
pub fn record(conn: &Connection, board: &Value) -> Result<usize> {
    let Some(market_id) = board.get("MarketID").and_then(Value::as_i64) else {
        return Ok(0);
    };
    let ts = board.get("timestamp").and_then(Value::as_str).unwrap_or_default().to_owned();
    let known: Option<String> = conn
        .query_row("SELECT MAX(ts) FROM outfitting_seen WHERE market_id = ?1", [market_id], |r| r.get(0))
        .ok()
        .flatten();
    if known.as_deref() == Some(ts.as_str()) {
        return Ok(0);
    }
    let mut by: std::collections::BTreeMap<String, (i64, i64, Vec<i64>)> = std::collections::BTreeMap::new();
    for item in board.get("Items").and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = item.get("Name").and_then(Value::as_str) else { continue };
        let credits = item.get("BuyPrice").and_then(Value::as_i64).unwrap_or(0);
        let merc = item.get("BuyMercCoinsPrice").and_then(Value::as_i64).unwrap_or(0);
        let e = by.entry(name.trim().to_ascii_lowercase()).or_insert((0, 0, Vec::new()));
        if credits > 0 && (e.0 == 0 || credits < e.0) {
            e.0 = credits;
        }
        if merc > 0 {
            if e.1 == 0 || merc < e.1 {
                e.1 = merc;
            }
            if let Some(id) = item.get("id").and_then(Value::as_i64) {
                e.2.push(id);
            }
        }
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM outfitting_seen WHERE market_id = ?1", [market_id])?;
    let mut ins = tx.prepare("INSERT INTO outfitting_seen (market_id, symbol, credits, merc, variant_ids, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")?;
    for (symbol, (credits, merc, ids)) in &by {
        let ids = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        ins.execute(params![market_id, symbol, credits, merc, if ids.is_empty() { None } else { Some(ids) }, ts])?;
    }
    drop(ins);
    tx.commit()?;
    Ok(by.len())
}

/// The latest Outfitting.json snapshot, recorded. Cheap when it was seen
/// already; the watcher calls it after every companion-file change.
pub fn record_latest(conn: &Connection) -> Result<usize> {
    let Some(raw) = crate::session::snapshot_raw(conn, "Outfitting.json")? else {
        return Ok(0);
    };
    let Ok(board) = serde_json::from_str::<Value>(&raw) else {
        return Ok(0);
    };
    record(conn, &board)
}

/// What the commander has seen at these stations (market ids).
pub fn for_markets(conn: &Connection, market_ids: &[i64]) -> Result<Vec<SeenPrice>> {
    if market_ids.is_empty() {
        return Ok(Vec::new());
    }
    let list = market_ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT market_id, symbol, credits, merc, variant_ids, ts FROM outfitting_seen WHERE market_id IN ({list})"
    ))?;
    let rows = stmt.query_map([], |r| {
        let ids: Option<String> = r.get(4)?;
        Ok(SeenPrice {
            market_id: r.get(0)?,
            symbol: r.get(1)?,
            credits: r.get(2)?,
            merc: r.get(3)?,
            variant_ids: ids.map(|s| s.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or_default(),
            ts: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    /// Omega Prospect, Merope, 2026-09-30: the plain and the merc-coin
    /// entry under one symbol fold to one row that says both prices; a
    /// merc-only symbol says 0 credits; the same board twice is one write.
    #[test]
    fn a_board_folds_to_one_row_per_symbol_with_both_prices() {
        let conn = db();
        let board = serde_json::json!({
            "timestamp": "2026-09-30T04:25:28Z", "MarketID": 128797355, "StationName": "Omega Prospect", "StarSystem": "Merope",
            "Items": [
                {"id": 128671335, "Name": "int_powerdistributor_size5_class5", "BuyPrice": 1591740},
                {"id": 129044376, "Name": "int_powerdistributor_size5_class5", "BuyPrice": 0, "BuyMercCoinsPrice": 500},
                {"id": 129044375, "Name": "hpt_railgun_fixed_medium", "BuyPrice": 0, "BuyMercCoinsPrice": 950},
                {"id": 128049381, "Name": "int_cargorack_size1_class1", "BuyPrice": 1000}
            ]
        });
        assert_eq!(record(&conn, &board).unwrap(), 3);
        assert_eq!(record(&conn, &board).unwrap(), 0, "same timestamp: nothing to do");
        let seen = for_markets(&conn, &[128797355, 1]).unwrap();
        let pd = seen.iter().find(|s| s.symbol == "int_powerdistributor_size5_class5").unwrap();
        assert_eq!((pd.credits, pd.merc, pd.variant_ids.clone()), (1_591_740, 500, vec![129044376]));
        let rail = seen.iter().find(|s| s.symbol == "hpt_railgun_fixed_medium").unwrap();
        assert_eq!((rail.credits, rail.merc), (0, 950), "merc coins only");
        let rack = seen.iter().find(|s| s.symbol == "int_cargorack_size1_class1").unwrap();
        assert_eq!((rack.credits, rack.merc), (1000, 0));
        assert!(for_markets(&conn, &[]).unwrap().is_empty());
    }
}
