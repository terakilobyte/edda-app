//! The commander's own state: journal-derived, first-hand.

use super::CapResult;
use crate::state::AppState;
use ed_store::query;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub fn engineers(state: &AppState) -> CapResult<Vec<query::Engineer>> {
    state.with_read(|s| Ok(query::engineers(s.conn())?))
}

pub fn module_types(state: &AppState) -> Vec<String> {
    state
        .engineering
        .module_types()
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// How far apart a sale and its merit event may be and still be paired.
pub const MERIT_MODEL_WINDOW_SECS: i64 = 5;

pub fn merit_model(state: &AppState) -> CapResult<ed_store::merits::MeritModel> {
    state.with_read(|s| {
        Ok(ed_store::merits::calibrate(
            s.conn(),
            MERIT_MODEL_WINDOW_SECS,
        )?)
    })
}

pub fn powerplay_seen(state: &AppState) -> CapResult<Vec<query::PowerplayState>> {
    state.with_read(|s| Ok(query::powerplay_all(s.conn())?))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CombatRequest {
    /// ISO-8601; all time when absent.
    pub since: Option<String>,
    /// `hour`, `day` or `week`.
    pub bucket: String,
}

impl Default for CombatRequest {
    fn default() -> Self {
        CombatRequest {
            since: None,
            bucket: "day".into(),
        }
    }
}

pub fn combat_summary(state: &AppState, req: &CombatRequest) -> CapResult<query::CombatSummary> {
    state.with_read(|s| Ok(query::combat_summary(s.conn(), req.since.as_deref())?))
}

pub fn combat_timeline(
    state: &AppState,
    req: &CombatRequest,
) -> CapResult<Vec<ed_store::session::CombatBucket>> {
    state.with_read(|s| {
        Ok(ed_store::session::combat_timeline(
            s.conn(),
            req.since.as_deref(),
            &req.bucket,
        )?)
    })
}

pub fn merit_timeline(
    state: &AppState,
    req: &CombatRequest,
) -> CapResult<Vec<ed_store::session::MeritBucket>> {
    state.with_read(|s| {
        Ok(ed_store::session::merit_timeline(
            s.conn(),
            req.since.as_deref(),
            &req.bucket,
        )?)
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RecentKillsRequest {
    pub limit: usize,
}

impl Default for RecentKillsRequest {
    fn default() -> Self {
        RecentKillsRequest { limit: 30 }
    }
}

pub fn recent_kills(
    state: &AppState,
    req: &RecentKillsRequest,
) -> CapResult<Vec<ed_store::session::KillRow>> {
    state.with_read(|s| Ok(ed_store::session::recent_kills(s.conn(), req.limit)?))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MissionsRequest {
    pub active_only: bool,
}

impl Default for MissionsRequest {
    fn default() -> Self {
        MissionsRequest { active_only: true }
    }
}

/// Missions and the `now` they were judged against.
pub fn missions(
    state: &AppState,
    req: &MissionsRequest,
) -> CapResult<(Vec<ed_store::missions::Mission>, String)> {
    let now = crate::commands::now_iso();
    state.with_read(|s| {
        let list = if req.active_only {
            ed_store::missions::active(s.conn(), &now)?
        } else {
            ed_store::missions::missions(s.conn(), "", &now)?
        };
        Ok((list, now))
    })
}

pub fn ranks(state: &AppState) -> CapResult<ed_store::session::Ranks> {
    state.with_read(|s| Ok(ed_store::session::ranks(s.conn())?))
}

/// Ship status as the model sees it: location, nav target, hull, cargo,
/// Powerplay for the current system.
pub fn status(state: &AppState) -> CapResult<Value> {
    state.with_read(|s| {
        let conn = s.conn();
        let location = query::location(conn)?;
        let nav = query::nav_target(conn)?;
        // The single current-ship source (swap-fed loadout row), shared
        // with the greeting and everything else.
        let current = ed_store::session::current_ship(conn)
            .ok()
            .flatten()
            .unwrap_or_default();
        let ship = current.symbol.clone();
        let ship_name = current.name.clone();
        let cargo_capacity: Option<i64> = conn
            .query_row("SELECT cargo_capacity FROM loadout WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap_or(None);
        let cargo_count: i64 = conn
            .query_row("SELECT COALESCE(SUM(count),0) FROM cargo", [], |r| r.get(0))
            .unwrap_or(0);
        let power = location
            .as_ref()
            .and_then(|l| l.system_name.as_deref())
            .and_then(|sys| query::powerplay_for_system(conn, sys).ok().flatten());
        let ship_display = ship.as_deref().map(ed_route::ships::display_name);
        Ok(json!({
            "location": location,
            "nav": nav,
            "ship": ship_display,
            "ship_symbol": ship,
            "ship_name": ship_name,
            "cargo_count": cargo_count,
            "cargo_capacity": cargo_capacity,
            "powerplay": power,
            "provenance": "journal",
        }))
    })
}

/// The commander's current system, from the latest journal location.
pub fn current_system_name(state: &AppState) -> Option<String> {
    state.with_read(|s| super::galaxy::current_system(s.conn()))
}

/// Materials and cargo held, with display names.
pub fn inventory(state: &AppState) -> CapResult<Vec<Value>> {
    state.with_read(|s| {
        let mut stmt = s.conn().prepare(
            "SELECT symbol, count, 'Material' FROM materials WHERE count > 0
             UNION ALL SELECT symbol, count, 'Cargo' FROM cargo WHERE count > 0",
        )?;
        let catalog = ed_journal::Catalog::load();
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?;
        Ok(rows
            .flatten()
            .map(|(symbol, count, kind)| {
                let item = catalog.by_symbol(&symbol);
                json!({
                    "name": item.map(|i| i.name.clone()).unwrap_or_else(|| format!("(unknown: {symbol})")),
                    "category": item.map(|i| i.category.clone()).unwrap_or(kind),
                    "count": count,
                })
            })
            .collect())
    })
}

/// Ship ids the commander owns now, replayed from shipyard events.
pub fn owned_ship_ids(state: &AppState) -> CapResult<std::collections::HashSet<i64>> {
    state.with_read(|s| Ok(ed_store::derive::owned_ships(s.conn())?))
}

/// A ship the commander owns or has owned, from its latest Loadout.
#[derive(Debug, Serialize)]
pub struct ShipSummary {
    pub ship_id: i64,
    pub ship: String,
    pub ship_name: Option<String>,
    pub ident: Option<String>,
    pub seen: String,
    pub unladen_mass: f64,
    pub max_jump_range: f64,
    pub cargo_capacity: i64,
    pub fuel_main: f64,
    pub hull_value: i64,
    pub modules_value: i64,
    pub rebuy: i64,
    pub current: bool,
    pub historical: bool,
    /// Item 53: where the ship sits (stored / in_transit / arrived /
    /// aboard_carrier), or `with_you` for the one being flown.
    pub location: Option<ed_store::ship_locations::ShipLocation>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ShipsListRequest {
    pub include_historical: bool,
}

/// Ships with a Loadout in the journal, currently owned by default.
pub fn ships_list(state: &AppState, req: &ShipsListRequest) -> CapResult<Vec<ShipSummary>> {
    let owned = owned_ship_ids(state)?;
    let current: Option<i64> = state
        .with_read(|s| {
            ed_store::session::latest_event_raw(s.conn(), "Loadout")
                .ok()
                .flatten()
        })
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| v.get("ShipID").and_then(Value::as_i64));
    let rows: Vec<String> = state.with_read(|s| -> CapResult<Vec<String>> {
        let mut st = s.conn().prepare(
            "SELECT raw FROM events WHERE event = 'Loadout' AND ts = (SELECT max(ts) FROM events e2 WHERE e2.event = 'Loadout' AND json_extract(e2.raw, '$.ShipID') = json_extract(events.raw, '$.ShipID')) ORDER BY ts DESC",
        )?;
        let rows: Vec<String> = st.query_map([], |r| r.get::<_, String>(0))?.flatten().collect();
        Ok(rows)
    })?;
    // Item 53: every stored ship's whereabouts, keyed by ShipID.
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut locations: std::collections::HashMap<i64, ed_store::ship_locations::ShipLocation> =
        state
            .with_read(|s| {
                Ok::<_, super::CapError>(ed_store::ship_locations::locations(s.conn(), &now)?)
            })?
            .into_iter()
            .map(|l| (l.ship_id, l))
            .collect();
    let mut out = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    for raw in rows {
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let Some(id) = v.get("ShipID").and_then(Value::as_i64) else {
            continue;
        };
        if !seen_ids.insert(id) {
            continue;
        }
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let i = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
        let historical = !owned.contains(&id);
        if historical && !req.include_historical {
            continue;
        }
        out.push(ShipSummary {
            ship_id: id,
            ship: s("Ship")
                .map(|t| ed_journal::ships::display_name_or(&t, s("Ship_Localised").as_deref()))
                .unwrap_or_default(),
            ship_name: s("ShipName")
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
            ident: s("ShipIdent")
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty()),
            seen: s("timestamp").unwrap_or_default(),
            unladen_mass: f("UnladenMass"),
            max_jump_range: f("MaxJumpRange"),
            cargo_capacity: i("CargoCapacity"),
            fuel_main: v
                .pointer("/FuelCapacity/Main")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            hull_value: i("HullValue"),
            modules_value: i("ModulesValue"),
            rebuy: i("Rebuy"),
            current: Some(id) == current,
            historical,
            location: if Some(id) == current {
                Some(ed_store::ship_locations::ShipLocation {
                    ship_id: id,
                    ship_type: s("Ship"),
                    name: s("ShipName"),
                    status: "with_you".into(),
                    system: None,
                    station: None,
                    market_id: None,
                    carrier: None,
                    arrival: None,
                    minutes_to_arrival: None,
                    as_of: now.clone(),
                    age_hours: 0.0,
                })
            } else {
                locations.remove(&id)
            },
        });
    }
    Ok(out)
}
