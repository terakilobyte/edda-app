//! `ed-api ingest`: the EDDN feed as its own process (maintainer, 2026-09-09:
//! "run the eddn watcher as a separate process … so we can do server
//! binary swaps without losing freshness").
//!
//! Measured before building (ledger, 2026-09-09): a `serve` restart
//! lost ~65 s of feed — ~250–300 boards at ~4 applied/s — because the
//! feed lived in the same process and EDDN has no replay. The feed's
//! only consumer was ever the Postgres writer, so the split is the
//! same three tasks (`ed_eddn::live::run_to_channel` → bounded channel
//! → `ed_store::postgres::run_writer`) under their own systemd unit,
//! with Postgres as the handoff. No socket, no queue between the
//! processes: `serve` reads what this wrote, like it always did.
//!
//! Observable on its own bind (`EDDA_API_INGEST_BIND`): `/healthz`,
//! `/readyz` (the feed applied something in the last five minutes),
//! `/metrics` (the `edda_eddn_*` series, `edda_db_pool_*`,
//! `edda_uptime_seconds`, `edda_build_info`). Both processes may run
//! the feed at once during the cutover: the writer is newer-wins per
//! station, so duplicates are idempotent.

use anyhow::{Context, Result};
use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use metrics_exporter_prometheus::PrometheusHandle;
use serde::Serialize;
use sqlx::PgPool;

use crate::config::ServiceConfig;

/// The feed applied a message inside the last five minutes. Recency,
/// not existence: a feed that once received and then stalled reported
/// "receiving" for 23 hours (2026-09-01). EDDN carries messages every
/// second; five silent minutes is down. Shared with `serve`'s readiness.
pub async fn eddn_receiving(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>(
        "SELECT COALESCE(last_message_at > now() - interval '5 minutes', false) FROM eddn_ingestion WHERE singleton",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

#[derive(Clone)]
pub struct IngestState {
    pub pool: PgPool,
    pub metrics: PrometheusHandle,
}

#[derive(Serialize)]
struct IngestReadiness {
    status: &'static str,
    eddn: &'static str,
    database: bool,
}

async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok", "role": "ingest"}))
}

async fn readyz(State(state): State<IngestState>) -> impl IntoResponse {
    let database = sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
        .is_ok();
    let receiving = database && eddn_receiving(&state.pool).await;
    let body = IngestReadiness {
        status: if receiving { "ready" } else { "not_ready" },
        eddn: if receiving { "receiving" } else { "waiting" },
        database,
    };
    if receiving {
        (StatusCode::OK, Json(body))
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(body))
    }
}

async fn scrape(State(state): State<IngestState>) -> impl IntoResponse {
    state.metrics.render()
}

pub fn router(state: IngestState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(scrape))
        .with_state(state)
}

/// Run the feed until the process is told to stop. The metrics recorder
/// is installed here (one per process); the caller must not have
/// installed one.
pub async fn run(config: ServiceConfig, pool: PgPool) -> Result<()> {
    let metrics = crate::metrics::install()?;
    let (sender, receiver) = tokio::sync::mpsc::channel(config.eddn_queue_capacity);
    tokio::spawn(crate::metrics::run_gauges(pool.clone(), sender.downgrade()));
    let writer = tokio::spawn(crate::eddn::run_writer(pool.clone(), receiver));
    let relay = config.eddn_relay.clone();
    let feed = tokio::spawn(async move {
        if let Err(error) = ed_eddn::live::run_to_channel(&relay, sender).await {
            tracing::error!(%error, "EDDN feed stopped");
        }
    });
    let listener = tokio::net::TcpListener::bind(config.ingest_bind)
        .await
        .with_context(|| format!("failed to bind {}", config.ingest_bind))?;
    tracing::info!(bind = %config.ingest_bind, relay = %config.eddn_relay, queue = config.eddn_queue_capacity, "EDDN ingest listening");
    let serve = axum::serve(listener, router(IngestState { pool, metrics }))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        });
    tokio::select! {
        result = serve => result.context("ingest HTTP failed"),
        _ = feed => Err(anyhow::anyhow!("EDDN feed task ended")),
        _ = writer => Err(anyhow::anyhow!("EDDN writer task ended")),
    }
}
