//! POST /v1/market/search — the server half of the market searches
//! (ledger: agenda item 4 design (b), maintainer-greenlit 2026-09-05 with the
//! thin-client arc).
//!
//! Contract: the response carries the SAME rows the client's local
//! sqlite search produces (`ed_store::lookup::CommoditySearchHit` and
//! friends), so the frontend swap is a data-source toggle, not a
//! rewrite. Differences are declared, not smuggled:
//! - `provenance` is `"server"` and `as_of` stamps the answer, so the
//!   UI can say "as of 2 min ago" against the local "as of yesterday".
//! - module/ship metadata (class, rating, category, ship) is NULL: the
//!   server tables carry symbols only; the client enriches display
//!   fields from its bundled catalog.
//! - the wire takes an explicit pad (or none): "my current hull" is a
//!   client-side fact the server must never learn.
//!
//! Pre-registered targets (ledger, 2026-09-04): P50 < 150 ms / P95 <
//! 600 ms server-side on the sizing box, indexed plans only. The result
//! cache from the design sketch is deliberately NOT built yet — measure
//! first (doctrine rule 1); it lands if the bench misses the targets.

use ed_domain::station::PadSize;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

const DEFAULT_RADIUS_LY: f64 = 100.0;
const MAX_RADIUS_LY: f64 = 500.0;
const DEFAULT_MAX_AGE_HOURS: f64 = 48.0;
const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
// NO demand-sentinel filter here, on purpose. F1's "999999-demand
// poison" was retracted 2026-09-04 (Inara cross-check: real demand
// spike) and re-confirmed 2026-09-06 in the field: every 999999 row on
// a 100M-row board was ONE real station (a busy sell market) using
// the game's unlimited-demand
// convention. This endpoint briefly resurrected the dead filter from
// the ledger's original wording and hid that station; prices and
// demands are shown as reported, exactly like the local search.

#[derive(Debug, Clone, Deserialize)]
pub struct MarketSearchApiRequest {
    /// `commodity`, `module` or `ship`.
    pub kind: String,
    pub text: String,
    /// Origin system name — required on the wire; the server has no
    /// notion of "current system" and must not acquire one.
    pub system: String,
    pub radius_ly: Option<f64>,
    /// `"s" | "m" | "l"`; absent or `"any"` = no pad filter.
    pub min_pad: Option<String>,
    #[serde(default)]
    pub include_carriers: bool,
    #[serde(default)]
    pub include_prohibited: bool,
    /// Commodities only; default 48.
    pub max_age_hours: Option<f64>,
    /// Commodities: `buy` (commander buys) or `sell`.
    #[serde(default, alias = "action")]
    pub side: String,
    pub min_quantity: Option<i64>,
    /// `price` (default) or `distance`.
    pub sort: Option<String>,
    pub limit: Option<i64>,
}

impl MarketSearchApiRequest {
    fn radius(&self) -> f64 {
        self.radius_ly
            .unwrap_or(DEFAULT_RADIUS_LY)
            .clamp(1.0, MAX_RADIUS_LY)
    }
    fn max_age(&self) -> f64 {
        // Cap at 30 days: the fresh-first plan materializes the
        // commodity's fresh slice, and 30 days already exceeds every
        // staleness policy the client offers. An uncapped window would
        // let one request materialize a common commodity's whole
        // 400k-row history.
        self.max_age_hours
            .unwrap_or(DEFAULT_MAX_AGE_HOURS)
            .clamp(0.25, 720.0)
    }
    fn limit(&self) -> i64 {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
    fn min_pad(&self) -> Result<Option<PadSize>, Refusal> {
        match self.min_pad.as_deref().map(str::trim) {
            None | Some("") | Some("any") | Some("Any") => Ok(None),
            Some(text) => PadSize::parse(text)
                .map(Some)
                .ok_or_else(|| Refusal::Invalid(format!("unknown pad size {text:?}"))),
        }
    }
}

#[derive(Debug)]
pub enum Refusal {
    UnknownSystem(String),
    UnknownCommodity { text: String, matches: Vec<String> },
    Invalid(String),
}

/// SQL fragment for the pad floor. Same semantics as the local search:
/// a floor of Small still demands SOME recorded pad — a station whose
/// pads are unknown never satisfies a pad requirement.
pub(crate) use crate::geo::cells_covering;

fn pad_clause(min_pad: Option<PadSize>) -> &'static str {
    match min_pad {
        None => "TRUE",
        Some(PadSize::Small) => {
            "(COALESCE(st.pad_large,0) > 0 OR COALESCE(st.pad_medium,0) > 0 OR COALESCE(st.pad_small,0) > 0)"
        }
        Some(PadSize::Medium) => "(COALESCE(st.pad_large,0) > 0 OR COALESCE(st.pad_medium,0) > 0)",
        Some(PadSize::Large) => "COALESCE(st.pad_large,0) > 0",
    }
}

pub(crate) async fn origin_coords(pool: &PgPool, name: &str) -> Result<(f64, f64, f64), Refusal> {
    let row: Option<(Option<f64>, Option<f64>, Option<f64>)> =
        sqlx::query_as("SELECT x, y, z FROM systems WHERE lower(name) = lower($1)")
            .bind(name)
            .fetch_optional(pool)
            .await
            .map_err(|e| Refusal::Invalid(e.to_string()))?;
    match row {
        Some((Some(x), Some(y), Some(z))) => Ok((x, y, z)),
        _ => Err(Refusal::UnknownSystem(name.to_owned())),
    }
}

async fn resolve_commodity(pool: &PgPool, text: &str) -> Result<(String, String, String), Refusal> {
    let hit: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT symbol, name, category FROM commodities \
         WHERE lower(symbol) = lower($1) OR lower(name) = lower($1) LIMIT 1",
    )
    .bind(text)
    .fetch_optional(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    if let Some((symbol, name, category)) = hit {
        let name = name.unwrap_or_else(|| symbol.clone());
        return Ok((symbol, name, category.unwrap_or_default()));
    }
    let matches: Vec<(String,)> = sqlx::query_as(
        "SELECT COALESCE(name, symbol) FROM commodities \
         WHERE name ILIKE '%' || $1 || '%' OR symbol ILIKE '%' || $1 || '%' \
         ORDER BY 1 LIMIT 8",
    )
    .bind(text)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    Err(Refusal::UnknownCommodity {
        text: text.to_owned(),
        matches: matches.into_iter().map(|(name,)| name).collect(),
    })
}

type StationRow = (
    i64,            // station id
    Option<String>, // station name
    String,         // system name
    f64,            // distance_ly
    Option<f64>,    // arrival_ls
    Option<i32>,    // pad_large
    Option<i32>,    // pad_medium
    Option<i32>,    // pad_small
    bool,           // is_carrier
);

fn station_json(row: &StationRow) -> serde_json::Value {
    json!({
        "station_id": row.0,
        "station": row.1.clone().unwrap_or_default(),
        "system": row.2,
        "distance_ly": row.3,
        "distance_to_arrival": row.4,
        "max_pad": PadSize::from_counts(row.5.map(i64::from), row.6.map(i64::from), row.7.map(i64::from)),
        "is_carrier": row.8,
    })
}

fn merge(base: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    let (mut base, extra) = (base, extra);
    if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    base
}

pub async fn search(
    pool: &PgPool,
    req: &MarketSearchApiRequest,
) -> Result<serde_json::Value, Refusal> {
    let origin = origin_coords(pool, req.system.trim()).await?;
    let min_pad = req.min_pad()?;
    let radius = req.radius();
    let as_of = now_iso();
    let kind = req.kind.trim().to_ascii_lowercase();
    // EVERYTHING that needs its own pool connection happens BEFORE the
    // transaction below takes one. Load bench 2026-09-07: resolving the
    // commodity inside the transaction acquired a SECOND connection per
    // request, so at ≥10 concurrent searches all ten pool connections
    // sat "idle in transaction" after SET LOCAL, each waiting for an
    // eleventh — a pool self-deadlock that stalled every market search
    // (and starved trade search and station completion, which share the
    // pool) until sqlx's acquire timeout. Measured: c=16 for 5 s, 16
    // requests, 0 responses, all "aborted due to deadline".
    let commodity = match kind.as_str() {
        "commodity" => Some(resolve_commodity(pool, req.text.trim()).await?),
        _ => None,
    };
    // No transaction: a read-only search runs each statement on one pool
    // connection, taken and returned around the fetch (maintainer, 2026-09-07:
    // "why are we opening a transaction for a read-only query?"). The
    // planner hint the transaction used to scope — SET LOCAL
    // plan_cache_mode = force_custom_plan, because the generic plan for
    // `commodity_symbol = $1` cannot see the parameter's selectivity and
    // ignores market_commodity_fresh_idx (measured 2026-09-06: 1.8 s
    // generic vs 170 ms custom) — is now `.persistent(false)` on the two
    // search statements: sqlx does not keep the prepared statement, so
    // Postgres plans every execution with the real parameters, ~1 ms of
    // planning, and the EDDN writer's cached plans are untouched.
    let value = match kind.as_str() {
        "commodity" => {
            let (symbol, name, category) = commodity.expect("resolved above for this kind");
            let selling = req.side.trim().eq_ignore_ascii_case("sell");
            let side = if selling { "sell" } else { "buy" };
            let results =
                commodity_rows(pool, origin, &symbol, selling, radius, min_pad, req).await?;
            json!({
                "origin": req.system.trim(), "commodity": name, "symbol": symbol, "category": category,
                "side": side, "results": results, "provenance": "server", "as_of": as_of,
                "price_note": if selling { "station pays commander" } else { "commander pays station" }
            })
        }
        "module" | "outfitting" => {
            let results =
                availability_rows(pool, origin, req, radius, min_pad, Availability::Outfitting)
                    .await?;
            json!({ "origin": req.system.trim(), "query": req.text, "results": results,
                    "provenance": "server", "as_of": as_of })
        }
        "ship" | "shipyard" => {
            let results =
                availability_rows(pool, origin, req, radius, min_pad, Availability::Shipyard)
                    .await?;
            json!({ "origin": req.system.trim(), "query": req.text, "results": results,
                    "provenance": "server", "as_of": as_of })
        }
        other => {
            return Err(Refusal::Invalid(format!(
                "unknown market search kind {other:?}; kind is commodity, module or ship"
            )))
        }
    };
    Ok(value)
}

async fn commodity_rows(
    pool: &PgPool,
    (ox, oy, oz): (f64, f64, f64),
    symbol: &str,
    selling: bool,
    radius: f64,
    min_pad: Option<PadSize>,
    req: &MarketSearchApiRequest,
) -> Result<Vec<serde_json::Value>, Refusal> {
    let (price, quantity, direction) = if selling {
        ("m.sell_price", "m.demand", "DESC")
    } else {
        ("m.buy_price", "m.supply", "ASC")
    };
    // Sell only: honor confiscation —
    // prohibited goods are opt-IN and only shown where a black market
    // makes the sale possible (maintainer ruling 2026-09-04, mirrored from
    // the local search).
    let sell_guards = if !selling {
        String::new()
    } else {
        let prohibited_gate = if req.include_prohibited {
            "OR EXISTS (SELECT 1 FROM station_services sv \
                        WHERE sv.station_id = st.id AND sv.service = 'blackmarket')"
                .to_owned()
        } else {
            String::new()
        };
        // Only the confiscation gate needs the joined station.
        format!(
            "AND (NOT EXISTS (SELECT 1 FROM station_prohibited pr \
                              WHERE pr.station_id = st.id \
                                AND (lower(pr.symbol) = lower($1) OR lower(pr.symbol) = lower($10))) \
                  {prohibited_gate})"
        )
    };
    let order = match req.sort.as_deref().map(str::trim).unwrap_or("price") {
        "" | "price" => format!("{price} {direction}, st.arrival_ls NULLS LAST"),
        "distance" => format!("distance_ly ASC, {price} {direction}"),
        other => {
            return Err(Refusal::Invalid(format!(
                "unknown sort {other:?}; price or distance"
            )))
        }
    };
    // Fresh-first, forced by shape: the MATERIALIZED CTE walks
    // market_commodity_fresh_idx (index-only — every referenced column
    // is INCLUDEd) and the joins fan out from the ~2% fresh slice.
    // Left to its own estimates the planner box-joins 129k stations
    // against the systems sphere instead (measured 2026-09-06:
    // sapphire 531 ms box-first vs 191 ms this shape; gold 175 ms).
    let sql = format!(
        "WITH fresh AS MATERIALIZED ( \
             SELECT station_id, sell_price, buy_price, demand, supply, observed_at \
             FROM market m \
             WHERE commodity_symbol = $1 \
               AND observed_at > now() - make_interval(secs => $6) \
               AND {price} > 0 AND {quantity} >= GREATEST($5::BIGINT, 1) \
         ) \
         SELECT st.id, st.name, sy.name, \
                sqrt((sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$4)^2) AS distance_ly, \
                st.arrival_ls, st.pad_large, st.pad_medium, st.pad_small, \
                COALESCE(st.is_carrier, false), \
                {price}, {quantity}, \
                EXTRACT(EPOCH FROM m.observed_at)::DOUBLE PRECISION \
         FROM fresh m \
         JOIN stations st ON st.id = m.station_id \
         JOIN systems sy ON sy.address = st.system_address \
         WHERE sy.cell = ANY($9) \
           AND (sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$4)^2 <= $7*$7 \
           AND {pad} \
           AND ($8 OR NOT COALESCE(st.is_carrier, false)) \
           {sell_guards} \
         ORDER BY {order} LIMIT {limit}",
        pad = pad_clause(min_pad),
        limit = req.limit(),
    );
    let mut query = sqlx::query_as(&sql)
        .bind(symbol)
        .bind(ox)
        .bind(oy)
        .bind(oz)
        .bind(req.min_quantity.unwrap_or(0))
        .bind(req.max_age() * 3600.0)
        .bind(radius)
        .bind(req.include_carriers)
        .bind(cells_covering(ox, oy, oz, radius));
    if selling {
        // $10 exists only in the sell guards (the confiscation match on
        // the display name); Postgres counts parameters from the SQL.
        query = query.bind(req.text.trim());
    }
    let rows: Vec<(
        i64,
        Option<String>,
        String,
        f64,
        Option<f64>,
        Option<i32>,
        Option<i32>,
        Option<i32>,
        bool,
        i64,
        i64,
        f64,
    )> = query
        // Fresh plan per execution — see `search`.
        .persistent(false)
        .fetch_all(pool)
        .await
        .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let now = now_unix();
    Ok(rows
        .into_iter()
        .map(|r| {
            let station: StationRow = (r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8);
            merge(
                station_json(&station),
                json!({
                    "price": r.9,
                    "quantity": r.10,
                    "updated": iso_from_unix(r.11),
                    "age_hours": (now - r.11).max(0.0) / 3600.0,
                }),
            )
        })
        .collect())
}

enum Availability {
    Outfitting,
    Shipyard,
}

async fn availability_rows(
    pool: &PgPool,
    (ox, oy, oz): (f64, f64, f64),
    req: &MarketSearchApiRequest,
    radius: f64,
    min_pad: Option<PadSize>,
    which: Availability,
) -> Result<Vec<serde_json::Value>, Refusal> {
    let (table, column, observed) = match which {
        Availability::Outfitting => ("outfitting", "module_symbol", "st.outfitting_observed_at"),
        Availability::Shipyard => ("shipyard", "ship_symbol", "st.shipyard_observed_at"),
    };
    let sql = format!(
        "SELECT st.id, st.name, sy.name, \
                sqrt((sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$4)^2) AS distance_ly, \
                st.arrival_ls, st.pad_large, st.pad_medium, st.pad_small, \
                COALESCE(st.is_carrier, false), \
                a.{column}, \
                EXTRACT(EPOCH FROM {observed})::DOUBLE PRECISION \
         FROM {table} a \
         JOIN stations st ON st.id = a.station_id \
         JOIN systems sy ON sy.address = st.system_address \
         WHERE a.{column} ILIKE '%' || $1 || '%' \
           AND sy.cell = ANY($7) \
           AND (sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$4)^2 <= $5*$5 \
           AND {pad} \
           AND ($6 OR NOT COALESCE(st.is_carrier, false)) \
         ORDER BY distance_ly ASC LIMIT {limit}",
        pad = pad_clause(min_pad),
        limit = req.limit(),
    );
    let rows: Vec<(
        i64,
        Option<String>,
        String,
        f64,
        Option<f64>,
        Option<i32>,
        Option<i32>,
        Option<i32>,
        bool,
        String,
        Option<f64>,
    )> = sqlx::query_as(&sql)
        .bind(req.text.trim())
        .bind(ox)
        .bind(oy)
        .bind(oz)
        .bind(radius)
        .bind(req.include_carriers)
        .bind(cells_covering(ox, oy, oz, radius))
        // Fresh plan per execution — see `search`.
        .persistent(false)
        .fetch_all(pool)
        .await
        .map_err(|e| Refusal::Invalid(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let station: StationRow = (r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8);
            merge(
                station_json(&station),
                // Symbol only: the server has no module/ship metadata
                // tables; the client's catalog fills the display fields.
                json!({
                    "symbol": r.9,
                    "name": r.9,
                    "class": null,
                    "rating": null,
                    "category": null,
                    "ship": null,
                    "updated": r.10.map(iso_from_unix),
                }),
            )
        })
        .collect())
}

fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn iso_from_unix(seconds: f64) -> String {
    // Whole-second ISO-8601 without pulling a date crate into the hot
    // path: civil-from-days (Howard Hinnant's algorithm).
    let total = seconds as i64;
    let (days, secs) = (total.div_euclid(86_400), total.rem_euclid(86_400));
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub(crate) fn now_iso() -> String {
    iso_from_unix(now_unix())
}

/// GET /v1/market/station/{id} — one station's full board, shaped like
/// the client's local `MarketEntry` rows so a thin client's station
/// view is the same drop-in the searches are.
pub async fn station_board(pool: &PgPool, station_id: i64) -> Result<serde_json::Value, Refusal> {
    let station: Option<(Option<String>, String)> = sqlx::query_as(
        "SELECT st.name, sy.name FROM stations st \
         JOIN systems sy ON sy.address = st.system_address WHERE st.id = $1",
    )
    .bind(station_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let Some((station_name, system_name)) = station else {
        return Err(Refusal::UnknownSystem(format!("station {station_id}")));
    };
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        i64,
        i64,
        i64,
        i64,
        f64,
    )> = sqlx::query_as(
        "SELECT m.commodity_symbol, NULLIF(c.name, ''), NULLIF(c.category, ''), \
                m.buy_price, m.sell_price, m.demand, m.supply, \
                EXTRACT(EPOCH FROM m.observed_at)::DOUBLE PRECISION \
         FROM market m LEFT JOIN commodities c ON c.symbol = m.commodity_symbol \
         WHERE m.station_id = $1 \
         ORDER BY c.category NULLS LAST, COALESCE(NULLIF(c.name, ''), m.commodity_symbol)",
    )
    .bind(station_id)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let now = now_unix();
    let entries: Vec<serde_json::Value> = rows
        .into_iter()
        .map(
            |(symbol, name, category, buy, sell, demand, supply, observed)| {
                json!({
                    "symbol": symbol,
                    "name": name,
                    "category": category,
                    "buy_price": buy,
                    "sell_price": sell,
                    "demand": demand,
                    "supply": supply,
                    "updated": iso_from_unix(observed),
                    "age_hours": (now - observed).max(0.0) / 3600.0,
                })
            },
        )
        .collect();
    Ok(json!({
        "station": station_name.unwrap_or_default(),
        "system": system_name,
        "entries": entries,
        "provenance": "server",
        "as_of": now_iso(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> MarketSearchApiRequest {
        MarketSearchApiRequest {
            kind: "commodity".into(),
            text: "Palladium".into(),
            system: "Sol".into(),
            radius_ly: None,
            min_pad: None,
            include_carriers: false,
            include_prohibited: false,
            max_age_hours: None,
            side: "sell".into(),
            min_quantity: None,
            sort: None,
            limit: None,
        }
    }

    /// Wire defaults and clamps: the ledger contract, not accidents.
    #[test]
    fn defaults_and_clamps_hold() {
        let r = request();
        assert_eq!(r.radius(), 100.0);
        assert_eq!(r.max_age(), 48.0);
        assert_eq!(r.limit(), 50);
        let mut r = request();
        r.radius_ly = Some(9_000.0);
        r.limit = Some(100_000);
        r.max_age_hours = Some(0.0);
        assert_eq!(r.radius(), 500.0);
        assert_eq!(r.limit(), 200);
        assert_eq!(r.max_age(), 0.25);
    }

    /// Pad semantics match the local search exactly: unknown pads never
    /// satisfy a requirement; "any"/absent means no filter.
    #[test]
    fn pad_floor_matches_the_local_search() {
        assert_eq!(pad_clause(None), "TRUE");
        assert!(pad_clause(Some(PadSize::Large)).contains("pad_large"));
        assert!(!pad_clause(Some(PadSize::Large)).contains("pad_medium"));
        assert!(pad_clause(Some(PadSize::Small)).contains("pad_small"));
        let mut r = request();
        r.min_pad = Some("any".into());
        assert!(r.min_pad().unwrap().is_none());
        r.min_pad = Some("L".into());
        assert_eq!(r.min_pad().unwrap(), Some(PadSize::Large));
        r.min_pad = Some("gigantic".into());
        assert!(r.min_pad().is_err());
    }

    /// The hand-rolled ISO stamp agrees with known fixtures (epoch,
    /// leap-year day, and a modern timestamp).
    #[test]
    fn iso_from_unix_matches_fixtures() {
        assert_eq!(iso_from_unix(0.0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_from_unix(951_782_400.0), "2000-02-29T00:00:00Z");
        assert_eq!(iso_from_unix(1_757_030_400.0), "2025-09-05T00:00:00Z");
    }

    /// The row JSON is the local CommoditySearchHit shape, field for
    /// field — the frontend contract that makes the API a data-source
    /// toggle instead of a rewrite.
    #[test]
    fn station_row_serialises_to_the_local_hit_shape() {
        let row: StationRow = (
            42,
            Some("Talaria Towers".into()),
            "HIP 1234".into(),
            12.5,
            Some(430.0),
            Some(2),
            Some(0),
            Some(4),
            false,
        );
        let value = merge(
            station_json(&row),
            serde_json::json!({"price": 190_000, "quantity": 4_000, "updated": "2026-09-05T00:00:00Z", "age_hours": 0.5}),
        );
        for key in [
            "station_id",
            "station",
            "system",
            "distance_ly",
            "distance_to_arrival",
            "max_pad",
            "is_carrier",
            "price",
            "quantity",
            "updated",
            "age_hours",
        ] {
            assert!(value.get(key).is_some(), "missing {key}: {value}");
        }
        assert_eq!(value["max_pad"], "large");
        assert_eq!(value["station"], "Talaria Towers");
    }
}
