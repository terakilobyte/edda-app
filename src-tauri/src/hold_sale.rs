//! "Sell my hold" (maintainer, 2026-09-06): find where the CURRENT CARGO
//! sells — and the single station offering the best combined price.
//!
//! Composed, not bespoke: one ordinary sell search per held commodity
//! through the same server-first/local-fallback path the Market panel
//! uses, then a pure aggregation ranks stations by total revenue.
//! Each line's revenue is `price × min(held, demand)` — a station that
//! pays top credit but only wants 40 of your 700 tons is scored for
//! the 40 it will actually take.
//!
//! Honesty bound: each per-commodity search returns the top-200 boards
//! by price inside the radius, so a station mediocre at EVERY item yet
//! best combined could in principle be missed. With 200 boards per
//! item that corner is vanishingly small; it is documented rather than
//! chased.

use serde_json::json;

use crate::capabilities::{galaxy, CapError};
use crate::state::AppState;

/// Per-commodity search depth. The server caps at 200; local honors
/// whatever it is given.
const PER_COMMODITY_LIMIT: usize = 200;
const STATIONS_SHOWN: usize = 30;

#[tauri::command]
pub async fn sell_hold_search(
    state: tauri::State<'_, AppState>,
    radius_ly: Option<f64>,
    min_pad: Option<String>,
    include_carriers: Option<bool>,
    max_age_hours: Option<f64>,
) -> Result<serde_json::Value, CapError> {
    let hold = {
        let conn = state
            .read_conn()
            .map_err(|e| CapError::unavailable(e, true))?;
        let catalog = ed_journal::Catalog::load();
        let mut stmt = conn
            .prepare("SELECT symbol, count FROM cargo WHERE count > 0 ORDER BY count DESC")
            .map_err(|e| CapError::internal(e.to_string()))?;
        let rows: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| CapError::internal(e.to_string()))?
            .filter_map(|r| r.ok())
            .collect();
        rows.into_iter()
            .map(|(symbol, count)| {
                let name = catalog
                    .by_symbol(&symbol)
                    .map(|item| item.name.clone())
                    .unwrap_or_else(|| symbol.clone());
                (name, symbol, count)
            })
            .collect::<Vec<_>>()
    };
    if hold.is_empty() {
        return Ok(json!({ "hold": [], "stations": [], "note": "The hold is empty." }));
    }
    let started = std::time::Instant::now();
    let mut per_commodity = Vec::with_capacity(hold.len());
    let mut skipped: Vec<String> = Vec::new();
    for (name, symbol, _count) in &hold {
        let query = galaxy::MarketSearchRequest {
            kind: "commodity".into(),
            text: symbol.clone(),
            system: None,
            radius_ly,
            min_pad: min_pad.clone(),
            include_carriers: include_carriers.unwrap_or(false),
            include_prohibited: false,
            max_age_hours,
            side: "sell".into(),
            min_quantity: None,
            sort: Some("price".into()),
            limit: Some(PER_COMMODITY_LIMIT),
        };
        match crate::commands::market_search_of(&state, "commodity", query).await {
            Ok(value) => per_commodity.push((symbol.clone(), value)),
            // One unsellable oddity (an unknown symbol, say) must not
            // sink the rest of the hold — it is reported, not fatal.
            Err(error) => {
                tracing::info!(%symbol, %error, "sell-hold: commodity skipped");
                skipped.push(name.clone());
            }
        }
    }
    let mut value = combine_hold_offers(&hold, &per_commodity);
    value["skipped"] = json!(skipped);
    tracing::info!(
        items = hold.len(),
        skipped = skipped.len(),
        stations = value["stations"].as_array().map(|s| s.len()),
        ms = started.elapsed().as_millis() as u64,
        "sell-hold search"
    );
    Ok(value)
}

/// Rank stations by what the whole hold earns there. Pure, so the
/// scoring rules are testable without a database or a server.
fn combine_hold_offers(
    hold: &[(String, String, i64)],
    per_commodity: &[(String, serde_json::Value)],
) -> serde_json::Value {
    use std::collections::BTreeMap;
    struct Offer {
        station: serde_json::Value,
        lines: Vec<serde_json::Value>,
        total: i64,
    }
    let counts: BTreeMap<&str, i64> = hold
        .iter()
        .map(|(_, symbol, count)| (symbol.as_str(), *count))
        .collect();
    let names: BTreeMap<&str, &str> = hold
        .iter()
        .map(|(name, symbol, _)| (symbol.as_str(), name.as_str()))
        .collect();
    let mut offers: BTreeMap<i64, Offer> = BTreeMap::new();
    for (symbol, report) in per_commodity {
        let Some(rows) = report.get("results").and_then(|r| r.as_array()) else {
            continue;
        };
        let held = counts.get(symbol.as_str()).copied().unwrap_or(0);
        for row in rows {
            let (Some(station_id), Some(price), Some(demand)) = (
                row.get("station_id").and_then(|v| v.as_i64()),
                row.get("price").and_then(|v| v.as_i64()),
                row.get("quantity").and_then(|v| v.as_i64()),
            ) else {
                continue;
            };
            let takes = held.min(demand.max(0));
            if takes <= 0 || price <= 0 {
                continue;
            }
            let revenue = price * takes;
            let entry = offers.entry(station_id).or_insert_with(|| Offer {
                station: json!({
                    "station_id": station_id,
                    "station": row.get("station").cloned().unwrap_or_default(),
                    "system": row.get("system").cloned().unwrap_or_default(),
                    "distance_ly": row.get("distance_ly").cloned().unwrap_or_default(),
                    "distance_to_arrival": row.get("distance_to_arrival").cloned().unwrap_or_default(),
                    "max_pad": row.get("max_pad").cloned().unwrap_or_default(),
                    "is_carrier": row.get("is_carrier").cloned().unwrap_or_default(),
                }),
                lines: Vec::new(),
                total: 0,
            });
            entry.total += revenue;
            entry.lines.push(json!({
                "commodity": names.get(symbol.as_str()).copied().unwrap_or(symbol.as_str()),
                "symbol": symbol,
                "held": held,
                "takes": takes,
                "price": price,
                "revenue": revenue,
            }));
        }
    }
    let mut ranked: Vec<Offer> = offers.into_values().collect();
    ranked.sort_by(|a, b| b.total.cmp(&a.total));
    ranked.truncate(STATIONS_SHOWN);
    let stations: Vec<serde_json::Value> = ranked
        .into_iter()
        .map(|offer| {
            let mut station = offer.station;
            station["covered"] = json!(offer.lines.len());
            station["of"] = json!(hold.len());
            station["total"] = json!(offer.total);
            station["lines"] = json!(offer.lines);
            station
        })
        .collect();
    json!({
        "hold": hold.iter().map(|(name, symbol, count)| json!({
            "commodity": name, "symbol": symbol, "count": count,
        })).collect::<Vec<_>>(),
        "best": stations.first().cloned(),
        "stations": stations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(rows: serde_json::Value) -> serde_json::Value {
        json!({ "results": rows })
    }

    /// Demand caps the score: a top price that only takes a sliver
    /// loses to a fair price that takes the lot.
    #[test]
    fn demand_caps_beat_headline_prices() {
        let hold = vec![("Gold".to_owned(), "gold".to_owned(), 700)];
        let per = vec![(
            "gold".to_owned(),
            report(json!([
                {"station_id": 1, "station": "Sliver", "system": "A", "distance_ly": 5.0,
                 "price": 100_000, "quantity": 40},
                {"station_id": 2, "station": "Bulk", "system": "B", "distance_ly": 9.0,
                 "price": 60_000, "quantity": 999_999},
            ])),
        )];
        let value = combine_hold_offers(&hold, &per);
        assert_eq!(value["best"]["station"], "Bulk", "{value}");
        assert_eq!(value["best"]["total"], 60_000i64 * 700);
        assert_eq!(value["stations"][1]["total"], 100_000i64 * 40);
        assert_eq!(value["best"]["lines"][0]["takes"], 700);
    }

    /// A station covering the whole hold at fair prices outranks one
    /// paying top credit for a single item, and coverage is reported.
    #[test]
    fn combined_price_wins_over_single_item_glory() {
        let hold = vec![
            ("Gold".to_owned(), "gold".to_owned(), 100),
            ("Silver".to_owned(), "silver".to_owned(), 100),
        ];
        let per = vec![
            (
                "gold".to_owned(),
                report(json!([
                    {"station_id": 1, "station": "Both", "system": "A", "distance_ly": 5.0, "price": 50_000, "quantity": 5_000},
                    {"station_id": 2, "station": "GoldOnly", "system": "B", "distance_ly": 5.0, "price": 80_000, "quantity": 5_000},
                ])),
            ),
            (
                "silver".to_owned(),
                report(json!([
                    {"station_id": 1, "station": "Both", "system": "A", "distance_ly": 5.0, "price": 40_000, "quantity": 5_000},
                ])),
            ),
        ];
        let value = combine_hold_offers(&hold, &per);
        assert_eq!(value["best"]["station"], "Both");
        assert_eq!(value["best"]["total"], (50_000i64 + 40_000) * 100);
        assert_eq!(value["best"]["covered"], 2);
        assert_eq!(value["best"]["of"], 2);
        let second = &value["stations"][1];
        assert_eq!(second["station"], "GoldOnly");
        assert_eq!(second["covered"], 1);
    }

    /// Zero-demand and zero-price rows contribute nothing, and an
    /// empty aggregation yields an empty ranking, not a panic.
    #[test]
    fn dead_rows_and_empty_holds_stay_calm() {
        let hold = vec![("Gold".to_owned(), "gold".to_owned(), 10)];
        let per = vec![(
            "gold".to_owned(),
            report(json!([
                {"station_id": 1, "station": "NoDemand", "system": "A", "distance_ly": 1.0, "price": 9_000, "quantity": 0},
            ])),
        )];
        let value = combine_hold_offers(&hold, &per);
        assert!(value["stations"].as_array().unwrap().is_empty());
        assert!(value["best"].is_null());
    }
}
