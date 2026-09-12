//! The station and system lookups, on the API (API-only spec, Phase
//! B.2, then B.4 on 2026-09-09): `GET /v1/stations`, the knowledge
//! endpoints and `/v1/names/complete`, deserialized straight into the
//! types the panels and the ship computer render. `None` means the
//! server could not be asked or did not answer usably; the caller turns
//! that into [`api_down`] - there is no local table to fall back to
//! (maintainer: "Local search should be limited to journal data").

use crate::exchange::SendApi;
use ed_store::lookup::{MarketEntry, NearbySystem, StationInfo, StationWithService, SystemInfo};

use crate::capabilities::galaxy::{
    FindStationRequest, FindSystemRequest, NearestServiceRequest, StationsInSystemRequest,
    SystemsNearRequest,
};
use crate::capabilities::CapError;
use crate::state::AppState;

const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// The one error every lookup returns when the API gave no usable
/// answer: retryable, named, and never dressed up as "nothing there".
pub fn api_down(what: &str) -> CapError {
    CapError::unavailable(format!("the community API did not answer ({what}); check the connection and try again in a moment"), true)
        .hint("EDDA's galaxy data lives on the community API; the journal alone cannot answer this")
}

/// One `/v1/stations` call; the rows as JSON, or `None` with the reason
/// logged.
async fn fetch(
    state: &AppState,
    what: &'static str,
    query: &[(&str, String)],
) -> Option<Vec<serde_json::Value>> {
    let api = crate::exchange::endpoint(state)?;
    let started = std::time::Instant::now();
    let response = state
        .http
        .get(format!("{api}/v1/stations"))
        .query(query)
        .timeout(TIMEOUT)
        .send_api()
        .await;
    let ms = started.elapsed().as_millis() as u64;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::info!(what, status = %response.status(), ms, "stations: server declined");
            return None;
        }
        Err(error) => {
            tracing::info!(what, %error, ms, "stations: server unreachable");
            return None;
        }
    };
    match response.json::<Vec<serde_json::Value>>().await {
        Ok(rows) => {
            tracing::info!(what, rows = rows.len(), ms, "stations served by API");
            Some(rows)
        }
        Err(error) => {
            tracing::info!(what, %error, ms, "stations: unreadable answer");
            None
        }
    }
}

/// A `/v1/stations` row is `StationInfo`'s shape (plus `distance_ly`,
/// which serde ignores here).
fn station(row: &serde_json::Value) -> Option<StationInfo> {
    serde_json::from_value(row.clone()).ok()
}

pub async fn stations_in_system(
    state: &AppState,
    req: &StationsInSystemRequest,
) -> Option<Vec<StationInfo>> {
    let rows = fetch(
        state,
        "system",
        &[
            ("system", req.system.clone()),
            ("include_carriers", req.include_carriers.to_string()),
            ("include_minor", req.include_minor.to_string()),
            ("limit", "100".into()),
        ],
    )
    .await?;
    rows.iter().map(station).collect()
}

pub async fn find_station(state: &AppState, req: &FindStationRequest) -> Option<Vec<StationInfo>> {
    let rows = fetch(
        state,
        "name",
        &[
            ("name", req.name.clone()),
            (
                "limit",
                crate::capabilities::galaxy::FIND_STATION_LIMIT.to_string(),
            ),
        ],
    )
    .await?;
    rows.iter().map(station).collect()
}

/// Nearest with a service; the origin resolves here (the request's
/// system or the commander's current one) so the answer names it, as
/// the local path does.
pub async fn nearest_service(
    state: &AppState,
    req: &NearestServiceRequest,
) -> Option<(String, Vec<StationWithService>)> {
    let system = {
        let conn = state.read_conn().ok()?;
        crate::capabilities::galaxy::system_or_current(&conn, req.system.as_deref()).ok()?
    };
    let mut query = vec![
        ("near", system.clone()),
        ("service", req.service_key()),
        ("radius_ly", req.radius_ly.to_string()),
        ("include_carriers", req.include_carriers.to_string()),
        (
            "limit",
            crate::capabilities::galaxy::NEAREST_SERVICE_LIMIT.to_string(),
        ),
    ];
    if let Some(pad) = &req.min_pad {
        query.push(("min_pad", pad.clone()));
    }
    let rows = fetch(state, "near", &query).await?;
    let hits: Option<Vec<StationWithService>> = rows
        .iter()
        .map(|row| {
            Some(StationWithService {
                station: station(row)?,
                distance_ly: row.get("distance_ly")?.as_f64()?,
            })
        })
        .collect();
    Some((system, hits?))
}

/// One `/v1/knowledge/system` answer (EDSM shape: coords, information,
/// primaryStar) as the panels' `SystemInfo`. `Some(None)` is a real
/// "no such system"; `None` means the server gave no answer.
async fn knowledge_system(state: &AppState, name: &str) -> Option<Option<serde_json::Value>> {
    let api = crate::exchange::endpoint(state)?;
    let response = state
        .http
        .get(format!("{api}/v1/knowledge/system"))
        .query(&[("name", name)])
        .timeout(TIMEOUT)
        .send_api()
        .await
        .ok()?;
    match response.status().as_u16() {
        200 => Some(response.json::<serde_json::Value>().await.ok()),
        404 | 422 => Some(None),
        _ => None,
    }
}

fn system_info(v: &serde_json::Value, station_count: i64) -> Option<SystemInfo> {
    let info = v
        .get("information")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let text = |k: &str| info.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let coords = v.get("coords").and_then(|c| {
        Some((
            c.get("x")?.as_f64()?,
            c.get("y")?.as_f64()?,
            c.get("z")?.as_f64()?,
        ))
    });
    Some(SystemInfo {
        id64: v.get("id64").and_then(|x| x.as_i64()),
        name: v.get("name")?.as_str()?.to_string(),
        coords,
        allegiance: text("allegiance"),
        government: text("government"),
        primary_economy: text("economy"),
        security: text("security"),
        population: info.get("population").and_then(|x| x.as_i64()),
        controlling_power: text("controllingPower"),
        power_state: text("powerState"),
        power_provenance: None,
        power_observed: None,
        control_progress: None,
        station_count,
    })
}

/// `find_system` on the API: the knowledge endpoint (index → Postgres →
/// EDSM on the server) plus a station count from `/v1/stations`.
pub async fn find_system(state: &AppState, req: &FindSystemRequest) -> Option<Option<SystemInfo>> {
    let answer = knowledge_system(state, req.name.trim()).await?;
    let Some(value) = answer else {
        return Some(None);
    };
    let stations = fetch(
        state,
        "system",
        &[
            ("system", req.name.trim().to_string()),
            ("include_carriers", "true".into()),
            ("limit", "100".into()),
        ],
    )
    .await
    .map(|rows| rows.len() as i64)
    .unwrap_or(0);
    Some(system_info(&value, stations))
}

/// `systems_near` on the API: the origin's coordinates from the
/// knowledge endpoint, then one sphere (the server caps it at 100 ly)
/// carrying distance, population and Powerplay per system.
pub async fn systems_near(state: &AppState, req: &SystemsNearRequest) -> Option<Vec<NearbySystem>> {
    let origin = knowledge_system(state, req.system.trim()).await??;
    let c = origin.get("coords")?;
    let (x, y, z) = (
        c.get("x")?.as_f64()?,
        c.get("y")?.as_f64()?,
        c.get("z")?.as_f64()?,
    );
    let api = crate::exchange::endpoint(state)?;
    let started = std::time::Instant::now();
    let items: Vec<serde_json::Value> = state
        .http
        .get(format!("{api}/v1/knowledge/sphere"))
        .query(&[
            ("x", x.to_string()),
            ("y", y.to_string()),
            ("z", z.to_string()),
            ("radius", req.radius_ly.clamp(1.0, 100.0).to_string()),
        ])
        .timeout(TIMEOUT)
        .send_api()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    let mut out: Vec<NearbySystem> = items
        .iter()
        .filter_map(|i| {
            Some(NearbySystem {
                name: i.get("name")?.as_str()?.to_string(),
                id64: i.get("id64")?.as_i64()?,
                distance_ly: i.get("distance")?.as_f64()?,
                controlling_power: i
                    .get("controllingPower")
                    .and_then(|x| x.as_str())
                    .map(str::to_string),
                power_state: i
                    .get("powerState")
                    .and_then(|x| x.as_str())
                    .map(str::to_string),
                population: i.get("population").and_then(|x| x.as_i64()),
            })
        })
        .filter(|n| n.distance_ly <= req.radius_ly)
        .collect();
    out.sort_by(|a, b| a.distance_ly.total_cmp(&b.distance_ly));
    out.truncate(crate::capabilities::galaxy::SYSTEMS_NEAR_LIMIT);
    tracing::info!(
        systems = out.len(),
        ms = started.elapsed().as_millis() as u64,
        "systems near served by API"
    );
    Some(out)
}

/// `GET /v1/names/complete` - the galaxy-wide completion behind the
/// search boxes. Short timeout: this runs per keystroke and a slow
/// answer is worse than a shorter list. `None` with the reason logged
/// (never the prefix) when the server cannot be asked.
pub async fn complete_names(
    state: &AppState,
    kind: crate::routing::NameKind,
    prefix: &str,
    limit: usize,
) -> Option<Vec<crate::routing::NameHit>> {
    let api = crate::exchange::endpoint(state)?;
    let kind = match kind {
        crate::routing::NameKind::System => "system",
        crate::routing::NameKind::Station => "station",
    };
    let started = std::time::Instant::now();
    let response = state
        .http
        .get(format!("{api}/v1/names/complete"))
        .query(&[
            ("kind", kind),
            ("prefix", prefix),
            ("limit", &limit.to_string()),
        ])
        .timeout(std::time::Duration::from_secs(3))
        .send_api()
        .await;
    let ms = started.elapsed().as_millis() as u64;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::info!(kind, status = %response.status(), ms, "names: server declined");
            return None;
        }
        Err(error) => {
            tracing::info!(kind, %error, ms, "names: server unreachable");
            return None;
        }
    };
    match response.json::<Vec<crate::routing::NameHit>>().await {
        Ok(hits) => {
            tracing::debug!(kind, hits = hits.len(), ms, "names served by API");
            Some(hits)
        }
        Err(error) => {
            tracing::info!(kind, %error, ms, "names: unreadable answer");
            None
        }
    }
}

/// Economies whose stations host each material-trader kind: the dump
/// (and so the server) says only "Material Trader"; the kind follows
/// the station economy, as the trader-finding guides have it.
fn trader_economies(kind: &str) -> Option<&'static [&'static str]> {
    Some(match kind.trim().to_ascii_lowercase().as_str() {
        "raw" => &["Extraction", "Refinery"],
        "manufactured" => &["Industrial"],
        "encoded" => &["High Tech", "Military"],
        _ => return None,
    })
}

/// Nearest material traders of one kind (`raw` / `manufactured` /
/// `encoded`) around a system: the API's nearest `material_trader`
/// stations, kept when their economy hosts that kind. `None` when the
/// kind is unknown or the API gave no answer.
pub async fn nearest_material_traders(
    state: &AppState,
    system: &str,
    kind: &str,
    radius_ly: f64,
    limit: usize,
) -> Option<Vec<StationWithService>> {
    let economies = trader_economies(kind)?;
    let req = NearestServiceRequest {
        system: Some(system.to_string()),
        service: "material_trader".into(),
        min_pad: None,
        radius_ly,
        include_carriers: false,
    };
    let (_, hits) = nearest_service(state, &req).await?;
    let mut out: Vec<StationWithService> = hits
        .into_iter()
        .filter(|h| {
            h.station
                .primary_economy
                .as_deref()
                .is_some_and(|e| economies.iter().any(|x| x.eq_ignore_ascii_case(e)))
        })
        .collect();
    out.truncate(limit);
    Some(out)
}

/// What `system` offers a ship that needs `pad`: (a non-carrier station
/// whose pads fit, a fleet carrier present). From a plain thread (the
/// fuel trap runs on the watcher): one `/v1/stations` call. `None` when
/// the API gave no answer - callers fail closed.
pub fn stations_at_blocking(
    state: &AppState,
    system: &str,
    pad: ed_store::lookup::PadSize,
) -> Option<(bool, bool)> {
    let req = StationsInSystemRequest {
        system: system.to_string(),
        include_carriers: true,
        include_minor: true,
    };
    let stations = tauri::async_runtime::block_on(stations_in_system(state, &req))?;
    let fits = stations
        .iter()
        .any(|s| !s.is_carrier && s.max_pad.is_some_and(|p| p.fits(pad)));
    let carrier = stations.iter().any(|s| s.is_carrier);
    Some((fits, carrier))
}

/// The nearest system (distance, name) within `radius_ly` of `system`
/// with a non-carrier refuel station whose pads fit `pad`, from a plain
/// thread. `None` when there is none or the API gave no answer.
pub fn nearest_refuel_blocking(
    state: &AppState,
    system: &str,
    pad: ed_store::lookup::PadSize,
    radius_ly: f64,
) -> Option<(f32, String)> {
    let req = NearestServiceRequest {
        system: Some(system.to_string()),
        service: "refuel".into(),
        min_pad: Some(format!("{pad:?}").to_ascii_lowercase()),
        radius_ly: radius_ly.clamp(1.0, 500.0),
        include_carriers: false,
    };
    let (_, hits) = tauri::async_runtime::block_on(nearest_service(state, &req))?;
    hits.into_iter()
        .filter(|h| h.distance_ly > 0.0)
        .min_by(|a, b| a.distance_ly.total_cmp(&b.distance_ly))
        .and_then(|h| Some((h.distance_ly as f32, h.station.system_name?)))
}

/// Which of `systems` hold a non-carrier dock whose pads fit `pad`:
/// `GET /v1/stations?systems=a,b,c` (the assistant session, 2fe8099), 200 names per
/// call. Lower-cased system names. `None` when any call went unanswered
/// - callers treat that as "no dock known" and warn rather than guess.
pub async fn docks_by_systems(
    state: &AppState,
    systems: &[String],
    pad: ed_store::lookup::PadSize,
) -> Option<std::collections::HashSet<String>> {
    let mut docks = std::collections::HashSet::new();
    for chunk in systems.chunks(200) {
        let rows = fetch(
            state,
            "systems",
            &[
                ("systems", chunk.join(",")),
                ("include_carriers", "false".into()),
                ("include_minor", "true".into()),
                ("limit", "1000".into()),
            ],
        )
        .await?;
        for st in rows.iter().filter_map(station) {
            if !st.is_carrier && st.max_pad.is_some_and(|p| p.fits(pad)) {
                if let Some(name) = st.system_name {
                    docks.insert(name.to_ascii_lowercase());
                }
            }
        }
    }
    Some(docks)
}

/// `POST /v1/mining/search` (the assistant session, d7bbc2c): ring hotspots, rings by
/// type and landable bodies around the journal's position. The answer
/// is the Mining page's shape minus the marks. `None` when the server
/// gave no usable answer.
pub async fn mining_search(
    state: &AppState,
    text: &str,
    system: &str,
    coords: (f64, f64, f64),
    radius_ly: f64,
) -> Option<serde_json::Value> {
    let api = crate::exchange::endpoint(state)?;
    let started = std::time::Instant::now();
    let body = serde_json::json!({ "text": text, "system": system, "coords": [coords.0, coords.1, coords.2], "radius_ly": radius_ly });
    let response = state
        .http
        .post(format!("{api}/v1/mining/search"))
        .json(&body)
        .timeout(TIMEOUT)
        .send_api()
        .await;
    let ms = started.elapsed().as_millis() as u64;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            tracing::info!(status = %response.status(), ms, "mining search: server declined");
            return None;
        }
        Err(error) => {
            tracing::info!(%error, ms, "mining search: server unreachable");
            return None;
        }
    };
    let value: serde_json::Value = response.json().await.ok()?;
    value.get("hotspots")?;
    tracing::info!(
        hotspots = value["hotspots"].as_array().map_or(0, |a| a.len()),
        rings = value["rings"].as_array().map_or(0, |a| a.len()),
        bodies = value["bodies"].as_array().map_or(0, |a| a.len()),
        ms,
        "mining search served by API"
    );
    Some(value)
}

/// `GET /v1/mining/materials` - the hotspot/surface vocabulary and the
/// laser list for the Mining autocomplete. `None` when unanswered.
pub async fn mining_materials(state: &AppState) -> Option<serde_json::Value> {
    let api = crate::exchange::endpoint(state)?;
    let value: serde_json::Value = state
        .http
        .get(format!("{api}/v1/mining/materials"))
        .timeout(TIMEOUT)
        .send_api()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    value.get("entries")?;
    Some(value)
}

/// `GET /v1/market/station/{id}` - one station's board. `None` when the
/// server gave no usable answer.
pub async fn station_board(state: &AppState, station_id: i64) -> Option<Vec<MarketEntry>> {
    let api = crate::exchange::endpoint(state)?;
    let started = std::time::Instant::now();
    let response = state
        .http
        .get(format!("{api}/v1/market/station/{station_id}"))
        .timeout(std::time::Duration::from_secs(8))
        .send_api()
        .await
        .ok()?;
    let ms = started.elapsed().as_millis() as u64;
    if !response.status().is_success() {
        tracing::info!(status = %response.status(), ms, "station board: server declined");
        return None;
    }
    let value = response.json::<serde_json::Value>().await.ok()?;
    let entries: Vec<MarketEntry> = serde_json::from_value(value.get("entries")?.clone()).ok()?;
    tracing::info!(
        station_id,
        entries = entries.len(),
        ms,
        "station board served by API"
    );
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_store::lookup::{PadSize, StationClass};

    fn row() -> serde_json::Value {
        serde_json::json!({
            "id": 22, "name": "B Dock", "system_name": "Beta", "kind": "Coriolis", "class": "starport",
            "distance_to_arrival": 120.5, "primary_economy": null, "government": null,
            "controlling_faction": null, "max_pad": "large", "has_market": true, "has_outfitting": false,
            "has_shipyard": false, "is_carrier": false, "updated": "2026-09-07 12:00:00+00",
            "age_hours": 1.5, "distance_ly": 10.0
        })
    }

    /// A server row lands in the type the panels render, class and pad
    /// included; a `near` row carries its distance alongside.
    /// A knowledge/system answer lands in the Galaxy tab's SystemInfo,
    /// Powerplay included, and a bare answer still maps.
    #[test]
    fn a_knowledge_answer_is_a_system_info() {
        let v = serde_json::json!({
            "name": "Deciat", "id64": 6681123623626_i64, "coords": {"x": 122.625, "y": -0.8125, "z": -47.28125},
            "information": {"allegiance": "Independent", "government": "Corporate", "population": 31778844,
                            "security": "High", "economy": "Industrial", "controllingPower": "Li Yong-Rui",
                            "powerState": "Stronghold", "powers": ["Li Yong-Rui"]},
            "primaryStar": {"type": "K (Yellow-Orange) Star", "isScoopable": true}
        });
        let info = system_info(&v, 12).expect("maps");
        assert_eq!(info.name, "Deciat");
        assert_eq!(info.coords, Some((122.625, -0.8125, -47.28125)));
        assert_eq!(info.controlling_power.as_deref(), Some("Li Yong-Rui"));
        assert_eq!(info.power_state.as_deref(), Some("Stronghold"));
        assert_eq!(info.primary_economy.as_deref(), Some("Industrial"));
        assert_eq!(info.population, Some(31778844));
        assert_eq!(info.station_count, 12);
        let bare = system_info(&serde_json::json!({"name": "Nowhere"}), 0)
            .expect("a bare answer still maps");
        assert!(bare.coords.is_none() && bare.controlling_power.is_none());
    }

    #[test]
    fn a_stations_row_is_a_station_info() {
        let info = station(&row()).expect("row parses");
        assert_eq!(info.id, 22);
        assert_eq!(info.system_name.as_deref(), Some("Beta"));
        assert_eq!(info.class, StationClass::Starport);
        assert_eq!(info.max_pad, Some(PadSize::Large));
        assert_eq!(info.age_hours, Some(1.5));
        let with = StationWithService {
            station: info,
            distance_ly: row()["distance_ly"].as_f64().unwrap(),
        };
        assert_eq!(with.distance_ly, 10.0);
        assert!(station(&serde_json::json!({"id": "not a number"})).is_none());
    }
}
