//! The material trader tab and its callout.
//!
//! The arithmetic and the planner live in `ed_journal::mat_trade`; this is
//! the app side: the commander's counts from the store, which trader type
//! to plan for (the docked station's if we have traded there, else its
//! economy, else the type with the most to spend), the nearest traders of
//! that type, and the one-line suggestion the voice makes when a pickup
//! takes a material over the source threshold and a trade exists for it
//! (boss, 2026-10-04: "smart enough not to prompt if there isn't a trade").

use crate::state::AppState;
use ed_journal::mat_trade::{self, FillOrder, Policy};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use tauri::State;

/// Lowercase symbol -> count, everything the commander holds.
pub fn inventory(conn: &rusqlite::Connection) -> HashMap<String, i64> {
    let mut out = HashMap::new();
    if let Ok(mut st) = conn.prepare("SELECT symbol, count FROM materials") {
        if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
            for (s, n) in rows.flatten() {
                out.insert(s.to_ascii_lowercase(), n);
            }
        }
    }
    out
}

/// The trader type of the station we are docked at, when the journal can
/// tell: a `MaterialTrade` we made at this MarketID, else the station's
/// economy (Extraction/Refinery raw, Industrial manufactured, High Tech /
/// Military encoded -- the same split `nearest_material_traders` uses).
pub fn docked_trader_kind(conn: &rusqlite::Connection) -> Option<(String, &'static str)> {
    let raw: String = conn
        .query_row("SELECT raw FROM events WHERE event = 'Docked' ORDER BY ts DESC, file DESC, offset DESC LIMIT 1", [], |r| r.get(0))
        .ok()?;
    let undocked: Option<String> = conn
        .query_row("SELECT ts FROM events WHERE event = 'Undocked' ORDER BY ts DESC, file DESC, offset DESC LIMIT 1", [], |r| r.get(0))
        .ok();
    let v: Value = serde_json::from_str(&raw).ok()?;
    let ts = v.get("timestamp").and_then(Value::as_str).unwrap_or("");
    if undocked.as_deref().is_some_and(|u| u > ts) {
        return None;
    }
    let services = v.get("StationServices").and_then(Value::as_array)?;
    if !services.iter().any(|s| s.as_str().is_some_and(|s| s.eq_ignore_ascii_case("materialtrader"))) {
        return None;
    }
    let market = v.get("MarketID").and_then(Value::as_i64)?;
    if let Ok(kind) = conn.query_row(
        "SELECT json_extract(raw, '$.TraderType') FROM events WHERE event = 'MaterialTrade' AND json_extract(raw, '$.MarketID') = ?1 ORDER BY ts DESC LIMIT 1",
        [market],
        |r| r.get::<_, String>(0),
    ) {
        return Some((kind.to_ascii_lowercase(), "traded here before"));
    }
    let economy = v.get("StationEconomy_Localised").or_else(|| v.get("StationEconomy")).and_then(Value::as_str).unwrap_or("");
    let e = economy.to_ascii_lowercase();
    let kind = if e.contains("extraction") || e.contains("refinery") {
        "raw"
    } else if e.contains("industrial") {
        "manufactured"
    } else if e.contains("high tech") || e.contains("hightech") || e.contains("military") {
        "encoded"
    } else {
        return None;
    };
    Some((kind.to_string(), "station economy"))
}

/// The type with the most to spend: the one whose sources' surplus above
/// the floor is largest.
pub fn busiest_kind(catalog: &ed_journal::Catalog, inv: &HashMap<String, i64>, policy: &Policy) -> &'static str {
    mat_trade::KINDS
        .iter()
        .map(|k| {
            let p = mat_trade::plan(catalog, inv, k, policy, None);
            let surplus: i64 = p.sources.iter().map(|l| l.count - (l.cap as f64 * policy.floor).floor() as i64).sum();
            (*k, surplus, p.trades.len())
        })
        .max_by_key(|(_, s, n)| (*n > 0, *s))
        .map(|(k, _, _)| k)
        .unwrap_or("manufactured")
}

#[derive(Debug, Serialize)]
pub struct TraderView {
    pub kind: String,
    /// Why this kind: "chosen", "traded here before", "station economy", "most to spend".
    pub kind_from: &'static str,
    pub docked_kind: Option<String>,
    pub plan: mat_trade::Plan,
    pub traders: Vec<ed_store::lookup::StationWithService>,
    pub traders_known: bool,
    pub origin_system: Option<String>,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // the tab's knobs, one argument each
pub async fn material_trade_plan(
    state: State<'_, AppState>,
    kind: Option<String>,
    source_min: Option<f64>,
    floor: Option<f64>,
    cross: Option<bool>,
    up: Option<bool>,
    order: Option<String>,
    min_source_grade: Option<u8>,
    room_below: Option<f64>,
) -> Result<TraderView, String> {
    let policy = Policy {
        min_source_grade: min_source_grade.unwrap_or(4).clamp(1, 5),
        // The tab sends 0 for "no room pass".
        room_below: room_below.filter(|r| *r > 0.0).map(|r| r.clamp(0.1, 1.0)),
        order: match order.as_deref().map(str::trim) {
            Some("nearest_first") | Some("nearest") => FillOrder::NearestFirst,
            _ => FillOrder::BottomFirst,
        },
        source_min: source_min.unwrap_or(0.9).clamp(0.1, 1.0),
        floor: floor.unwrap_or(0.5).clamp(0.0, 1.0),
        cross: cross.unwrap_or(false),
        up: up.unwrap_or(false),
    };
    let catalog = ed_journal::Catalog::load();
    let (inv, docked, system) = state.with_read(|s| {
        let conn = s.conn();
        (inventory(conn), docked_trader_kind(conn), ed_store::query::location(conn).ok().flatten().and_then(|l| l.system_name))
    });
    let (kind, kind_from) = match kind.map(|k| k.trim().to_ascii_lowercase()).filter(|k| mat_trade::KINDS.contains(&k.as_str())) {
        Some(k) => (k, "chosen"),
        None => match &docked {
            Some((k, why)) => (k.clone(), *why),
            None => (busiest_kind(&catalog, &inv, &policy).to_string(), "most to spend"),
        },
    };
    let plan = mat_trade::plan(&catalog, &inv, &kind, &policy, None);
    tracing::info!(kind = %kind, kind_from, sources = plan.sources.len(), trades = plan.trades.len(), "material trade plan");
    let (traders, traders_known) = match &system {
        Some(sys) => match crate::remote_lookup::nearest_material_traders_all(&state, sys, 300.0).await {
            Some(hits) => {
                let split = crate::remote_lookup::traders_of_kind(&hits, &kind, 5);
                (split.stations, split.kind_known)
            }
            None => (Vec::new(), false),
        },
        None => (Vec::new(), false),
    };
    Ok(TraderView { kind, kind_from, docked_kind: docked.map(|(k, _)| k), plan, traders, traders_known, origin_system: system })
}

/// One cell of the Inventory tab's grid: every catalogued material, held
/// or not, with what the game's trader screen shows about it.
#[derive(Debug, Serialize)]
pub struct MaterialCell {
    pub symbol: String,
    pub name: String,
    /// "raw", "manufactured" or "encoded".
    pub kind: String,
    /// Trader group ("Thermic", or "4" for a raw category); None for the
    /// Guardian and Thargoid materials no trader takes.
    pub group: Option<String>,
    pub grade: u8,
    pub cap: i64,
    pub count: i64,
}

/// When the game last stated every count itself: the `Materials` snapshot
/// it writes at login. Everything after that is EDDA's arithmetic on the
/// events the game chooses to write -- and a reward collected from a
/// mission shared by a wingmate writes none (boss, 2026-10-06: three such
/// turn-ins, each leaving only a ShipLocker line), so until the next login
/// those are invisible.
pub fn verified_at(conn: &rusqlite::Connection) -> Option<String> {
    conn.query_row("SELECT ts FROM events WHERE event = 'Materials' ORDER BY ts DESC, file DESC, offset DESC LIMIT 1", [], |r| r.get(0)).ok()
}

#[derive(Debug, Serialize)]
pub struct MaterialGrid {
    pub cells: Vec<MaterialCell>,
    /// Timestamp of the game's last `Materials` snapshot, if any.
    pub verified_at: Option<String>,
}

/// The whole material inventory, zeros included, laid out for the grid,
/// with when the game last vouched for the counts.
#[tauri::command]
pub async fn material_grid(state: State<'_, AppState>) -> Result<MaterialGrid, String> {
    let catalog = ed_journal::Catalog::load();
    let (inv, verified_at) = state.with_read(|s| (inventory(s.conn()), verified_at(s.conn())));
    let mut out: Vec<MaterialCell> = catalog
        .materials()
        .map(|i| MaterialCell {
            symbol: i.symbol.to_lowercase(),
            name: i.name.clone(),
            kind: i.category.to_ascii_lowercase(),
            group: Some(i.group.clone()).filter(|g| !g.is_empty() && g != "None"),
            grade: i.grade,
            cap: mat_trade::cap(i.grade),
            count: inv.get(&i.symbol.to_lowercase()).copied().unwrap_or(0),
        })
        .collect();
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.group.cmp(&b.group)).then(a.grade.cmp(&b.grade)).then(a.name.cmp(&b.name)));
    Ok(MaterialGrid { cells: out, verified_at })
}

/// The callout's question after a pickup: did `symbol` just cross the
/// source threshold, and is there a trade for it? One sentence when both,
/// else None. `before` is the count before this pickup.
pub fn suggestion(conn: &rusqlite::Connection, symbol: &str, before: i64, now: i64) -> Option<String> {
    let catalog = ed_journal::Catalog::load();
    let item = catalog.by_symbol(symbol)?;
    if !mat_trade::tradeable(item) {
        return None;
    }
    let policy = Policy::default();
    let threshold = (mat_trade::cap(item.grade) as f64 * policy.source_min).ceil() as i64;
    if !(before < threshold && now >= threshold) {
        return None;
    }
    let p = mat_trade::plan(&catalog, &inventory(conn), &item.category.to_ascii_lowercase(), &policy, Some(&item.symbol));
    if p.trades.is_empty() {
        return None;
    }
    let direction = p.trades[0].direction;
    let purpose = if direction == "room" { " to make room" } else { "" };
    let direction = if direction == "room" { "" } else { direction };
    let mut names: Vec<String> = Vec::new();
    for t in &p.trades {
        if !names.contains(&t.recv_name) {
            names.push(t.recv_name.clone());
        }
        if names.len() == 2 {
            break;
        }
    }
    let into = match names.len() {
        1 => names[0].clone(),
        _ => format!("{} and {}", names[0], names[1]),
    };
    let more = if p.trades.len() > names.len() { ", among others" } else { "" };
    Some(format!(
        "{} is nearly full, {now} of {}. A {} trader would trade the surplus {direction}{}into {into}{more}{purpose}.",
        item.name,
        mat_trade::cap(item.grade),
        item.category.to_ascii_lowercase(),
        if direction.is_empty() { "" } else { " " }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn_with(materials: &[(&str, i64)], docked: Option<&str>, trades: &[(i64, &str)]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        for (s, n) in materials {
            conn.execute("INSERT INTO materials (symbol, count) VALUES (?1, ?2)", rusqlite::params![s, n]).unwrap();
        }
        let mut off = 1;
        if let Some(raw) = docked {
            conn.execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('J', ?1, '2026-10-04T12:00:00Z', 'Docked', ?2)", rusqlite::params![off, raw]).unwrap();
            off += 1;
        }
        for (market, kind) in trades {
            let raw = format!("{{\"timestamp\":\"2026-09-01T00:00:00Z\",\"event\":\"MaterialTrade\",\"MarketID\":{market},\"TraderType\":\"{kind}\",\"Paid\":{{\"Material\":\"iron\",\"Quantity\":6}},\"Received\":{{\"Material\":\"zinc\",\"Quantity\":1}}}}");
            conn.execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('J', ?1, '2026-09-01T00:00:00Z', 'MaterialTrade', ?2)", rusqlite::params![off, raw]).unwrap();
            off += 1;
        }
        conn
    }

    /// Docked where we traded before: that trader's type. Docked somewhere
    /// with a trader and no history: the economy. No trader: nothing.
    #[test]
    fn the_docked_station_names_its_trader_type() {
        let docked = r#"{"timestamp":"2026-10-04T12:00:00Z","event":"Docked","StationName":"X","MarketID":3223843584,"StationEconomy":"$economy_Industrial;","StationEconomy_Localised":"Industrial","StationServices":["dock","materialtrader"]}"#;
        let c = conn_with(&[], Some(docked), &[(3223843584, "manufactured")]);
        assert_eq!(docked_trader_kind(&c), Some(("manufactured".into(), "traded here before")));
        let c = conn_with(&[], Some(docked), &[]);
        assert_eq!(docked_trader_kind(&c), Some(("manufactured".into(), "station economy")));
        let no_trader = docked.replace(r#","materialtrader""#, "");
        let c = conn_with(&[], Some(&no_trader), &[]);
        assert_eq!(docked_trader_kind(&c), None);
    }

    #[test]
    fn verified_at_is_the_games_last_materials_snapshot() {
        let c = conn_with(&[], None, &[]);
        assert_eq!(verified_at(&c), None);
        c.execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('J', 50, '2026-10-06T00:17:33Z', 'Materials', '{}')", []).unwrap();
        c.execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('J', 51, '2026-10-06T03:47:47Z', 'Materials', '{}')", []).unwrap();
        c.execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('J', 52, '2026-10-06T04:53:32Z', 'ShipLocker', '{}')", []).unwrap();
        assert_eq!(verified_at(&c).as_deref(), Some("2026-10-06T03:47:47Z"), "the latest login snapshot, not a later unrelated event");
    }

    /// The prompt fires once, on the pickup that crosses the threshold, and
    /// only when the plan has a trade for that material.
    #[test]
    fn the_suggestion_speaks_once_at_the_threshold_and_only_with_a_trade() {
        let c = conn_with(&[("militarygradealloys", 90), ("thermicalloys", 20)], None, &[]);
        assert_eq!(
            suggestion(&c, "militarygradealloys", 89, 90).as_deref(),
            Some("Military Grade Alloys is nearly full, 90 of 100. A manufactured trader would trade the surplus down into Tempered Alloys and Heat Resistant Ceramics, among others.")
        );
        assert!(suggestion(&c, "militarygradealloys", 90, 91).is_none(), "already over: said once");
        assert!(suggestion(&c, "militarygradealloys", 80, 85).is_none(), "not there yet");
        let full = conn_with(&[("militarygradealloys", 90), ("thermicalloys", 150), ("precipitatedalloys", 200), ("heatresistantceramics", 250), ("temperedalloys", 300)], None, &[]);
        // Everything below is full and cross is off by default, but the room
        // pass finds G4/G5 elsewhere with room: 90 is over the 85 ceiling.
        let s = suggestion(&full, "militarygradealloys", 89, 90).unwrap();
        assert!(s.ends_with("to make room.") && s.contains("would trade the surplus into"), "{s}");
        assert!(suggestion(&c, "guardian_powerconduit", 0, 100).is_none(), "untradeable");
    }
}
