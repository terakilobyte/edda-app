//! `POST /v1/loadout/physics` — a pasted SLEF export (EDSY, Coriolis) or
//! journal `Loadout` event in, the ship physics `POST /v1/route` wants
//! out (maintainer, 2026-09-09: the web router "accepts a slef/json paste from
//! edsy/coriolis"). The derivation is `ed_galaxy::loadout`, the same
//! code the desktop app plans with, so a build pasted here plots
//! exactly as it would from the commander's own journal. Nothing is
//! stored; nothing here names a commander.

use axum::{extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;

use crate::http::AppState;

/// The body: the paste as text (`{"paste": "..."}`) so the page never
/// has to parse JSON itself, or the SLEF/Loadout JSON directly.
#[derive(Debug, Deserialize)]
pub struct PhysicsRequest {
    pub paste: Option<String>,
    /// Tonnes aboard to plan with; default none.
    #[serde(default)]
    pub cargo_t: f32,
}

/// At most this many bytes of paste: a SLEF export is a few KB.
pub const MAX_PASTE_BYTES: usize = 256 * 1024;

pub fn physics(body: &serde_json::Value) -> Result<serde_json::Value, String> {
    let (text, cargo) = match serde_json::from_value::<PhysicsRequest>(body.clone()) {
        Ok(PhysicsRequest {
            paste: Some(paste),
            cargo_t,
        }) => (paste, cargo_t),
        _ => (body.to_string(), 0.0),
    };
    if text.len() > MAX_PASTE_BYTES {
        return Err(format!(
            "paste is {} bytes; at most {MAX_PASTE_BYTES}",
            text.len()
        ));
    }
    let loadout = ed_galaxy::loadout::loadout_from_paste(&text).map_err(|e| e.to_string())?;
    let p = ed_galaxy::loadout::physics_from_loadout(&loadout, cargo.max(0.0), None)
        .map_err(|e| e.to_string())?;
    let full_tank = p.model.range_at(p.model.capacity);
    let one_jump = p.model.range_at(p.model.max_fuel_per_jump);
    Ok(serde_json::json!({
        "ship": p.ship,
        "ship_name": p.ship_name,
        "summary": p.summary(),
        "fuel_model": p.model,
        "boost": p.boost,
        "capacity": p.model.capacity,
        "max_fuel_per_jump": p.model.max_fuel_per_jump,
        "cargo_capacity": p.cargo_capacity,
        "max_jump_range": p.max_jump_range,
        "full_tank_range_ly": full_tank,
        "one_jump_range_ly": one_jump,
        "fsd": {"size": p.fsd_size, "rating": p.fsd_rating.to_string(), "sco": p.sco, "mk2": p.mk2},
        "booster_ly": p.booster_ly,
    }))
}

pub async fn handler(
    State(_state): State<AppState>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let counter = |outcome: &'static str| {
        metrics::counter!("edda_loadout_physics_total", "outcome" => outcome).increment(1)
    };
    match physics(&body) {
        Ok(value) => {
            counter("ok");
            axum::Json(value).into_response()
        }
        Err(message) => {
            counter("invalid");
            (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": message})),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLEF: &str = r#"[{"header":{"appName":"EDSY","appVersion":"4.0"},"data":{"event":"Loadout","Ship":"Cutter","ShipName":"Treasure Goblin","UnladenMass":1163.6,"CargoCapacity":720,"MaxJumpRange":25.83,"FuelCapacity":{"Main":32,"Reserve":1.16},"Modules":[{"Slot":"FrameShiftDrive","Item":"Int_Hyperdrive_Size7_Class5","Engineering":{"Modifiers":[{"Label":"MaxFuelPerJump","Value":12.8}]}},{"Slot":"Slot01_Size6","Item":"Int_GuardianFSDBooster_Size5"}]}}]"#;

    /// Both body shapes work — the paste as a string field, or the SLEF
    /// itself — and the answer carries what /v1/route consumes verbatim
    /// (`fuel_model`, `boost`) plus the figures the page shows.
    #[test]
    fn a_paste_or_the_slef_itself_yields_route_physics() {
        let wrapped = physics(&serde_json::json!({"paste": SLEF, "cargo_t": 200})).unwrap();
        let direct = physics(&serde_json::from_str::<serde_json::Value>(SLEF).unwrap()).unwrap();
        assert_eq!(wrapped["ship"], "cutter");
        assert_eq!(wrapped["fuel_model"]["cargo"], 200.0);
        assert_eq!(direct["fuel_model"]["cargo"], 0.0);
        assert_eq!(wrapped["fsd"]["size"], 7);
        assert_eq!(wrapped["booster_ly"], 10.5);
        assert!(
            wrapped["full_tank_range_ly"].as_f64().unwrap()
                < wrapped["one_jump_range_ly"].as_f64().unwrap()
        );
        assert!(direct["summary"].as_str().unwrap().contains("size 7A"));
        assert!(physics(&serde_json::json!({"paste": "not json"}))
            .unwrap_err()
            .contains("not JSON"));
        assert!(physics(&serde_json::json!({"event": "Docked"}))
            .unwrap_err()
            .contains("not a Loadout"));
        let huge = serde_json::json!({"paste": "x".repeat(MAX_PASTE_BYTES + 1)});
        assert!(physics(&huge).unwrap_err().contains("at most"));
    }
}
