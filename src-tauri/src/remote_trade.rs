//! Remote-mode trade finding: `/v1/trade/search` v2 answers with the
//! SAME `ProfitReport` the local finder produces, computed on the
//! server through the finder's own pipeline (API-only spec, Phase B.1;
//! maintainer, 2026-09-07: "the server API assumes there is no local data,
//! ever"). Nothing is paired or timed here any more: the client sends
//! its resolved plan — ship and constraints — and renders what comes
//! back. Before this, the client paired round trips from the server's
//! top-100 legs and never found one (ledger, first defect under the
//! ruling).
//!
//! Since B.4 (2026-09-09) this is the only finder the client has: the
//! journal names the docked station and the server resolves it to a
//! station id through `/v1/stations`; a server that does not answer
//! usably is a named, retryable error, never an empty report.

use ed_route::profit::ProfitReport;
use crate::exchange::SendApi;
use ed_route::request::{self, ProfitRequest};
use serde_json::json;

use crate::capabilities::CapError;
use crate::state::AppState;

const TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

pub async fn report(state: &AppState, req: &ProfitRequest) -> Result<ProfitReport, CapError> {
    let conn = state.read_conn().map_err(|e| CapError::unavailable(e, true))?;
    let (system_name, plan, docked) = {
        let system = crate::capabilities::trade::origin_for(&conn, req)
            .ok_or_else(|| CapError::invalid("no system given and the current location is unknown").hint("pass system"))?;
        let pledged = crate::capabilities::trade::pledged_power(&conn);
        let ship = crate::capabilities::trade::ship_for(&conn, req);
        let mut plan = request::plan(req, &ship, pledged.as_deref())?;
        // The pilot's own leg timing, sampled only while EDDA followed a
        // trade route (maintainer, 2026-09-09): a phase with enough samples
        // replaces the server's constant; the rest ride the defaults.
        let profile = crate::trade_timing::profile(&conn, ship.hull.as_deref());
        let timing = profile.timing();
        tracing::info!(
            samples = ?profile.counts(),
            jump = timing.jump_seconds, undock = timing.undock_seconds, docking = timing.docking_seconds,
            supercruise = timing.supercruise_base_seconds, market = timing.market_seconds, measured = timing.measured,
            "trade search: leg timing"
        );
        plan.constraints.timing = timing;
        let docked = if req.from_current_station.unwrap_or(false) {
            Some(
                ed_store::query::location(&conn)?
                    .filter(|l| l.docked)
                    .and_then(|l| l.station_name)
                    .ok_or_else(|| CapError::invalid("not docked, so there is no current station to buy from").hint("drop from_current_station to search every station in range"))?,
            )
        } else {
            None
        };
        (system, plan, docked)
    };
    drop(conn);
    let api = crate::exchange::endpoint(state).ok_or_else(|| crate::remote_lookup::api_down("no API endpoint"))?;
    // Buying only at the docked station: the journal knows its name, the
    // server knows its id.
    let from_station_id = match (req.from_station_id, docked) {
        (Some(id), _) => Some(id),
        (None, Some(name)) => {
            let lookup = crate::capabilities::galaxy::StationsInSystemRequest { system: system_name.clone(), include_carriers: true, include_minor: true };
            let stations = crate::remote_lookup::stations_in_system(state, &lookup)
                .await
                .ok_or_else(|| crate::remote_lookup::api_down("stations"))?;
            Some(
                stations
                    .into_iter()
                    .find(|s| s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&name)))
                    .map(|s| s.id)
                    .ok_or_else(|| CapError::not_found(format!("the community API does not list {name:?} in {system_name}")).hint("the station may be newly built; search every station in range instead"))?,
            )
        }
        (None, None) => None,
    };
    // The commander's own board (Market.json) rides along when the
    // search sources from that very station: the server uses it for
    // this one request when it is newer than EDDN's copy, and never
    // keeps it (the assistant's gap-2 contract, 2026-09-10).
    let board = match from_station_id {
        Some(id) => state
            .read_conn()
            .ok()
            .and_then(|conn| ed_store::session::snapshot_raw(&conn, "Market.json").ok().flatten())
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|market| docked_board(&market, id)),
        None => None,
    };
    if let Some(board) = &board {
        tracing::info!(station_id = from_station_id, rows = board["rows"].as_array().map_or(0, |r| r.len()), "trade search: sending the commander's own board");
    }
    let body = json!({
        "system": system_name,
        "ship": plan.ship,
        "constraints": plan.constraints,
        "from_station_id": from_station_id,
        "board": board,
        "limit": req.limit(),
    });
    let started = std::time::Instant::now();
    let response = state
        .http
        .post(format!("{api}/v1/trade/search"))
        .json(&body)
        .timeout(TOTAL_TIMEOUT)
        .send_api()
        .await;
    let ms = started.elapsed().as_millis() as u64;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            tracing::info!(%status, ms, detail = %detail.chars().take(200).collect::<String>(), "trade search: server declined");
            return Err(match status.as_u16() {
                400 | 422 => CapError::invalid(format!("the community API refused the search: {}", detail.trim())),
                429 => CapError::unavailable("the community API is rate-limiting this install; wait a moment and search again", true),
                _ => crate::remote_lookup::api_down(&format!("trade search, HTTP {}", status.as_u16())),
            });
        }
        Err(error) => {
            tracing::info!(%error, ms, "trade search: server unreachable");
            return Err(crate::remote_lookup::api_down("trade search"));
        }
    };
    let value: serde_json::Value = response.json().await.map_err(|_| crate::remote_lookup::api_down("trade search answer"))?;
    let mut report = parse_report(&value).ok_or_else(|| {
        tracing::info!(ms, "trade search: server answered in the legacy shape (older server)");
        CapError::unavailable("the community API is older than this EDDA and answered without round trips; it updates shortly", true)
    })?;
    request::resolve_commodity_names(&mut report, &ed_journal::Catalog::load());
    if let Some(board) = value.get("board") {
        // Which board the server used for a docked-station search
        // (5793500): newer / older / absent / mismatch, in the trace.
        tracing::info!(board = %board, "trade search: board verdict");
    }
    tracing::info!(
        legs = report.legs.len(),
        trips = report.round_trips.len(),
        rings = report.rings.len(),
        stations = report.stations_considered,
        ms,
        "trade search served by API"
    );
    Ok(report)
}

/// The v2 answer is the report itself (plus `provenance`/`as_of`, which
/// serde ignores). The legacy answer has no `round_trips` key and its
/// `legs` rows are not `Leg`s, so it fails to parse — and the caller
/// says so rather than showing a report with no loops.
fn parse_report(value: &serde_json::Value) -> Option<ProfitReport> {
    value.get("round_trips")?;
    serde_json::from_value(value.clone()).ok()
}

/// The largest board the wire carries; a fleet carrier's can exceed it.
const BOARD_ROWS_MAX: usize = 400;

/// `Market.json` as the trade request's `board`, when it is the board
/// of `station_id`: `{station_id, observed_at, rows[{symbol, buy_price,
/// sell_price, demand, supply}]}` with symbols as lowercase wire names
/// (`$gold_name;` → `gold`). A board for another station, or one
/// without a timestamp or items, is `None`. Over the cap, the rows with
/// the highest price on either side are kept.
pub fn docked_board(market: &serde_json::Value, station_id: i64) -> Option<serde_json::Value> {
    if market.get("MarketID")?.as_i64()? != station_id {
        return None;
    }
    let observed_at = market.get("timestamp")?.as_str()?;
    let items = market.get("Items")?.as_array()?;
    let n = |item: &serde_json::Value, key: &str| item.get(key).and_then(serde_json::Value::as_i64).unwrap_or(0);
    let mut rows: Vec<(i64, serde_json::Value)> = items
        .iter()
        .filter_map(|item| {
            let symbol = wire_symbol(item.get("Name")?.as_str()?);
            let (buy, sell) = (n(item, "BuyPrice"), n(item, "SellPrice"));
            Some((buy.max(sell), json!({
                "symbol": symbol,
                "buy_price": buy,
                "sell_price": sell,
                "demand": n(item, "Demand"),
                "supply": n(item, "Stock"),
            })))
        })
        .collect();
    if rows.len() > BOARD_ROWS_MAX {
        tracing::info!(rows = rows.len(), kept = BOARD_ROWS_MAX, "trade search: own board truncated to the priciest rows");
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        rows.truncate(BOARD_ROWS_MAX);
    }
    Some(json!({
        "station_id": station_id,
        "observed_at": observed_at,
        "rows": rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>(),
    }))
}

/// `$gold_name;` → `gold`; anything else lower-cased as it came.
fn wire_symbol(name: &str) -> String {
    name.trim()
        .strip_prefix('$')
        .and_then(|s| s.strip_suffix("_name;"))
        .unwrap_or(name.trim())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod board_tests {
    use super::*;

    /// The commander's Market.json becomes the wire board only for its
    /// own station, with wire symbols and journal field names mapped.
    #[test]
    fn market_json_becomes_the_wire_board_for_its_own_station() {
        let market = json!({
            "timestamp": "2026-09-10T03:00:00Z", "event": "Market", "MarketID": 3700000001_i64,
            "Items": [
                {"Name": "$platinum_name;", "BuyPrice": 0, "SellPrice": 42220, "Stock": 0, "Demand": 9182},
                {"Name": "$Tritium_name;", "BuyPrice": 51000, "SellPrice": 0, "Stock": 300, "Demand": 0},
                {"Name": "odd", "BuyPrice": 1}
            ]
        });
        let board = docked_board(&market, 3700000001).expect("own station");
        assert_eq!(board["station_id"], 3700000001_i64);
        assert_eq!(board["observed_at"], "2026-09-10T03:00:00Z");
        let rows = board["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["symbol"], "platinum");
        assert_eq!(rows[0]["sell_price"], 42220);
        assert_eq!(rows[0]["demand"], 9182);
        assert_eq!(rows[1]["symbol"], "tritium");
        assert_eq!(rows[1]["supply"], 300);
        assert_eq!(rows[2]["symbol"], "odd");
        assert!(docked_board(&market, 42).is_none(), "another station's search sends no board");
        assert!(docked_board(&json!({"MarketID": 1, "Items": []}), 1).is_none(), "no timestamp, no board");
        let big: Vec<serde_json::Value> = (0..500).map(|i| json!({"Name": format!("$c{i}_name;"), "BuyPrice": i, "SellPrice": 0})).collect();
        let board = docked_board(&json!({"timestamp": "t", "MarketID": 1, "Items": big}), 1).unwrap();
        let rows = board["rows"].as_array().unwrap();
        assert_eq!(rows.len(), BOARD_ROWS_MAX);
        assert_eq!(rows[0]["symbol"], "c499", "the priciest rows survive the cap");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_store::lookup::{PadSize, StationClass};
    use ed_route::cost::{Confidence, Ship};
    use ed_route::profit::{Constraints, Excluded, MarketRow, SearchTiming, ShipSummary, StationRef};

    fn report() -> ProfitReport {
        let station = |id: i64, x: f64| StationRef {
            station_id: id, station: format!("S{id}"), system: format!("Sys{id}"), system_id64: id * 10,
            x, y: 0.0, z: 0.0, arrival_ls: Some(100.0), max_pad: Some(PadSize::Large),
            class: StationClass::of(Some("Coriolis")), is_carrier: false,
            controlling_power: None, power_state: None, powers: Vec::new(),
        };
        let ship = Ship { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 };
        let row = |st: i64, sym: &str, buy: i64, sell: i64| MarketRow {
            station_id: st, symbol: sym.into(), name: None, buy_price: buy, sell_price: sell,
            demand: if sell > 0 { 1000 } else { 0 }, supply: if buy > 0 { 1000 } else { 0 }, age_hours: 1.0,
        };
        let timing = ed_route::cost::Timing::default();
        let out = ed_route::profit::make_leg(&station(1, 0.0), &station(2, 10.0), &row(1, "gold", 100, 0), &row(2, "gold", 0, 200), &ship, &timing);
        let back = ed_route::profit::make_leg(&station(2, 10.0), &station(1, 0.0), &row(2, "silver", 50, 0), &row(1, "silver", 0, 150), &ship, &timing);
        let trips = ed_route::profit::round_trips(&[out.clone(), back.clone()], 10);
        ProfitReport {
            origin: "Sys1".into(),
            timing: SearchTiming::default(),
            constraints: Constraints::default(),
            ship: ShipSummary { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 },
            stations_considered: 2,
            excluded: Excluded::default(),
            legs: vec![out, back],
            round_trips: trips,
            rings: Vec::new(),
            confidence: Confidence::Estimated,
            note: "server".into(),
            coverage: None,
            offer: None,
            fallback: None,
            reach_ly: None, board: None,
        }
    }

    /// The v2 answer parses whole — loops included — and the legacy
    /// answer (top legs, no `round_trips`) is refused so the caller
    /// falls back to local instead of showing a report with no loops.
    #[test]
    fn v2_parses_whole_and_legacy_is_refused() {
        let mut value = serde_json::to_value(report()).unwrap();
        value["provenance"] = json!("server");
        value["as_of"] = json!("2026-09-07T12:00:00Z");
        let parsed = parse_report(&value).expect("v2 parses");
        assert_eq!(parsed.round_trips.len(), 1);
        assert_eq!(parsed.legs.len(), 2);
        assert_eq!(parsed.round_trips[0].out.commodity, parsed.legs[0].commodity);
        let legacy = json!({
            "origin": "Sol", "provenance": "server", "as_of": "2026-09-07T12:00:00Z",
            "legs": [{"symbol": "gold", "commodity": "Gold", "profit_t": 100, "buy": 1, "sell": 101,
                      "supply": 10, "demand": 10, "buy_age_hours": 1.0, "sell_age_hours": 1.0,
                      "from": {"station_id": 1}, "to": {"station_id": 2}}]
        });
        assert!(parse_report(&legacy).is_none(), "legacy shape is refused");
    }
}
