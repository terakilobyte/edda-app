//! Live ship status for the overlay panel: fuel, cargo capacity, current
//! system, docked state. Combines the always-fresh `Status.json` file with
//! the most recent relevant journal events (Status.json doesn't carry the
//! system name).

use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ShipStatus {
    pub current_system: Option<String>,
    pub docked: bool,
    pub station_name: Option<String>,
    pub fuel_main: Option<f64>,
    pub fuel_reservoir: Option<f64>,
    pub cargo_capacity: Option<i64>,
    pub cargo_count: Option<i64>,
    pub ship_name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct StatusFile {
    #[serde(rename = "Fuel")]
    fuel: Option<FuelStatus>,
    #[serde(rename = "Cargo")]
    cargo: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct FuelStatus {
    #[serde(rename = "FuelMain")]
    fuel_main: f64,
    #[serde(rename = "FuelReservoir")]
    fuel_reservoir: f64,
}

fn read_status_json(journal_dir: &Path) -> StatusFile {
    std::fs::read_to_string(journal_dir.join("Status.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Scans journal lines (oldest to newest) and keeps updating a running
/// status snapshot -- so the caller gets whatever was last true as of the
/// end of the scanned range, same idea as the materials replay.
pub fn scan_for_status(lines: impl Iterator<Item = String>, status: &mut ShipStatus) {
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(ev) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(etype) = ev.get("event").and_then(Value::as_str) else {
            continue;
        };

        match etype {
            "Location" | "FSDJump" | "CarrierJump" => {
                if let Some(sys) = ev.get("StarSystem").and_then(Value::as_str) {
                    status.current_system = Some(sys.to_string());
                }
                status.docked = ev
                    .get("Docked")
                    .and_then(Value::as_bool)
                    .unwrap_or(status.docked);
                status.station_name = ev
                    .get("StationName")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or(status.station_name.take());
            }
            "Docked" => {
                status.docked = true;
                if let Some(name) = ev.get("StationName").and_then(Value::as_str) {
                    status.station_name = Some(name.to_string());
                }
            }
            "Undocked" => {
                status.docked = false;
                status.station_name = None;
            }
            "Loadout" => {
                status.cargo_capacity = ev.get("CargoCapacity").and_then(Value::as_i64);
                status.ship_name = ev
                    .get("ShipName")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| ev.get("Ship").and_then(Value::as_str).map(str::to_string));
            }
            _ => {}
        }
    }
}

pub fn current_status(
    journal_dir: &Path,
    journal_lines: impl Iterator<Item = String>,
) -> ShipStatus {
    let mut status = ShipStatus::default();
    scan_for_status(journal_lines, &mut status);

    let status_file = read_status_json(journal_dir);
    if let Some(fuel) = status_file.fuel {
        status.fuel_main = Some(fuel.fuel_main);
        status.fuel_reservoir = Some(fuel.fuel_reservoir);
    }
    status.cargo_count = status_file.cargo.map(|c| c as i64);

    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_system_and_dock_state_across_events() {
        let lines = vec![
            r#"{"event":"FSDJump","StarSystem":"Paesia"}"#.to_string(),
            r#"{"event":"Docked","StationName":"Ramon City"}"#.to_string(),
        ];
        let mut status = ShipStatus::default();
        scan_for_status(lines.into_iter(), &mut status);
        assert_eq!(status.current_system.as_deref(), Some("Paesia"));
        assert!(status.docked);
        assert_eq!(status.station_name.as_deref(), Some("Ramon City"));
    }

    #[test]
    fn undock_clears_station_but_keeps_system() {
        let lines = vec![
            r#"{"event":"FSDJump","StarSystem":"Paesia"}"#.to_string(),
            r#"{"event":"Docked","StationName":"Ramon City"}"#.to_string(),
            r#"{"event":"Undocked","StationName":"Ramon City"}"#.to_string(),
        ];
        let mut status = ShipStatus::default();
        scan_for_status(lines.into_iter(), &mut status);
        assert_eq!(status.current_system.as_deref(), Some("Paesia"));
        assert!(!status.docked);
        assert_eq!(status.station_name, None);
    }
}
