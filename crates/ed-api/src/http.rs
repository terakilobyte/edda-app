use std::{
    io::SeekFrom,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use serde::Serialize;
use sqlx::PgPool;

use ed_sync::{Manifest, MANIFEST_ROUTE};

#[derive(Clone)]
pub struct AppState {
    pool: PgPool,
    artifact_dir: Arc<PathBuf>,
    manifest_path: Arc<PathBuf>,
    metrics: metrics_exporter_prometheus::PrometheusHandle,
    feedback_limiter: Arc<crate::feedback::Limiter>,
    telemetry_limiter: Arc<crate::feedback::Limiter>,
    callsites: Arc<crate::telemetry::CallsiteGuard>,
    galaxy: Arc<crate::galaxy_service::GalaxyService>,
    route_service: Arc<crate::plot::RouteService>,
    route_limiter: Arc<crate::feedback::Budget>,
    knowledge_limiter: Arc<crate::feedback::Budget>,
    names_limiter: Arc<crate::feedback::Budget>,
    market_limiter: Arc<crate::feedback::Budget>,
    trade_service: Arc<crate::trade_search::TradeService>,
    knowledge_flight: Arc<crate::knowledge::SingleFlight>,
    edsm_pacer: Arc<crate::knowledge::Pacer>,
    http_client: reqwest::Client,
}

impl AppState {
    pub fn new(
        pool: PgPool,
        artifact_dir: PathBuf,
        metrics: metrics_exporter_prometheus::PrometheusHandle,
    ) -> Self {
        Self {
            pool,
            artifact_dir: Arc::new(artifact_dir.clone()),
            manifest_path: Arc::new(artifact_dir.join("current.json")),
            metrics,
            feedback_limiter: Arc::new(crate::feedback::Limiter::default()),
            telemetry_limiter: Arc::new(crate::feedback::Limiter::default()),
            callsites: Arc::new(crate::telemetry::CallsiteGuard::default()),
            galaxy: Arc::new(crate::galaxy_service::GalaxyService::new(artifact_dir)),
            route_service: Arc::new(crate::plot::RouteService::default()),
            // Each budget is two speeds: the hourly scraper ceiling and a
            // ten-second burst cap that is what actually protects the box
            // (ledger 2026-09-07). Burst defaults are ~10× a commander's
            // fastest real pattern and well under one bench thread.
            route_limiter: Arc::new(crate::feedback::Budget::new(
                crate::feedback::Limiter::new(
                    crate::plot::RATE_WINDOW,
                    env_rate("EDDA_API_ROUTE_RATE_PER_HOUR", crate::plot::RATE_PER_WINDOW),
                ),
                env_rate("EDDA_API_ROUTE_BURST_PER_10S", 30),
            )),
            // A long route sweeps many cells; the knowledge budget is per
            // request we serve, not per EDSM call we make (cache +
            // single-flight keep upstream far below it).
            knowledge_limiter: Arc::new(crate::feedback::Budget::new(
                crate::feedback::Limiter::new(
                    std::time::Duration::from_secs(3_600),
                    env_rate("EDDA_API_KNOWLEDGE_RATE_PER_HOUR", 2_500),
                ),
                env_rate("EDDA_API_KNOWLEDGE_BURST_PER_10S", 60),
            )),
            // Completion fires per debounced keystroke, several per typed
            // name: 600/min per source is a fast typist with headroom,
            // and still stops a scraper walking the alphabet.
            names_limiter: Arc::new(crate::feedback::Budget::new(
                crate::feedback::Limiter::new(
                    std::time::Duration::from_secs(60),
                    env_rate("EDDA_API_NAMES_RATE_PER_MINUTE", 600),
                ),
                env_rate("EDDA_API_NAMES_BURST_PER_10S", 120),
            )),
            // 120/h was "a panel search per user action" — the load
            // bench (2026-09-07) showed it is one trade loop's worth of
            // searches, and per IP it is shared by everyone behind a NAT.
            // Maintainer: 2,500/h across route/market/knowledge, monitored.
            market_limiter: Arc::new(crate::feedback::Budget::new(
                crate::feedback::Limiter::new(
                    std::time::Duration::from_secs(3_600),
                    env_rate("EDDA_API_MARKET_RATE_PER_HOUR", 2_500),
                ),
                env_rate("EDDA_API_MARKET_BURST_PER_10S", 60),
            )),
            trade_service: Arc::new(crate::trade_search::TradeService::default()),
            knowledge_flight: Arc::new(crate::knowledge::SingleFlight::default()),
            edsm_pacer: Arc::new(crate::knowledge::Pacer::default()),
            http_client: reqwest::Client::new(),
        }
    }
}

/// A per-source rate budget, overridable by environment for benches and
/// load tests (the 2026-09-06 latency bench measured a limiter instead
/// of a query for five of nine cases; the 2026-09-07 load bench hit the
/// route limiter after three requests). Production leaves the variables
/// unset and gets the default; a systemd drop-in sets them for a run.
fn env_rate(var: &str, default: u32) -> u32 {
    std::env::var(var)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// The rate-limit key: first X-Forwarded-For hop (Caddy sets it), used
/// in memory only — the privacy law forbids identity at rest.
/// The limiter key: the install ID when the client sends one (API-only
/// spec, "Per-install keys" — a random 32-hex string that names an
/// install to the limiter and nothing else), else the first forwarded
/// hop. Per-IP budgets are shared by everyone behind one NAT.
fn source_of(headers: &HeaderMap) -> String {
    if let Some(id) = headers
        .get("x-edda-install")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| v.len() == 32 && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    {
        return format!("install:{id}");
    }
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

/// GET /v1/stations — see stations.rs. `near=` resolves the system
/// through the routing index first (the ruling: the server knows more
/// than every client), then Postgres for the station set; a sphere
/// with no stations is a valid empty answer.
async fn stations(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<crate::stations::StationsQuery>,
) -> impl IntoResponse {
    use crate::stations::Mode;
    let mode = match query.mode() {
        Ok(mode) => mode,
        Err(message) => {
            metrics::counter!("edda_stations_requests_total", "mode" => "invalid", "outcome" => "invalid")
                .increment(1);
            return (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": message})),
            )
                .into_response();
        }
    };
    let mode_label: &'static str = match mode {
        Mode::InSystem(_) => "system",
        Mode::InSystems(_) => "systems",
        Mode::Near { .. } => "near",
        Mode::Name(_) => "name",
    };
    let counter = |outcome: &'static str| {
        metrics::counter!("edda_stations_requests_total", "mode" => mode_label, "outcome" => outcome).increment(1)
    };
    if state
        .knowledge_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let started = std::time::Instant::now();
    let result = match mode {
        Mode::InSystem(system) => crate::stations::in_system(&state.pool, &system, &query).await,
        Mode::InSystems(systems) => {
            crate::stations::in_systems(&state.pool, &systems, &query).await
        }
        Mode::Name(prefix) => crate::stations::by_name(&state.pool, &prefix, &query).await,
        Mode::Near {
            system,
            service,
            radius_ly,
            min_pad,
        } => {
            let indexed = match state.galaxy.current().await {
                Ok(Some(handle)) => handle.galaxy.find(&system).map(|idx| {
                    let p = handle.galaxy.pos_of(idx);
                    (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))
                }),
                _ => None,
            };
            let origin = match indexed {
                Some(origin) => origin,
                None => {
                    match crate::market_search::origin_coords(&state.pool, &system).await {
                        Ok(origin) => origin,
                        Err(_) => {
                            counter("unknown_system");
                            return (
                            StatusCode::UNPROCESSABLE_ENTITY,
                            axum::Json(serde_json::json!({ "error": "unknown_system", "system": system })),
                        )
                            .into_response();
                        }
                    }
                }
            };
            crate::stations::near(&state.pool, origin, service, radius_ly, min_pad, &query).await
        }
    };
    match result {
        Ok(list) => {
            metrics::histogram!("edda_stations_seconds", "mode" => mode_label)
                .record(started.elapsed().as_secs_f64());
            counter("ok");
            tracing::info!(
                mode = mode_label,
                hits = list.len(),
                ms = started.elapsed().as_millis() as u64,
                "stations served"
            );
            axum::Json(list).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, mode = mode_label, "stations: query failed");
            counter("error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct ReadinessResponse {
    status: &'static str,
    checks: ReadinessChecks,
}

#[derive(Serialize)]
struct ReadinessChecks {
    database: bool,
    hydration: bool,
    manifest: bool,
    eddn: &'static str,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/readyz", get(readiness))
        .route(MANIFEST_ROUTE, get(manifest))
        .route("/v1/artifacts/{*path}", get(artifact))
        .route("/v1/app/{*path}", get(app_release))
        .route("/v1/feedback", axum::routing::post(feedback))
        .route("/v1/telemetry", axum::routing::post(telemetry))
        .route("/v1/stars", get(stars))
        .route("/v1/route", axum::routing::post(plot_route))
        .route(
            "/v1/loadout/physics",
            axum::routing::post(crate::loadout::handler),
        )
        .route("/v1/market/search", axum::routing::post(market_search))
        .route("/v1/market/station/{id}", get(station_board))
        .route("/v1/mining/search", axum::routing::post(mining_search))
        .route("/v1/mining/materials", get(mining_materials))
        .route("/v1/trade/search", axum::routing::post(trade_search))
        .route("/v1/knowledge/sphere", get(knowledge_sphere))
        .route("/v1/knowledge/system", get(knowledge_system))
        .route("/v1/knowledge/bodies", get(knowledge_bodies))
        .route("/v1/names/complete", get(names_complete))
        .route("/v1/stations", get(stations))
        .route("/metrics", get(scrape))
        .layer(axum::middleware::from_fn(crate::metrics::track_http))
        .with_state(state)
}

/// POST /v1/route — item 48. Names + client-derived physics in, the
/// engine's Route out. Never logs where anyone is going.
/// POST /v1/trade/search — one query at a time behind a semaphore,
/// cached a minute, 429 when the line is full. A body carrying `ship`
/// gets the whole `ProfitReport` computed here (API-only spec); a body
/// without one gets the legacy one-way legs for 0.2.9 clients.
async fn trade_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    use crate::market_search::Refusal;
    use crate::trade_search::TradeOutcome;
    let outcome_counter = |outcome: &'static str| {
        metrics::counter!("edda_trade_search_requests_total", "outcome" => outcome).increment(1)
    };
    if state
        .market_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        outcome_counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let started = std::time::Instant::now();
    // A body carrying `ship` is a v2 report request and nothing else: a
    // parse failure is the caller's error, never a quiet drop to the
    // legacy legs (which would cost a 0.3.0 client its round trips
    // without a word - the assistant session, 2026-09-08).
    let outcome = if body.get("ship").is_some() {
        let report: crate::trade_search::ReportRequest = match serde_json::from_value(body) {
            Ok(request) => request,
            Err(error) => {
                outcome_counter("invalid");
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": error.to_string()})),
                )
                    .into_response();
            }
        };
        outcome_counter("report");
        state.trade_service.report(&state.pool, &report).await
    } else {
        let legacy: crate::trade_search::TradeSearchApiRequest = match serde_json::from_value(body)
        {
            Ok(request) => request,
            Err(error) => {
                outcome_counter("invalid");
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": error.to_string()})),
                )
                    .into_response();
            }
        };
        state.trade_service.search(&state.pool, &legacy).await
    };
    match outcome {
        Ok(TradeOutcome::Legs(value, cache_verdict)) => {
            metrics::histogram!("edda_trade_search_seconds")
                .record(started.elapsed().as_secs_f64());
            // "filter_miss" = an entry with the same sphere+window sat
            // in cache under different post-filterable knobs — the
            // field count that rules the superset-then-filter design.
            metrics::counter!("edda_trade_search_cache_total", "result" => cache_verdict)
                .increment(1);
            outcome_counter("ok");
            tracing::info!(
                cache = cache_verdict,
                ms = started.elapsed().as_millis() as u64,
                "trade search served"
            );
            axum::Json(value).into_response()
        }
        Ok(TradeOutcome::Saturated) => {
            outcome_counter("saturated");
            (
                StatusCode::TOO_MANY_REQUESTS,
                "the trade search queue is full — try again shortly",
            )
                .into_response()
        }
        Err(Refusal::UnknownSystem(name)) => {
            outcome_counter("unknown_system");
            (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": format!("unknown system {name:?}")})),
            )
                .into_response()
        }
        Err(Refusal::Invalid(message)) | Err(Refusal::UnknownCommodity { text: message, .. }) => {
            outcome_counter("invalid");
            tracing::warn!(%message, "trade search refused");
            (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": message})),
            )
                .into_response()
        }
    }
}

/// GET /v1/market/station/{id} — one station's board, MarketEntry-shaped.
async fn station_board(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Path(station_id): axum::extract::Path<i64>,
) -> impl IntoResponse {
    let outcome_counter = |outcome: &'static str| {
        metrics::counter!("edda_station_board_requests_total", "outcome" => outcome).increment(1)
    };
    if state
        .market_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        outcome_counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let started = std::time::Instant::now();
    match crate::market_search::station_board(&state.pool, station_id).await {
        Ok(value) => {
            metrics::histogram!("edda_station_board_seconds")
                .record(started.elapsed().as_secs_f64());
            outcome_counter("ok");
            axum::Json(value).into_response()
        }
        Err(crate::market_search::Refusal::UnknownSystem(what)) => {
            outcome_counter("unknown_station");
            (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": format!("unknown {what}")})),
            )
                .into_response()
        }
        Err(crate::market_search::Refusal::Invalid(message))
        | Err(crate::market_search::Refusal::UnknownCommodity { text: message, .. }) => {
            outcome_counter("error");
            tracing::warn!(%message, "station board refused");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// POST /v1/market/search — design (b): the same rows the local sqlite
/// search produces, served from the live-EDDN board. Refusals are
/// structured (unknown system/commodity carry suggestions) so the
/// client can show the same near-miss UX it shows locally.
async fn market_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<crate::market_search::MarketSearchApiRequest>,
) -> impl IntoResponse {
    use crate::market_search::Refusal;
    let outcome_counter = |outcome: &'static str| {
        metrics::counter!("edda_market_search_requests_total", "outcome" => outcome).increment(1)
    };
    if state
        .market_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        outcome_counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let started = std::time::Instant::now();
    match crate::market_search::search(&state.pool, &body).await {
        Ok(value) => {
            metrics::histogram!("edda_market_search_seconds")
                .record(started.elapsed().as_secs_f64());
            outcome_counter("ok");
            axum::Json(value).into_response()
        }
        Err(Refusal::UnknownSystem(name)) => {
            outcome_counter("unknown_system");
            (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": format!("unknown system {name:?}")})),
            )
                .into_response()
        }
        Err(Refusal::UnknownCommodity { text, matches }) => {
            outcome_counter("unknown_commodity");
            (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({
                    "error": format!("unknown or ambiguous commodity {text:?}"),
                    "matches": matches,
                })),
            )
                .into_response()
        }
        Err(Refusal::Invalid(message)) => {
            outcome_counter("invalid");
            tracing::warn!(%message, "market search refused");
            (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": message})),
            )
                .into_response()
        }
    }
}

/// POST /v1/mining/search — the Mining page's hotspots, rings and
/// could-have bodies (B.4: the client keeps only its marks). Shares the
/// market budget: it is the same kind of sphere query. Never logs the
/// origin.
async fn mining_search(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<crate::mining::MiningSearchRequest>,
) -> impl IntoResponse {
    use crate::market_search::Refusal;
    let outcome_counter = |outcome: &'static str| {
        metrics::counter!("edda_mining_search_requests_total", "outcome" => outcome).increment(1)
    };
    if state
        .market_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        outcome_counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let started = std::time::Instant::now();
    match crate::mining::search(&state.pool, &body).await {
        Ok(value) => {
            metrics::histogram!("edda_mining_search_seconds")
                .record(started.elapsed().as_secs_f64());
            outcome_counter("ok");
            axum::Json(value).into_response()
        }
        Err(Refusal::UnknownSystem(name)) => {
            outcome_counter("unknown_system");
            (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({"error": format!("unknown system {name:?}")})),
            )
                .into_response()
        }
        Err(Refusal::UnknownCommodity { text, .. }) => {
            outcome_counter("invalid");
            (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": format!("unknown material {text:?}")})),
            )
                .into_response()
        }
        Err(Refusal::Invalid(message)) => {
            outcome_counter("invalid");
            tracing::warn!(%message, "mining search refused");
            (
                StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({"error": message})),
            )
                .into_response()
        }
    }
}

/// GET /v1/mining/materials — the stored vocabulary (hotspot and surface
/// spellings) plus the laser-mined goods, for the page's autocomplete.
async fn mining_materials(State(state): State<AppState>) -> impl IntoResponse {
    match crate::mining::vocabulary(&state.pool).await {
        Ok(entries) => axum::Json(serde_json::json!({
            "entries": entries,
            "laser": crate::mining::LASER_GOODS,
        }))
        .into_response(),
        Err(error) => {
            tracing::warn!(?error, "mining vocabulary failed");
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }
}

/// A route endpoint by name, under the ruling that the server consults
/// everything it holds: the routing index first (a planner node), then
/// Postgres `systems` (address + coordinates — everything EDDN and the
/// EDSM proxy have taught us), then one paced, single-flight EDSM fetch
/// that is learned for the fleet on the way back. Counted per source so
/// the dashboards show how often a plot rides the fallback. `None` only
/// when all three miss. Never logs the name.
async fn resolve_endpoint(
    state: &AppState,
    handle: &crate::galaxy_service::GalaxyHandle,
    name: &str,
    client_coords: Option<[f32; 3]>,
) -> Option<crate::plot::Endpoint> {
    let counter = |source: &'static str| {
        metrics::counter!("edda_route_resolution_total", "source" => source).increment(1)
    };
    if let Some(idx) = handle.galaxy.find(name) {
        counter("index");
        return Some(crate::plot::Endpoint::Indexed(idx));
    }
    // The commander's own journal position (the no-EDMC case) is the
    // last resort, never the first: what the fleet knows wins when it
    // knows anything, and a miss with no coords stays unknown_system.
    let from_client = || {
        client_coords.map(|pos| {
            counter("client_coords");
            crate::plot::Endpoint::Position {
                name: name.to_owned(),
                pos,
            }
        })
    };
    use crate::knowledge::SystemOutcome;
    let (answer, source) = match crate::knowledge::system_document(
        &state.pool,
        &state.http_client,
        &state.edsm_pacer,
        &state.knowledge_flight,
        Some(&handle.galaxy),
        name,
    )
    .await
    {
        Ok(SystemOutcome::Local(a)) | Ok(SystemOutcome::Coalesced(Some(a))) => (a, "postgres"),
        Ok(SystemOutcome::Indexed(a)) => (a, "index"),
        Ok(SystemOutcome::Fetched(a)) => (a, "edsm"),
        Ok(SystemOutcome::Coalesced(None)) | Ok(SystemOutcome::Unknown) => {
            return from_client().or_else(|| {
                counter("unknown");
                None
            });
        }
        Err(error) => {
            tracing::warn!(%error, "route: name resolution failed");
            return from_client().or_else(|| {
                counter("error");
                None
            });
        }
    };
    let Some(coords) = answer.coords.as_ref() else {
        return from_client();
    };
    let axis = |k: &str| coords.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
    let (Some(x), Some(y), Some(z)) = (axis("x"), axis("y"), axis("z")) else {
        return from_client();
    };
    counter(source);
    Some(crate::plot::Endpoint::Position {
        name: answer.name,
        pos: [x, y, z],
    })
}

async fn plot_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<crate::plot::RouteApiRequest>,
) -> impl IntoResponse {
    use crate::plot::{PlotOutcome, PlotRefusal};
    let outcome_counter = |outcome: &'static str| {
        metrics::counter!("edda_route_requests_total", "outcome" => outcome).increment(1)
    };
    if state
        .route_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        outcome_counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let handle = match state.galaxy.current().await {
        Ok(Some(handle)) => handle,
        Ok(None) => {
            outcome_counter("no_index");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "no routing index published yet",
            )
                .into_response();
        }
        Err(error) => {
            tracing::warn!(%error, "route: index unavailable");
            outcome_counter("error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let started = std::time::Instant::now();
    // Two lanes (Phase A step 3): every outcome past resolution is
    // labelled with its lane so the dashboards can show the interactive
    // and long gates filling independently.
    // Resolve each name the way the ruling says — index, then Postgres
    // (which holds what EDDN and the EDSM proxy taught us, coordinates
    // included), then one paced EDSM fetch learned for the fleet. A
    // system the index does not hold yet becomes a position endpoint
    // bridged to its nearest indexed neighbour (plot.rs).
    let (from, to) = match (
        resolve_endpoint(&state, &handle, &body.from, body.from_coords).await,
        resolve_endpoint(&state, &handle, &body.to, body.to_coords).await,
    ) {
        (Some(from), Some(to)) => (from, to),
        (from, _) => {
            let missing = if from.is_none() { &body.from } else { &body.to };
            outcome_counter("unknown_system");
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({ "error": "unknown_system", "system": missing })),
            )
                .into_response();
        }
    };
    let (lane, outcome, bridges) = match state
        .route_service
        .plot_endpoints(&handle, &body, from, to)
        .await
    {
        Ok(triple) => triple,
        Err(error) => {
            tracing::warn!(%error, "route plot failed");
            outcome_counter("error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let lane_name = lane.as_str();
    let lane_counter = |outcome: &'static str| {
        metrics::counter!("edda_route_requests_total", "outcome" => outcome, "lane" => lane_name)
            .increment(1)
    };
    match outcome {
        PlotOutcome::Route(route, cached) => {
            metrics::histogram!("edda_route_wall_seconds", "lane" => lane_name)
                .record(started.elapsed().as_secs_f64());
            metrics::counter!("edda_route_cache_total", "result" => if cached { "hit" } else { "miss" })
                .increment(1);
            lane_counter("ok");
            let bridged = bridges.from.is_some() || bridges.to.is_some();
            if bridged {
                metrics::counter!("edda_route_bridged_total").increment(1);
            }
            tracing::info!(
                cached,
                lane = lane_name,
                bridged,
                ms = started.elapsed().as_millis() as u64,
                "route served"
            );
            axum::Json(crate::plot::augment(&route, &bridges)).into_response()
        }
        PlotOutcome::Refused(PlotRefusal::UnknownSystem(name)) => {
            lane_counter("unknown_system");
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({ "error": "unknown_system", "system": name })),
            )
                .into_response()
        }
        PlotOutcome::Refused(PlotRefusal::Unindexed { name, radius_ly }) => {
            lane_counter("unindexed_destination");
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "unindexed_destination",
                    "system": name,
                    "radius_ly": radius_ly,
                    "detail": "known only by position; nothing in the routing index within the ship's range of it",
                })),
            )
                .into_response()
        }
        PlotOutcome::Refused(PlotRefusal::NoRange) => {
            lane_counter("no_range");
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({
                    "error": "no_range",
                    "detail": "send range_ly or a fuel_model to plot with",
                })),
            )
                .into_response()
        }
        PlotOutcome::Refused(PlotRefusal::NoRoute) => {
            lane_counter("no_route");
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(serde_json::json!({ "error": "no_route" })),
            )
                .into_response()
        }
        PlotOutcome::Refused(PlotRefusal::Budget) => {
            metrics::histogram!("edda_route_wall_seconds", "lane" => lane_name)
                .record(started.elapsed().as_secs_f64());
            lane_counter("budget");
            (
                StatusCode::GATEWAY_TIMEOUT,
                axum::Json(serde_json::json!({ "error": "budget", "budget_ms": lane.budget_ms(), "lane": lane_name })),
            )
                .into_response()
        }
        PlotOutcome::Saturated => {
            lane_counter("queue_full");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({ "error": "queue_full", "lane": lane_name, "detail": "try again shortly" })),
            )
                .into_response()
        }
    }
}

#[derive(serde::Deserialize)]
struct SphereQuery {
    x: f32,
    y: f32,
    z: f32,
    #[serde(default = "default_sphere_radius")]
    radius: f32,
}

fn default_sphere_radius() -> f32 {
    crate::knowledge::CELL_LY
}

/// GET /v1/knowledge/sphere — the EDSM proxy. EDSM-shaped answer from
/// our own galaxy + stars knowledge; one upstream fetch per stale cell
/// for the whole fleet.
async fn knowledge_sphere(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<SphereQuery>,
) -> impl IntoResponse {
    let counter = |outcome: &'static str| {
        metrics::counter!("edda_knowledge_requests_total", "endpoint" => "sphere", "outcome" => outcome)
            .increment(1)
    };
    if ![query.x, query.y, query.z, query.radius]
        .iter()
        .all(|v| v.is_finite())
    {
        return (StatusCode::BAD_REQUEST, "coordinates must be finite").into_response();
    }
    let radius = query.radius.clamp(1.0, crate::knowledge::CELL_LY);
    if state
        .knowledge_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let handle = match state.galaxy.current().await {
        Ok(Some(handle)) => handle,
        Ok(None) => {
            counter("no_index");
            return (StatusCode::SERVICE_UNAVAILABLE, "no galaxy published yet").into_response();
        }
        Err(error) => {
            tracing::warn!(%error, "knowledge: index unavailable");
            counter("error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let pos = [query.x, query.y, query.z];
    let cell = crate::knowledge::cell_key(pos);
    match crate::knowledge::sweep_fresh(&state.pool, &cell).await {
        Ok(true) => counter("fresh"),
        Ok(false) => {
            if let Some(_guard) = state.knowledge_flight.begin(&cell) {
                let _permit = state.edsm_pacer.permit().await;
                match crate::knowledge::fetch_and_learn_sphere(
                    &state.pool,
                    &state.http_client,
                    pos,
                    radius,
                    &cell,
                )
                .await
                {
                    Ok((systems, learned)) => {
                        counter("fetched");
                        tracing::info!(systems, learned, "knowledge: cell swept via EDSM");
                    }
                    Err(error) => {
                        // Serve what we already know; the cell stays
                        // stale so the next request retries.
                        counter("edsm_error");
                        tracing::warn!(%error, "knowledge: EDSM sphere fetch failed");
                    }
                }
            } else {
                state.knowledge_flight.wait(&cell).await;
                counter("coalesced");
            }
        }
        Err(error) => {
            tracing::warn!(%error, "knowledge: sweep lookup failed");
            counter("error");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }
    match crate::knowledge::answer_sphere(&state.pool, &handle.galaxy, pos, radius).await {
        Ok((answer, truncated)) => {
            if truncated {
                tracing::info!(
                    cap = crate::knowledge::MAX_SPHERE_SYSTEMS,
                    "knowledge: sphere answer truncated"
                );
            }
            axum::Json(answer).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, "knowledge: sphere answer failed");
            counter("error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(serde::Deserialize)]
struct SystemQuery {
    name: String,
}

/// GET /v1/knowledge/system — the by-name twin of the sphere proxy, so a
/// commander's Galaxy-tab lookup reaches EDSM through US or not at all
/// (maintainer, 2026-09-06). Served from our own tables when we know the name;
/// one upstream fetch for the whole fleet when we do not, learned on the
/// way back. Never logs the name.
async fn knowledge_system(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<SystemQuery>,
) -> impl IntoResponse {
    let counter = |outcome: &'static str| {
        metrics::counter!("edda_knowledge_system_requests_total", "outcome" => outcome).increment(1)
    };
    let name = query.name.trim();
    if name.is_empty() || name.len() > 128 {
        counter("invalid");
        return (StatusCode::BAD_REQUEST, "name required").into_response();
    }
    if state
        .knowledge_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let started = std::time::Instant::now();
    // The routing index mapped for /v1/route: consulted before EDSM
    // (RULING 2026-09-07). None before the first publication — then the
    // lookup simply goes upstream as it did.
    let galaxy = state.galaxy.current().await.ok().flatten();
    let result = crate::knowledge::system_document(
        &state.pool,
        &state.http_client,
        &state.edsm_pacer,
        &state.knowledge_flight,
        galaxy.as_ref().map(|h| &*h.galaxy),
        name,
    )
    .await;
    metrics::histogram!("edda_knowledge_system_seconds").record(started.elapsed().as_secs_f64());
    use crate::knowledge::SystemOutcome;
    match result {
        Ok(SystemOutcome::Local(answer)) => {
            counter("local");
            axum::Json(answer).into_response()
        }
        Ok(SystemOutcome::Indexed(answer)) => {
            counter("indexed");
            axum::Json(answer).into_response()
        }
        Ok(SystemOutcome::Fetched(answer)) => {
            counter("fetched");
            axum::Json(answer).into_response()
        }
        Ok(SystemOutcome::Coalesced(Some(answer))) => {
            counter("coalesced");
            axum::Json(answer).into_response()
        }
        // A name nobody knows is a 404, not an error: the Galaxy tab says
        // "not in the galaxy database" and means it.
        Ok(SystemOutcome::Coalesced(None)) | Ok(SystemOutcome::Unknown) => {
            counter("unknown");
            StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => {
            // The name is deliberately absent from this line.
            tracing::warn!(%error, "knowledge: system lookup failed");
            counter("error");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

#[derive(serde::Deserialize)]
struct CompleteQuery {
    kind: String,
    prefix: String,
    limit: Option<usize>,
}

/// GET /v1/names/complete?kind=system|station&prefix=&limit= — the
/// server half of the client's autocomplete (RULING 2026-09-07: the
/// server knows more than every client; a remote-first install carries
/// only the bundled bubble's names). Systems come from the mapped
/// routing index, stations from the identity table. A too-short prefix
/// is an empty list, not an error. Never logs the prefix.
async fn names_complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<CompleteQuery>,
) -> impl IntoResponse {
    use crate::names;
    let kind: &'static str = match query.kind.as_str() {
        "system" => "system",
        "station" => "station",
        _ => {
            metrics::counter!("edda_names_complete_requests_total", "kind" => "invalid", "outcome" => "invalid")
                .increment(1);
            return (StatusCode::BAD_REQUEST, "kind is \"system\" or \"station\"").into_response();
        }
    };
    let counter = |outcome: &'static str| {
        metrics::counter!("edda_names_complete_requests_total", "kind" => kind, "outcome" => outcome).increment(1)
    };
    let Some(prefix) = names::usable_prefix(&query.prefix) else {
        counter("short");
        return axum::Json(Vec::<names::NameHit>::new()).into_response();
    };
    if state
        .names_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let limit = names::clamp_limit(query.limit);
    let started = std::time::Instant::now();
    let result: anyhow::Result<Vec<names::NameHit>> = match kind {
        "system" => match state.galaxy.current().await {
            Ok(Some(handle)) => Ok(names::complete_systems(&handle.galaxy, prefix, limit)),
            Ok(None) => {
                counter("no_galaxy");
                return (StatusCode::SERVICE_UNAVAILABLE, "no galaxy published yet")
                    .into_response();
            }
            Err(error) => Err(error),
        },
        _ => names::complete_stations(&state.pool, prefix, limit).await,
    };
    metrics::histogram!("edda_names_complete_seconds", "kind" => kind)
        .record(started.elapsed().as_secs_f64());
    match result {
        Ok(hits) => {
            counter(if hits.is_empty() { "empty" } else { "hits" });
            axum::Json(hits).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, kind, "names: completion failed");
            counter("error");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

#[derive(serde::Deserialize)]
struct BodiesQuery {
    #[serde(rename = "systemName")]
    system_name: String,
}

/// GET /v1/knowledge/bodies — cached verbatim EDSM passthrough.
async fn knowledge_bodies(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<BodiesQuery>,
) -> impl IntoResponse {
    if query.system_name.trim().is_empty() || query.system_name.len() > 128 {
        return (StatusCode::BAD_REQUEST, "systemName required").into_response();
    }
    if state
        .knowledge_limiter
        .allow(&source_of(&headers), std::time::Instant::now())
        .is_err()
    {
        metrics::counter!("edda_knowledge_requests_total", "endpoint" => "bodies", "outcome" => "rate_limited")
            .increment(1);
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    match crate::knowledge::bodies_document(
        &state.pool,
        &state.http_client,
        &state.edsm_pacer,
        &state.knowledge_flight,
        query.system_name.trim(),
    )
    .await
    {
        Ok(Some(body)) => ([(header::CONTENT_TYPE, "application/json")], body).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(error) => {
            metrics::counter!("edda_knowledge_requests_total", "endpoint" => "bodies", "outcome" => "edsm_error")
                .increment(1);
            tracing::warn!(%error, "knowledge: bodies fetch failed");
            StatusCode::BAD_GATEWAY.into_response()
        }
    }
}

/// Prometheus-format scrape of everything the process has emitted.
/// The text exposition format is the lingua franca: Prometheus,
/// VictoriaMetrics/vmagent, and Grafana Alloy all ingest it as-is.
async fn scrape(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render(),
    )
}

/// Main-star classes for the given system addresses: what the store knows
/// now, and which it has queued for an EDSM lookup (ask again later).
async fn stars(
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<crate::stars::StarsQuery>,
) -> impl IntoResponse {
    let ids: Vec<i64> = query
        .ids
        .split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect();
    if ids.len() > crate::stars::MAX_LOOKUP_IDS {
        return (StatusCode::BAD_REQUEST, "too many ids").into_response();
    }
    match crate::stars::answer_stars(&state.pool, &ids).await {
        Ok(answer) => axum::Json(answer).into_response(),
        Err(error) => {
            tracing::warn!(%error, "stars lookup failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn manifest(State(state): State<AppState>) -> impl IntoResponse {
    let Ok(bytes) = tokio::fs::read(&*state.manifest_path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if manifest.validate().is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let mut response = Response::new(Body::from(bytes));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response.into_response()
}

async fn artifact(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !safe_relative_path(&path) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Ok(manifest_bytes) = tokio::fs::read(&*state.manifest_path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&manifest_bytes) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let Some(file) = manifest.artifact(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(mut source) = tokio::fs::File::open(state.artifact_dir.join(&path)).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(metadata) = source.metadata().await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    if metadata.len() != file.bytes {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let range = match requested_range(headers.get(header::RANGE), metadata.len()) {
        Ok(range) => range,
        Err(status) => return status.into_response(),
    };
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    if source.seek(SeekFrom::Start(range.0)).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let length = range.1 - range.0 + 1;
    let Ok(length_usize) = usize::try_from(length) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let mut bytes = vec![0; length_usize];
    if source.read_exact(&mut bytes).await.is_err() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    let partial = length != metadata.len();
    metrics::counter!("edda_artifact_bytes_total").increment(length);
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let response_headers = response.headers_mut();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    response_headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    response_headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    if let Ok(etag) = HeaderValue::from_str(&format!("\"{}\"", file.sha256)) {
        response_headers.insert(header::ETAG, etag);
    }
    if partial {
        if let Ok(content_range) =
            HeaderValue::from_str(&format!("bytes {}-{}/{}", range.0, range.1, metadata.len()))
        {
            response_headers.insert(header::CONTENT_RANGE, content_range);
        }
    }
    response.into_response()
}

/// App self-update releases: `latest.json` plus the signed installer
/// packages, dropped into `<artifact_dir>/app/` by the release script.
/// Unlike data artifacts these are not manifest-gated — the updater's
/// signature verification is the integrity check that matters, and the
/// manifest is the update's own `latest.json`. The path rules are the
/// artifact route's.
/// Classify who is fetching a release file, from the User-Agent alone:
/// the SITE's download buttons arrive as browsers (a fresh install, to
/// a first approximation), the in-app updater as its own client, and
/// anything else (curl, mirrors) as other. An aggregate label, never
/// stored per-request — the maintainer's "can we track downloads?" answered
/// inside the surveillance law.
fn download_via(headers: &HeaderMap) -> &'static str {
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if ua.contains("Mozilla/") {
        "browser"
    } else if ua.contains("tauri") || ua.contains("EDDA") {
        "updater"
    } else {
        "other"
    }
}

async fn app_release(
    State(state): State<AppState>,
    AxumPath(path): AxumPath<String>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if !safe_relative_path(&path) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let full = state.artifact_dir.join("app").join(&path);
    let Ok(bytes) = tokio::fs::read(&full).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    metrics::counter!("edda_app_release_bytes_total").increment(bytes.len() as u64);
    // Fleet pulse: manifest polls are update CHECKS; package fetches are
    // downloads, split browser (site button ≈ fresh install) vs updater
    // (existing install updating). File names are a bounded set — one
    // label value per published artifact.
    if path.ends_with("latest.json") {
        metrics::counter!("edda_update_checks_total", "via" => download_via(&headers)).increment(1);
    } else {
        let file = path.rsplit('/').next().unwrap_or(&path).to_string();
        metrics::counter!(
            "edda_app_downloads_total",
            "file" => file,
            "via" => download_via(&headers)
        )
        .increment(1);
    }
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    let json = path.ends_with(".json");
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(if json {
            "application/json"
        } else {
            "application/octet-stream"
        }),
    );
    // latest.json must always be revalidated; the packages are immutable
    // (their names carry the version).
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if json {
            "no-cache"
        } else {
            "public, max-age=31536000, immutable"
        }),
    );
    response.into_response()
}

/// Anonymous problem reports (wire contract, ledger 2026-09-05):
/// truncation-not-rejection on every field, per-source rate limit from
/// the proxy's X-Forwarded-For (in memory only — never stored), 202
/// {id} on success. Empty text is the one refusal: there is nothing to
/// keep.
#[derive(serde::Deserialize)]
struct FeedbackIn {
    #[serde(default)]
    version: String,
    #[serde(default)]
    os: String,
    text: String,
    #[serde(default)]
    log_tail: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
}

async fn feedback(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(body): axum::Json<FeedbackIn>,
) -> impl IntoResponse {
    use crate::feedback::{clip, LOG_CAP, META_CAP, TEXT_CAP};
    let source = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("unknown")
        .to_owned();
    if !state
        .feedback_limiter
        .allow(&source, std::time::Instant::now())
    {
        metrics::counter!("edda_feedback_total", "outcome" => "rate_limited").increment(1);
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if body.text.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "say a few words first").into_response();
    }
    let inserted = sqlx::query_scalar::<_, i64>(
        "INSERT INTO feedback (version, os, body, log_tail, client_created_at)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(clip(&body.version, META_CAP))
    .bind(clip(&body.os, META_CAP))
    .bind(clip(body.text.trim(), TEXT_CAP))
    .bind(body.log_tail.as_deref().map(|l| clip(l, LOG_CAP)))
    .bind(body.created_at.as_deref().map(|c| clip(c, META_CAP)))
    .fetch_one(&state.pool)
    .await;
    match inserted {
        Ok(id) => {
            metrics::counter!("edda_feedback_total", "outcome" => "accepted").increment(1);
            tracing::info!(id, "feedback received");
            (
                StatusCode::ACCEPTED,
                axum::Json(serde_json::json!({ "id": id })),
            )
                .into_response()
        }
        Err(error) => {
            tracing::warn!(%error, "feedback insert failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// /v1/telemetry: validate against the ledgered contract, then re-emit
/// as edda_client_* metrics — no rows, no identity, cardinality fenced.
async fn telemetry(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(batch): axum::Json<crate::telemetry::Batch>,
) -> impl IntoResponse {
    use crate::telemetry::{
        validate, MAX_COUNT, MAX_EVENTS, MAX_FEATURES, MAX_TIMINGS, MAX_TIMING_MS,
    };
    let source = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("unknown")
        .to_owned();
    if !state
        .telemetry_limiter
        .allow(&source, std::time::Instant::now())
    {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    if let Err(reason) = validate(&batch) {
        metrics::counter!("edda_telemetry_batches_total", "outcome" => "rejected").increment(1);
        return (StatusCode::BAD_REQUEST, reason).into_response();
    }
    let version = batch.version.clone();
    for event in batch.events.iter().take(MAX_EVENTS) {
        let callsite = state.callsites.admit(&event.callsite);
        metrics::counter!(
            "edda_client_events_total",
            "callsite" => callsite,
            "level" => event.level.clone(),
            "version" => version.clone()
        )
        .increment(event.count.clamp(1, MAX_COUNT));
    }
    for timing in batch.timings.iter().take(MAX_TIMINGS) {
        metrics::histogram!(
            "edda_client_timing_ms",
            "kind" => timing.kind.clone(),
            "ok" => if timing.ok { "true" } else { "false" },
            "version" => version.clone()
        )
        .record(timing.ms.min(MAX_TIMING_MS));
    }
    for name in batch.enabled_features.iter().take(MAX_FEATURES) {
        metrics::counter!(
            "edda_client_feature_total",
            "name" => name.clone(),
            "enabled" => "true"
        )
        .increment(1);
    }
    for search in batch.searches.iter().take(crate::telemetry::MAX_SEARCHES) {
        // Validated above: the kind maps to its own histogram (hours
        // and light-years carry different bucket scales).
        if let Some(metric) = crate::telemetry::search_metric(&search.kind) {
            metrics::histogram!(metric, "version" => version.clone())
                .record(f64::from(search.value));
        }
    }
    metrics::counter!("edda_telemetry_batches_total", "outcome" => "accepted").increment(1);
    StatusCode::ACCEPTED.into_response()
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn requested_range(header: Option<&HeaderValue>, size: u64) -> Result<(u64, u64), StatusCode> {
    if size == 0 {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    let Some(header) = header else {
        return Ok((0, size - 1));
    };
    let value = header.to_str().map_err(|_| StatusCode::BAD_REQUEST)?;
    let value = value
        .strip_prefix("bytes=")
        .ok_or(StatusCode::BAD_REQUEST)?;
    if value.contains(',') {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    let (start, end) = value.split_once('-').ok_or(StatusCode::BAD_REQUEST)?;
    let (start, end) = if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| StatusCode::BAD_REQUEST)?;
        if suffix == 0 {
            return Err(StatusCode::RANGE_NOT_SATISFIABLE);
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start = start.parse::<u64>().map_err(|_| StatusCode::BAD_REQUEST)?;
        let end = if end.is_empty() {
            size - 1
        } else {
            end.parse::<u64>().map_err(|_| StatusCode::BAD_REQUEST)?
        };
        (start, end.min(size - 1))
    };
    if start >= size || start > end {
        return Err(StatusCode::RANGE_NOT_SATISFIABLE);
    }
    Ok((start, end))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    let database = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
        .is_ok();
    let hydration = if database {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM service_hydrations WHERE status = 'complete')",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap_or(false)
    } else {
        false
    };
    let manifest = valid_manifest(&state.manifest_path).await;
    // Whichever process runs the feed (in here, or edda-eddn.service),
    // the table says whether boards are landing.
    let eddn_receiving = database && crate::ingest::eddn_receiving(&state.pool).await;
    let ready = database && hydration && manifest;
    let response = ReadinessResponse {
        status: if ready { "ready" } else { "not_ready" },
        checks: ReadinessChecks {
            database,
            hydration,
            manifest,
            eddn: if eddn_receiving {
                "receiving"
            } else {
                "waiting"
            },
        },
    };

    if ready {
        (StatusCode::OK, Json(response))
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(response))
    }
}

async fn valid_manifest(path: &PathBuf) -> bool {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return false;
    };
    let Ok(manifest) = serde_json::from_slice::<Manifest>(&bytes) else {
        return false;
    };
    manifest.validate().is_ok()
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use sqlx::postgres::PgPoolOptions;
    use tower::ServiceExt;

    use super::*;

    #[test]
    fn an_install_id_keys_the_limiter_and_a_bad_one_is_ignored() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.9, 10.0.0.1".parse().unwrap());
        assert_eq!(source_of(&h), "203.0.113.9");
        h.insert(
            "x-edda-install",
            "0123456789abcdef0123456789abcdef".parse().unwrap(),
        );
        assert_eq!(source_of(&h), "install:0123456789abcdef0123456789abcdef");
        h.insert("x-edda-install", "not-hex".parse().unwrap());
        assert_eq!(source_of(&h), "203.0.113.9");
    }

    #[tokio::test]
    async fn health_does_not_require_database() {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://edda:edda@127.0.0.1:1/edda")
            .unwrap();
        let metrics = metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle();
        let response = router(AppState::new(pool, PathBuf::from("missing"), metrics))
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&body[..], br#"{"status":"ok"}"#);
    }

    fn state_without_data() -> AppState {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://edda:edda@127.0.0.1:1/edda")
            .unwrap();
        let metrics = metrics_exporter_prometheus::PrometheusBuilder::new()
            .build_recorder()
            .handle();
        AppState::new(pool, PathBuf::from("missing"), metrics)
    }

    async fn get(path: &str) -> (StatusCode, axum::body::Bytes) {
        let response = router(state_without_data())
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        (
            status,
            response.into_body().collect().await.unwrap().to_bytes(),
        )
    }

    /// The completion endpoint's contract that needs no data: a bad kind
    /// is a 400, a one-letter prefix is an empty list (not an error — the
    /// client asks on every keystroke), and systems before the first
    /// publication are 503 like the sphere proxy, never a silent [].
    #[tokio::test]
    async fn names_complete_contract_without_data() {
        let (status, _) = get("/v1/names/complete?kind=planet&prefix=so").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, body) = get("/v1/names/complete?kind=system&prefix=s").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(&body[..], b"[]");
        let (status, _) = get("/v1/names/complete?kind=system&prefix=so").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        let (status, _) = get("/v1/names/complete?kind=system").await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "prefix is required");
    }

    /// A v2 trade body (it carries `ship`) that does not parse is a 400
    /// naming the field, never a quiet drop to the legacy legs; the short
    /// pad spelling parses (so it reaches the trade gate, which needs a
    /// database: 5xx here, not 400).
    #[tokio::test]
    async fn a_v2_trade_body_never_falls_through_to_legacy() {
        async fn post(body: serde_json::Value) -> (StatusCode, String) {
            let response = router(state_without_data())
                .oneshot(
                    Request::post("/v1/trade/search")
                        .header("content-type", "application/json")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            (status, String::from_utf8_lossy(&bytes).into_owned())
        }
        let ship = serde_json::json!({"cargo_capacity": 720, "jump_range_ly": 30.5, "laden_range_ly": 22.1});
        let (status, body) = post(
            serde_json::json!({"system": "Sol", "ship": ship, "constraints": {"min_pad": "xl"}}),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("pad size"), "{body}");
        let (status, body) = post(
            serde_json::json!({"system": "Sol", "ship": ship, "constraints": {"min_pad": "l"}}),
        )
        .await;
        // Parsed, so it reached the trade gate; without a database that is
        // a pool error - what matters is that it is neither the pad-size
        // complaint nor the legacy `legs` shape.
        assert!(
            !body.contains("pad size") && !body.contains("\"legs\""),
            "the short spelling parses and stays v2: {status} {body}"
        );
    }

    #[test]
    fn parses_single_byte_ranges() {
        assert_eq!(requested_range(None, 10).unwrap(), (0, 9));
        assert_eq!(
            requested_range(Some(&HeaderValue::from_static("bytes=2-4")), 10).unwrap(),
            (2, 4)
        );
        assert_eq!(
            requested_range(Some(&HeaderValue::from_static("bytes=-3")), 10).unwrap(),
            (7, 9)
        );
        assert!(requested_range(Some(&HeaderValue::from_static("bytes=11-")), 10).is_err());
    }
}
