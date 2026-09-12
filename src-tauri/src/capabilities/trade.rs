//! What the journal contributes to a profit search: the ship, the
//! pledge, the origin. The request-to-constraints policy lives in
//! `ed_route::request`; the finder itself runs on the community API
//! (`remote_trade`).

use ed_route::request::{LoadoutShip, ProfitRequest};
use ed_store::query;
use rusqlite::Connection;
use serde_json::Value;

/// The ship as the latest `Loadout` event describes it, falling back to the
/// derived `loadout` row when the event is not in the log.
/// The ship a profit search plans for: a stored ship when the request
/// names one (`ship_id`, from the journal's latest Loadout for that
/// ShipID), else the one being flown. An id the journal never saw falls
/// back to the live ship and says so in the trace, so the UI's own list
/// is the only way to pick one.
pub fn ship_for(conn: &Connection, req: &ProfitRequest) -> LoadoutShip {
    if let Some(id) = req.ship_id {
        let raw: Option<String> = conn
            .query_row(
                "SELECT raw FROM events WHERE event = 'Loadout' AND json_extract(raw, '$.ShipID') = ?1 \
                 ORDER BY ts DESC LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .ok();
        if let Some(v) = raw.and_then(|raw| serde_json::from_str::<Value>(&raw).ok()) {
            return LoadoutShip::from_loadout(&v);
        }
        tracing::warn!(
            ship_id = id,
            "trade: no Loadout for that ship id; planning for the live ship"
        );
    }
    live_ship(conn)
}

/// Where a profit search starts: the request's system if given; else,
/// for a stored ship, where the journal says it is stored (maintainer,
/// 2026-09-07: "origin ... we can get that from them already when they
/// select which ship"); else the commander's current system.
pub fn origin_for(conn: &Connection, req: &ProfitRequest) -> Option<String> {
    if let Some(system) = req.system.clone().filter(|s| !s.trim().is_empty()) {
        return Some(system);
    }
    if let Some(id) = req.ship_id {
        let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        if let Some(system) = ed_store::ship_locations::locations(conn, &now)
            .ok()
            .and_then(|all| {
                all.into_iter()
                    .find(|l| l.ship_id == id)
                    .and_then(|l| l.system)
            })
        {
            return Some(system);
        }
    }
    query::location(conn)
        .ok()
        .flatten()
        .and_then(|l| l.system_name)
}

pub fn live_ship(conn: &Connection) -> LoadoutShip {
    if let Some(v) = ed_store::session::latest_event_raw(conn, "Loadout")
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    {
        return LoadoutShip::from_loadout(&v);
    }
    conn.query_row(
        "SELECT ship, cargo_capacity, max_jump_range, unladen_mass FROM loadout WHERE id = 1",
        [],
        |r| {
            Ok(LoadoutShip {
                hull: r
                    .get::<_, Option<String>>(0)?
                    .map(|s| s.to_ascii_lowercase()),
                cargo_capacity: r.get(1)?,
                max_jump_range: r.get(2)?,
                unladen_mass: r.get(3)?,
                fuel_main: None,
            })
        },
    )
    .unwrap_or_default()
}

/// The power the commander is pledged to, from the latest `Powerplay` event.
pub fn pledged_power(conn: &Connection) -> Option<String> {
    ed_store::session::latest_event_raw(conn, "Powerplay")
        .ok()
        .flatten()
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| v.get("Power").and_then(Value::as_str).map(str::to_string))
}
