//! Prometheus observability for the API server (ROUTING-NEXT item 15).
//!
//! Architecture: every crate emits through the `metrics` facade —
//! `counter!` / `gauge!` / `histogram!` are a few atomic ops with a
//! recorder installed and no-ops without one, the same contract as
//! `tracing` with no subscriber. This module owns the recorder: it
//! installs the Prometheus exporter, samples the gauges that have to be
//! read rather than emitted (DB pool, sink queue), and renders the
//! scrape text served at `/metrics`.
//!
//! ## Series naming (the conventions the whole workspace follows)
//!
//! Everything is prefixed `edda_`; durations are `_seconds` histograms,
//! monotonic counts are `_total`, sampled values are gauges.
//!
//! | Concern | Series |
//! |---|---|
//! | EDDN feed | `edda_eddn_frames_received_total`, `edda_eddn_decode_errors_total`, `edda_eddn_reconnects_total{reason}` |
//! | EDDN writer | `edda_eddn_batches_total{outcome}`, `edda_eddn_operations_total{outcome}`, `edda_eddn_batch_apply_seconds`, `edda_eddn_messages_applied_total`, `edda_eddn_messages_skipped_total`, `edda_eddn_rows_total{kind}`, `edda_eddn_last_apply_unix_seconds` |
//! | EDDN sink | `edda_eddn_queue_depth`, `edda_eddn_queue_capacity` |
//! | Database | `edda_db_pool_connections`, `edda_db_pool_idle` |
//! | HTTP | `edda_http_requests_total{route,method,status}`, `edda_http_request_seconds{route}`, `edda_artifact_bytes_total` |
//! | EDSM | `edda_stars_requests_total`, `edda_stars_ids_total{answer}`, `edda_edsm_lookups_total{outcome}` |
//! | Routing (server-side plots, when the endpoint lands; the client already uses these names locally) | `edda_route_requests_total{outcome}`, `edda_route_wall_seconds` |
//! | Market search (design (b)) | `edda_market_search_requests_total{outcome}`, `edda_market_search_seconds` |
//! | Station board | `edda_station_board_requests_total{outcome}`, `edda_station_board_seconds` |
//! | Client telemetry (opt-in, pooled, unlabelled) | `edda_client_search_age_hours`, `edda_client_router_gate_ly`, `edda_client_carrier_stats_age_hours` |
//! | Trade search (thin clients) | `edda_trade_search_requests_total{outcome}`, `edda_trade_search_seconds`, `edda_trade_search_cache_total{result}` |
//! | Stations (system / near+service / name) | `edda_stations_requests_total{mode,outcome}`, `edda_stations_seconds{mode}` |
//! | Trade report (v2, the whole ProfitReport server-side) | `edda_trade_search_requests_total{outcome="report"}`, `edda_trade_report_phase_seconds{phase}` (candidates, market, guards, pairing, rings), `edda_trade_report_stations` |
//! | Knowledge proxy (EDSM through us: sphere, bodies, by-name system) | `edda_knowledge_requests_total{endpoint,outcome}`, `edda_knowledge_system_requests_total{outcome}`, `edda_knowledge_system_seconds`, `edda_edsm_proxy_seconds{endpoint}` |
//! | Reconcile (item 47; no-ops from the CLI, live when reconcile moves in-process) | `edda_reconcile_runs_total{outcome}`, `edda_reconcile_ops_total{op}`, `edda_reconcile_overlay_bytes_total`, `edda_reconcile_diff_seconds`, `edda_reconcile_apply_seconds` |
//! | Publications (the delta-build cost, read back from `artifact_publications` because the builder is a short-lived CLI) | `edda_publish_build_seconds{product}`, `edda_publish_cpu_seconds{product}`, `edda_publish_phase_seconds{product,phase}`, `edda_publish_bytes{product}`, `edda_publish_rows{product}`, `edda_publish_last_complete_unix_seconds{product}` |
//! | Process | `edda_build_info{version}`, `edda_uptime_seconds` |
//!
//! Alert-worthy: `edda_eddn_last_apply_unix_seconds` going stale is the
//! 23-silent-hours wedge; `edda_eddn_queue_depth` pinned at capacity is
//! wedge #2 (writer stuck behind a poison batch); `edda_eddn_batches_total{outcome="dropped"}`
//! rising means data loss the next observation must heal.

use std::time::{Duration, Instant};

use axum::{
    extract::{MatchedPath, Request},
    middleware::Next,
    response::IntoResponse,
};
use ed_domain::Operation;
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use sqlx::PgPool;
use tokio::sync::mpsc::WeakSender;

/// Histogram buckets for request/batch durations: 1 ms to 60 s,
/// roughly x3 per step. Explicit buckets rather than the exporter's
/// summary default so Grafana gets `histogram_quantile` and rates.
const SECONDS_BUCKETS: &[f64] = &[
    0.001, 0.003, 0.01, 0.03, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0, 60.0,
];

/// Client timings arrive in milliseconds (`edda_client_timing_ms`, the
/// telemetry contract): 10 ms to 20 min, x3-ish steps — plots run
/// 1-20 s, hydrates minutes.
const MS_BUCKETS: &[f64] = &[
    10.0, 30.0, 100.0, 300.0, 1_000.0, 3_000.0, 10_000.0, 30_000.0, 100_000.0, 300_000.0, 1_200_000.0,
];

/// Search-parameter distributions (wire expansion 2026-09-05): what
/// freshness people search trade at, and where their router-distance
/// GATE sits (default 1,000 ly; 0 = EDDA always plans, which the
/// le="0" bucket captures for the always-plans-share panel).
const SEARCH_AGE_BUCKETS: &[f64] = &[1.0, 2.0, 3.0, 6.0, 12.0, 24.0, 48.0, 72.0, 168.0, 336.0, 720.0];
const ROUTER_GATE_BUCKETS: &[f64] = &[0.0, 50.0, 100.0, 250.0, 500.0, 1_000.0, 2_000.0, 5_000.0, 20_000.0];
/// Carrier-picture staleness (Item 52 A). CarrierStats fires only when
/// the commander opens carrier management, so these buckets are the
/// hours-to-a-month scale review asked for: 1 h, 6 h, a day, three
/// days, a week, a month.
const CARRIER_AGE_BUCKETS: &[f64] = &[1.0, 6.0, 24.0, 72.0, 168.0, 720.0];

/// Install the process-wide recorder. Call once, before anything emits;
/// a second call (tests, embedders) returns the error rather than
/// panicking.
pub fn install() -> Result<PrometheusHandle, anyhow::Error> {
    let handle = PrometheusBuilder::new()
        .set_buckets_for_metric(Matcher::Suffix("_seconds".into()), SECONDS_BUCKETS)?
        .set_buckets_for_metric(Matcher::Suffix("_ms".into()), MS_BUCKETS)?
        .set_buckets_for_metric(Matcher::Full("edda_client_search_age_hours".into()), SEARCH_AGE_BUCKETS)?
        .set_buckets_for_metric(Matcher::Full("edda_client_router_gate_ly".into()), ROUTER_GATE_BUCKETS)?
        .set_buckets_for_metric(Matcher::Full("edda_client_carrier_stats_age_hours".into()), CARRIER_AGE_BUCKETS)?
        .install_recorder()?;
    metrics::gauge!("edda_build_info", "version" => env!("CARGO_PKG_VERSION")).set(1.0);
    Ok(handle)
}

/// Sample the read-not-emitted gauges every few seconds for as long as
/// the server runs: pool health, sink queue depth, uptime. The sink
/// sender is held weakly so this task never keeps the writer's channel
/// alive on its own. Publication build costs piggyback here on a slower
/// cadence: the builder is a short-lived CLI, so its numbers live in
/// `artifact_publications` and this loop re-emits the latest complete
/// row per product.
pub async fn run_gauges(pool: PgPool, sink: WeakSender<Operation>) {
    let started = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks: u64 = 0;
    loop {
        tick.tick().await;
        metrics::gauge!("edda_uptime_seconds").set(started.elapsed().as_secs_f64());
        metrics::gauge!("edda_db_pool_connections").set(pool.size() as f64);
        metrics::gauge!("edda_db_pool_idle").set(pool.num_idle() as f64);
        if let Some(sender) = sink.upgrade() {
            let capacity = sender.max_capacity();
            metrics::gauge!("edda_eddn_queue_capacity").set(capacity as f64);
            metrics::gauge!("edda_eddn_queue_depth").set((capacity - sender.capacity()) as f64);
        }
        // Every 60 s is plenty for numbers that change once a day.
        if ticks % 12 == 0 {
            match latest_publication_costs(&pool).await {
                Ok(rows) => {
                    for row in &rows {
                        for (name, labels, value) in publication_series(row) {
                            metrics::gauge!(name, labels).set(value);
                        }
                    }
                }
                Err(error) => tracing::debug!(%error, "publication cost sampling failed"),
            }
        }
        ticks += 1;
    }
}

/// The cost columns of the newest complete publication per product, as
/// the daily/weekly CLI builders recorded them (migration 0011).
pub(crate) struct PublicationCost {
    pub product: String,
    pub build_seconds: f64,
    pub cpu_seconds: Option<f64>,
    pub bytes: Option<i64>,
    pub rows: Option<i64>,
    pub phase_seconds: Option<String>,
    pub completed_unix: f64,
}

async fn latest_publication_costs(pool: &PgPool) -> Result<Vec<PublicationCost>, sqlx::Error> {
    let rows: Vec<(String, f64, Option<f64>, Option<i64>, Option<i64>, Option<String>, f64)> =
        sqlx::query_as(
            "SELECT DISTINCT ON (product) product, \
             EXTRACT(EPOCH FROM completed_at - created_at)::DOUBLE PRECISION, \
             cpu_seconds, artifact_bytes, rows_published, phase_seconds::text, \
             EXTRACT(EPOCH FROM completed_at)::DOUBLE PRECISION \
             FROM artifact_publications WHERE status = 'complete' \
             ORDER BY product, id DESC",
        )
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(product, build_seconds, cpu_seconds, bytes, rows, phase_seconds, completed_unix)| {
            PublicationCost { product, build_seconds, cpu_seconds, bytes, rows, phase_seconds, completed_unix }
        })
        .collect())
}

/// Pure row→gauge mapping so the shape is testable without a recorder:
/// columns that pre-date migration 0011 are absent, never zero, and the
/// phase JSON fans out into one labelled gauge per phase.
pub(crate) fn publication_series(
    row: &PublicationCost,
) -> Vec<(&'static str, Vec<metrics::Label>, f64)> {
    let product = |extra: Option<(&str, String)>| -> Vec<metrics::Label> {
        let mut labels = vec![metrics::Label::new("product", row.product.clone())];
        if let Some((key, value)) = extra {
            labels.push(metrics::Label::new(key.to_owned(), value));
        }
        labels
    };
    let mut series = vec![
        ("edda_publish_build_seconds", product(None), row.build_seconds),
        ("edda_publish_last_complete_unix_seconds", product(None), row.completed_unix),
    ];
    if let Some(cpu) = row.cpu_seconds {
        series.push(("edda_publish_cpu_seconds", product(None), cpu));
    }
    if let Some(bytes) = row.bytes {
        series.push(("edda_publish_bytes", product(None), bytes as f64));
    }
    if let Some(rows) = row.rows {
        series.push(("edda_publish_rows", product(None), rows as f64));
    }
    if let Some(phases) = row
        .phase_seconds
        .as_deref()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
    {
        if let Some(map) = phases.as_object() {
            for (phase, value) in map {
                if let Some(seconds) = value.as_f64() {
                    series.push((
                        "edda_publish_phase_seconds",
                        product(Some(("phase", phase.clone()))),
                        seconds,
                    ));
                }
            }
        }
    }
    series
}

/// Axum middleware: every request becomes a labelled count and a
/// duration observation. The route label is the matched PATTERN
/// (`/v1/artifacts/{*path}`), never the raw path — raw paths are
/// unbounded cardinality, which is how a scrape endpoint ends up
/// larger than the data it measures.
pub async fn track_http(request: Request, next: Next) -> impl IntoResponse {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());
    let method = request.method().as_str().to_owned();
    let started = Instant::now();
    let response = next.run(request).await;
    let status = response.status().as_u16().to_string();
    metrics::counter!("edda_http_requests_total", "route" => route.clone(), "method" => method, "status" => status).increment(1);
    metrics::histogram!("edda_http_request_seconds", "route" => route).record(started.elapsed().as_secs_f64());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One recorder per process: install() must hand out the handle,
    /// emissions must show up in the render with the edda_ prefix, and
    /// _seconds histograms must render as bucketed histograms (not the
    /// exporter's summary default) so Grafana can histogram_quantile.
    #[test]
    fn install_renders_emitted_series_with_buckets() {
        let handle = install().expect("first install in this process");
        metrics::counter!("edda_eddn_batches_total", "outcome" => "applied").increment(3);
        metrics::histogram!("edda_eddn_batch_apply_seconds").record(0.02);
        metrics::gauge!("edda_eddn_queue_depth").set(7.0);
        let text = handle.render();
        assert!(text.contains(r#"edda_eddn_batches_total{outcome="applied"} 3"#), "{text}");
        assert!(text.contains(r#"edda_eddn_batch_apply_seconds_bucket{le="0.03"} 1"#), "{text}");
        assert!(text.contains("edda_eddn_queue_depth 7"), "{text}");
        assert!(text.contains(r#"edda_build_info{version=""#), "{text}");
        // A second install must fail loudly instead of silently
        // replacing the recorder everything already emits into.
        assert!(install().is_err());
    }

    fn series_value(series: &[(&'static str, Vec<metrics::Label>, f64)], name: &str) -> Option<f64> {
        series.iter().find(|(n, _, _)| *n == name).map(|(_, _, v)| *v)
    }

    /// A fully instrumented row (migration 0011 columns present) fans
    /// out into all six series, with one phase gauge per JSON key.
    #[test]
    fn publication_rows_fan_out_to_labelled_gauges() {
        let row = PublicationCost {
            product: "market_daily".to_owned(),
            build_seconds: 42.5,
            cpu_seconds: Some(37.2),
            bytes: Some(9_000_000),
            rows: Some(1_234_567),
            phase_seconds: Some(r#"{"identity":1.5,"stream":30.0,"compress":8.0}"#.to_owned()),
            completed_unix: 1_757_000_000.0,
        };
        let series = publication_series(&row);
        assert_eq!(series_value(&series, "edda_publish_build_seconds"), Some(42.5));
        assert_eq!(series_value(&series, "edda_publish_cpu_seconds"), Some(37.2));
        assert_eq!(series_value(&series, "edda_publish_bytes"), Some(9_000_000.0));
        assert_eq!(series_value(&series, "edda_publish_rows"), Some(1_234_567.0));
        assert_eq!(series_value(&series, "edda_publish_last_complete_unix_seconds"), Some(1_757_000_000.0));
        let phases: Vec<_> = series
            .iter()
            .filter(|(name, _, _)| *name == "edda_publish_phase_seconds")
            .collect();
        assert_eq!(phases.len(), 3);
        for (_, labels, _) in &phases {
            assert!(labels.iter().any(|l| l.key() == "product" && l.value() == "market_daily"));
            assert!(labels.iter().any(|l| l.key() == "phase"));
        }
    }

    /// Rows written before migration 0011 have NULL cost columns: they
    /// must emit only what they know (duration + completion), never a
    /// fabricated zero that a dashboard would read as "free build".
    #[test]
    fn pre_migration_rows_emit_no_fabricated_zeroes() {
        let row = PublicationCost {
            product: "community".to_owned(),
            build_seconds: 300.0,
            cpu_seconds: None,
            bytes: None,
            rows: None,
            phase_seconds: None,
            completed_unix: 1_756_000_000.0,
        };
        let series = publication_series(&row);
        assert_eq!(series.len(), 2, "{series:?}");
        assert_eq!(series_value(&series, "edda_publish_build_seconds"), Some(300.0));
        assert_eq!(series_value(&series, "edda_publish_last_complete_unix_seconds"), Some(1_756_000_000.0));
    }
}
