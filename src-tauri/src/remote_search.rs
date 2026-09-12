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

/// What a ship or module search must say to a symbol-only server.
/// The market tables hold journal symbols, and the client owns the
/// symbol->name catalog, so the translation belongs here (maintainer,
/// 2026-09-12: "I can't find a Type-10 Defender" — its symbol is
/// `type9_military`, and the server matched the typed words against that).
/// Commodities are already searched by name server-side and pass through.
fn wire_text(kind: &str, text: &str) -> String {
    match kind {
        "ship" => ed_journal::ships::resolve(text).map(str::to_owned).unwrap_or_else(|| text.to_owned()),
        "module" => ed_journal::modules::search_fragment(text).unwrap_or_else(|| text.to_owned()),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
mod wire_text_tests {
    use super::wire_text;
    /// A hull or module named the way a commander says it; anything the
    /// catalog cannot place goes up unchanged, so the server's own
    /// substring match still gets its chance.
    #[test]
    fn a_ship_or_module_goes_up_as_a_symbol() {
        assert_eq!(wire_text("ship", "Type-10 Defender"), "type9_military");
        assert_eq!(wire_text("ship", "Imperial Cutter"), "cutter");
        assert_eq!(wire_text("ship", "Mandalay"), "mandalay");
        assert_eq!(wire_text("ship", "Krait"), "Krait", "ambiguous: let the server try");
        assert_eq!(wire_text("module", "5A fuel scoop"), "fuelscoop_size5_class5");
        assert_eq!(wire_text("module", "beam laser"), "beamlaser");
        assert_eq!(wire_text("commodity", "Gold"), "Gold");
    }
}

/// The wire body for POST /v1/market/search, mirrored from the local
/// request plus locally-resolved context.
fn wire_body(
    query: &MarketSearchRequest,
    system: &str,
    min_pad: Option<ed_store::lookup::PadSize>,
) -> serde_json::Value {
    serde_json::json!({
        "kind": query.kind,
        "text": wire_text(&query.kind, &query.text),
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
        // Narrow to the Powers that discount this item rather than
        // ranking fifty stations and throwing most away.
        "powers": if query.discounted_only { discount_powers(&query.kind, &wire_text(&query.kind, &query.text)) } else { Vec::new() },
        "include_stronghold_carriers": query.stronghold_carriers.as_deref() != Some("none"),
    })
}

/// The Powers whose space discounts the item being searched for.
fn discount_powers(kind: &str, symbol: &str) -> Vec<String> {
    let item = match kind {
        "ship" => ed_domain::discount::Item::Ship(symbol),
        "module" => ed_domain::discount::Item::Module(symbol),
        _ => return Vec::new(),
    };
    ed_domain::discount::powers_offering(item).into_iter().map(str::to_owned).collect()
}

/// The Power the commander is pledged to, from the journal's own
/// `Powerplay` event; None when unpledged or not yet seen.
fn pledged_power(state: &AppState) -> Option<String> {
    state
        .with_read(|s| Ok::<_, CapError>(ed_store::session::latest_event_raw(s.conn(), "Powerplay")?))
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("Power").and_then(|p| p.as_str()).map(str::to_owned))
}

/// The filters the server cannot apply for us: "my Power's stronghold
/// carriers" needs the commander's pledge, which never leaves the machine,
/// and "discounted only" has to hold even against a server too old to know
/// the `powers` narrowing (the request field is ignored there, not
/// refused, so the rows arrive unfiltered).
fn apply_local_filters(query: &MarketSearchRequest, value: &mut serde_json::Value, pledged: Option<&str>) {
    let mode = query.stronghold_carriers.as_deref().unwrap_or("all");
    let Some(results) = value.get_mut("results").and_then(|r| r.as_array_mut()) else {
        return;
    };
    results.retain(|row| {
        let station = row.get("station").and_then(|v| v.as_str()).unwrap_or_default();
        let stronghold = station == ed_api_stronghold_carrier();
        let keep_stronghold = match mode {
            "none" => !stronghold,
            "mine" => !stronghold || pledged.is_some_and(|p| row.get("controlling_power").and_then(|v| v.as_str()) == Some(p)),
            _ => true,
        };
        let keep_discount = !query.discounted_only
            || row.get("discount_percent").and_then(serde_json::Value::as_f64).unwrap_or(0.0) > 0.0;
        keep_stronghold && keep_discount
    });
}

/// The station name a Power's own carrier always carries. Spelled once
/// here so the client and the server cannot drift.
fn ed_api_stronghold_carrier() -> &'static str {
    "Stronghold Carrier"
}

/// Does the commander hold Elite in any field? The 2.5% galaxy-wide
/// discount is theirs if so, and it stacks with everything else. Read from
/// the journal's own Rank event; unknown reads as "no", so a discount is
/// never promised that the commander cannot get.
fn holds_elite(state: &AppState) -> bool {
    state
        .with_read(|s| Ok::<_, CapError>(ed_store::session::ranks(s.conn())?))
        .map(|r| r.ranks.iter().any(|row| row.name.eq_ignore_ascii_case("Elite")))
        .unwrap_or(false)
}

/// The server holds symbols only; display names come from the bundled
/// catalog. Metadata the local search would carry (class, rating,
/// category, ship) stays null on server rows — declared, not smuggled.
/// The discount is filled in here for the same reason: the rules are
/// static game knowledge the client carries, and the server publishes no
/// prices to compare (2026-09-12).
fn enrich_names(kind: &str, value: &mut serde_json::Value, elite: bool) {
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
        let str_of = |k: &str| row.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        let (station, system) = (str_of("station").unwrap_or_default(), str_of("system").unwrap_or_default());
        let (power, power_state) = (str_of("controlling_power"), str_of("power_state"));
        let at = ed_domain::discount::At {
            station: &station,
            system: &system,
            power: power.as_deref(),
            power_state: power_state.as_deref(),
        };
        let item = if kind == "ship" {
            ed_domain::discount::Item::Ship(&symbol)
        } else {
            ed_domain::discount::Item::Module(&symbol)
        };
        let applied = ed_domain::discount::discounts(item, &at, elite);
        row["discount_percent"] = serde_json::json!(ed_domain::discount::best_percent(&applied));
        row["discounts"] = serde_json::to_value(&applied).unwrap_or(serde_json::Value::Null);
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
            enrich_names(query.kind.trim(), &mut value, holds_elite(state));
            apply_local_filters(query, &mut value, pledged_power(state).as_deref());
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
            discounted_only: false,
            stronghold_carriers: None,
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

    /// The two filters the client owns. A server that ignores `powers`
    /// (an older one) still cannot show an undiscounted row under
    /// "discounted only", and a pledge never goes on the wire.
    #[test]
    fn local_filters_drop_what_the_server_kept() {
        let rows = || serde_json::json!({"results": [
            {"station": "Jameson Memorial", "system": "Shinrarta Dezhra", "discount_percent": 10.0},
            {"station": "Somewhere", "system": "Elsewhere", "discount_percent": 0.0},
            {"station": "Stronghold Carrier", "system": "A", "controlling_power": "Aisling Duval", "discount_percent": 0.0},
            {"station": "Stronghold Carrier", "system": "B", "controlling_power": "Li Yong-Rui", "discount_percent": 15.0},
        ]});
        let q = |discounted: bool, strongholds: Option<&str>| MarketSearchRequest {
            kind: "ship".into(),
            discounted_only: discounted,
            stronghold_carriers: strongholds.map(str::to_owned),
            ..Default::default()
        };

        let mut v = rows();
        apply_local_filters(&q(false, None), &mut v, Some("Aisling Duval"));
        assert_eq!(v["results"].as_array().unwrap().len(), 4, "all four by default");

        let mut v = rows();
        apply_local_filters(&q(true, None), &mut v, None);
        let kept: Vec<&str> = v["results"].as_array().unwrap().iter().map(|r| r["station"].as_str().unwrap()).collect();
        assert_eq!(kept, vec!["Jameson Memorial", "Stronghold Carrier"], "only the discounted rows");

        let mut v = rows();
        apply_local_filters(&q(false, Some("none")), &mut v, Some("Aisling Duval"));
        assert_eq!(v["results"].as_array().unwrap().len(), 2, "no stronghold carriers at all");

        let mut v = rows();
        apply_local_filters(&q(false, Some("mine")), &mut v, Some("Aisling Duval"));
        let systems: Vec<&str> = v["results"].as_array().unwrap().iter().map(|r| r["system"].as_str().unwrap()).collect();
        assert_eq!(systems, vec!["Shinrarta Dezhra", "Elsewhere", "A"], "only the commander's own Power's carrier");

        // Unpledged: "mine" cannot mean anything, so it hides none of them.
        let mut v = rows();
        apply_local_filters(&q(false, Some("mine")), &mut v, None);
        assert_eq!(v["results"].as_array().unwrap().len(), 2, "unpledged keeps the docks, drops carriers it cannot vouch for");
    }

    /// Server module/ship rows carry symbols; the bundled catalog gives
    /// them their display names, and absent metadata stays null.
    #[test]
    fn server_rows_get_catalog_names() {
        let mut value = serde_json::json!({"results": [
            {"symbol": "python", "name": "python", "class": null},
        ]});
        enrich_names("ship", &mut value, false);
        assert_eq!(value["results"][0]["name"], "Python");
        assert!(value["results"][0]["class"].is_null());
        let mut commodity = serde_json::json!({"results": [{"station": "X", "price": 1}]});
        enrich_names("commodity", &mut commodity, false);
        assert_eq!(commodity["results"][0]["price"], 1, "commodity rows pass through untouched");
    }
}
