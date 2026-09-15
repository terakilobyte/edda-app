pub mod cli;
pub mod config;
pub mod eddn;
pub mod fdev_data;
pub mod fdev_ids;
pub mod feedback;
pub mod galaxy_service;
pub mod geo;
pub mod http;
pub mod hydration;
pub mod ingest;
pub mod knowledge;
pub mod loadout;
pub mod market_search;
pub mod metrics;
pub mod mining;
pub mod names;
pub mod plot;
pub mod reconcile;
pub mod routing;
pub mod snapshot;
pub mod stars;
pub mod stations;
pub mod telemetry;
pub mod trade_report;
pub mod trade_search;
pub mod version;

use anyhow::{Context, Result};
use config::ServiceConfig;
use sqlx::{postgres::PgPoolOptions, PgPool};

/// Every migration file, in order. Each runs ONCE per database and is
/// recorded in `schema_migrations`; until 2026-09-09 all of them re-ran
/// on every start, and two did real work each time — 0009 scans the
/// whole market table for wrapper symbols, 0015 re-checks 830k stations
/// and re-ANALYZEs at statistics target 2000 — which was the ~55 s a
/// restart spent before "listening" (the assistant's 13:26:50 → 13:27:47).
/// A file's contents are frozen once applied: edit by adding a new one.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_service_skeleton.sql",
        include_str!("../migrations/0001_service_skeleton.sql"),
    ),
    ("0002_eddn.sql", include_str!("../migrations/0002_eddn.sql")),
    (
        "0003_publications.sql",
        include_str!("../migrations/0003_publications.sql"),
    ),
    (
        "0004_station_system_cascade.sql",
        include_str!("../migrations/0004_station_system_cascade.sql"),
    ),
    (
        "0005_stars.sql",
        include_str!("../migrations/0005_stars.sql"),
    ),
    (
        "0006_commodity_metadata.sql",
        include_str!("../migrations/0006_commodity_metadata.sql"),
    ),
    (
        "0007_station_identity.sql",
        include_str!("../migrations/0007_station_identity.sql"),
    ),
    (
        "0008_feedback.sql",
        include_str!("../migrations/0008_feedback.sql"),
    ),
    (
        "0009_commodity_merge.sql",
        include_str!("../migrations/0009_commodity_merge.sql"),
    ),
    (
        "0010_knowledge.sql",
        include_str!("../migrations/0010_knowledge.sql"),
    ),
    (
        "0011_publication_cost.sql",
        include_str!("../migrations/0011_publication_cost.sql"),
    ),
    (
        "0012_market_freshness_index.sql",
        include_str!("../migrations/0012_market_freshness_index.sql"),
    ),
    (
        "0013_system_identity.sql",
        include_str!("../migrations/0013_system_identity.sql"),
    ),
    (
        "0014_station_name_prefix.sql",
        include_str!("../migrations/0014_station_name_prefix.sql"),
    ),
    (
        "0015_systems_cell.sql",
        include_str!("../migrations/0015_systems_cell.sql"),
    ),
    (
        "0016_bodies.sql",
        include_str!("../migrations/0016_bodies.sql"),
    ),
    (
        "0017_station_economy.sql",
        include_str!("../migrations/0017_station_economy.sql"),
    ),
];

pub async fn database_pool(config: &ServiceConfig) -> Result<PgPool> {
    // 30 readers (maintainer, 2026-09-07, after the load bench: "set the reader
    // count to 30"). Postgres max_connections is 100 on the box; the EDDN
    // writer and the publisher take a few more. Raise again only when a
    // bench shows pool_idle at 0 with CPU still idle.
    let started = std::time::Instant::now();
    let pool = PgPoolOptions::new()
        .max_connections(30)
        .connect(&config.database_url)
        .await
        .context("failed to connect to PostgreSQL")?;
    ::metrics::histogram!("edda_startup_phase_seconds", "phase" => "connect")
        .record(started.elapsed().as_secs_f64());
    let started = std::time::Instant::now();
    let applied = migrate(&pool).await?;
    ::metrics::histogram!("edda_startup_phase_seconds", "phase" => "migrations")
        .record(started.elapsed().as_secs_f64());
    tracing::info!(
        applied,
        known = MIGRATIONS.len(),
        ms = started.elapsed().as_millis() as u64,
        "migrations checked"
    );
    Ok(pool)
}

/// Apply every migration not yet recorded, in order; returns how many
/// ran. Each file runs as one `raw_sql` batch exactly as before and is
/// recorded with its wall time, so the trace says which one cost what.
/// Two processes starting at once (serve and ingest) serialise on an
/// advisory lock, so a file never runs twice.
async fn migrate(pool: &PgPool) -> Result<u64> {
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS schema_migrations ( \
             name TEXT PRIMARY KEY, \
             applied_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
             millis BIGINT NOT NULL)",
    )
    .execute(pool)
    .await
    .context("failed to create schema_migrations")?;
    let mut lock = pool
        .acquire()
        .await
        .context("failed to acquire a connection for migrations")?;
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(MIGRATION_LOCK)
        .execute(&mut *lock)
        .await
        .context("failed to take the migration lock")?;
    let outcome = async {
        let done: Vec<(String,)> = sqlx::query_as("SELECT name FROM schema_migrations")
            .fetch_all(pool)
            .await
            .context("failed to read schema_migrations")?;
        let done: std::collections::HashSet<String> = done.into_iter().map(|(n,)| n).collect();
        let mut applied = 0;
        for (name, sql) in MIGRATIONS {
            if done.contains(*name) {
                continue;
            }
            let started = std::time::Instant::now();
            sqlx::raw_sql(sql)
                .execute(pool)
                .await
                .with_context(|| format!("failed to apply migration {name}"))?;
            let millis = started.elapsed().as_millis() as i64;
            sqlx::query("INSERT INTO schema_migrations (name, millis) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING")
                .bind(name)
                .bind(millis)
                .execute(pool)
                .await
                .with_context(|| format!("failed to record migration {name}"))?;
            tracing::info!(migration = name, ms = millis, "migration applied");
            applied += 1;
        }
        Ok::<u64, anyhow::Error>(applied)
    }
    .await;
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK)
        .execute(&mut *lock)
        .await;
    outcome
}

/// An arbitrary, fixed key for `pg_advisory_lock`: "edda migrations".
const MIGRATION_LOCK: i64 = 0x45444441_4d494752;

#[cfg(test)]
mod migration_tests {
    /// Every file in migrations/ must be wired into database_pool — a
    /// migration written but not registered ships a server that 500s on
    /// its new tables (field case 2026-09-06: 0010_knowledge.sql existed,
    /// knowledge_sweeps did not).
    #[test]
    fn every_migration_file_is_registered() {
        let source = include_str!("lib.rs");
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        for entry in std::fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            if name.ends_with(".sql") {
                assert!(
                    source.contains(&format!("../migrations/{name}")),
                    "{name} exists but database_pool never runs it"
                );
            }
        }
    }
}
