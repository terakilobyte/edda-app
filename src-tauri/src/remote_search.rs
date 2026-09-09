//! The market searches (commodity / module / ship), on the API - the
//! only place they run since B.4 (2026-09-09): the server's board is
//! live-EDDN fresh and the client holds no market table at all.
//!
//! Only an HTTP 200 with a JSON `results` body is an answer; anything
//! else is [`api_down`](crate::remote_lookup::api_down) - a named,
//! retryable error, never an empty list pretending to be a fact.
//!
//! Privacy invariants on the wire: the origin SYSTEM NAME travels
//! (it's the search's subject), but the hull does not — the pad
//! requirement is resolved locally and sent as an explicit size.

use crate::capabilities::galaxy::{market_search_context, MarketSearchRequest};
use crate::exchange::SendApi;
use crate::capabilities::CapError;
use crate::state::AppState;

/// A healthy-but-busy server gets room to answer (bench P95 is ~0.6 s
/// on a 10x-prod board); a refused connection fails in milliseconds.
/// Only a blackholed host pays the full budget.
const TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// The wire body for POST /v1/market/search, mirrored from the local
/// request plus locally-resolved context.
fn wire_body(
    query: &MarketSearchRequest,
    system: &str,
    min_pad: Option<ed_store::lookup::PadSize>,
) -> serde_json::Value {
    serde_json::json!({
        "kind": query.kind,
        "text": query.text,
        "system": system,
        "radius_ly": query.radius_ly,
        "min_pad": match min_pad {
            Some(ed_store::lookup::PadSize::Small) => "s",
            Some(ed_store::lookup::PadSize::Medium) => "m",
            Some(ed_store::lookup::PadSize::Large) => "l",
            None => "any",
        },
        "include_carriers": query.include_carriers,
        "include_prohibited": query.include_prohibited,
        "max_age_hours": query.max_age_hours,
        "side": query.side,
        "min_quantity": query.min_quantity,
        "sort": query.sort,
        "limit": query.limit,
    })
}

/// The server holds symbols only; display names come from the bundled
/// catalog. Metadata the local search would carry (class, rating,
/// category, ship) stays null on server rows — declared, not smuggled.
fn enrich_names(kind: &str, value: &mut serde_json::Value) {
    let Some(results) = value.get_mut("results").and_then(|r| r.as_array_mut()) else {
        return;
    };
    for row in results {
        let Some(symbol) = row.get("symbol").and_then(|s| s.as_str()).map(str::to_owned) else {
            continue;
        };
        let name = match kind {
            "module" => ed_journal::modules::item_name(&symbol),
            "ship" => ed_journal::ships::display_name(&symbol),
            _ => continue,
        };
        row["name"] = serde_json::Value::String(name);
    }
}

/// One market search on the API. Errors: the local context (no current
/// system, unknown hull) as the typed capability error, or `api_down`.
pub async fn search(state: &AppState, query: &MarketSearchRequest) -> Result<serde_json::Value, CapError> {
    let conn = state.read_conn().map_err(|e| CapError::unavailable(e, true))?;
    let (system, min_pad) = {
        let query = query.clone();
        tauri::async_runtime::spawn_blocking(move || market_search_context(&conn, &query))
            .await
            .map_err(|e| CapError::internal(e.to_string()))??
    };
    let api = crate::exchange::endpoint(state).ok_or_else(|| crate::remote_lookup::api_down("no API endpoint"))?;
    let body = wire_body(query, &system, min_pad);
    let started = std::time::Instant::now();
    let response = state
        .http
        .post(format!("{api}/v1/market/search"))
        .json(&body)
        .timeout(TOTAL_TIMEOUT)
        .send_api()
        .await;
    let ms = started.elapsed().as_millis() as u64;
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            tracing::info!(%error, ms, "market search: server unreachable");
            return Err(crate::remote_lookup::api_down("market search"));
        }
    };
    let status = response.status();
    if !status.is_success() {
        tracing::info!(%status, ms, "market search: server declined");
        return Err(crate::remote_lookup::api_down(&format!("market search, HTTP {}", status.as_u16())));
    }
    match response.json::<serde_json::Value>().await {
        Ok(mut value) if value.get("results").is_some() => {
            let results = value["results"].as_array().map(|r| r.len());
            tracing::info!(kind = %query.kind, results, ms, "market search served by API");
            enrich_names(query.kind.trim(), &mut value);
            Ok(value)
        }
        _ => {
            tracing::info!(ms, "market search: server answer unusable");
            Err(crate::remote_lookup::api_down("market search answer"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_store::lookup::PadSize;

    fn query() -> MarketSearchRequest {
        MarketSearchRequest {
            kind: "commodity".into(),
            text: "Palladium".into(),
            system: None,
            radius_ly: Some(120.0),
            min_pad: None,
            include_carriers: false,
            include_prohibited: true,
            max_age_hours: Some(2.0),
            side: "sell".into(),
            limit: Some(75),
            min_quantity: Some(500),
            sort: Some("distance".into()),
        }
    }

    /// The wire carries the locally-resolved facts: the system by name,
    /// the pad as an explicit size — never the hull.
    #[test]
    fn wire_body_carries_resolved_context_not_the_hull() {
        let body = wire_body(&query(), "Ega", Some(PadSize::Large));
        assert_eq!(body["system"], "Ega");
        assert_eq!(body["min_pad"], "l");
        assert_eq!(body["side"], "sell");
        assert_eq!(body["min_quantity"], 500);
        assert_eq!(body["max_age_hours"], 2.0);
        assert_eq!(body["include_prohibited"], true);
        assert!(body.get("hull").is_none() && body.to_string().to_lowercase().contains("panther") == false);
        let unpadded = wire_body(&query(), "Ega", None);
        assert_eq!(unpadded["min_pad"], "any");
    }

    /// Server module/ship rows carry symbols; the bundled catalog gives
    /// them their display names, and absent metadata stays null.
    #[test]
    fn server_rows_get_catalog_names() {
        let mut value = serde_json::json!({"results": [
            {"symbol": "python", "name": "python", "class": null},
        ]});
        enrich_names("ship", &mut value);
        assert_eq!(value["results"][0]["name"], "Python");
        assert!(value["results"][0]["class"].is_null());
        let mut commodity = serde_json::json!({"results": [{"station": "X", "price": 1}]});
        enrich_names("commodity", &mut commodity);
        assert_eq!(commodity["results"][0]["price"], 1, "commodity rows pass through untouched");
    }
}
