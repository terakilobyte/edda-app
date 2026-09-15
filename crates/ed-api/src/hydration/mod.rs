//! Hydration: explicit PostgreSQL import jobs with a recorded source
//! identity, timestamp, byte count and result (`service_hydrations`).
//!
//! Two sources feed the same tables: the checked-in synthetic fixture
//! ([`hydrate_fixture`]) and a Spansh dump ([`spansh::hydrate_spansh`]),
//! the latter through the `ed_store::galaxy` seam with a PostgreSQL
//! adapter. Both go through `ed_store::postgres` write paths, so the
//! strictly-newer rule is applied once, in SQL, for every source.

pub mod spansh;

use std::path::Path;

use anyhow::{Context, Result};
use ed_domain::freshness::parse_timestamp;
use ed_store::postgres::{apply_source_system, SourceSystem, SystemWrite};
use serde::Deserialize;
use sqlx::{PgPool, Postgres, Transaction};

pub use spansh::hydrate_spansh;

#[derive(Debug, Deserialize)]
pub struct Fixture {
    pub source: String,
    pub observed_at: String,
    pub systems: Vec<FixtureSystem>,
}

#[derive(Debug, Deserialize)]
pub struct FixtureSystem {
    pub address: i64,
    pub name: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub population: i64,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct HydrationResult {
    pub source: String,
    pub systems_seen: usize,
    pub systems_applied: u64,
    /// Systems left out because another address already holds their name
    /// (their stations and boards with them).
    pub systems_unfiled: u64,
    pub stations_seen: u64,
    /// Market, outfitting and shipyard boards written.
    pub snapshots_applied: u64,
    /// Boards a stored observation at least as new outranked.
    pub snapshots_skipped: u64,
    /// Station identities (pads, type, arrival, services) written from
    /// the dump (2026-09-07: the field 96 % of prod's stations lacked).
    #[serde(default)]
    pub identities_applied: u64,
    /// Identities a newer stored one (an EDDN docking) outranked.
    #[serde(default)]
    pub identities_skipped: u64,
    pub market_rows: u64,
    /// Main-star classes taught to the stars table (item 47 stage 1c) —
    /// the routing reconcile's update source.
    #[serde(default)]
    pub stars_taught: u64,
    /// Prospecting bodies written newer-wins (the mining search, B.4),
    /// and the ring hotspots rewritten with them.
    #[serde(default)]
    pub bodies_applied: u64,
    #[serde(default)]
    pub hotspots_applied: u64,
    pub parse_errors: u64,
}

pub async fn hydrate_fixture(pool: &PgPool, path: &Path) -> Result<HydrationResult> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("failed to read fixture {}", path.display()))?;
    let fixture: Fixture = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse fixture {}", path.display()))?;
    validate_fixture(&fixture)?;
    let observed_at = parse_timestamp(&fixture.observed_at).with_context(|| {
        format!(
            "fixture observed_at {:?} is not a timestamp",
            fixture.observed_at
        )
    })?;

    let mut transaction = pool.begin().await?;
    let run_id = start_job(
        &mut transaction,
        &fixture.source,
        observed_at,
        i64::try_from(bytes.len()).context("fixture is too large")?,
    )
    .await?;

    let mut systems_applied = 0;
    for system in &fixture.systems {
        systems_applied += u64::from(
            apply_source_system(
                &mut transaction,
                &SourceSystem {
                    address: system.address,
                    name: system.name.clone(),
                    position: Some([system.x, system.y, system.z]),
                    population: Some(system.population),
                    security: None,
                    allegiance: None,
                    controlling_power: None,
                    power_state: None,
                    powers: None,
                    observed_at: Some(observed_at),
                    provenance: fixture.source.clone(),
                },
            )
            .await?
                == SystemWrite::Written,
        );
    }

    complete_job(&mut transaction, run_id, systems_applied, None).await?;
    transaction.commit().await?;

    Ok(HydrationResult {
        source: fixture.source,
        systems_seen: fixture.systems.len(),
        systems_applied,
        systems_unfiled: 0,
        ..HydrationResult::default()
    })
}

/// Record a hydration as running. Returns its id.
async fn start_job(
    transaction: &mut Transaction<'_, Postgres>,
    source: &str,
    observed_at: i64,
    source_bytes: i64,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO service_hydrations (source, source_observed_at, source_bytes, status) \
         VALUES ($1, to_timestamp($2), $3, 'running') RETURNING id",
    )
    .bind(source)
    .bind(observed_at)
    .bind(source_bytes)
    .fetch_one(&mut **transaction)
    .await?)
}

async fn complete_job(
    transaction: &mut Transaction<'_, Postgres>,
    run_id: i64,
    rows_applied: u64,
    observed_at: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "UPDATE service_hydrations \
         SET status = 'complete', completed_at = now(), rows_applied = $1, \
             source_observed_at = COALESCE(to_timestamp($3), source_observed_at) \
         WHERE id = $2",
    )
    .bind(i64::try_from(rows_applied).context("applied row count is too large")?)
    .bind(run_id)
    .bind(observed_at)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn fail_job(pool: &PgPool, run_id: i64, rows_applied: u64) {
    let _ = sqlx::query(
        "UPDATE service_hydrations SET status = 'failed', completed_at = now(), rows_applied = $1 \
         WHERE id = $2",
    )
    .bind(i64::try_from(rows_applied).unwrap_or(i64::MAX))
    .bind(run_id)
    .execute(pool)
    .await;
}

fn validate_fixture(fixture: &Fixture) -> Result<()> {
    anyhow::ensure!(!fixture.source.trim().is_empty(), "fixture source is empty");
    anyhow::ensure!(
        !fixture.observed_at.trim().is_empty(),
        "fixture observed_at is empty"
    );
    for system in &fixture.systems {
        anyhow::ensure!(system.address > 0, "system address must be positive");
        anyhow::ensure!(!system.name.trim().is_empty(), "system name is empty");
        anyhow::ensure!(
            system.x.is_finite() && system.y.is_finite() && system.z.is_finite(),
            "system coordinates must be finite"
        );
        anyhow::ensure!(system.population >= 0, "system population is negative");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_parses_and_validates() {
        let fixture: Fixture =
            serde_json::from_str(include_str!("../../fixtures/synthetic-galaxy.json")).unwrap();

        validate_fixture(&fixture).unwrap();
        assert_eq!(fixture.systems.len(), 3);
    }
}
