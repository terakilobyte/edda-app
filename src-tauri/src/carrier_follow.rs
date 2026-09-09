//! The carrier's route, followed across sessions (Item 52 C; maintainer's item
//! 6: "I want to move my carrier anywhere in the galaxy and need to plot
//! a route. Since carrier movement takes time, this needs to be longer
//! lived").
//!
//! One plan lives in `carrier_route` with a cursor. The journal advances
//! it: `CarrierJumpRequest` for the next hop marks it scheduled (the
//! countdown callout comes from `callouts`), `CarrierJump` /
//! `CarrierLocation` arriving at the next hop moves the cursor and speaks
//! what remains, and each request → arrival gap is measured so the ETA
//! prefers the commander's own cadence over the game's 20-minute floor.
//! "Next" puts the next system's name on the clipboard: the carrier's
//! navigation panel takes a pasted name and there is no macro for it.

use crate::callouts::Callout;
use crate::state::AppState;
use ed_galaxy::carrier::{CarrierHop, CarrierRoute, Verdict};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scheduled {
    pub system: String,
    pub departure: Option<String>,
    pub requested_ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarrierPlan {
    pub carrier_id: Option<i64>,
    pub callsign: Option<String>,
    pub from: String,
    pub to: String,
    pub hops: Vec<CarrierHop>,
    /// Index into `hops` of the next jump to make; `hops.len()` = done.
    pub next: usize,
    pub scheduled: Option<Scheduled>,
    pub started_ts: String,
    pub tank_start_t: f32,
    pub fuel_t: u32,
    pub verdict: Verdict,
    pub minutes_per_jump: f32,
    /// Request → arrival minutes for jumps made on this plan.
    pub measured_minutes: Vec<f32>,
}

impl CarrierPlan {
    pub fn from_route(route: &CarrierRoute, carrier_id: Option<i64>, callsign: Option<String>, tank_start_t: f32, now: &str) -> Self {
        CarrierPlan {
            carrier_id,
            callsign,
            from: route.from.clone(),
            to: route.to.clone(),
            hops: route.hops.clone(),
            next: 0,
            scheduled: None,
            started_ts: now.to_string(),
            tank_start_t,
            fuel_t: route.fuel_t,
            verdict: route.verdict.clone(),
            minutes_per_jump: route.minutes_per_jump,
            measured_minutes: Vec::new(),
        }
    }

    pub fn done(&self) -> bool {
        self.next >= self.hops.len()
    }

    pub fn next_hop(&self) -> Option<&CarrierHop> {
        self.hops.get(self.next)
    }

    pub fn remaining(&self) -> usize {
        self.hops.len().saturating_sub(self.next)
    }

    /// Minutes per jump: the commander's own measured cadence when there
    /// is one, else the game's floor.
    pub fn cadence(&self) -> f32 {
        if self.measured_minutes.is_empty() {
            self.minutes_per_jump
        } else {
            self.measured_minutes.iter().sum::<f32>() / self.measured_minutes.len() as f32
        }
    }

    pub fn eta_minutes(&self) -> u32 {
        (self.remaining() as f32 * self.cadence()).round() as u32
    }

    /// The journal said the carrier requested a jump. Returns the line to
    /// speak after the countdown callout, or nothing when it is not ours.
    pub fn on_jump_request(&mut self, system: &str, departure: Option<&str>, ts: &str) -> Option<String> {
        let Some(next) = self.next_hop() else { return None };
        if next.name.eq_ignore_ascii_case(system) {
            self.scheduled = Some(Scheduled { system: system.to_string(), departure: departure.map(str::to_string), requested_ts: ts.to_string() });
            let after = self.remaining().saturating_sub(1);
            Some(if after == 0 {
                "That is the last jump on your carrier route.".to_string()
            } else {
                format!("That is hop {} of {} on your carrier route; {after} more after it.", self.next + 1, self.hops.len())
            })
        } else {
            Some(format!("That jump is off your carrier route, which expects {} next.", next.name))
        }
    }

    /// The journal said the carrier is at `system`. Advances the cursor
    /// when it is the next hop (or any later hop — a skipped stop is
    /// fine). Returns the line to speak.
    pub fn on_arrival(&mut self, system: &str, ts: &str) -> Option<String> {
        let pos = self.hops.iter().skip(self.next).position(|h| h.name.eq_ignore_ascii_case(system))?;
        let arrived = self.next + pos;
        if let Some(s) = self.scheduled.take() {
            if let (Ok(a), Ok(b)) = (chrono::DateTime::parse_from_rfc3339(&s.requested_ts), chrono::DateTime::parse_from_rfc3339(ts)) {
                let minutes = (b - a).num_seconds() as f32 / 60.0;
                if minutes > 0.0 && minutes < 240.0 {
                    self.measured_minutes.push(minutes);
                }
            }
        }
        self.next = arrived + 1;
        if self.done() {
            return Some(format!("Carrier route complete: {} reached.", self.to));
        }
        let next = &self.hops[self.next];
        let mut line = format!(
            "{} carrier jump{} remain. Next: {}, {:.0} light years, {} tonnes of tritium.",
            self.remaining(),
            if self.remaining() == 1 { "" } else { "s" },
            next.name,
            next.distance_ly,
            next.fuel_t
        );
        if let Verdict::ShortBy { tons, at_hop } = &self.verdict {
            if *at_hop as usize == self.next + 1 {
                line.push_str(&format!(" The plan runs {tons} tonnes short on this jump: refuel first."));
            }
        }
        Some(line)
    }
}

pub fn load(conn: &Connection) -> Option<CarrierPlan> {
    conn.query_row("SELECT json FROM carrier_route WHERE id = 1", [], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
}

pub fn save(conn: &Connection, plan: &CarrierPlan) -> Result<(), String> {
    let json = serde_json::to_string(plan).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO carrier_route (id, json, updated) VALUES (1, ?1, datetime('now'))
         ON CONFLICT(id) DO UPDATE SET json = excluded.json, updated = excluded.updated",
        params![json],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn clear(conn: &Connection) {
    let _ = conn.execute("DELETE FROM carrier_route WHERE id = 1", []);
}

/// What the Route tab and the HUD show.
pub fn view(plan: Option<&CarrierPlan>) -> Value {
    let Some(p) = plan else { return json!({ "active": false }) };
    json!({
        "active": true,
        "callsign": p.callsign,
        "from": p.from,
        "to": p.to,
        "jumps": p.hops.len(),
        "next": p.next + 1,
        "remaining": p.remaining(),
        "done": p.done(),
        "next_system": p.next_hop().map(|h| h.name.clone()),
        "next_distance_ly": p.next_hop().map(|h| h.distance_ly),
        "next_fuel_t": p.next_hop().map(|h| h.fuel_t),
        "scheduled": p.scheduled,
        "eta_minutes": p.eta_minutes(),
        "minutes_per_jump": p.cadence(),
        "cadence_measured": !p.measured_minutes.is_empty(),
        "fuel_t": p.fuel_t,
        "verdict": p.verdict,
        "hops": p.hops,
    })
}

fn emit(state: &AppState, plan: Option<&CarrierPlan>) {
    use crate::events::EmitExt as _;
    state.events.emit(crate::events::CARRIER_ROUTE, view(plan));
}

/// Watcher hook: carrier events advance the followed route.
pub fn on_event(app: &AppHandle, conn: &Connection, v: &Value, out: &mut Vec<(Callout, Option<Value>)>) {
    let Some(mut plan) = load(conn) else { return };
    let ts = v.get("timestamp").and_then(Value::as_str).unwrap_or("");
    let line = match v.get("event").and_then(Value::as_str) {
        Some("CarrierJumpRequest") => {
            let system = v.get("SystemName").and_then(Value::as_str).unwrap_or("");
            plan.on_jump_request(system, v.get("DepartureTime").and_then(Value::as_str), ts)
        }
        Some("CarrierJump") | Some("CarrierLocation") => {
            let system = v.get("StarSystem").and_then(Value::as_str).unwrap_or("");
            plan.on_arrival(system, ts)
        }
        _ => None,
    };
    if let Some(line) = line {
        if save(conn, &plan).is_ok() {
            out.push((Callout::new("carrier", ts, 1, true, line), None));
            emit(&app.state::<AppState>(), Some(&plan));
        }
    }
}

/// Plot a carrier route on the local index from the carrier's position
/// (or `from`) to `to`, with the tank, capacity and hold tritium the
/// journal last saw. The server home (POST /v1/carrier/route) rides the
/// data-source choice once it exists; until then a missing index is an
/// honest error.
pub fn plot(
    conn: &Connection,
    routing: &crate::routing::RoutingState,
    data_dir: &std::path::Path,
    to: &str,
    from: Option<&str>,
) -> Result<(CarrierRoute, Option<i64>, Option<String>, f32), String> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let carriers = ed_store::carrier::status(conn, &now).map_err(|e| e.to_string())?;
    let carrier = carriers.iter().find(|c| c.owned && !c.decommissioned).or(carriers.first());
    let from_name = from
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| carrier.and_then(|c| c.location.as_ref().map(|l| l.value.clone())))
        .ok_or("no origin: EDDA has not seen where your carrier is (open Carrier Management once), or pass from")?;
    let tank_t = carrier.and_then(|c| c.tank_tritium_t.as_ref().map(|t| t.value as f32)).unwrap_or(ed_galaxy::carrier::TANK_T);
    let capacity_used_t = carrier.and_then(|c| c.capacity.as_ref().and_then(|cap| cap.value["used_t"].as_f64())).unwrap_or(0.0) as f32;
    let hold_tritium_t = carrier
        .map(|c| c.hold_moved.iter().filter(|h| h.commodity == "tritium").map(|h| h.tons as f32).sum::<f32>())
        .unwrap_or(0.0);
    let galaxy = routing
        .galaxy(data_dir)
        .ok_or("the local routing index is not installed: install it from Settings → System data (the community API's carrier route is coming)")?;
    let a = galaxy.find(&from_name).ok_or_else(|| format!("unknown system {from_name:?}"))?;
    let b = galaxy.find(to).ok_or_else(|| format!("unknown system {to:?}"))?;
    let req = ed_galaxy::carrier::CarrierRequest { from: a, to: b, capacity_used_t, tank_t, hold_tritium_t, time_budget_ms: 120_000, ..Default::default() };
    let cancelled = || false;
    let route = ed_galaxy::carrier::plan(&galaxy, &req, &cancelled).map_err(|e| e.to_string())?;
    tracing::info!(from = %from_name, to, jumps = route.jumps, fuel_t = route.fuel_t, ms = route.wall_ms, expansions = route.expansions, "carrier plot");
    Ok((route, carrier.map(|c| c.carrier_id), carrier.and_then(|c| c.callsign.clone()), tank_t))
}

#[tauri::command]
pub async fn carrier_route_plot(state: State<'_, AppState>, to: String, from: Option<String>) -> Result<CarrierRoute, String> {
    let conn = state.read_conn()?;
    let routing = state.routing.clone();
    let data_dir = state.data_dir.clone();
    tauri::async_runtime::spawn_blocking(move || plot(&conn, &routing, &data_dir, &to, from.as_deref()).map(|(r, ..)| r))
        .await
        .map_err(|e| e.to_string())?
}

/// Plot and start following in one step (the tool's default).
pub fn plot_and_follow(state: &AppState, to: &str, from: Option<&str>) -> Result<Value, String> {
    let conn = state.read_conn()?;
    let (route, carrier_id, callsign, tank) = plot(&conn, &state.routing, &state.data_dir, to, from)?;
    drop(conn);
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let plan = CarrierPlan::from_route(&route, carrier_id, callsign, tank, &now);
    state.with_store(|s| save(s.conn(), &plan))?;
    emit(state, Some(&plan));
    Ok(json!({ "route": route, "following": view(Some(&plan)) }))
}

#[tauri::command]
pub async fn carrier_route_start(state: State<'_, AppState>, route: CarrierRoute) -> Result<Value, String> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let carriers = state.with_read(|s| ed_store::carrier::status(s.conn(), &now)).map_err(|e| e.to_string())?;
    let carrier = carriers.iter().find(|c| c.owned && !c.decommissioned).or(carriers.first());
    let plan = CarrierPlan::from_route(
        &route,
        carrier.map(|c| c.carrier_id),
        carrier.and_then(|c| c.callsign.clone()),
        carrier.and_then(|c| c.tank_tritium_t.as_ref().map(|t| t.value as f32)).unwrap_or(ed_galaxy::carrier::TANK_T),
        &now,
    );
    state.with_store(|s| save(s.conn(), &plan))?;
    emit(&state, Some(&plan));
    Ok(view(Some(&plan)))
}

#[tauri::command]
pub async fn carrier_route_status(state: State<'_, AppState>) -> Result<Value, String> {
    Ok(view(state.with_read(|s| load(s.conn())).as_ref()))
}

#[tauri::command]
pub async fn carrier_route_clear(state: State<'_, AppState>) -> Result<Value, String> {
    state.with_store(|s| clear(s.conn()));
    emit(&state, None);
    Ok(view(None))
}

/// The next system's name onto the clipboard, for the carrier's
/// navigation panel. Returns the name.
#[tauri::command]
pub async fn carrier_route_next(state: State<'_, AppState>) -> Result<String, String> {
    let plan = state.with_read(|s| load(s.conn())).ok_or("no carrier route is being followed")?;
    let next = plan.next_hop().ok_or("the carrier route is complete")?;
    crate::follow::set_clipboard(&next.name)?;
    Ok(next.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hop(name: &str, d: f32, fuel: u32) -> CarrierHop {
        CarrierHop { idx: 0, name: name.into(), pos: [0.0; 3], distance_ly: d, fuel_t: fuel, topped_up_t: 0, tank_after_t: 900 }
    }

    fn plan() -> CarrierPlan {
        CarrierPlan {
            carrier_id: Some(1),
            callsign: Some("K3X-9ZQ".into()),
            from: "Alpha".into(),
            to: "Delta".into(),
            hops: vec![hop("Beta", 480.0, 86), hop("Gamma", 470.0, 84), hop("Delta", 300.0, 55)],
            next: 0,
            scheduled: None,
            started_ts: "2026-09-07T00:00:00Z".into(),
            tank_start_t: 1000.0,
            fuel_t: 225,
            verdict: Verdict::ShortBy { tons: 10, at_hop: 3 },
            minutes_per_jump: 20.0,
            measured_minutes: vec![],
        }
    }

    #[test]
    fn a_request_for_the_next_hop_is_ours_and_counts_the_rest() {
        let mut p = plan();
        let line = p.on_jump_request("Beta", Some("2026-09-07T00:30:00Z"), "2026-09-07T00:00:00Z").unwrap();
        assert_eq!(line, "That is hop 1 of 3 on your carrier route; 2 more after it.");
        assert!(p.scheduled.is_some());
        let off = p.on_jump_request("Elsewhere", None, "2026-09-07T00:01:00Z").unwrap();
        assert!(off.contains("off your carrier route") && off.contains("Beta"));
    }

    #[test]
    fn arrival_advances_measures_the_cadence_and_warns_at_the_short_hop() {
        let mut p = plan();
        p.on_jump_request("Beta", Some("2026-09-07T00:30:00Z"), "2026-09-07T00:00:00Z");
        let line = p.on_arrival("Beta", "2026-09-07T00:31:00Z").unwrap();
        assert_eq!(line, "2 carrier jumps remain. Next: Gamma, 470 light years, 84 tonnes of tritium.");
        assert_eq!(p.measured_minutes, vec![31.0]);
        assert_eq!(p.eta_minutes(), 62, "two jumps at the measured 31 minutes, not the 20-minute floor");
        let line = p.on_arrival("Gamma", "2026-09-07T01:10:00Z").unwrap();
        assert!(line.starts_with("1 carrier jump remain."), "{line}");
        assert!(line.contains("10 tonnes short"), "the plan's shortfall is at hop 3: {line}");
        assert_eq!(p.on_arrival("Delta", "2026-09-07T01:50:00Z").unwrap(), "Carrier route complete: Delta reached.");
        assert!(p.done());
    }

    #[test]
    fn a_skipped_hop_still_advances_and_an_unknown_system_is_ignored() {
        let mut p = plan();
        assert!(p.on_arrival("Nowhere", "2026-09-07T00:31:00Z").is_none());
        assert_eq!(p.next, 0);
        p.on_arrival("Gamma", "2026-09-07T00:31:00Z").unwrap();
        assert_eq!(p.next, 2);
    }

    #[test]
    fn view_reports_the_floor_until_a_cadence_is_measured() {
        let p = plan();
        let v = view(Some(&p));
        assert_eq!((v["remaining"].as_u64(), v["eta_minutes"].as_u64(), v["cadence_measured"].as_bool()), (Some(3), Some(60), Some(false)));
        assert_eq!(v["next_system"], "Beta");
        assert_eq!(view(None)["active"], false);
    }
}
