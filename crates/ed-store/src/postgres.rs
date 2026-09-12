use std::time::Duration;

use anyhow::Result;
use ed_domain::{ApplyStats, Commodity, Operation, Snapshot, SystemObservation};
use sqlx::{PgPool, Postgres, Transaction};
use tokio::sync::mpsc;

const MAX_BATCH: usize = 500;
const MAX_BATCH_WAIT: Duration = Duration::from_millis(250);

pub type Applied = ApplyStats;

pub async fn apply_operations(pool: &PgPool, operations: &[Operation]) -> Result<Applied> {
    let mut transaction = pool.begin().await?;
    let applied = apply_in_transaction(&mut transaction, operations).await?;
    sqlx::query(
        "UPDATE eddn_ingestion SET received = received + $1, applied = applied + $2, \
         skipped = skipped + $3, last_message_at = now(), updated_at = now() WHERE singleton",
    )
    .bind(i64::try_from(operations.len())?)
    .bind(i64::try_from(applied.messages)?)
    .bind(i64::try_from(applied.skipped)?)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(applied)
}

/// Apply operations inside a transaction the caller owns, without touching
/// the EDDN ingestion counters. The live feed and bulk hydration share
/// this path, so a Spansh board obeys the same strictly-newer rule as an
/// EDDN one.
pub async fn apply_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    operations: &[Operation],
) -> Result<Applied> {
    let mut applied = Applied::default();
    for operation in operations {
        applied += apply_one(transaction, operation).await?;
    }
    Ok(applied)
}

/// A system as a bulk source (a Spansh dump, a synthetic fixture) describes
/// it. `observed_at` is the source's own timestamp for the row, compared
/// against both the stored source watermark and the stored EDDN watermark:
/// a re-hydration must never regress a fresher EDDN observation.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSystem {
    pub address: i64,
    pub name: String,
    pub position: Option<[f64; 3]>,
    pub population: Option<i64>,
    pub security: Option<String>,
    pub allegiance: Option<String>,
    pub controlling_power: Option<String>,
    pub power_state: Option<String>,
    /// Comma-separated, as the EDDN adapter stores it.
    pub powers: Option<String>,
    pub observed_at: Option<i64>,
    pub provenance: String,
}

/// What [`apply_source_system`] did with a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemWrite {
    /// The row was inserted or updated.
    Written,
    /// A stored observation at least as new stood; the row exists.
    Unchanged,
    /// Another address already holds this name (the galaxy has duplicate
    /// system names; `systems.name` is unique case-insensitively), so the
    /// row was not filed and nothing may reference its address.
    NameTaken,
}

/// Upsert a source system (ed_domain::freshness::accept in SQL: strictly
/// newer wins, equal is a no-op, nothing stored accepts).
pub async fn apply_source_system(
    transaction: &mut Transaction<'_, Postgres>,
    system: &SourceSystem,
) -> Result<SystemWrite> {
    let position = system.position;
    // The EDDN adapter files a system it only knows by name under a
    // provisional negative address. The source knows the real one: move
    // the row (stations follow through ON UPDATE CASCADE) so the name
    // index does not reject the real address as a duplicate.
    sqlx::query(
        "UPDATE systems SET address = $1 \
         WHERE lower(name) = lower($2) AND address < 0 AND address <> $1 \
           AND NOT EXISTS (SELECT 1 FROM systems WHERE address = $1)",
    )
    .bind(system.address)
    .bind(&system.name)
    .execute(&mut **transaction)
    .await?;
    let name_taken: Option<i64> = sqlx::query_scalar(
        "SELECT address FROM systems WHERE lower(name) = lower($1) AND address <> $2",
    )
    .bind(&system.name)
    .bind(system.address)
    .fetch_optional(&mut **transaction)
    .await?;
    if let Some(other) = name_taken {
        // An expected EDDN/journal data condition (Elite carries duplicate
        // and renamed system names across different addresses): we skip the
        // write and move on. Debug, not warn — it is routine, and as a warn
        // it dominated the honest warn/error signal we ship as telemetry.
        tracing::debug!(
            name = %system.name,
            address = system.address,
            other,
            "system name is already held by another address; skipping"
        );
        return Ok(SystemWrite::NameTaken);
    }
    let written = sqlx::query(
        "INSERT INTO systems \
         (address, name, x, y, z, population, provenance, security, allegiance, \
          controlling_power, power_state, powers, source_observed_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,to_timestamp($13)) \
         ON CONFLICT (address) DO UPDATE SET \
           name = EXCLUDED.name, x = COALESCE(EXCLUDED.x, systems.x), \
           y = COALESCE(EXCLUDED.y, systems.y), z = COALESCE(EXCLUDED.z, systems.z), \
           population = COALESCE(EXCLUDED.population, systems.population), \
           security = COALESCE(EXCLUDED.security, systems.security), \
           allegiance = COALESCE(EXCLUDED.allegiance, systems.allegiance), \
           controlling_power = COALESCE(EXCLUDED.controlling_power, systems.controlling_power), \
           power_state = COALESCE(EXCLUDED.power_state, systems.power_state), \
           powers = COALESCE(EXCLUDED.powers, systems.powers), \
           provenance = EXCLUDED.provenance, \
           source_observed_at = EXCLUDED.source_observed_at \
         WHERE (systems.source_observed_at IS NULL \
                OR systems.source_observed_at < EXCLUDED.source_observed_at) \
           AND (systems.eddn_observed_at IS NULL \
                OR systems.eddn_observed_at < EXCLUDED.source_observed_at)",
    )
    .bind(system.address)
    .bind(&system.name)
    .bind(position.map(|value| value[0]))
    .bind(position.map(|value| value[1]))
    .bind(position.map(|value| value[2]))
    .bind(system.population)
    .bind(&system.provenance)
    .bind(&system.security)
    .bind(&system.allegiance)
    .bind(&system.controlling_power)
    .bind(&system.power_state)
    .bind(&system.powers)
    .bind(system.observed_at)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    Ok(if written > 0 {
        SystemWrite::Written
    } else {
        SystemWrite::Unchanged
    })
}

/// Make sure a station row exists with this id and name; the boards (and
/// their watermarks) are applied separately through operations. Returns
/// true when the row was inserted.
pub async fn ensure_station(
    transaction: &mut Transaction<'_, Postgres>,
    station_id: i64,
    system_address: i64,
    name: Option<&str>,
) -> Result<bool> {
    Ok(
        station_id_for(transaction, system_address, Some(station_id), name)
            .await?
            .is_some_and(|(_, inserted)| inserted),
    )
}

pub async fn run_writer(pool: PgPool, mut receiver: mpsc::Receiver<Operation>) {
    while let Some(first) = receiver.recv().await {
        let mut batch = Vec::with_capacity(MAX_BATCH);
        batch.push(first);
        let deadline = tokio::time::Instant::now() + MAX_BATCH_WAIT;
        while batch.len() < MAX_BATCH {
            match tokio::time::timeout_at(deadline, receiver.recv()).await {
                Ok(Some(envelope)) => batch.push(envelope),
                Ok(None) | Err(_) => break,
            }
        }

        let mut backoff = Duration::from_secs(1);
        let mut attempts = 0u32;
        loop {
            let started = std::time::Instant::now();
            match apply_operations(&pool, &batch).await {
                Ok(stats) => {
                    tracing::debug!(?stats, batch = batch.len(), "applied EDDN batch");
                    metrics::histogram!("edda_eddn_batch_apply_seconds")
                        .record(started.elapsed().as_secs_f64());
                    metrics::counter!("edda_eddn_batches_total", "outcome" => "applied")
                        .increment(1);
                    metrics::counter!("edda_eddn_operations_total", "outcome" => "applied")
                        .increment(batch.len() as u64);
                    // Freshness signal: a Grafana alert on this going
                    // stale is the 23-silent-hours guard.
                    if let Ok(now) =
                        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    {
                        metrics::gauge!("edda_eddn_last_apply_unix_seconds").set(now.as_secs_f64());
                    }
                    record_apply_stats(&stats);
                    break;
                }
                Err(error) => {
                    attempts += 1;
                    // A transient fault (DB restart, lock) clears within
                    // a few retries; one that survives five is a poison
                    // batch, and retrying it forever wedges the whole
                    // feed behind the bounded channel (measured: 23
                    // silent hours, 2026-09-01). Drop it, count it
                    // where /readyz-adjacent checks can see it, and let
                    // the feed live -- boards are replace-per-station
                    // and the next observation heals the loss.
                    if attempts >= 5 {
                        tracing::error!(%error, batch = batch.len(), "EDDN batch dropped after {attempts} failed attempts");
                        metrics::counter!("edda_eddn_batches_total", "outcome" => "dropped")
                            .increment(1);
                        metrics::counter!("edda_eddn_operations_total", "outcome" => "dropped")
                            .increment(batch.len() as u64);
                        let _ = sqlx::query(
                            "UPDATE eddn_ingestion SET errors = errors + $1, updated_at = now() WHERE singleton",
                        )
                        .bind(batch.len() as i64)
                        .execute(&pool)
                        .await;
                        break;
                    }
                    // A transient fault that self-heals on retry is a WARN,
                    // not an error — the ERROR is the terminal drop above,
                    // after five attempts. Levelling them the same made a
                    // recovered blip look like a real failure in telemetry.
                    tracing::warn!(%error, batch = batch.len(), ?backoff, "EDDN batch failed; retrying");
                    metrics::counter!("edda_eddn_batches_total", "outcome" => "retried")
                        .increment(1);
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            }
        }
    }
}

/// Metrics mirror of the `applied EDDN batch` debug event: the same
/// [`ApplyStats`] fields, as counters a Prometheus scrape can graph.
/// Free (a few atomic adds) when no recorder is installed, like
/// tracing with no subscriber.
fn record_apply_stats(stats: &ApplyStats) {
    metrics::counter!("edda_eddn_messages_applied_total").increment(stats.messages);
    metrics::counter!("edda_eddn_messages_skipped_total").increment(stats.skipped);
    metrics::counter!("edda_eddn_rows_total", "kind" => "system").increment(stats.systems);
    metrics::counter!("edda_eddn_rows_total", "kind" => "station").increment(stats.stations);
    metrics::counter!("edda_eddn_rows_total", "kind" => "market").increment(stats.market_rows);
    metrics::counter!("edda_eddn_rows_total", "kind" => "market_removed")
        .increment(stats.market_rows_removed);
    metrics::counter!("edda_eddn_rows_total", "kind" => "outfitting")
        .increment(stats.outfitting_rows);
    metrics::counter!("edda_eddn_rows_total", "kind" => "shipyard").increment(stats.shipyard_rows);
    metrics::counter!("edda_eddn_rows_total", "kind" => "star").increment(stats.stars);
    metrics::counter!("edda_eddn_rows_total", "kind" => "body").increment(stats.bodies);
    metrics::counter!("edda_eddn_rows_total", "kind" => "hotspot").increment(stats.hotspots);
    metrics::counter!("edda_eddn_rows_total", "kind" => "body_signals")
        .increment(stats.body_signals);
}

async fn apply_one(
    transaction: &mut Transaction<'_, Postgres>,
    operation: &Operation,
) -> Result<Applied> {
    match operation {
        Operation::Market(snapshot) => apply_market(transaction, snapshot).await,
        Operation::Outfitting(snapshot) => apply_outfitting(transaction, snapshot).await,
        Operation::Shipyard(snapshot) => apply_shipyard(transaction, snapshot).await,
        Operation::System(observation) => apply_journal(transaction, observation).await,
        Operation::StationIdentity(identity) => apply_station_identity(transaction, identity).await,
        Operation::Star(star) => apply_star(transaction, star).await,
        Operation::Body(body) => {
            let written = apply_bodies(
                transaction,
                std::slice::from_ref(body),
                &std::collections::HashSet::new(),
            )
            .await?;
            Ok(if written.bodies == 0 {
                skipped()
            } else {
                Applied {
                    messages: 1,
                    bodies: written.bodies,
                    hotspots: written.hotspots,
                    ..Applied::default()
                }
            })
        }
        Operation::RingHotspots(hotspots) => apply_ring_hotspots(transaction, hotspots).await,
        Operation::BodySignals(signals) => apply_body_signals(transaction, signals).await,
    }
}

/// A `Docked` event: pads, carrier flag (authoritative — `StationType`,
/// not the name heuristic), arrival distance, type and services, gated
/// on its own freshness watermark so replays never regress identity.
async fn apply_station_identity(
    transaction: &mut Transaction<'_, Postgres>,
    identity: &ed_domain::StationIdentity,
) -> Result<Applied> {
    let system_address = system_address_for(transaction, &identity.system_name).await?;
    let Some((station_id, station_inserted)) = station_id_for(
        transaction,
        system_address,
        Some(identity.market_id),
        Some(&identity.station_name),
    )
    .await?
    else {
        return Ok(skipped());
    };
    // Newer than what the row holds, OR the row never learned a column
    // this write can fill. Without the second clause a column added
    // later stays null forever on every station whose identity is
    // already current: the freshness guard would skip the very write
    // that would fill it (2026-09-12, the economy columns).
    let fresh = sqlx::query_scalar::<_, bool>(
        "SELECT identity_observed_at IS NULL OR identity_observed_at < to_timestamp($2) \
                OR ($3 AND primary_economy IS NULL) \
                OR ($4 AND government IS NULL) \
                OR ($5 AND controlling_faction IS NULL) \
         FROM stations WHERE id = $1",
    )
    .bind(station_id)
    .bind(identity.observed_at.epoch_seconds)
    .bind(identity.primary_economy.is_some())
    .bind(identity.government.is_some())
    .bind(identity.controlling_faction.is_some())
    .fetch_one(&mut **transaction)
    .await?;
    if !fresh {
        return Ok(skipped());
    }
    sqlx::query(
        "UPDATE stations SET \
             pad_small = COALESCE($2, pad_small), \
             pad_medium = COALESCE($3, pad_medium), \
             pad_large = COALESCE($4, pad_large), \
             is_carrier = $5, \
             arrival_ls = COALESCE($6, arrival_ls), \
             station_type = COALESCE($7, station_type), \
             primary_economy = COALESCE($9, primary_economy), \
             government = COALESCE($10, government), \
             controlling_faction = COALESCE($11, controlling_faction), \
             identity_observed_at = GREATEST(identity_observed_at, to_timestamp($8)) \
         WHERE id = $1",
    )
    .bind(station_id)
    .bind(identity.pad_small)
    .bind(identity.pad_medium)
    .bind(identity.pad_large)
    .bind(identity.is_carrier())
    .bind(identity.arrival_ls)
    .bind(identity.station_type.as_deref())
    .bind(identity.observed_at.epoch_seconds)
    .bind(identity.primary_economy.as_deref())
    .bind(identity.government.as_deref())
    .bind(identity.controlling_faction.as_deref())
    .execute(&mut **transaction)
    .await?;
    if !identity.services.is_empty() {
        sqlx::query("DELETE FROM station_services WHERE station_id = $1")
            .bind(station_id)
            .execute(&mut **transaction)
            .await?;
        let services: Vec<String> = identity
            .services
            .iter()
            .map(|s| s.trim().to_lowercase())
            .collect();
        sqlx::query(
            "INSERT INTO station_services (station_id, service) \
             SELECT $1, unnest($2::text[]) ON CONFLICT DO NOTHING",
        )
        .bind(station_id)
        .bind(&services)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(Applied {
        messages: 1,
        stations: u64::from(station_inserted),
        ..Applied::default()
    })
}

async fn system_address_for(
    transaction: &mut Transaction<'_, Postgres>,
    name: &str,
) -> Result<i64> {
    let address = sqlx::query_scalar(
        "INSERT INTO systems (address, name, provenance) \
         VALUES (nextval('provisional_system_address_seq'), $1, 'eddn') \
         ON CONFLICT ((lower(name))) DO UPDATE SET name = EXCLUDED.name RETURNING address",
    )
    .bind(name)
    .fetch_one(&mut **transaction)
    .await?;
    Ok(address)
}

async fn station_id_for(
    transaction: &mut Transaction<'_, Postgres>,
    system_address: i64,
    market_id: Option<i64>,
    station_name: Option<&str>,
) -> Result<Option<(i64, bool)>> {
    if let Some(station_id) = market_id {
        let inserted = sqlx::query_scalar::<_, bool>(
            "INSERT INTO stations (id, system_address, name) VALUES ($1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET system_address = EXCLUDED.system_address, \
             name = COALESCE(EXCLUDED.name, stations.name) RETURNING xmax = 0",
        )
        .bind(station_id)
        .bind(system_address)
        .bind(station_name)
        .fetch_one(&mut **transaction)
        .await?;
        return Ok(Some((station_id, inserted)));
    }
    let Some(name) = station_name else {
        return Ok(None);
    };
    let station_id = sqlx::query_scalar(
        "SELECT id FROM stations WHERE system_address = $1 AND lower(name) = lower($2)",
    )
    .bind(system_address)
    .bind(name)
    .fetch_optional(&mut **transaction)
    .await?;
    Ok(station_id.map(|id| (id, false)))
}

async fn apply_market(
    transaction: &mut Transaction<'_, Postgres>,
    message: &Snapshot<Commodity>,
) -> Result<Applied> {
    let system_address = system_address_for(transaction, &message.system_name).await?;
    let Some((station_id, station_inserted)) = station_id_for(
        transaction,
        system_address,
        message.market_id,
        message.station_name.as_deref(),
    )
    .await?
    else {
        return Ok(skipped());
    };
    // ed_domain::freshness::accept in SQL: the station-level watermark
    // (`market_observed_at`, not MAX over rows) must be strictly older.
    let fresh = sqlx::query_scalar::<_, bool>(
        "SELECT market_observed_at IS NULL OR market_observed_at < to_timestamp($2) \
         FROM stations WHERE id = $1",
    )
    .bind(station_id)
    .bind(message.observed_at.epoch_seconds)
    .fetch_one(&mut **transaction)
    .await?;
    if !fresh {
        return Ok(skipped());
    }

    let removed = sqlx::query("DELETE FROM market WHERE station_id = $1")
        .bind(station_id)
        .execute(&mut **transaction)
        .await?
        .rows_affected();
    // One round trip per board, not per row: a Spansh hydration writes
    // millions of rows and the live feed benefits just the same.
    // A board that lists a symbol twice keeps the last entry, as a
    // keyed SQLite upsert would; the rows were deleted above so a plain
    // insert of the deduplicated board cannot conflict.
    let board: std::collections::BTreeMap<String, &Commodity> = message
        .values
        .iter()
        // Canonicalized: `$magnesite_name;` from journal-format messages
        // must land on the same row as `magnesite` (fragmentation census
        // 2026-09-05: 371 goods split, one stranded row each).
        .map(|commodity| (crate::market::canonical_symbol(&commodity.name), commodity))
        .collect();
    if !board.is_empty() {
        let symbols: Vec<&str> = board.keys().map(String::as_str).collect();
        sqlx::query(
            "INSERT INTO commodities (symbol) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING",
        )
        .bind(&symbols)
        .execute(&mut **transaction)
        .await?;
        let buy: Vec<i64> = board.values().map(|c| c.buy_price).collect();
        let sell: Vec<i64> = board.values().map(|c| c.sell_price).collect();
        let demand: Vec<i64> = board.values().map(|c| c.demand).collect();
        let supply: Vec<i64> = board.values().map(|c| c.stock).collect();
        sqlx::query(
            "INSERT INTO market (station_id, commodity_symbol, buy_price, sell_price, demand, supply, observed_at) \
             SELECT $1, symbol, buy, sell, demand, supply, to_timestamp($7) \
             FROM unnest($2::text[], $3::bigint[], $4::bigint[], $5::bigint[], $6::bigint[]) \
               AS rows(symbol, buy, sell, demand, supply)",
        )
        .bind(station_id)
        .bind(&symbols)
        .bind(&buy)
        .bind(&sell)
        .bind(&demand)
        .bind(&supply)
        .bind(message.observed_at.epoch_seconds)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        "UPDATE stations SET has_market = true, market_observed_at = to_timestamp($2) WHERE id = $1",
    )
    .bind(station_id)
    .bind(message.observed_at.epoch_seconds)
    .execute(&mut **transaction)
    .await?;
    // Confiscated goods travel with the board and share its freshness
    // gate: a fresh snapshot replaces the prohibition list wholesale
    // (an empty list legitimately clears it — jurisdiction changed).
    sqlx::query("DELETE FROM station_prohibited WHERE station_id = $1")
        .bind(station_id)
        .execute(&mut **transaction)
        .await?;
    if !message.prohibited.is_empty() {
        sqlx::query(
            "INSERT INTO station_prohibited (station_id, symbol) \
             SELECT $1, unnest($2::text[]) ON CONFLICT DO NOTHING",
        )
        .bind(station_id)
        .bind(&message.prohibited)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(Applied {
        messages: 1,
        stations: u64::from(station_inserted),
        market_rows: message.values.len() as u64,
        market_rows_removed: removed,
        ..Applied::default()
    })
}

async fn apply_outfitting(
    transaction: &mut Transaction<'_, Postgres>,
    message: &Snapshot<String>,
) -> Result<Applied> {
    let system_address = system_address_for(transaction, &message.system_name).await?;
    let Some((station_id, station_inserted)) = station_id_for(
        transaction,
        system_address,
        message.market_id,
        message.station_name.as_deref(),
    )
    .await?
    else {
        return Ok(skipped());
    };
    if !snapshot_is_fresh(
        transaction,
        station_id,
        "outfitting",
        message.observed_at.epoch_seconds,
    )
    .await?
    {
        return Ok(skipped());
    }
    sqlx::query("DELETE FROM outfitting WHERE station_id = $1")
        .bind(station_id)
        .execute(&mut **transaction)
        .await?;
    replace_availability(
        transaction,
        "modules",
        "outfitting",
        "module_symbol",
        station_id,
        &message.values,
    )
    .await?;
    update_station_snapshot(
        transaction,
        station_id,
        "outfitting",
        message.observed_at.epoch_seconds,
    )
    .await?;
    Ok(Applied {
        messages: 1,
        stations: u64::from(station_inserted),
        outfitting_rows: message.values.len() as u64,
        ..Applied::default()
    })
}

async fn apply_shipyard(
    transaction: &mut Transaction<'_, Postgres>,
    message: &Snapshot<String>,
) -> Result<Applied> {
    let system_address = system_address_for(transaction, &message.system_name).await?;
    let Some((station_id, station_inserted)) = station_id_for(
        transaction,
        system_address,
        message.market_id,
        message.station_name.as_deref(),
    )
    .await?
    else {
        return Ok(skipped());
    };
    if !snapshot_is_fresh(
        transaction,
        station_id,
        "shipyard",
        message.observed_at.epoch_seconds,
    )
    .await?
    {
        return Ok(skipped());
    }
    sqlx::query("DELETE FROM shipyard WHERE station_id = $1")
        .bind(station_id)
        .execute(&mut **transaction)
        .await?;
    replace_availability(
        transaction,
        "ships",
        "shipyard",
        "ship_symbol",
        station_id,
        &message.values,
    )
    .await?;
    update_station_snapshot(
        transaction,
        station_id,
        "shipyard",
        message.observed_at.epoch_seconds,
    )
    .await?;
    Ok(Applied {
        messages: 1,
        stations: u64::from(station_inserted),
        shipyard_rows: message.values.len() as u64,
        ..Applied::default()
    })
}

/// Write one station's catalog availability (its outfitting or shipyard
/// board) in two statements rather than two per row: a Spansh hydration
/// writes hundreds of thousands of boards. A board listing a symbol twice
/// is applied once.
async fn replace_availability(
    transaction: &mut Transaction<'_, Postgres>,
    catalog: &str,
    relation: &str,
    column: &str,
    station_id: i64,
    values: &[String],
) -> Result<()> {
    let symbols: Vec<&str> = values
        .iter()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    if symbols.is_empty() {
        return Ok(());
    }
    let (catalog_sql, relation_sql) = match (catalog, relation, column) {
        ("modules", "outfitting", "module_symbol") => (
            "INSERT INTO modules (symbol) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING",
            "INSERT INTO outfitting (station_id, module_symbol) SELECT $1, unnest($2::text[])",
        ),
        ("ships", "shipyard", "ship_symbol") => (
            "INSERT INTO ships (symbol) SELECT unnest($1::text[]) ON CONFLICT DO NOTHING",
            "INSERT INTO shipyard (station_id, ship_symbol) SELECT $1, unnest($2::text[])",
        ),
        _ => unreachable!(),
    };
    sqlx::query(catalog_sql)
        .bind(&symbols)
        .execute(&mut **transaction)
        .await?;
    sqlx::query(relation_sql)
        .bind(station_id)
        .bind(&symbols)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

/// ed_domain::freshness::accept in SQL, against the product's watermark.
async fn snapshot_is_fresh(
    transaction: &mut Transaction<'_, Postgres>,
    station_id: i64,
    product: &str,
    timestamp: i64,
) -> Result<bool> {
    let query = match product {
        "outfitting" => {
            "SELECT outfitting_observed_at IS NULL OR outfitting_observed_at < to_timestamp($2) FROM stations WHERE id = $1"
        }
        "shipyard" => {
            "SELECT shipyard_observed_at IS NULL OR shipyard_observed_at < to_timestamp($2) FROM stations WHERE id = $1"
        }
        _ => unreachable!(),
    };
    Ok(sqlx::query_scalar(query)
        .bind(station_id)
        .bind(timestamp)
        .fetch_one(&mut **transaction)
        .await?)
}

async fn update_station_snapshot(
    transaction: &mut Transaction<'_, Postgres>,
    station_id: i64,
    product: &str,
    timestamp: i64,
) -> Result<()> {
    let query = match product {
        "outfitting" => {
            "UPDATE stations SET has_outfitting = true, outfitting_observed_at = to_timestamp($2) WHERE id = $1"
        }
        "shipyard" => {
            "UPDATE stations SET has_shipyard = true, shipyard_observed_at = to_timestamp($2) WHERE id = $1"
        }
        _ => unreachable!(),
    };
    sqlx::query(query)
        .bind(station_id)
        .bind(timestamp)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn apply_journal(
    transaction: &mut Transaction<'_, Postgres>,
    message: &SystemObservation,
) -> Result<Applied> {
    let name = &message.system_name;
    let address = match message.system_address {
        Some(address) => {
            // The boards-first race (live poison, 2026-09-01): a market
            // message names a system the store has never seen, so it
            // holds the name under a provisional (negative) address;
            // when the journal later brings the REAL address, inserting
            // it would trip systems_name_ci_idx -- and one such message
            // in a batch poisoned the whole feed. The provisional row
            // IS this system: write the journal's data onto it and let
            // a reconcile job migrate the address (stations reference
            // it, and the FK does not cascade). A POSITIVE holder is a
            // genuine name conflict: skip the write, keep the batch.
            let held: Option<i64> = sqlx::query_scalar(
                "SELECT address FROM systems WHERE lower(name) = lower($1) AND address <> $2",
            )
            .bind(name)
            .bind(address)
            .fetch_optional(&mut **transaction)
            .await?;
            match held {
                Some(provisional) if provisional < 0 => provisional,
                Some(other) => {
                    // Same routine data condition on the journal path —
                    // debug, not warn (see the EDDN write above).
                    tracing::debug!(
                        %name,
                        address,
                        other,
                        "system name is already held by another address; skipping the journal write"
                    );
                    return Ok(Applied::default());
                }
                None => address,
            }
        }
        None => system_address_for(transaction, name).await?,
    };
    let position = message.position;
    let powers = message.powers.as_ref().map(|powers| powers.join(", "));
    let changed = sqlx::query(
        "INSERT INTO systems \
         (address, name, x, y, z, population, provenance, security, allegiance, controlling_power, power_state, powers, eddn_observed_at) \
         VALUES ($1,$2,$3,$4,$5,$6,'eddn',$7,$8,$9,$10,$11,to_timestamp($12)) \
         ON CONFLICT (address) DO UPDATE SET \
           name = EXCLUDED.name, x = COALESCE(EXCLUDED.x, systems.x), \
           y = COALESCE(EXCLUDED.y, systems.y), z = COALESCE(EXCLUDED.z, systems.z), \
           population = COALESCE(EXCLUDED.population, systems.population), \
           security = COALESCE(EXCLUDED.security, systems.security), \
           allegiance = COALESCE(EXCLUDED.allegiance, systems.allegiance), \
           controlling_power = COALESCE(EXCLUDED.controlling_power, systems.controlling_power), \
           power_state = COALESCE(EXCLUDED.power_state, systems.power_state), \
           powers = COALESCE(EXCLUDED.powers, systems.powers), eddn_observed_at = EXCLUDED.eddn_observed_at \
         WHERE systems.eddn_observed_at IS NULL OR systems.eddn_observed_at < EXCLUDED.eddn_observed_at", // ed_domain::freshness::accept
    )
    .bind(address)
    .bind(name)
    .bind(position.map(|value| value[0]))
    .bind(position.map(|value| value[1]))
    .bind(position.map(|value| value[2]))
    .bind(message.population)
    .bind(&message.security)
    .bind(&message.allegiance)
    .bind(&message.controlling_power)
    .bind(&message.powerplay_state)
    .bind(powers)
    .bind(message.observed_at.epoch_seconds)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if changed == 0 {
        Ok(skipped())
    } else {
        Ok(Applied {
            messages: 1,
            systems: 1,
            ..Applied::default()
        })
    }
}

fn skipped() -> Applied {
    Applied {
        skipped: 1,
        ..Applied::default()
    }
}

// ------------------------------------------------------------ 2026-09-09
// The feed's new arms: star classes, bodies, ring hotspots, body signals
// (maintainer: whatever the dump adds that EDDN carried, the feed parses).
// Same tables the dump hydration fills (`bodies`, `body_materials`,
// `rings`, `ring_hotspots`, `stars`); newer-wins throughout.

/// What a bodies write did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BodiesWritten {
    pub bodies: u64,
    pub hotspots: u64,
}

/// The arrival star's class into `stars`, newer-wins. An unknown class
/// spelling teaches nothing (the index has no code for it).
async fn apply_star(
    transaction: &mut Transaction<'_, Postgres>,
    star: &ed_domain::StarTeaching,
) -> Result<Applied> {
    use ed_domain::star::{StarClass, StarClassCode as _};
    if star.system_address <= 0 {
        return Ok(skipped());
    }
    let class = StarClass::from_journal(&star.star_type);
    if class == StarClass::Unknown {
        return Ok(skipped());
    }
    let changed = sqlx::query(
        "INSERT INTO stars (address, class, scoopable, subtype, source, observed_at) \
         VALUES ($1, $2, $3, $4, $5, to_timestamp($6)) \
         ON CONFLICT (address) DO UPDATE SET \
           class = EXCLUDED.class, scoopable = EXCLUDED.scoopable, subtype = EXCLUDED.subtype, \
           source = EXCLUDED.source, observed_at = EXCLUDED.observed_at \
         WHERE stars.observed_at < EXCLUDED.observed_at",
    )
    .bind(star.system_address)
    .bind(i16::from(class.code()))
    .bind(class.scoopable())
    .bind(star.star_type.trim())
    .bind(&star.source)
    .bind(star.observed_at.epoch_seconds)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    Ok(if changed == 0 {
        skipped()
    } else {
        Applied {
            messages: 1,
            stars: 1,
            ..Applied::default()
        }
    })
}

/// Newer-wins per body; the children of every body that was written are
/// rewritten with it (an older observation can neither regress a body
/// nor resurrect a hotspot it no longer has). Bodies whose system could
/// not be filed (`unfiled`, the dump's namesake case) are left out.
/// Shared by the feed (one body per operation) and the dump hydration
/// (a batch per transaction).
pub async fn apply_bodies(
    transaction: &mut Transaction<'_, Postgres>,
    rows: &[ed_domain::BodyTeaching],
    unfiled: &std::collections::HashSet<i64>,
) -> Result<BodiesWritten> {
    let rows: Vec<&ed_domain::BodyTeaching> = rows
        .iter()
        .filter(|r| !unfiled.contains(&r.system_address))
        .collect();
    if rows.is_empty() {
        return Ok(BodiesWritten::default());
    }
    let written: Vec<(i64,)> = sqlx::query_as(
        "INSERT INTO bodies (id64, system_address, body_id, name, type, sub_type, is_landable, \
                             distance_to_arrival, gravity, atmosphere, volcanism, bio_signals, \
                             geo_signals, observed_at, provenance) \
         SELECT u.id64, u.system_address, u.body_id, u.name, u.type, u.sub_type, u.is_landable, \
                u.distance_to_arrival, u.gravity, u.atmosphere, u.volcanism, u.bio_signals, \
                u.geo_signals, to_timestamp(u.observed_epoch), u.provenance \
         FROM UNNEST($1::bigint[], $2::bigint[], $3::int[], $4::text[], $5::text[], $6::text[], \
                     $7::bool[], $8::float8[], $9::float8[], $10::text[], $11::text[], \
                     $12::int[], $13::int[], $14::bigint[], $15::text[]) \
              AS u(id64, system_address, body_id, name, type, sub_type, is_landable, \
                   distance_to_arrival, gravity, atmosphere, volcanism, bio_signals, \
                   geo_signals, observed_epoch, provenance) \
         ON CONFLICT (id64) DO UPDATE SET \
             system_address = EXCLUDED.system_address, body_id = EXCLUDED.body_id, \
             name = EXCLUDED.name, type = EXCLUDED.type, sub_type = EXCLUDED.sub_type, \
             is_landable = EXCLUDED.is_landable, distance_to_arrival = EXCLUDED.distance_to_arrival, \
             gravity = EXCLUDED.gravity, atmosphere = EXCLUDED.atmosphere, volcanism = EXCLUDED.volcanism, \
             bio_signals = COALESCE(EXCLUDED.bio_signals, bodies.bio_signals), \
             geo_signals = COALESCE(EXCLUDED.geo_signals, bodies.geo_signals), \
             observed_at = EXCLUDED.observed_at, provenance = EXCLUDED.provenance \
         WHERE bodies.observed_at <= EXCLUDED.observed_at \
         RETURNING id64",
    )
    .bind(rows.iter().map(|r| r.id64).collect::<Vec<i64>>())
    .bind(rows.iter().map(|r| r.system_address).collect::<Vec<i64>>())
    .bind(rows.iter().map(|r| r.body_id).collect::<Vec<Option<i32>>>())
    .bind(rows.iter().map(|r| r.name.clone()).collect::<Vec<Option<String>>>())
    .bind(rows.iter().map(|r| r.kind.clone()).collect::<Vec<Option<String>>>())
    .bind(rows.iter().map(|r| r.sub_type.clone()).collect::<Vec<Option<String>>>())
    .bind(rows.iter().map(|r| r.is_landable).collect::<Vec<bool>>())
    .bind(rows.iter().map(|r| r.distance_to_arrival).collect::<Vec<Option<f64>>>())
    .bind(rows.iter().map(|r| r.gravity).collect::<Vec<Option<f64>>>())
    .bind(rows.iter().map(|r| r.atmosphere.clone()).collect::<Vec<Option<String>>>())
    .bind(rows.iter().map(|r| r.volcanism.clone()).collect::<Vec<Option<String>>>())
    .bind(rows.iter().map(|r| r.bio_signals).collect::<Vec<Option<i32>>>())
    .bind(rows.iter().map(|r| r.geo_signals).collect::<Vec<Option<i32>>>())
    .bind(rows.iter().map(|r| r.observed_at.epoch_seconds).collect::<Vec<i64>>())
    .bind(rows.iter().map(|r| r.provenance.clone()).collect::<Vec<String>>())
    .fetch_all(&mut **transaction)
    .await?;
    let written: std::collections::HashSet<i64> = written.into_iter().map(|(id,)| id).collect();
    if written.is_empty() {
        return Ok(BodiesWritten::default());
    }
    let ids: Vec<i64> = written.iter().copied().collect();
    // A feed Scan carries no hotspots; leave a ring's hotspots to the
    // SAASignalsFound that follows (or the dump), and only rewrite them
    // when the row itself carries some.
    for table in ["body_materials", "rings"] {
        sqlx::query(&format!("DELETE FROM {table} WHERE body_id64 = ANY($1)"))
            .bind(&ids)
            .execute(&mut **transaction)
            .await?;
    }
    let fresh: Vec<&&ed_domain::BodyTeaching> =
        rows.iter().filter(|r| written.contains(&r.id64)).collect();
    let with_hotspots: Vec<i64> = fresh
        .iter()
        .filter(|r| !r.hotspots.is_empty())
        .map(|r| r.id64)
        .collect();
    if !with_hotspots.is_empty() {
        sqlx::query("DELETE FROM ring_hotspots WHERE body_id64 = ANY($1)")
            .bind(&with_hotspots)
            .execute(&mut **transaction)
            .await?;
    }
    let (mut m_id, mut m_mat, mut m_pct) = (Vec::new(), Vec::new(), Vec::new());
    let (mut r_id, mut r_name, mut r_kind, mut r_mass, mut r_in, mut r_out) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    let (mut h_id, mut h_ring, mut h_mat, mut h_count) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for r in &fresh {
        for (material, percent) in &r.materials {
            m_id.push(r.id64);
            m_mat.push(material.clone());
            m_pct.push(*percent);
        }
        for ring in &r.rings {
            r_id.push(r.id64);
            r_name.push(ring.name.clone());
            r_kind.push(ring.kind.clone());
            r_mass.push(ring.mass);
            r_in.push(ring.inner_radius);
            r_out.push(ring.outer_radius);
        }
        for (ring, material, count) in &r.hotspots {
            h_id.push(r.id64);
            h_ring.push(ring.clone());
            h_mat.push(material.clone());
            h_count.push(*count);
        }
    }
    if !m_id.is_empty() {
        sqlx::query(
            "INSERT INTO body_materials (body_id64, material, percent) \
             SELECT * FROM UNNEST($1::bigint[], $2::text[], $3::float8[]) \
             ON CONFLICT (body_id64, material) DO UPDATE SET percent = EXCLUDED.percent",
        )
        .bind(&m_id)
        .bind(&m_mat)
        .bind(&m_pct)
        .execute(&mut **transaction)
        .await?;
    }
    if !r_id.is_empty() {
        sqlx::query(
            "INSERT INTO rings (body_id64, name, type, mass, inner_radius, outer_radius) \
             SELECT * FROM UNNEST($1::bigint[], $2::text[], $3::text[], $4::float8[], $5::float8[], $6::float8[]) \
             ON CONFLICT (body_id64, name) DO UPDATE SET type = EXCLUDED.type, mass = EXCLUDED.mass, \
                 inner_radius = EXCLUDED.inner_radius, outer_radius = EXCLUDED.outer_radius",
        )
        .bind(&r_id)
        .bind(&r_name)
        .bind(&r_kind)
        .bind(&r_mass)
        .bind(&r_in)
        .bind(&r_out)
        .execute(&mut **transaction)
        .await?;
    }
    if !h_id.is_empty() {
        sqlx::query(
            "INSERT INTO ring_hotspots (body_id64, ring_name, material, count) \
             SELECT * FROM UNNEST($1::bigint[], $2::text[], $3::text[], $4::int[]) \
             ON CONFLICT (body_id64, ring_name, material) DO UPDATE SET count = EXCLUDED.count",
        )
        .bind(&h_id)
        .bind(&h_ring)
        .bind(&h_mat)
        .bind(&h_count)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(BodiesWritten {
        bodies: written.len() as u64,
        hotspots: h_id.len() as u64,
    })
}

/// `SAASignalsFound` on a ring: the parent body is found by name in the
/// system; its hotspots for that ring are replaced. No parent row yet
/// (the Scan has not arrived, or the system is unfiled) is a skip — the
/// dump or the next scan brings it.
async fn apply_ring_hotspots(
    transaction: &mut Transaction<'_, Postgres>,
    h: &ed_domain::RingHotspots,
) -> Result<Applied> {
    let Some(parent) = ed_eddn::ring_parent_name(&h.ring_name) else {
        return Ok(skipped());
    };
    let body: Option<(i64,)> =
        sqlx::query_as("SELECT id64 FROM bodies WHERE system_address = $1 AND name = $2 LIMIT 1")
            .bind(h.system_address)
            .bind(parent)
            .fetch_optional(&mut **transaction)
            .await?;
    let Some((id64,)) = body else {
        return Ok(skipped());
    };
    sqlx::query("INSERT INTO rings (body_id64, name) VALUES ($1, $2) ON CONFLICT (body_id64, name) DO NOTHING")
        .bind(id64)
        .bind(&h.ring_name)
        .execute(&mut **transaction)
        .await?;
    sqlx::query("DELETE FROM ring_hotspots WHERE body_id64 = $1 AND ring_name = $2")
        .bind(id64)
        .bind(&h.ring_name)
        .execute(&mut **transaction)
        .await?;
    let materials: Vec<String> = h.signals.iter().map(|(m, _)| m.clone()).collect();
    let counts: Vec<i32> = h.signals.iter().map(|(_, c)| *c).collect();
    let written = sqlx::query(
        "INSERT INTO ring_hotspots (body_id64, ring_name, material, count) \
         SELECT $1, $2, m, c FROM UNNEST($3::text[], $4::int[]) AS t(m, c) \
         ON CONFLICT (body_id64, ring_name, material) DO UPDATE SET count = EXCLUDED.count",
    )
    .bind(id64)
    .bind(&h.ring_name)
    .bind(&materials)
    .bind(&counts)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    Ok(Applied {
        messages: 1,
        hotspots: written,
        ..Applied::default()
    })
}

/// Bio/geo counts onto the body; a body the store has not seen gets a
/// stub row (name, ids, signals) the Scan fills in later.
async fn apply_body_signals(
    transaction: &mut Transaction<'_, Postgres>,
    s: &ed_domain::BodySignals,
) -> Result<Applied> {
    let updated = sqlx::query(
        "UPDATE bodies SET bio_signals = COALESCE($2, bio_signals), geo_signals = COALESCE($3, geo_signals) WHERE id64 = $1",
    )
    .bind(s.id64)
    .bind(s.bio_signals)
    .bind(s.geo_signals)
    .execute(&mut **transaction)
    .await?
    .rows_affected();
    if updated == 0 {
        sqlx::query(
            "INSERT INTO bodies (id64, system_address, body_id, name, is_landable, bio_signals, geo_signals, observed_at, provenance) \
             VALUES ($1, $2, $3, $4, false, $5, $6, to_timestamp($7), 'eddn:signals') \
             ON CONFLICT (id64) DO NOTHING",
        )
        .bind(s.id64)
        .bind(s.system_address)
        .bind(s.body_id)
        .bind(&s.name)
        .bind(s.bio_signals)
        .bind(s.geo_signals)
        .bind(s.observed_at.epoch_seconds)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(Applied {
        messages: 1,
        body_signals: 1,
        ..Applied::default()
    })
}
