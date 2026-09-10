//! The commander's fleet carrier, from the journal alone (Item 52 A).
//!
//! Every `Carrier*` line was already in `events`, ingested raw and never
//! read. This module replays them into `carriers` and `carrier_hold` on
//! every derive pass — a few hundred rows at most, so a full rebuild is
//! cheaper than being clever. Every value carries the timestamp it was
//! true at, and `status` reports the age, because `CarrierStats` only
//! lands when the commander opens Carrier Management.
//!
//! Rules that came from the fixtures (census, 2026-09-06):
//! - Never read `CarrierType` from `CarrierNameChange`: the game writes
//!   it under an EMPTY-STRING key. The type rides `CarrierBuy`,
//!   `CarrierStats`, `CarrierLocation`, `CarrierJumpRequest`.
//! - The stable key is `CarrierID`, which equals the `MarketID` on
//!   `Docked`/`CarrierJump`. Names change; callsigns and ids do not.
//! - Ownership: `CarrierBuy` seen, or `CarrierStats` with
//!   `CarrierType == "FleetCarrier"` (a squadron officer opening the
//!   squadron carrier's management writes `SquadronCarrier`).
//! - `CargoTransfer` carries no carrier id: it is attributed to the
//!   single owned carrier and skipped when there is none or several.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

/// The events the rebuild reads, in one place so `derive::DERIVED_FROM`
/// can include them for incremental passes.
pub const EVENTS: &[&str] = &[
    "CarrierBuy",
    "CarrierStats",
    "CarrierNameChange",
    "CarrierJumpRequest",
    "CarrierJumpCancelled",
    "CarrierJump",
    "CarrierLocation",
    "CarrierDepositFuel",
    "CarrierCrewServices",
    "CarrierDecommission",
    "CargoTransfer",
];

#[derive(Debug, Default, Clone)]
struct Row {
    carrier_type: Option<String>,
    callsign: Option<String>,
    name: Option<String>,
    owned: bool,
    decommissioned: bool,
    system_name: Option<String>,
    system_address: Option<i64>,
    body: Option<String>,
    location_ts: Option<String>,
    fuel_t: Option<i64>,
    fuel_ts: Option<String>,
    capacity_total: Option<i64>,
    capacity_used: Option<i64>,
    free_space: Option<i64>,
    stats_ts: Option<String>,
    jump_range_curr: Option<f64>,
    jump_range_max: Option<f64>,
    docking_access: Option<String>,
    balance_cr: Option<i64>,
    services: Vec<String>,
    pending_jump_system: Option<String>,
    pending_jump_body: Option<String>,
    pending_departure: Option<String>,
    pending_jump_ts: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}
fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}
fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

/// Replay every carrier event into the two tables. Idempotent.
pub fn rebuild(conn: &Connection) -> Result<usize> {
    let placeholders = EVENTS.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT ts, event, raw FROM events WHERE event IN ({placeholders}) ORDER BY file, offset"
    ))?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map(rusqlite::params_from_iter(EVENTS.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut carriers: HashMap<i64, Row> = HashMap::new();
    // (carrier, commodity) -> (count, ts)
    let mut hold: HashMap<(i64, String), (i64, String)> = HashMap::new();

    for (ts, event, raw) in rows {
        let Ok(v) = serde_json::from_str::<Value>(&raw) else { continue };
        if event == "CargoTransfer" {
            // No carrier id on the event: the single owned carrier, or nothing.
            let owned: Vec<i64> = carriers.iter().filter(|(_, r)| r.owned && !r.decommissioned).map(|(id, _)| *id).collect();
            let [carrier_id] = owned[..] else { continue };
            for t in v.get("Transfers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
                let Some(kind) = s(t, "Type") else { continue };
                let count = i(t, "Count").unwrap_or(0);
                let delta = match s(t, "Direction").as_deref() {
                    Some("tocarrier") => count,
                    Some("toship") | Some("tosrv") => -count,
                    _ => continue,
                };
                let entry = hold.entry((carrier_id, kind.to_lowercase())).or_insert((0, ts.clone()));
                entry.0 = (entry.0 + delta).max(0);
                entry.1 = ts.clone();
            }
            continue;
        }
        // CarrierJump names the carrier by MarketID only when the commander
        // is docked aboard; the undocked-aboard form carries neither
        // CarrierID nor MarketID (field case 2026-09-09: a jump that never
        // cleared, "departs in -1350 min" a day later). Attribute it to the
        // carrier whose pending jump this completes, else to the one owned
        // fleet carrier.
        let id = i(&v, "CarrierID")
            .or_else(|| (event == "CarrierJump").then(|| i(&v, "MarketID")).flatten())
            .or_else(|| {
                if event != "CarrierJump" {
                    return None;
                }
                let star = s(&v, "StarSystem")?;
                let by_pending = carriers
                    .iter()
                    .find(|(_, r)| r.pending_jump_system.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(&star)))
                    .map(|(id, _)| *id);
                by_pending.or_else(|| {
                    let owned: Vec<i64> = carriers.iter().filter(|(_, r)| r.owned).map(|(id, _)| *id).collect();
                    (owned.len() == 1).then(|| owned[0])
                })
            });
        let Some(id) = id else { continue };
        let row = carriers.entry(id).or_default();
        // The type rides every carrier event EXCEPT CarrierNameChange,
        // whose key is "" (Frontier bug, census 2026-09-06).
        if event != "CarrierNameChange" {
            if let Some(t) = s(&v, "CarrierType") {
                row.carrier_type = Some(t);
            }
        }
        match event.as_str() {
            "CarrierBuy" => {
                row.owned = true;
                row.callsign = s(&v, "Callsign").or(row.callsign.take());
                row.system_name = s(&v, "Location").or(row.system_name.take());
                row.system_address = i(&v, "SystemAddress").or(row.system_address);
                row.location_ts = Some(ts.clone());
            }
            "CarrierStats" => {
                if row.carrier_type.as_deref() == Some("FleetCarrier") {
                    row.owned = true;
                }
                row.callsign = s(&v, "Callsign").or(row.callsign.take());
                row.name = s(&v, "Name").or(row.name.take());
                row.docking_access = s(&v, "DockingAccess").or(row.docking_access.take());
                row.jump_range_curr = f(&v, "JumpRangeCurr").or(row.jump_range_curr);
                row.jump_range_max = f(&v, "JumpRangeMax").or(row.jump_range_max);
                if let Some(fuel) = i(&v, "FuelLevel") {
                    row.fuel_t = Some(fuel);
                    row.fuel_ts = Some(ts.clone());
                }
                if let Some(space) = v.get("SpaceUsage") {
                    row.capacity_total = i(space, "TotalCapacity");
                    row.free_space = i(space, "FreeSpace");
                    row.capacity_used = match (row.capacity_total, row.free_space) {
                        (Some(t), Some(fr)) => Some((t - fr).max(0)),
                        _ => None,
                    };
                }
                if let Some(fin) = v.get("Finance") {
                    row.balance_cr = i(fin, "CarrierBalance").or(row.balance_cr);
                }
                if let Some(crew) = v.get("Crew").and_then(Value::as_array) {
                    row.services = crew
                        .iter()
                        .filter(|c| c.get("Activated").and_then(Value::as_bool).unwrap_or(true))
                        .filter_map(|c| s(c, "CrewRole"))
                        .collect();
                }
                row.stats_ts = Some(ts.clone());
            }
            "CarrierNameChange" => {
                row.name = s(&v, "Name").or(row.name.take());
                row.callsign = s(&v, "Callsign").or(row.callsign.take());
            }
            "CarrierJumpRequest" => {
                row.pending_jump_system = s(&v, "SystemName");
                row.pending_jump_body = s(&v, "Body");
                row.pending_departure = s(&v, "DepartureTime");
                row.pending_jump_ts = Some(ts.clone());
            }
            "CarrierJumpCancelled" => {
                row.pending_jump_system = None;
                row.pending_jump_body = None;
                row.pending_departure = None;
                row.pending_jump_ts = None;
            }
            "CarrierJump" | "CarrierLocation" => {
                row.system_name = s(&v, "StarSystem").or(row.system_name.take());
                row.system_address = i(&v, "SystemAddress").or(row.system_address);
                row.body = s(&v, "Body").or(row.body.take());
                row.location_ts = Some(ts.clone());
                // A jump is over when the journal says so, OR when a later
                // location heartbeat finds the carrier AT the pending
                // destination after its departure time - the CarrierJump
                // event is not written when the commander is elsewhere.
                let arrived_by_heartbeat = event == "CarrierLocation"
                    && row.pending_jump_system.as_deref().zip(row.system_name.as_deref()).is_some_and(|(p, here)| p.eq_ignore_ascii_case(here))
                    && row.pending_departure.as_deref().is_none_or(|d| ts.as_str() >= d);
                if event == "CarrierJump" || arrived_by_heartbeat {
                    row.pending_jump_system = None;
                    row.pending_jump_body = None;
                    row.pending_departure = None;
                    row.pending_jump_ts = None;
                }
            }
            "CarrierDepositFuel" => {
                if let Some(total) = i(&v, "Total") {
                    if row.fuel_ts.as_deref().is_none_or(|prev| ts.as_str() >= prev) {
                        row.fuel_t = Some(total);
                        row.fuel_ts = Some(ts.clone());
                    }
                }
            }
            "CarrierCrewServices" => {
                if let (Some(role), Some(op)) = (s(&v, "CrewRole"), s(&v, "Operation")) {
                    match op.as_str() {
                        "Activate" | "Replace" => {
                            if !row.services.contains(&role) {
                                row.services.push(role);
                            }
                        }
                        "Deactivate" => row.services.retain(|r| r != &role),
                        _ => {}
                    }
                }
            }
            "CarrierDecommission" => {
                row.decommissioned = true;
            }
            _ => {}
        }
    }

    conn.execute("DELETE FROM carriers", [])?;
    conn.execute("DELETE FROM carrier_hold", [])?;
    let mut ins = conn.prepare(
        "INSERT INTO carriers (carrier_id, carrier_type, callsign, name, owned, decommissioned,
             system_name, system_address, body, location_ts, fuel_t, fuel_ts,
             capacity_total, capacity_used, free_space, stats_ts, jump_range_curr, jump_range_max,
             docking_access, balance_cr, services,
             pending_jump_system, pending_jump_body, pending_departure, pending_jump_ts)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25)",
    )?;
    for (id, r) in &carriers {
        ins.execute(params![
            id,
            r.carrier_type,
            r.callsign,
            r.name,
            r.owned as i64,
            r.decommissioned as i64,
            r.system_name,
            r.system_address,
            r.body,
            r.location_ts,
            r.fuel_t,
            r.fuel_ts,
            r.capacity_total,
            r.capacity_used,
            r.free_space,
            r.stats_ts,
            r.jump_range_curr,
            r.jump_range_max,
            r.docking_access,
            r.balance_cr,
            serde_json::to_string(&r.services)?,
            r.pending_jump_system,
            r.pending_jump_body,
            r.pending_departure,
            r.pending_jump_ts,
        ])?;
    }
    let mut ins_hold = conn.prepare("INSERT INTO carrier_hold (carrier_id, commodity, count, ts) VALUES (?1,?2,?3,?4)")?;
    for ((carrier_id, commodity), (count, ts)) in &hold {
        if *count > 0 {
            ins_hold.execute(params![carrier_id, commodity, count, ts])?;
        }
    }
    Ok(carriers.len())
}

/// A value with the time it was true at and its age from `now`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Aged<T: Serialize> {
    pub value: T,
    pub as_of: String,
    pub age_hours: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HoldLine {
    pub commodity: String,
    pub tons: i64,
    pub as_of: String,
    pub age_hours: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PendingJump {
    pub system: String,
    pub body: Option<String>,
    pub departure: Option<String>,
    /// Minutes from `now` to departure; negative when it has passed.
    pub minutes_to_departure: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct CarrierStatus {
    pub carrier_id: i64,
    pub carrier_type: Option<String>,
    pub callsign: Option<String>,
    pub name: Option<String>,
    pub owned: bool,
    pub decommissioned: bool,
    pub location: Option<Aged<String>>,
    pub body: Option<String>,
    pub tank_tritium_t: Option<Aged<i64>>,
    pub capacity: Option<Aged<serde_json::Value>>,
    pub jump_range_ly: Option<f64>,
    pub docking_access: Option<String>,
    pub balance_cr: Option<i64>,
    pub services: Vec<String>,
    pub pending_jump: Option<PendingJump>,
    /// What the commander moved aboard, net — never "what is aboard".
    pub hold_moved: Vec<HoldLine>,
}

fn age_hours(now: &str, then: &str) -> f64 {
    match (ed_domain::freshness::parse_timestamp(now), ed_domain::freshness::parse_timestamp(then)) {
        (Some(a), Some(b)) => ((a - b) as f64 / 3600.0).max(0.0),
        _ => 0.0,
    }
}

fn aged<T: Serialize>(now: &str, value: Option<T>, ts: Option<String>) -> Option<Aged<T>> {
    let (value, ts) = (value?, ts?);
    let age = age_hours(now, &ts);
    Some(Aged { value, as_of: ts, age_hours: (age * 10.0).round() / 10.0 })
}

/// Every carrier the journal knows, owned first, with ages from `now`
/// (ISO-8601, so tests are deterministic).
pub fn status(conn: &Connection, now: &str) -> Result<Vec<CarrierStatus>> {
    let mut stmt = conn.prepare(
        "SELECT carrier_id, carrier_type, callsign, name, owned, decommissioned,
                system_name, body, location_ts, fuel_t, fuel_ts,
                capacity_total, capacity_used, free_space, stats_ts, jump_range_curr,
                docking_access, balance_cr, services,
                pending_jump_system, pending_jump_body, pending_departure
         FROM carriers ORDER BY owned DESC, decommissioned ASC, callsign",
    )?;
    let mut out = Vec::new();
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, i64>(4)? != 0,
            r.get::<_, i64>(5)? != 0,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, Option<String>>(8)?,
            r.get::<_, Option<i64>>(9)?,
            r.get::<_, Option<String>>(10)?,
            r.get::<_, Option<i64>>(11)?,
            r.get::<_, Option<i64>>(12)?,
            r.get::<_, Option<i64>>(13)?,
            r.get::<_, Option<String>>(14)?,
            r.get::<_, Option<f64>>(15)?,
            r.get::<_, Option<String>>(16)?,
            r.get::<_, Option<i64>>(17)?,
            r.get::<_, Option<String>>(18)?,
            r.get::<_, Option<String>>(19)?,
            r.get::<_, Option<String>>(20)?,
            r.get::<_, Option<String>>(21)?,
        ))
    })?;
    for row in rows {
        let (id, carrier_type, callsign, name, owned, decommissioned, system, body, location_ts, fuel, fuel_ts, cap_total, cap_used, free, stats_ts, jump_range, docking, balance, services, pj_system, pj_body, pj_departure) = row?;
        let capacity = cap_total.map(|t| serde_json::json!({ "total_t": t, "used_t": cap_used, "free_t": free }));
        let minutes_to_departure = pj_departure.as_deref().and_then(|d| {
            let dep = ed_domain::freshness::parse_timestamp(d)?;
            let now = ed_domain::freshness::parse_timestamp(now)?;
            Some((dep - now) / 60)
        });
        let pending_jump = pj_system.map(|system| PendingJump {
            minutes_to_departure,
            system,
            body: pj_body,
            departure: pj_departure,
        });
        let mut hold_stmt = conn.prepare("SELECT commodity, count, ts FROM carrier_hold WHERE carrier_id = ?1 ORDER BY count DESC")?;
        let hold_moved = hold_stmt
            .query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(|(commodity, tons, ts)| HoldLine { age_hours: (age_hours(now, &ts) * 10.0).round() / 10.0, commodity, tons, as_of: ts })
            .collect();
        out.push(CarrierStatus {
            carrier_id: id,
            carrier_type,
            callsign,
            name,
            owned,
            decommissioned,
            location: aged(now, system, location_ts),
            body,
            tank_tritium_t: aged(now, fuel, fuel_ts),
            capacity: aged(now, capacity, stats_ts),
            jump_range_ly: jump_range,
            docking_access: docking,
            balance_cr: balance,
            services: services.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default(),
            pending_jump,
            hold_moved,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        conn
    }

    fn ev(conn: &Connection, n: i64, raw: &str) {
        let v: Value = serde_json::from_str(raw).unwrap();
        conn.execute(
            "INSERT INTO events (file, offset, ts, event, system_address, market_id, raw) VALUES ('j.log', ?1, ?2, ?3, NULL, ?4, ?5)",
            params![n, v["timestamp"].as_str().unwrap(), v["event"].as_str().unwrap(), v.get("MarketID").and_then(Value::as_i64), raw],
        )
        .unwrap();
    }

    // Shapes from the census of 2026-09-06 (identifiers, names, figures and timestamps replaced).
    const BUY: &str = r#"{"timestamp":"2026-01-15T22:14:00Z","event":"CarrierBuy","CarrierType":"FleetCarrier","CarrierID":3700000001,"BoughtAtMarket":128000001,"Location":"Alpha","SystemAddress":11,"Price":5000000000,"Variant":"CarrierDockB","Callsign":"K3X-9ZQ"}"#;
    const STATS: &str = r#"{"timestamp":"2026-01-15T22:22:00Z","event":"CarrierStats","CarrierType":"FleetCarrier","CarrierID":3700000001,"Callsign":"K3X-9ZQ","Name":"ENDEAVOUR","DockingAccess":"all","AllowNotorious":false,"FuelLevel":500,"JumpRangeCurr":500.0,"JumpRangeMax":500.0,"PendingDecommission":false,"SpaceUsage":{"TotalCapacity":25000,"Crew":6270,"Cargo":0,"CargoSpaceReserved":0,"ShipPacks":0,"ModulePacks":0,"FreeSpace":18730},"Finance":{"CarrierBalance":100000000,"ReserveBalance":0,"AvailableBalance":100000000,"TaxRate":0},"Crew":[{"CrewRole":"Captain","Activated":true,"Enabled":true,"CrewName":"A"},{"CrewRole":"Commodities","Activated":true,"Enabled":true,"CrewName":"B"},{"CrewRole":"CarrierFuel","Activated":true,"Enabled":true,"CrewName":"C"},{"CrewRole":"Refuel","Activated":false}],"ShipPacks":[],"ModulePacks":[]}"#;
    // The Frontier bug: CarrierType under an empty-string key.
    const RENAME: &str = r#"{"timestamp":"2026-01-15T22:23:00Z","event":"CarrierNameChange","CarrierID":3700000001,"":"FleetCarrier","Name":"Endeavour","Callsign":"K3X-9ZQ"}"#;
    const REQUEST: &str = r#"{"timestamp":"2026-01-15T22:27:00Z","event":"CarrierJumpRequest","CarrierType":"FleetCarrier","CarrierID":3700000001,"SystemName":"Beta","Body":"Beta 1","SystemAddress":22,"BodyID":1,"DepartureTime":"2026-01-15T22:57:00Z"}"#;
    const JUMP: &str = r#"{"timestamp":"2026-01-15T22:58:00Z","event":"CarrierJump","Docked":true,"StationName":"K3X-9ZQ","StationType":"FleetCarrier","MarketID":3700000001,"StarSystem":"Beta","SystemAddress":22,"Body":"Beta 1","BodyID":1}"#;
    const CANCEL: &str = r#"{"timestamp":"2026-01-15T22:30:00Z","event":"CarrierJumpCancelled","CarrierType":"FleetCarrier","CarrierID":3700000001}"#;
    const LOCATION: &str = r#"{"timestamp":"2026-01-16T01:00:00Z","event":"CarrierLocation","CarrierType":"FleetCarrier","CarrierID":3700000001,"StarSystem":"Gamma","SystemAddress":33,"BodyID":0}"#;
    const SQUADRON: &str = r#"{"timestamp":"2026-08-28T10:00:00Z","event":"CarrierStats","CarrierType":"SquadronCarrier","CarrierID":3700000002,"Callsign":"ABCD","Name":"Squad","FuelLevel":1000,"SpaceUsage":{"TotalCapacity":60000,"FreeSpace":23244},"Finance":{"CarrierBalance":1},"Crew":[]}"#;

    fn one(conn: &Connection) -> CarrierStatus {
        rebuild(conn).unwrap();
        status(conn, "2026-01-16T02:00:00Z").unwrap().into_iter().find(|c| c.carrier_id == 3700000001).unwrap()
    }

    #[test]
    fn a_bought_carrier_is_owned_and_named_and_survives_the_empty_key_rename() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, STATS);
        ev(&conn, 3, RENAME);
        let c = one(&conn);
        assert!(c.owned);
        assert_eq!(c.carrier_type.as_deref(), Some("FleetCarrier"), "type never read from the rename");
        assert_eq!((c.callsign.as_deref(), c.name.as_deref()), (Some("K3X-9ZQ"), Some("Endeavour")));
        let tank = c.tank_tritium_t.unwrap();
        assert_eq!((tank.value, tank.as_of.as_str()), (500, "2026-01-15T22:22:00Z"));
        assert!((tank.age_hours - 3.6).abs() < 0.11, "{}", tank.age_hours);
        assert_eq!(c.capacity.unwrap().value["used_t"], 6270);
        assert_eq!(c.services, vec!["Captain", "Commodities", "CarrierFuel"], "unactivated crew is not a service");
        assert_eq!(c.balance_cr, Some(100000000));
    }

    #[test]
    fn a_squadron_carrier_is_known_but_not_owned() {
        let conn = db();
        ev(&conn, 1, SQUADRON);
        rebuild(&conn).unwrap();
        let all = status(&conn, "2026-01-16T02:00:00Z").unwrap();
        assert_eq!(all.len(), 1);
        assert!(!all[0].owned);
        assert_eq!(all[0].carrier_type.as_deref(), Some("SquadronCarrier"));
        assert_eq!(all[0].callsign.as_deref(), Some("ABCD"));
    }

    #[test]
    fn jump_request_then_jump_moves_it_and_clears_the_pending_jump() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, REQUEST);
        let c = one(&conn);
        let pj = c.pending_jump.clone().expect("pending");
        assert_eq!(pj.system, "Beta");
        assert_eq!(pj.departure.as_deref(), Some("2026-01-15T22:57:00Z"));
        assert_eq!(c.location.as_ref().unwrap().value, "Alpha");
        ev(&conn, 3, JUMP);
        let c = one(&conn);
        assert!(c.pending_jump.is_none());
        let loc = c.location.unwrap();
        assert_eq!((loc.value.as_str(), loc.as_of.as_str()), ("Beta", "2026-01-15T22:58:00Z"));
        assert_eq!(c.body.as_deref(), Some("Beta 1"));
    }

    /// The commander is aboard but not docked: the journal's CarrierJump
    /// then names NO carrier (no CarrierID, no MarketID). It still ends
    /// the pending jump of the carrier it completes.
    #[test]
    fn an_undocked_carrier_jump_still_clears_the_pending_jump() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, REQUEST);
        ev(&conn, 3, r#"{"timestamp":"2026-01-15T22:58:00Z","event":"CarrierJump","Docked":false,"StarSystem":"Beta","SystemAddress":22,"Body":"Beta 1","BodyID":1}"#);
        let c = one(&conn);
        assert!(c.pending_jump.is_none(), "{:?}", c.pending_jump);
        assert_eq!(c.location.unwrap().value, "Beta");
    }

    /// Nobody aboard: no CarrierJump is ever written. The next location
    /// heartbeat that finds the carrier at its destination, after the
    /// departure time, ends the pending jump; one before departure does not.
    #[test]
    fn a_heartbeat_at_the_destination_after_departure_clears_the_pending_jump() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, REQUEST);
        ev(&conn, 3, r#"{"timestamp":"2026-01-15T22:40:00Z","event":"CarrierLocation","CarrierType":"FleetCarrier","CarrierID":3700000001,"StarSystem":"Alpha","SystemAddress":11,"BodyID":0}"#);
        assert!(one(&conn).pending_jump.is_some(), "still scheduled while it sits at the origin");
        ev(&conn, 4, r#"{"timestamp":"2026-01-16T01:00:00Z","event":"CarrierLocation","CarrierType":"FleetCarrier","CarrierID":3700000001,"StarSystem":"Beta","SystemAddress":22,"BodyID":0}"#);
        let c = one(&conn);
        assert!(c.pending_jump.is_none());
        assert_eq!(c.location.unwrap().value, "Beta");
    }

    #[test]
    fn jump_request_then_cancel_clears_the_pending_jump_without_moving() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, REQUEST);
        ev(&conn, 3, CANCEL);
        let c = one(&conn);
        assert!(c.pending_jump.is_none());
        assert_eq!(c.location.unwrap().value, "Alpha");
    }

    #[test]
    fn carrier_location_is_the_heartbeat() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, STATS);
        ev(&conn, 3, LOCATION);
        let c = one(&conn);
        assert_eq!(c.location.unwrap().value, "Gamma");
        assert_eq!(c.tank_tritium_t.unwrap().value, 500, "the heartbeat moves the carrier and nothing else");
    }

    #[test]
    fn deposit_fuel_total_is_newer_than_stats_and_older_deposits_do_not_regress() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, STATS);
        ev(&conn, 3, r#"{"timestamp":"2026-01-15T23:06:00Z","event":"CarrierDepositFuel","CarrierType":"FleetCarrier","CarrierID":3700000001,"Amount":0,"Total":484}"#);
        let c = one(&conn);
        let tank = c.tank_tritium_t.unwrap();
        assert_eq!((tank.value, tank.as_of.as_str()), (484, "2026-01-15T23:06:00Z"));
        // A stats row from before the deposit, replayed later in file order, must not win.
        ev(&conn, 4, &STATS.replace("22:22:00Z", "22:00:00Z").replace("\"FuelLevel\":500", "\"FuelLevel\":999"));
        let c = one(&conn);
        assert_eq!(c.tank_tritium_t.unwrap().value, 999, "CarrierStats is authoritative when it lands (it is the newest observation in journal order)");
    }

    #[test]
    fn cargo_transfer_sums_per_commodity_and_floors_at_zero() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, r#"{"timestamp":"2026-01-15T23:10:00Z","event":"CargoTransfer","Transfers":[{"Type":"tritium","Type_Localised":"Tritium","Count":10,"Direction":"tocarrier"},{"Type":"palladium","Count":1008,"Direction":"tocarrier"}]}"#);
        ev(&conn, 3, r#"{"timestamp":"2026-01-15T23:20:00Z","event":"CargoTransfer","Transfers":[{"Type":"tritium","Count":3,"Direction":"toship"},{"Type":"gold","Count":100,"Direction":"toship"}]}"#);
        let c = one(&conn);
        let lines: Vec<(&str, i64)> = c.hold_moved.iter().map(|h| (h.commodity.as_str(), h.tons)).collect();
        assert_eq!(lines, vec![("palladium", 1008), ("tritium", 7)], "gold floored at zero and dropped");
    }

    #[test]
    fn cargo_transfer_is_skipped_without_exactly_one_owned_carrier() {
        let conn = db();
        ev(&conn, 1, SQUADRON);
        ev(&conn, 2, r#"{"timestamp":"2026-01-15T23:10:00Z","event":"CargoTransfer","Transfers":[{"Type":"tritium","Count":10,"Direction":"tocarrier"}]}"#);
        rebuild(&conn).unwrap();
        let all = status(&conn, "2026-01-16T02:00:00Z").unwrap();
        assert!(all[0].hold_moved.is_empty());
    }

    #[test]
    fn status_reports_owned_first_and_crew_services_update() {
        let conn = db();
        ev(&conn, 1, SQUADRON);
        ev(&conn, 2, BUY);
        ev(&conn, 3, STATS);
        ev(&conn, 4, r#"{"timestamp":"2026-01-15T22:25:00Z","event":"CarrierCrewServices","CarrierType":"FleetCarrier","CarrierID":3700000001,"CrewRole":"Refuel","Operation":"Activate","CrewName":"D"}"#);
        rebuild(&conn).unwrap();
        let all = status(&conn, "2026-01-16T02:00:00Z").unwrap();
        assert_eq!(all.len(), 2);
        assert!(all[0].owned && !all[1].owned);
        assert!(all[0].services.contains(&"Refuel".to_string()));
    }

    #[test]
    fn rebuild_is_idempotent_and_decommission_marks() {
        let conn = db();
        ev(&conn, 1, BUY);
        ev(&conn, 2, r#"{"timestamp":"2026-01-17T00:00:00Z","event":"CarrierDecommission","CarrierType":"FleetCarrier","CarrierID":3700000001,"ScrapRefund":1,"ScrapTime":1,"ScrapDateTime":"2026-01-24T00:00:00Z"}"#);
        rebuild(&conn).unwrap();
        rebuild(&conn).unwrap();
        let all = status(&conn, "2026-01-18T00:00:00Z").unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].decommissioned);
    }
}
