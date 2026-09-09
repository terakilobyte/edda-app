//! POST /v1/trade/search — server-side trade legs for thin clients
//! (maintainer, 2026-09-06: "wire up the rest of the endpoints").
//!
//! Shape: per-commodity best-K buy boards and best-K sell boards
//! inside the sphere, joined into one-way legs ranked by per-ton
//! profit — the LEGACY answer, kept for 0.2.9 clients. A request that
//! carries `ship` gets the v2 answer instead: the whole `ProfitReport`
//! (round trips, rings, exclusions, timings) computed here through
//! `ed_route::profit::assemble` — API-only spec, Phase A.2.
//!
//! Measured before building (edda_dev, 100.2M market rows, 100 ly,
//! 48 h, warm): ~3.0 s, dominated by the 2.3M-row fresh scan — so this
//! endpoint runs ONE search at a time behind a small queue and caches
//! results for a minute (EDDN moves slower than that). Pre-registered
//! honestly at the measured cost: P50 < 5 s cold, cache hits in
//! milliseconds. The local finder stays the authority for data-rich
//! installs; this exists so a thin client can trade at all.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ed_domain::station::PadSize;
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use crate::market_search::Refusal;

const DEFAULT_RADIUS_LY: f64 = 100.0;
const MAX_RADIUS_LY: f64 = 500.0;
const DEFAULT_MIN_QTY: i64 = 500;
const BEST_K: i64 = 8;
const DEFAULT_LEGS: i64 = 50;
const MAX_LEGS: i64 = 100;
const CACHE_TTL: Duration = Duration::from_secs(60);
const CACHE_CAP: usize = 64;

#[derive(Debug, Clone, Deserialize)]
pub struct TradeSearchApiRequest {
    pub system: String,
    pub radius_ly: Option<f64>,
    pub min_supply: Option<i64>,
    pub min_demand: Option<i64>,
    pub max_age_hours: Option<f64>,
    /// `"s" | "m" | "l"`; absent or `"any"` = no pad filter.
    pub min_pad: Option<String>,
    #[serde(default)]
    pub include_carriers: bool,
    pub limit: Option<i64>,
}

impl TradeSearchApiRequest {
    fn radius(&self) -> f64 {
        self.radius_ly.unwrap_or(DEFAULT_RADIUS_LY).clamp(1.0, MAX_RADIUS_LY)
    }
    fn max_age(&self) -> f64 {
        self.max_age_hours.unwrap_or(48.0).clamp(0.25, 720.0)
    }
    fn limit(&self) -> i64 {
        self.limit.unwrap_or(DEFAULT_LEGS).clamp(1, MAX_LEGS)
    }
    fn min_pad(&self) -> Result<Option<PadSize>, Refusal> {
        match self.min_pad.as_deref().map(str::trim) {
            None | Some("") | Some("any") | Some("Any") => Ok(None),
            Some(text) => PadSize::parse(text)
                .map(Some)
                .ok_or_else(|| Refusal::Invalid(format!("unknown pad size {text:?}"))),
        }
    }
    /// The cache key quantizes floats so recomputed defaults collide.
    /// Two-part on purpose — "spatial#filters" — so a miss can tell a
    /// truly-new query from one that differs only in post-filterable
    /// knobs (pad, floors, carriers, limit). The maintainer's proposed
    /// superset-then-filter cache (2026-09-06) is pre-registered in the
    /// ledger; the filter_miss count is the measurement that rules it.
    fn cache_key(&self, origin: (f64, f64, f64)) -> String {
        format!("{}#{}", self.spatial_key(origin), self.filter_key())
    }
    fn spatial_key(&self, origin: (f64, f64, f64)) -> String {
        format!(
            "{:.0},{:.0},{:.0}|{:.0}|{:.0}",
            origin.0,
            origin.1,
            origin.2,
            self.radius(),
            self.max_age(),
        )
    }
    fn filter_key(&self) -> String {
        format!(
            "{}|{}|{:?}|{}|{}",
            self.min_supply.unwrap_or(DEFAULT_MIN_QTY),
            self.min_demand.unwrap_or(DEFAULT_MIN_QTY),
            self.min_pad.as_deref().unwrap_or(""),
            self.include_carriers,
            self.limit(),
        )
    }
}

/// The v2 request (API-only spec, Phase A.2): the client's resolved
/// plan — ship and constraints — verbatim, so the server computes the
/// same report the local finder would. `ship` present selects this
/// path; absent, the legacy legs answer (0.2.9 clients) is served.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ReportRequest {
    pub system: String,
    pub ship: ed_route::cost::Ship,
    #[serde(default)]
    pub constraints: ed_route::profit::Constraints,
    pub from_station_id: Option<i64>,
    pub limit: Option<usize>,
    /// The commander's own docked board (B.4 gap 2); see
    /// `trade_report::ClientBoard`. A request carrying one bypasses
    /// the shared cache both ways.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<crate::trade_report::ClientBoard>,
}

impl ReportRequest {
    pub fn parse(body: &serde_json::Value) -> Option<Self> {
        body.get("ship")?;
        serde_json::from_value(body.clone()).ok()
    }
    pub fn limit(&self) -> usize {
        self.limit
            .unwrap_or(ed_route::request::ProfitRequest::DEFAULT_LIMIT)
            .clamp(1, MAX_LEGS as usize)
    }
    /// A shared box gives everyone the same ceiling.
    pub fn clamp(&mut self) {
        let c = &mut self.constraints;
        c.radius_ly = c.radius_ly.clamp(1.0, crate::trade_report::MAX_RADIUS_LY);
        // The cap is the server's resource guard, not a client knob: the
        // wire value (the local finder's 2,500 default) is replaced, not
        // clamped, so a freshness-gated search reaches its whole radius.
        c.max_stations = crate::trade_report::DEFAULT_MAX_STATIONS;
        c.max_age_hours = c.max_age_hours.clamp(0.25, 720.0);
        c.max_stops = c.max_stops.min(5);
        // The commander's own time constants, inside the bounds a shared
        // box accepts; omitted fields are already the defaults.
        c.timing = c.timing.clamped();
        self.limit = Some(self.limit());
    }
    fn cache_key(&self, origin: (f64, f64, f64)) -> String {
        format!(
            "report|{:.0},{:.0},{:.0}|{}|{:?}",
            origin.0,
            origin.1,
            origin.2,
            serde_json::to_string(&self.constraints).unwrap_or_default(),
            (
                self.ship.cargo_capacity,
                (self.ship.jump_range_ly * 10.0) as i64,
                (self.ship.laden_range_ly * 10.0) as i64,
                self.from_station_id,
                self.limit,
            )
        )
    }
}

/// One search at a time plus a minute of memory: the query is ~3 s of
/// real work, and popular origins repeat.
pub struct TradeService {
    gate: tokio::sync::Semaphore,
    cache: Mutex<HashMap<String, (Instant, serde_json::Value)>>,
}

/// Concurrent trade searches. One was sized when the query cost ~3 s;
/// after the fresh-market index and the cell predicate (2026-09-07) a
/// report costs ~160–260 ms server-side, and the flight-binary ladder
/// showed both trade forms queueing behind the single runner. Swept
/// 1/2/3 on 2026-09-08 (docs/benches/trade-gate-sweep-2026-09-08.csv):
/// reports served at c=16 204 → 370 → 549, p50 2.3 s → 1.3 s → 0.86 s,
/// Postgres mean flat (607/535/557 ms), pool idle ≥ 9/30, trade behind
/// two plots unchanged at every step. Three is the highest MEASURED
/// point with nothing binding; 4 and 6 are unmeasured — sweep them
/// (`EDDA_API_TRADE_CONCURRENCY`) before going higher.
pub const CONCURRENCY: usize = 3;

impl Default for TradeService {
    fn default() -> Self {
        let concurrency = std::env::var("EDDA_API_TRADE_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or(CONCURRENCY);
        tracing::info!(concurrency, "trade gate sized");
        Self { gate: tokio::sync::Semaphore::new(concurrency), cache: Mutex::new(HashMap::new()) }
    }
}

pub enum TradeOutcome {
    /// The label is the cache verdict: "hit", "miss", or "filter_miss"
    /// — a miss where an entry with the same sphere and window existed
    /// under different post-filterable knobs, i.e. a query the
    /// superset-then-filter design would have served warm.
    Legs(serde_json::Value, &'static str),
    Saturated,
}

impl TradeService {
    pub async fn search(
        &self,
        pool: &PgPool,
        req: &TradeSearchApiRequest,
    ) -> Result<TradeOutcome, Refusal> {
        let origin = crate::market_search::origin_coords(pool, req.system.trim()).await?;
        let key = req.cache_key(origin);
        if let Some(value) = self.cache_hit(&key) {
            return Ok(TradeOutcome::Legs(value, "hit"));
        }
        // One runner, a short line behind it: a full queue answers 429
        // rather than stacking 3-second queries to the horizon.
        let Ok(_permit) = tokio::time::timeout(Duration::from_secs(15), self.gate.acquire()).await
        else {
            return Ok(TradeOutcome::Saturated);
        };
        // Re-check after the wait — the previous holder may have filled
        // exactly this key.
        if let Some(value) = self.cache_hit(&key) {
            return Ok(TradeOutcome::Legs(value, "hit"));
        }
        let miss_kind = if self.same_sphere_cached(&req.spatial_key(origin), &key) {
            "filter_miss"
        } else {
            "miss"
        };
        let value = legs(pool, origin, req).await?;
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), value.clone()));
        Ok(TradeOutcome::Legs(value, miss_kind))
    }

    /// The v2 answer: `trade_report::prepare` (Postgres) then
    /// `ed_route::profit::assemble` (the finder's own pipeline) behind
    /// the same one-at-a-time gate and minute cache as the legacy legs.
    pub async fn report(&self, pool: &PgPool, req: &ReportRequest) -> Result<TradeOutcome, Refusal> {
        let mut req = req.clone();
        req.clamp();
        let origin = crate::market_search::origin_coords(pool, req.system.trim()).await?;
        let key = req.cache_key(origin);
        // A report fused with one commander's own board is that
        // commander's: never served from the cache, never put in it.
        let cacheable = req.board.is_none();
        if cacheable {
            if let Some(value) = self.cache_hit(&key) {
                return Ok(TradeOutcome::Legs(value, "hit"));
            }
        }
        let Ok(_permit) = tokio::time::timeout(Duration::from_secs(15), self.gate.acquire()).await
        else {
            return Ok(TradeOutcome::Saturated);
        };
        if cacheable {
            if let Some(value) = self.cache_hit(&key) {
                return Ok(TradeOutcome::Legs(value, "hit"));
            }
        }
        let mut prepared = crate::trade_report::prepare(pool, origin, &req.constraints).await?;
        let board_use = match &req.board {
            Some(board) => {
                let outcome = crate::trade_report::fuse(pool, &mut prepared, board, req.from_station_id).await?;
                metrics::counter!("edda_trade_board_total", "reason" => outcome.reason).increment(1);
                Some(outcome)
            }
            None => None,
        };
        metrics::histogram!("edda_trade_report_stations").record(prepared.stations.len() as f64);
        for (phase, ms) in [
            ("candidates", prepared.timing.candidates_ms),
            ("market", prepared.timing.market_ms),
            ("guards", prepared.timing.guards_ms),
        ] {
            metrics::histogram!("edda_trade_report_phase_seconds", "phase" => phase).record(ms as f64 / 1000.0);
        }
        let system = req.system.trim().to_owned();
        let (ship, constraints, from_station, limit) =
            (req.ship, req.constraints.clone(), req.from_station_id, req.limit());
        let report = tokio::task::spawn_blocking(move || {
            ed_route::profit::assemble(
                &system,
                origin,
                from_station,
                &ship,
                &constraints,
                limit,
                prepared,
                &ed_route::profit::SearchControl::none(),
            )
        })
        .await
        .map_err(|join| Refusal::Invalid(format!("report panicked: {join}")))?;
        for (phase, ms) in [("pairing", report.timing.pairing_ms), ("rings", report.timing.rings_ms)] {
            metrics::histogram!("edda_trade_report_phase_seconds", "phase" => phase).record(ms as f64 / 1000.0);
        }
        tracing::info!(
            stations = report.stations_considered,
            legs = report.legs.len(),
            trips = report.round_trips.len(),
            rings = report.rings.len(),
            candidates_ms = report.timing.candidates_ms,
            market_ms = report.timing.market_ms,
            pairing_ms = report.timing.pairing_ms,
            rings_ms = report.timing.rings_ms,
            board_used = board_use.as_ref().map(|b| b.used),
            board_reason = board_use.as_ref().map(|b| b.reason),
            "trade report built"
        );
        let mut report = report;
        report.board = board_use.as_ref().map(|b| ed_route::profit::BoardVerdict {
            used: b.used,
            reason: b.reason.to_owned(),
            rows: b.rows,
        });
        let mut value = serde_json::to_value(&report).map_err(|e| Refusal::Invalid(e.to_string()))?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("provenance".into(), json!("server"));
            obj.insert("as_of".into(), json!(crate::market_search::now_iso()));
        }
        if !cacheable {
            return Ok(TradeOutcome::Legs(value, "bypass"));
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_CAP {
            cache.clear();
        }
        cache.insert(key, (Instant::now(), value.clone()));
        Ok(TradeOutcome::Legs(value, "miss"))
    }

    fn cache_hit(&self, key: &str) -> Option<serde_json::Value> {
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache
            .get(key)
            .filter(|(at, _)| at.elapsed() < CACHE_TTL)
            .map(|(_, value)| value.clone())
    }

    /// A live entry shares this query's sphere+window under different
    /// filter knobs — the measurement behind the superset design.
    fn same_sphere_cached(&self, spatial: &str, full_key: &str) -> bool {
        let prefix = format!("{spatial}#");
        let cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.iter().any(|(k, (at, _))| {
            k != full_key && k.starts_with(&prefix) && at.elapsed() < CACHE_TTL
        })
    }
}

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

async fn legs(
    pool: &PgPool,
    (ox, oy, oz): (f64, f64, f64),
    req: &TradeSearchApiRequest,
) -> Result<serde_json::Value, Refusal> {
    let sql = format!(
        "WITH box AS MATERIALIZED ( \
             SELECT st.id, st.name, st.arrival_ls, st.pad_small, st.pad_medium, st.pad_large, \
                    COALESCE(st.is_carrier, false) AS is_carrier, sy.name AS system_name, \
                    sqrt((sy.x-$1)^2 + (sy.y-$2)^2 + (sy.z-$3)^2) AS distance_ly, \
                    sy.address, sy.x, sy.y, sy.z, st.station_type, \
                    sy.controlling_power, sy.power_state, sy.powers \
             FROM stations st JOIN systems sy ON sy.address = st.system_address \
             WHERE sy.cell = ANY($9) \
               AND (sy.x-$1)^2 + (sy.y-$2)^2 + (sy.z-$3)^2 <= $4*$4 \
               AND {pad} \
               AND ($7 OR NOT COALESCE(st.is_carrier, false)) \
         ), \
         fresh AS MATERIALIZED ( \
             SELECT m.commodity_symbol, m.station_id, m.buy_price, m.sell_price, m.demand, m.supply, m.observed_at \
             FROM market m JOIN box ON box.id = m.station_id \
             WHERE m.observed_at > now() - make_interval(secs => $8) \
         ), \
         best_buy AS ( \
             SELECT * FROM ( \
                 SELECT commodity_symbol, station_id, buy_price, supply, observed_at, \
                        row_number() OVER (PARTITION BY commodity_symbol ORDER BY buy_price ASC) AS rn \
                 FROM fresh WHERE buy_price > 0 AND supply >= $5 \
             ) b WHERE rn <= {best_k} \
         ), \
         best_sell AS ( \
             SELECT * FROM ( \
                 SELECT commodity_symbol, station_id, sell_price, demand, observed_at, \
                        row_number() OVER (PARTITION BY commodity_symbol ORDER BY sell_price DESC) AS rn \
                 FROM fresh WHERE sell_price > 0 AND demand >= $6 \
             ) s WHERE rn <= {best_k} \
         ) \
         SELECT b.commodity_symbol, COALESCE(NULLIF(c.name, ''), b.commodity_symbol), \
                s.sell_price - b.buy_price AS profit_t, b.buy_price, s.sell_price, b.supply, s.demand, \
                EXTRACT(EPOCH FROM now() - b.observed_at)::DOUBLE PRECISION / 3600.0, \
                EXTRACT(EPOCH FROM now() - s.observed_at)::DOUBLE PRECISION / 3600.0, \
                sqrt((fb.x-fs.x)^2 + (fb.y-fs.y)^2 + (fb.z-fs.z)^2) AS leg_ly, \
                fb.id, fb.name, fb.system_name, fb.distance_ly, fb.arrival_ls, \
                fb.pad_small, fb.pad_medium, fb.pad_large, fb.is_carrier, \
                fb.address, fb.x, fb.y, fb.z, fb.station_type, fb.controlling_power, fb.power_state, fb.powers, \
                fs.id, fs.name, fs.system_name, fs.distance_ly, fs.arrival_ls, \
                fs.pad_small, fs.pad_medium, fs.pad_large, fs.is_carrier, \
                fs.address, fs.x, fs.y, fs.z, fs.station_type, fs.controlling_power, fs.power_state, fs.powers \
         FROM best_buy b \
         JOIN best_sell s ON s.commodity_symbol = b.commodity_symbol AND s.station_id <> b.station_id \
         JOIN box fb ON fb.id = b.station_id \
         JOIN box fs ON fs.id = s.station_id \
         LEFT JOIN commodities c ON c.symbol = b.commodity_symbol \
         WHERE s.sell_price > b.buy_price \
           AND NOT EXISTS (SELECT 1 FROM station_prohibited pr \
                           WHERE pr.station_id = s.station_id \
                             AND lower(pr.symbol) = lower(b.commodity_symbol)) \
         ORDER BY profit_t DESC LIMIT {limit}",
        pad = pad_clause(req.min_pad()?),
        best_k = BEST_K,
        limit = req.limit(),
    );
    // 25 columns is past sqlx's tuple limit; extract by index.
    let rows = sqlx::query(&sql)
        .bind(ox)
        .bind(oy)
        .bind(oz)
        .bind(req.radius())
        .bind(req.min_supply.unwrap_or(DEFAULT_MIN_QTY).max(0))
        .bind(req.min_demand.unwrap_or(DEFAULT_MIN_QTY).max(0))
        .bind(req.include_carriers)
        .bind(req.max_age() * 3600.0)
        .bind(crate::market_search::cells_covering(ox, oy, oz, req.radius()))
        // Fresh plan per execution: pg_stat_statements (2026-09-07) showed
        // this statement at plan_time 0 and a 697 ms mean — a generic plan
        // that cannot see the cell list or the origin — while the same SQL
        // planned with its parameters runs in 110–154 ms.
        .persistent(false)
        .fetch_all(pool)
        .await
        .map_err(|e| Refusal::Invalid(e.to_string()))?;
    fn endpoint(row: &sqlx::postgres::PgRow, base: usize) -> serde_json::Value {
        use sqlx::Row;
        let (ps, pm, pl): (Option<i32>, Option<i32>, Option<i32>) =
            (row.get(base + 5), row.get(base + 6), row.get(base + 7));
        json!({
            "station_id": row.get::<i64, _>(base),
            "station": row.get::<Option<String>, _>(base + 1).unwrap_or_default(),
            "system": row.get::<String, _>(base + 2),
            "distance_ly": row.get::<f64, _>(base + 3),
            "arrival_ls": row.get::<Option<f64>, _>(base + 4),
            "max_pad": PadSize::from_counts(pl.map(i64::from), pm.map(i64::from), ps.map(i64::from)),
            "is_carrier": row.get::<bool, _>(base + 8),
            "system_address": row.get::<i64, _>(base + 9),
            "x": row.get::<Option<f64>, _>(base + 10),
            "y": row.get::<Option<f64>, _>(base + 11),
            "z": row.get::<Option<f64>, _>(base + 12),
            "station_type": row.get::<Option<String>, _>(base + 13),
            "controlling_power": row.get::<Option<String>, _>(base + 14),
            "power_state": row.get::<Option<String>, _>(base + 15),
            "powers": row.get::<Option<String>, _>(base + 16),
        })
    }
    let legs: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            use sqlx::Row;
            json!({
                "symbol": row.get::<String, _>(0),
                "commodity": row.get::<String, _>(1),
                "profit_t": row.get::<i64, _>(2),
                "buy": row.get::<i64, _>(3),
                "sell": row.get::<i64, _>(4),
                "supply": row.get::<i64, _>(5),
                "demand": row.get::<i64, _>(6),
                "buy_age_hours": row.get::<f64, _>(7),
                "sell_age_hours": row.get::<f64, _>(8),
                "distance_ly": row.get::<Option<f64>, _>(9),
                "from": endpoint(row, 10),
                "to": endpoint(row, 27),
            })
        })
        .collect();
    Ok(json!({
        "origin": req.system.trim(),
        "legs": legs,
        "provenance": "server",
        "as_of": crate::market_search::now_iso(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_with_a_ship_is_a_report_request() {
        let body: serde_json::Value = serde_json::json!({
            "system": "Deciat",
            "ship": {"cargo_capacity": 720, "jump_range_ly": 30.5, "laden_range_ly": 22.1},
            "constraints": {"radius_ly": 60.0, "max_stops": 3}
        });
        let req = ReportRequest::parse(&body).unwrap();
        assert_eq!(req.constraints.radius_ly, 60.0);
        assert_eq!(req.constraints.max_stops, 3);
        assert_eq!(req.constraints.max_age_hours, ed_route::profit::Constraints::default().max_age_hours);
        assert_eq!(req.limit(), ed_route::request::ProfitRequest::DEFAULT_LIMIT);
        assert!(ReportRequest::parse(&serde_json::json!({"system": "Sol"})).is_none(), "no ship: legacy");
    }

    #[test]
    fn report_request_clamps_to_the_server_budget() {
        let mut req = ReportRequest {
            system: "Sol".into(),
            ship: ed_route::cost::Ship { cargo_capacity: 1, jump_range_ly: 1.0, laden_range_ly: 1.0 },
            constraints: ed_route::profit::Constraints {
                radius_ly: 9_000.0,
                max_stations: 1_000_000,
                max_age_hours: 0.0,
                ..Default::default()
            },
            from_station_id: None, board: None,
            limit: Some(9_000),
        };
        req.clamp();
        assert_eq!(req.constraints.radius_ly, crate::trade_report::MAX_RADIUS_LY);
        assert_eq!(req.constraints.max_stations, crate::trade_report::DEFAULT_MAX_STATIONS);
        assert_eq!(req.constraints.max_age_hours, 0.25);
        assert_eq!(req.limit(), 100);
    }

    fn request() -> TradeSearchApiRequest {
        TradeSearchApiRequest {
            system: "Sol".into(),
            radius_ly: None,
            min_supply: None,
            min_demand: None,
            max_age_hours: None,
            min_pad: None,
            include_carriers: false,
            limit: None,
        }
    }

    /// Defaults and clamps are the contract, not accidents.
    #[test]
    fn defaults_and_clamps_hold() {
        let r = request();
        assert_eq!(r.radius(), 100.0);
        assert_eq!(r.max_age(), 48.0);
        assert_eq!(r.limit(), 50);
        let mut r = request();
        r.radius_ly = Some(9_000.0);
        r.limit = Some(9_000);
        r.max_age_hours = Some(90_000.0);
        assert_eq!(r.radius(), 500.0);
        assert_eq!(r.limit(), 100);
        assert_eq!(r.max_age(), 720.0);
    }

    /// The cache key ignores float noise and covers every filter that
    /// changes the answer — and its two parts separate the sphere from
    /// the knobs, so a filter-only change shares the spatial prefix
    /// (the filter_miss measurement depends on exactly this).
    #[test]
    fn cache_key_is_stable_and_filter_complete() {
        let origin = (10.04, -20.02, 30.01);
        let a = request().cache_key(origin);
        let b = request().cache_key((10.01, -19.98, 29.96));
        assert_eq!(a, b, "sub-ly origin noise must collide");
        let mut r = request();
        r.min_demand = Some(1);
        assert_ne!(a, r.cache_key(origin), "filters must key");
        assert_eq!(request().spatial_key(origin), r.spatial_key(origin), "a filter change keeps the sphere");
        let mut r = request();
        r.include_carriers = true;
        assert_ne!(a, r.cache_key(origin));
        assert_eq!(request().spatial_key(origin), r.spatial_key(origin));
        let mut r = request();
        r.radius_ly = Some(250.0);
        assert_ne!(request().spatial_key(origin), r.spatial_key(origin), "a sphere change is a real miss");
    }
}
