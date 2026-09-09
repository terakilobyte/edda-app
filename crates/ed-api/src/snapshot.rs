use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use ed_ebex::{
    build_string_table, encode_market_auxiliary, encode_station_snapshots, encode_symbol_catalog,
    string_id, AvailabilityRecord, CommodityCatalogRecord, MarketRecord, SnapshotHeader,
    SnapshotWriter, StationRecord, SystemRecord, ALL_V1_SECTIONS, AVAILABILITY_RECORD_BYTES,
    COMMODITY_RECORD_BYTES, MARKET_RECORD_BYTES,
    SECTION_COMMODITIES, SECTION_MARKETS, SECTION_MODULES, SECTION_OUTFITTING, SECTION_SHIPS,
    SECTION_SHIPYARDS, SECTION_STATIONS, SECTION_SYSTEMS, PROHIBITED_RECORD_BYTES,
    STATION_DETAILS_RECORD_BYTES, STATION_RECORD_BYTES, SYMBOL_CATALOG_RECORD_BYTES,
    SYSTEM_RECORD_BYTES,
};
use ed_sync::{ArtifactFile, Manifest, Product, ProductKey};
use serde::Serialize;
use sqlx::PgPool;

type SystemRow = (
    i64,
    String,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<i64>,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);
type StationRow = (i64, i64, Option<String>, bool, bool, bool, i64, i64, i64);

#[derive(Clone, Debug, Serialize)]
pub struct Publication {
    pub sequence: i64,
    pub artifact: PathBuf,
    pub bytes: u64,
    pub sha256: String,
    pub rows: u64,
}

/// What one publication build cost (measurement doctrine rule 2). The
/// builder is a short-lived CLI process, so these numbers survive in
/// `artifact_publications` and the serve process re-emits the latest
/// complete row per product as `edda_publish_*` gauges.
#[derive(Clone, Debug, Default)]
pub struct BuildCost {
    /// user+system CPU over the build (getrusage delta); None where the
    /// platform has no getrusage (the Windows dev build).
    pub cpu_seconds: Option<f64>,
    /// Wall seconds per phase, in build order.
    pub phases: Vec<(&'static str, f64)>,
}

impl BuildCost {
    fn phase_json(&self) -> String {
        let map: serde_json::Map<String, serde_json::Value> = self
            .phases
            .iter()
            .map(|(name, seconds)| ((*name).to_owned(), serde_json::json!(seconds)))
            .collect();
        serde_json::Value::Object(map).to_string()
    }
}

struct PhaseClock {
    cpu_at_start: Option<f64>,
    last: std::time::Instant,
    phases: Vec<(&'static str, f64)>,
}

impl PhaseClock {
    fn start() -> Self {
        Self { cpu_at_start: process_cpu_seconds(), last: std::time::Instant::now(), phases: Vec::new() }
    }
    fn mark(&mut self, name: &'static str) {
        let now = std::time::Instant::now();
        self.phases.push((name, (now - self.last).as_secs_f64()));
        self.last = now;
    }
    fn finish(self) -> BuildCost {
        let cpu_seconds = match (self.cpu_at_start, process_cpu_seconds()) {
            (Some(start), Some(end)) => Some(end - start),
            _ => None,
        };
        BuildCost { cpu_seconds, phases: self.phases }
    }
}

#[cfg(unix)]
fn process_cpu_seconds() -> Option<f64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    let usage = unsafe { usage.assume_init() };
    let seconds = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    Some(seconds(usage.ru_utime) + seconds(usage.ru_stime))
}

#[cfg(not(unix))]
fn process_cpu_seconds() -> Option<f64> {
    None
}

pub async fn publish_community(pool: &PgPool, artifact_dir: &Path) -> Result<Publication> {
    publish_snapshot(pool, artifact_dir, None).await
}

/// Item 49: the rolling-window market product. Same EBEX shape as the
/// community snapshot, but only the systems, stations and boards whose
/// observations fall inside the window — a few MB against ~500. The
/// window reaches back [`MARKET_DAILY_WINDOW_SECS`] so the 7-day client
/// staleness policy always has a day of slack.
pub const MARKET_DAILY_WINDOW_SECS: i64 = 8 * 24 * 3600;

pub async fn publish_market_daily(pool: &PgPool, artifact_dir: &Path) -> Result<Publication> {
    let cutoff = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64)
        - MARKET_DAILY_WINDOW_SECS;
    publish_snapshot(pool, artifact_dir, Some(cutoff)).await
}

async fn publish_snapshot(
    pool: &PgPool,
    artifact_dir: &Path,
    window_cutoff: Option<i64>,
) -> Result<Publication> {
    let product = if window_cutoff.is_some() { "market_daily" } else { "community" };
    let (sequence, created_at, generated_at): (i64, i64, String) = sqlx::query_as(
        "INSERT INTO artifact_publications (product, status) VALUES ($1, 'building') \
         RETURNING id, EXTRACT(EPOCH FROM created_at)::BIGINT, \
         to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')",
    )
    .bind(product)
    .fetch_one(pool)
    .await
    .context("starting snapshot publication")?;

    match build_market(pool, artifact_dir, sequence, created_at, &generated_at, window_cutoff).await
    {
        Ok((publication, cost)) => {
            sqlx::query(
                "UPDATE artifact_publications SET status = 'complete', artifact_path = $2, \
                 artifact_bytes = $3, artifact_sha256 = $4, rows_published = $5, \
                 cpu_seconds = $6, phase_seconds = $7::jsonb, completed_at = now() WHERE id = $1",
            )
            .bind(sequence)
            .bind(publication.artifact.to_string_lossy().as_ref())
            .bind(i64::try_from(publication.bytes)?)
            .bind(&publication.sha256)
            .bind(i64::try_from(publication.rows)?)
            .bind(cost.cpu_seconds)
            .bind(cost.phase_json())
            .execute(pool)
            .await?;
            tracing::info!(
                product,
                rows = publication.rows,
                bytes = publication.bytes,
                cpu_seconds = cost.cpu_seconds,
                phases = %cost.phase_json(),
                "publication built"
            );
            Ok(publication)
        }
        Err(error) => {
            let _ = sqlx::query("UPDATE artifact_publications SET status = 'failed', completed_at = now() WHERE id = $1")
                .bind(sequence)
                .execute(pool)
                .await;
            Err(error)
        }
    }
}

async fn build_market(
    pool: &PgPool,
    artifact_dir: &Path,
    sequence: i64,
    created_at: i64,
    generated_at: &str,
    window_cutoff: Option<i64>,
) -> Result<(Publication, BuildCost)> {
    // 0 disables every window guard below: `($N = 0 OR observed > …)`
    // keeps one SQL string per query for both products.
    let cutoff = window_cutoff.unwrap_or(0);
    let mut clock = PhaseClock::start();
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let (watermark, watermark_text): (i64, String) = sqlx::query_as(
        "WITH observations AS ( \
             SELECT market_observed_at AS observed_at FROM stations WHERE has_market \
             UNION ALL SELECT outfitting_observed_at FROM stations WHERE has_outfitting \
             UNION ALL SELECT shipyard_observed_at FROM stations WHERE has_shipyard \
         ) SELECT COALESCE(EXTRACT(EPOCH FROM MIN(observed_at))::BIGINT, 0), \
         COALESCE(to_char(MIN(observed_at) AT TIME ZONE 'UTC', \
         'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), '1970-01-01T00:00:00Z') FROM observations",
    )
    .fetch_one(&mut *transaction)
    .await?;
    let mut commodities: Vec<(String, String, String)> =
        sqlx::query_as("SELECT symbol, name, category FROM commodities ORDER BY symbol")
            .fetch_all(&mut *transaction)
            .await?;
    commodities.sort();
    if commodities.len() > usize::from(u16::MAX) {
        bail!("market snapshot contains too many commodities");
    }
    let dictionary = commodities
        .iter()
        .enumerate()
        .map(|(index, (symbol, _, _))| (symbol.clone(), u16::try_from(index + 1).unwrap()))
        .collect::<BTreeMap<_, _>>();
    let station_snapshots: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, EXTRACT(EPOCH FROM market_observed_at)::BIGINT
         FROM stations WHERE has_market AND market_observed_at IS NOT NULL \
         AND ($1 = 0 OR market_observed_at > to_timestamp($1)) ORDER BY id",
    )
    .bind(cutoff)
    .fetch_all(&mut *transaction)
    .await?;
    let mut modules: Vec<String> = sqlx::query_scalar("SELECT symbol FROM modules ORDER BY symbol")
        .fetch_all(&mut *transaction)
        .await?;
    modules.sort();
    let outfitting_snapshots: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, EXTRACT(EPOCH FROM outfitting_observed_at)::BIGINT FROM stations \
         WHERE has_outfitting AND outfitting_observed_at IS NOT NULL \
         AND ($1 = 0 OR outfitting_observed_at > to_timestamp($1)) ORDER BY id",
    )
    .bind(cutoff)
    .fetch_all(&mut *transaction)
    .await?;
    let mut ships: Vec<String> = sqlx::query_scalar("SELECT symbol FROM ships ORDER BY symbol")
        .fetch_all(&mut *transaction)
        .await?;
    ships.sort();
    let shipyard_snapshots: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT id, EXTRACT(EPOCH FROM shipyard_observed_at)::BIGINT FROM stations \
         WHERE has_shipyard AND shipyard_observed_at IS NOT NULL \
         AND ($1 = 0 OR shipyard_observed_at > to_timestamp($1)) ORDER BY id",
    )
    .bind(cutoff)
    .fetch_all(&mut *transaction)
    .await?;
    // Windowed: only stations something touched inside the window, and
    // only the systems those stations live in — the daily product's
    // identity sections stay proportional to the churn, not the galaxy.
    //
    // Two explicit SQL variants, NOT a `($1 = 0 OR …subquery…)` guard:
    // the OR defeats the planner's semi-join and degrades to a per-row
    // correlated filter — measured live as a 17-minute stall where the
    // plain join costs under a second (2026-09-04). Scalar guards are
    // fine; subquery guards are not.
    let systems: Vec<SystemRow> = if cutoff == 0 {
        sqlx::query_as(
            "SELECT address, name, x, y, z, population, \
             COALESCE(EXTRACT(EPOCH FROM GREATEST(source_observed_at, eddn_observed_at))::BIGINT, 0), \
             security, allegiance, controlling_power, power_state, powers \
             FROM systems ORDER BY address",
        )
        .fetch_all(&mut *transaction)
        .await?
    } else {
        sqlx::query_as(
            "SELECT address, name, x, y, z, population, \
             COALESCE(EXTRACT(EPOCH FROM GREATEST(source_observed_at, eddn_observed_at))::BIGINT, 0), \
             security, allegiance, controlling_power, power_state, powers \
             FROM systems WHERE address IN ( \
                 SELECT system_address FROM stations \
                 WHERE GREATEST(COALESCE(market_observed_at, 'epoch'), \
                                COALESCE(outfitting_observed_at, 'epoch'), \
                                COALESCE(shipyard_observed_at, 'epoch')) > to_timestamp($1)) \
             ORDER BY address",
        )
        .bind(cutoff)
        .fetch_all(&mut *transaction)
        .await?
    };
    let stations: Vec<StationRow> = sqlx::query_as(
        "SELECT id, system_address, name, has_market, has_outfitting, has_shipyard, \
             COALESCE(EXTRACT(EPOCH FROM market_observed_at)::BIGINT, 0), \
             COALESCE(EXTRACT(EPOCH FROM outfitting_observed_at)::BIGINT, 0), \
             COALESCE(EXTRACT(EPOCH FROM shipyard_observed_at)::BIGINT, 0) \
             FROM stations \
             WHERE ($1 = 0 OR GREATEST(COALESCE(market_observed_at, 'epoch'), \
                                       COALESCE(outfitting_observed_at, 'epoch'), \
                                       COALESCE(shipyard_observed_at, 'epoch')) > to_timestamp($1)) \
             ORDER BY id",
    )
    .bind(cutoff)
    .fetch_all(&mut *transaction)
    .await?;

    let (system_string_ids, system_strings) = build_string_table(systems.iter().flat_map(|row| {
        [
            Some(row.1.as_str()),
            row.7.as_deref(),
            row.8.as_deref(),
            row.9.as_deref(),
            row.10.as_deref(),
            row.11.as_deref(),
        ]
        .into_iter()
        .flatten()
    }))?;
    let mut system_records =
        Vec::with_capacity(systems.len().saturating_mul(SYSTEM_RECORD_BYTES as usize));
    for row in &systems {
        let has_coordinates = row.2.is_some() && row.3.is_some() && row.4.is_some();
        let mut flags = 0;
        if has_coordinates {
            flags |= SystemRecord::HAS_COORDINATES;
        }
        if row.5.is_some() {
            flags |= SystemRecord::HAS_POPULATION;
        }
        SystemRecord {
            address: row.0,
            x: row.2.unwrap_or(0.0),
            y: row.3.unwrap_or(0.0),
            z: row.4.unwrap_or(0.0),
            population: row
                .5
                .map_or(Ok(0), |value| checked_u64(value, "population"))?,
            observed_at: row.6,
            name_id: string_id(&system_string_ids, Some(&row.1))?,
            security_id: string_id(&system_string_ids, row.7.as_ref())?,
            allegiance_id: string_id(&system_string_ids, row.8.as_ref())?,
            controlling_power_id: string_id(&system_string_ids, row.9.as_ref())?,
            power_state_id: string_id(&system_string_ids, row.10.as_ref())?,
            powers_id: string_id(&system_string_ids, row.11.as_ref())?,
            flags,
        }
        .encode_into(&mut system_records);
    }
    let (station_string_ids, station_strings) =
        build_string_table(stations.iter().filter_map(|row| row.2.as_deref()))?;
    let mut station_records =
        Vec::with_capacity(stations.len().saturating_mul(STATION_RECORD_BYTES as usize));
    for row in &stations {
        let mut flags = 0;
        if row.3 {
            flags |= StationRecord::HAS_MARKET;
        }
        if row.4 {
            flags |= StationRecord::HAS_OUTFITTING;
        }
        if row.5 {
            flags |= StationRecord::HAS_SHIPYARD;
        }
        StationRecord {
            id: checked_u64(row.0, "station id")?,
            system_address: row.1,
            name_id: string_id(&station_string_ids, row.2.as_ref())?,
            flags,
            market_observed_at: row.6,
            outfitting_observed_at: row.7,
            shipyard_observed_at: row.8,
        }
        .encode_into(&mut station_records);
    }

    let (commodity_string_ids, commodity_strings) = build_string_table(
        commodities
            .iter()
            .flat_map(|(symbol, name, category)| [symbol.as_str(), name, category]),
    )?;
    let mut commodity_records = Vec::with_capacity(
        commodities
            .len()
            .saturating_mul(COMMODITY_RECORD_BYTES as usize),
    );
    for (index, (symbol, name, category)) in commodities.iter().enumerate() {
        CommodityCatalogRecord {
            id: u16::try_from(index + 1)?,
            symbol_id: string_id(&commodity_string_ids, Some(symbol))?,
            name_id: string_id(&commodity_string_ids, Some(name))?,
            category_id: string_id(&commodity_string_ids, Some(category))?,
        }
        .encode_into(&mut commodity_records);
    }

    let modules_encoded = encode_symbol_catalog(&modules)?;
    let ships_encoded = encode_symbol_catalog(&ships)?;
    let outfitting_auxiliary = encode_station_snapshots(&outfitting_snapshots)?;
    let shipyard_auxiliary = encode_station_snapshots(&shipyard_snapshots)?;

    let auxiliary = encode_market_auxiliary(&commodities, &station_snapshots)?;

    // The artifact is written to disk as it is built: the market section
    // alone is ~100 M records, and holding it (let alone every section)
    // in memory is what killed the first real publication at 15.6 GB. The
    // uncompressed stream goes to a staging file through the streaming
    // writer, the big sections come straight out of PostgreSQL cursors in
    // the container's own order (byte order, hence COLLATE "C"), and the
    // validators read the file through a memory map.
    clock.mark("identity");
    let token = if window_cutoff.is_some() { "market-daily" } else { "community" };
    let version = crate::version::short_version(
        if window_cutoff.is_some() { "market_daily" } else { "community" },
        sequence,
        generated_at,
    );
    let dir_name = if window_cutoff.is_some() { "market_daily" } else { "community" };
    let filename = format!("{token}-{version}.ebex.zst");
    let relative = PathBuf::from(dir_name).join(&version).join(&filename);
    let staging = artifact_dir.join(format!(".staging-{token}-{version}"));
    tokio::fs::create_dir_all(&staging).await?;
    let uncompressed_path = staging.join(format!("{token}-{version}.ebex"));
    let compressed_path = staging.join(&filename);
    let plan = ed_ebex::community_section_plans();
    let mut writer = SnapshotWriter::create(
        &uncompressed_path,
        SnapshotHeader {
            sequence: checked_u64(sequence, "snapshot sequence")?,
            created_at,
            watermark,
        },
        &plan,
    )?;
    let write_whole = |writer: &mut SnapshotWriter, id: u16, size: u32, records: &[u8], auxiliary: &[u8]| -> Result<()> {
        writer.begin_section(id)?;
        for record in records.chunks(size as usize) {
            writer.write_record(record)?;
        }
        writer.write_auxiliary(auxiliary)?;
        writer.end_section()
    };
    write_whole(&mut writer, SECTION_SYSTEMS, SYSTEM_RECORD_BYTES, &system_records, &system_strings)?;
    write_whole(&mut writer, SECTION_STATIONS, STATION_RECORD_BYTES, &station_records, &station_strings)?;
    write_whole(&mut writer, SECTION_COMMODITIES, COMMODITY_RECORD_BYTES, &commodity_records, &commodity_strings)?;

    // Markets: one cursor, one record at a time, in (station, dictionary
    // id) order -- the dictionary is the byte-sorted symbol list, so
    // COLLATE "C" gives the same order without a sort in memory.
    writer.begin_section(SECTION_MARKETS)?;
    let mut market_rows: usize = 0;
    {
        use futures_util::TryStreamExt;
        let mut buffer = Vec::with_capacity(MARKET_RECORD_BYTES as usize);
        let mut stream = sqlx::query_as::<_, (i64, String, i64, i64, i64, i64, i64)>(
            "SELECT station_id, commodity_symbol, buy_price, sell_price, demand, supply, \
             EXTRACT(EPOCH FROM observed_at)::BIGINT FROM market \
             WHERE ($1 = 0 OR observed_at > to_timestamp($1)) \
             ORDER BY station_id, commodity_symbol COLLATE \"C\"",
        )
        .bind(cutoff)
        .fetch(&mut *transaction);
        while let Some((station_id, symbol, buy_price, sell_price, demand, supply, observed_at)) = stream.try_next().await? {
            buffer.clear();
            MarketRecord {
                station_id: checked_u64(station_id, "station id")?,
                commodity_id: *dictionary
                    .get(&symbol)
                    .context("market commodity missing from dictionary")?,
                buy_price: checked_u32(buy_price, "buy price")?,
                sell_price: checked_u32(sell_price, "sell price")?,
                demand: checked_u32(demand, "demand")?,
                supply: checked_u32(supply, "supply")?,
                observed_at,
            }
            .encode_into(&mut buffer);
            writer.write_record(&buffer)?;
            market_rows += 1;
        }
    }
    writer.write_auxiliary(&auxiliary)?;
    writer.end_section()?;

    write_whole(&mut writer, SECTION_MODULES, SYMBOL_CATALOG_RECORD_BYTES, &modules_encoded.records, &modules_encoded.strings)?;
    let outfitting_rows = stream_availability(
        &mut writer,
        &mut transaction,
        SECTION_OUTFITTING,
        // Two variants, not an OR-guard: see the systems query's planner
        // note — an OR around a subquery degrades to a correlated filter.
        if cutoff == 0 {
            "SELECT station_id, module_symbol FROM outfitting \
             ORDER BY station_id, module_symbol COLLATE \"C\""
        } else {
            "SELECT station_id, module_symbol FROM outfitting \
             WHERE station_id IN (SELECT id FROM stations \
                 WHERE outfitting_observed_at > to_timestamp($1)) \
             ORDER BY station_id, module_symbol COLLATE \"C\""
        },
        cutoff,
        &modules_encoded.ids,
        &outfitting_auxiliary,
    )
    .await?;
    write_whole(&mut writer, SECTION_SHIPS, SYMBOL_CATALOG_RECORD_BYTES, &ships_encoded.records, &ships_encoded.strings)?;
    let shipyard_rows = stream_availability(
        &mut writer,
        &mut transaction,
        SECTION_SHIPYARDS,
        if cutoff == 0 {
            "SELECT station_id, ship_symbol FROM shipyard \
             ORDER BY station_id, ship_symbol COLLATE \"C\""
        } else {
            "SELECT station_id, ship_symbol FROM shipyard \
             WHERE station_id IN (SELECT id FROM stations \
                 WHERE shipyard_observed_at > to_timestamp($1)) \
             ORDER BY station_id, ship_symbol COLLATE \"C\""
        },
        cutoff,
        &ships_encoded.ids,
        &shipyard_auxiliary,
    )
    .await?;
    // 2026-09-04 addendum, both optional and deliberately lean: the
    // sections carry whatever the Docked-event/prohibited[] ingest has
    // learned so far and fatten naturally as the columns fill.
    let details: Vec<(i64, bool, Option<i32>, Option<i32>, Option<i32>, Option<f64>, Option<String>, bool)> =
        sqlx::query_as(
            "SELECT s.id, s.is_carrier, s.pad_small, s.pad_medium, s.pad_large, s.arrival_ls, s.station_type, \
                    EXISTS(SELECT 1 FROM station_services sv \
                           WHERE sv.station_id = s.id AND sv.service = 'blackmarket') \
             FROM stations s WHERE s.identity_observed_at IS NOT NULL ORDER BY s.id",
        )
        .fetch_all(&mut *transaction)
        .await?;
    let (type_string_ids, type_strings) = build_string_table(
        details.iter().filter_map(|row| row.6.as_deref()),
    )?;
    let mut details_records =
        Vec::with_capacity(details.len() * STATION_DETAILS_RECORD_BYTES as usize);
    for (id, is_carrier, pad_small, pad_medium, pad_large, arrival, station_type, black_market) in
        &details
    {
        let mut flags = 0u32;
        if *is_carrier {
            flags |= ed_ebex::StationDetailsRecord::IS_CARRIER;
        }
        if *black_market {
            flags |= ed_ebex::StationDetailsRecord::HAS_BLACK_MARKET;
        }
        if pad_small.is_some() || pad_medium.is_some() || pad_large.is_some() {
            flags |= ed_ebex::StationDetailsRecord::HAS_PADS;
        }
        if arrival.is_some() {
            flags |= ed_ebex::StationDetailsRecord::HAS_ARRIVAL;
        }
        let pad = |value: &Option<i32>| {
            u16::try_from(value.unwrap_or(0).max(0)).unwrap_or(u16::MAX)
        };
        ed_ebex::StationDetailsRecord {
            station_id: checked_u64(*id, "station id")?,
            flags,
            pad_small: pad(pad_small),
            pad_medium: pad(pad_medium),
            pad_large: pad(pad_large),
            arrival_ls: arrival.unwrap_or(0.0) as f32,
            type_id: string_id(&type_string_ids, station_type.as_ref())?,
        }
        .encode_into(&mut details_records);
    }
    write_whole(
        &mut writer,
        ed_ebex::SECTION_STATION_DETAILS,
        STATION_DETAILS_RECORD_BYTES,
        &details_records,
        &type_strings,
    )?;

    let confiscations: Vec<(i64, String)> = sqlx::query_as(
        "SELECT station_id, symbol FROM station_prohibited ORDER BY station_id, symbol",
    )
    .fetch_all(&mut *transaction)
    .await?;
    // The stored values are lowercase wire strings — symbols from EDDN,
    // display names from dumps. Map through the artifact's own catalog
    // (ids are index+1 by construction); unmapped values are dropped and
    // counted, never guessed.
    let mut catalog_lookup: BTreeMap<String, u32> = BTreeMap::new();
    for (index, (symbol, name, _)) in commodities.iter().enumerate() {
        let id = u32::try_from(index + 1)?;
        catalog_lookup.insert(symbol.to_lowercase(), id);
        catalog_lookup.insert(name.to_lowercase(), id);
    }
    let mut pairs: Vec<(u64, u32)> = Vec::with_capacity(confiscations.len());
    let mut unmapped = 0usize;
    for (station_id, value) in &confiscations {
        match catalog_lookup.get(&value.to_lowercase()) {
            Some(commodity_id) => pairs.push((checked_u64(*station_id, "station id")?, *commodity_id)),
            None => unmapped += 1,
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    if unmapped > 0 {
        tracing::info!(unmapped, "prohibited entries without a catalog match were dropped");
    }
    let mut prohibited_bytes = Vec::with_capacity(pairs.len() * PROHIBITED_RECORD_BYTES as usize);
    for (station_id, commodity_id) in &pairs {
        ed_ebex::ProhibitedRecord { station_id: *station_id, commodity_id: *commodity_id }
            .encode_into(&mut prohibited_bytes);
    }
    // An empty string table keeps the auxiliary region well-formed.
    write_whole(
        &mut writer,
        ed_ebex::SECTION_PROHIBITED,
        PROHIBITED_RECORD_BYTES,
        &prohibited_bytes,
        &0u32.to_le_bytes(),
    )?;
    tracing::info!(
        stations = details.len(),
        confiscation_pairs = pairs.len(),
        "addendum sections written"
    );
    transaction.commit().await?;
    writer.finish()?;
    clock.mark("stream");

    let counts = CatalogCounts {
        commodities: commodities.len(),
        modules: modules.len(),
        outfitting: outfitting_rows,
        outfitting_snapshots: outfitting_snapshots.len(),
        ships: ships.len(),
        shipyard: shipyard_rows,
        shipyard_snapshots: shipyard_snapshots.len(),
    };
    {
        let uncompressed = ed_ebex::map_file(&uncompressed_path)?;
        validate_identity_export(&uncompressed, systems.len(), stations.len())?;
        validate_catalogs_and_relations(&uncompressed, counts)?;
        validate_market_export(&uncompressed, market_rows, station_snapshots.len())?;
        validate_publication_policy(&uncompressed)?;
    }
    clock.mark("validate");
    let (bytes, sha256) = ed_ebex::compress_file(&uncompressed_path, &compressed_path, 9)?;
    clock.mark("compress");
    // An independent read of what will be served: decompressed back to a
    // file and validated again, exactly as a client would.
    {
        let verified_path = staging.join(format!("{token}-{version}.verify"));
        ed_ebex::decompress_file(&compressed_path, &verified_path)?;
        let verified = ed_ebex::map_file(&verified_path)?;
        validate_identity_export(&verified, systems.len(), stations.len())?;
        validate_catalogs_and_relations(&verified, counts)?;
        validate_market_export(&verified, market_rows, station_snapshots.len())?;
        validate_publication_policy(&verified)?;
        drop(verified);
        tokio::fs::remove_file(&verified_path).await?;
    }
    tokio::fs::remove_file(&uncompressed_path).await?;
    clock.mark("verify");
    let artifact = ArtifactFile {
        path: relative.to_string_lossy().replace('\\', "/"),
        bytes,
        sha256: sha256.clone(),
    };
    artifact.validate()?;
    let published = artifact_dir.join(dir_name).join(&version);
    tokio::fs::create_dir_all(artifact_dir.join(dir_name)).await?;
    tokio::fs::rename(&staging, &published)
        .await
        .context("atomically publishing EBEX artifact")?;

    let existing = crate::routing::read_current_manifest(artifact_dir)?;
    let grace = existing
        .as_ref()
        .and_then(|manifest| {
            manifest.products.get(&if window_cutoff.is_some() {
                ProductKey::MarketDaily
            } else {
                ProductKey::Community
            })
        })
        .map(|product| product.version.clone());
    let manifest = if let Some(cutoff) = window_cutoff {
        Manifest::with_product(
            existing,
            generated_at,
            None,
            ProductKey::MarketDaily,
            Product {
                version: version.clone(),
                schema: ed_sync::COMMUNITY_SCHEMA_V1,
                minimum_client: None,
                files: vec![artifact],
                overlays: Vec::new(),
                covers_from: Some(cutoff),
            },
        )
    } else {
        community_manifest_over(existing, generated_at, watermark_text, &version, artifact)
    };
    crate::routing::write_manifest(artifact_dir, &manifest, &sequence.to_string())
        .context("atomically publishing EBEX manifest")?;
    // Snapshot dirs are ~500 MB (full) — prune to current + grace, same
    // policy as routing versions.
    let pruned = crate::routing::prune_version_dirs(artifact_dir, dir_name, &version, grace.as_deref(), &[])?;
    if !pruned.is_empty() {
        tracing::info!(?pruned, product = dir_name, "pruned retired snapshot versions");
    }

    clock.mark("publish");
    Ok((
        Publication {
            sequence,
            artifact: relative,
            bytes,
            sha256,
            rows: u64::try_from(market_rows)?,
        },
        clock.finish(),
    ))
}

pub async fn publish_market(pool: &PgPool, artifact_dir: &Path) -> Result<Publication> {
    publish_community(pool, artifact_dir).await
}

/// The manifest the service publishes for one community baseline. Pure so
/// the client's selection logic can be tested against it without PostgreSQL.
pub fn community_manifest(
    generated_at: &str,
    eddn_watermark: String,
    version: &str,
    artifact: ArtifactFile,
) -> Manifest {
    community_manifest_over(None, generated_at, eddn_watermark, version, artifact)
}

/// [`community_manifest`] over the currently published manifest: every
/// other product (the routing index) is carried forward, so publishing a
/// community baseline never unpublishes anything else.
pub fn community_manifest_over(
    existing: Option<Manifest>,
    generated_at: &str,
    eddn_watermark: String,
    version: &str,
    artifact: ArtifactFile,
) -> Manifest {
    Manifest::with_product(
        existing,
        generated_at,
        Some(eddn_watermark),
        ProductKey::Community,
        Product {
            version: version.to_owned(),
            schema: ed_sync::COMMUNITY_SCHEMA_V1,
            minimum_client: None,
            files: vec![artifact],
            overlays: Vec::new(),
        covers_from: None,
        },
    )
}

/// A published baseline must decode with this crate's own section support
/// and must be installable by a market-only client.
///
/// ONE certifying container walk. The old body's two helpers each re-ran
/// `validate_snapshot` internally — part of the 10+ hidden walks per
/// publication the 2026-09-04 review chain surfaced; every lookup below
/// the single walk uses the prevalidated directory parse.
fn validate_publication_policy(bytes: &[u8]) -> Result<()> {
    ed_ebex::validate_snapshot(bytes).context("publication does not validate")?;
    for section in ed_ebex::sections_prevalidated(bytes)? {
        ensure!(
            !section.required || ALL_V1_SECTIONS.contains(&(section.id, section.schema)),
            "publication contains a section the publisher cannot decode: {} schema {}",
            section.id,
            section.schema
        );
    }
    ed_ebex::market_baseline_prevalidated(bytes, ed_ebex::MARKET_BASELINE_SECTIONS)
        .context("publication is not installable by a market-only client")?;
    Ok(())
}

/// The prevalidated directory lookup every validator below uses: the
/// buffer's single certifying walk already happened (encode_snapshot
/// validates its own output; validate_publication_policy walks once),
/// so a section lookup must never cost another 100M-record pass.
fn find_section<'a>(
    sections: &[ed_ebex::SectionRef<'a>],
    id: u16,
) -> Option<ed_ebex::SectionRef<'a>> {
    sections.iter().find(|section| section.id == id).copied()
}

/// Stream an availability section (outfitting or shipyard) from a cursor
/// already ordered by `(station_id, symbol COLLATE "C")` -- the catalog's
/// own order -- into the writer, one record at a time. Returns the row
/// count.
async fn stream_availability(
    writer: &mut SnapshotWriter,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    section: u16,
    sql: &'static str,
    cutoff: i64,
    item_ids: &BTreeMap<String, u32>,
    auxiliary: &[u8],
) -> Result<usize> {
    use futures_util::TryStreamExt;
    writer.begin_section(section)?;
    let mut rows = 0usize;
    let mut buffer = Vec::with_capacity(AVAILABILITY_RECORD_BYTES as usize);
    {
        let query = sqlx::query_as::<_, (i64, String)>(sql);
        // The windowed variant carries $1; the full variant has no
        // parameters and an extra bind would be rejected.
        let query = if cutoff != 0 { query.bind(cutoff) } else { query };
        let mut stream = query.fetch(&mut **transaction);
        while let Some((station_id, symbol)) = stream.try_next().await? {
            buffer.clear();
            AvailabilityRecord {
                station_id: checked_u64(station_id, "station id")?,
                item_id: *item_ids
                    .get(&symbol)
                    .context("availability item missing from catalog")?,
            }
            .encode_into(&mut buffer);
            writer.write_record(&buffer)?;
            rows += 1;
        }
    }
    writer.write_auxiliary(auxiliary)?;
    writer.end_section()?;
    Ok(rows)
}

fn validate_identity_export(
    bytes: &[u8],
    expected_systems: usize,
    expected_stations: usize,
) -> Result<()> {
    let sections = ed_ebex::sections_prevalidated(bytes)?;
    let systems_section = find_section(&sections, SECTION_SYSTEMS)
        .context("identity export has no systems section")?;
    let stations_section = find_section(&sections, SECTION_STATIONS)
        .context("identity export has no stations section")?;
    let system_strings = ed_ebex::string_table(systems_section)?
        .into_iter()
        .map(|value| value.id)
        .collect::<std::collections::BTreeSet<_>>();
    let station_strings = ed_ebex::string_table(stations_section)?
        .into_iter()
        .map(|value| value.id)
        .collect::<std::collections::BTreeSet<_>>();
    let mut addresses = std::collections::BTreeSet::new();
    let mut previous_address = None;
    let mut system_count = 0usize;
    for system in ed_ebex::system_records(systems_section)? {
        ensure!(
            system.name_id != 0 && system_strings.contains(&system.name_id),
            "system name does not resolve"
        );
        for id in [
            system.security_id,
            system.allegiance_id,
            system.controlling_power_id,
            system.power_state_id,
            system.powers_id,
        ] {
            ensure!(
                id == 0 || system_strings.contains(&id),
                "system string reference does not resolve"
            );
        }
        ensure!(
            previous_address.is_none_or(|previous| previous < system.address),
            "systems are not strictly sorted"
        );
        previous_address = Some(system.address);
        addresses.insert(system.address);
        system_count += 1;
    }
    ensure!(
        system_count == expected_systems,
        "system count changed during encoding"
    );
    let mut previous_station = None;
    let mut station_count = 0usize;
    for station in ed_ebex::station_records(stations_section)? {
        ensure!(
            addresses.contains(&station.system_address),
            "station references unknown system"
        );
        ensure!(
            station.name_id == 0 || station_strings.contains(&station.name_id),
            "station name does not resolve"
        );
        ensure!(
            previous_station.is_none_or(|previous| previous < station.id),
            "stations are not strictly sorted"
        );
        previous_station = Some(station.id);
        station_count += 1;
    }
    ensure!(
        station_count == expected_stations,
        "station count changed during encoding"
    );
    Ok(())
}

fn validate_market_export(
    bytes: &[u8],
    expected_rows: usize,
    expected_stations: usize,
) -> Result<()> {
    let sections = ed_ebex::sections_prevalidated(bytes)?;
    let section =
        find_section(&sections, SECTION_MARKETS).context("market export has no market section")?;
    let auxiliary = ed_ebex::market_auxiliary(section)?;
    let embedded = auxiliary
        .commodities
        .iter()
        .map(|commodity| (commodity.id, commodity.symbol.clone()))
        .collect::<Vec<_>>();
    let catalog = match find_section(&sections, SECTION_COMMODITIES) {
        Some(commodity_section) => {
            let commodity_strings = ed_ebex::string_table(commodity_section)?
                .into_iter()
                .map(|value| (value.id, value.value))
                .collect::<BTreeMap<_, _>>();
            ed_ebex::commodity_records(commodity_section)?
                .map(|record| {
                    Ok((
                        record.id,
                        commodity_strings
                            .get(&record.symbol_id)
                            .context("commodity symbol does not resolve")?
                            .clone(),
                    ))
                })
                .collect::<Result<Vec<_>>>()?
        }
        None => embedded.clone(),
    };
    ensure!(
        catalog == embedded,
        "market dictionary differs from commodity catalog"
    );
    ensure!(
        auxiliary.stations.len() == expected_stations,
        "market export station snapshot count changed during encoding"
    );
    ed_ebex::validate_market_section(section)?;
    let decoded_rows = ed_ebex::market_records(section)?.count();
    ensure!(
        decoded_rows == expected_rows,
        "market export row count changed during encoding"
    );
    Ok(())
}

#[derive(Clone, Copy)]
struct CatalogCounts {
    commodities: usize,
    modules: usize,
    outfitting: usize,
    outfitting_snapshots: usize,
    ships: usize,
    shipyard: usize,
    shipyard_snapshots: usize,
}

fn validate_catalogs_and_relations(bytes: &[u8], expected: CatalogCounts) -> Result<()> {
    let sections = ed_ebex::sections_prevalidated(bytes)?;
    let stations_section = find_section(&sections, SECTION_STATIONS)
        .context("catalog export has no stations section")?;
    let stations = ed_ebex::station_records(stations_section)?
        .map(|station| (station.id, station))
        .collect::<BTreeMap<_, _>>();

    let commodity_section = find_section(&sections, SECTION_COMMODITIES)
        .context("catalog export has no commodities section")?;
    let commodity_ids = validate_catalog(
        ed_ebex::commodity_records(commodity_section)?.map(|record| {
            (
                u32::from(record.id),
                record.symbol_id,
                record.name_id,
                record.category_id,
            )
        }),
        ed_ebex::string_table(commodity_section)?,
        expected.commodities,
    )?;

    let module_section = find_section(&sections, SECTION_MODULES)
        .context("catalog export has no modules section")?;
    let module_ids = validate_catalog(
        ed_ebex::module_records(module_section)?.map(|record| (record.id, record.symbol_id, 0, 0)),
        ed_ebex::string_table(module_section)?,
        expected.modules,
    )?;

    let ship_section =
        find_section(&sections, SECTION_SHIPS).context("catalog export has no ships section")?;
    let ship_ids = validate_catalog(
        ed_ebex::ship_records(ship_section)?.map(|record| (record.id, record.symbol_id, 0, 0)),
        ed_ebex::string_table(ship_section)?,
        expected.ships,
    )?;

    ensure!(
        commodity_ids.len() == expected.commodities,
        "commodity count changed during encoding"
    );
    validate_availability(
        find_section(&sections, SECTION_OUTFITTING)
            .context("catalog export has no outfitting section")?,
        &stations,
        &module_ids,
        expected.outfitting,
        expected.outfitting_snapshots,
        true,
    )?;
    validate_availability(
        find_section(&sections, SECTION_SHIPYARDS)
            .context("catalog export has no shipyard section")?,
        &stations,
        &ship_ids,
        expected.shipyard,
        expected.shipyard_snapshots,
        false,
    )?;
    Ok(())
}

fn validate_catalog(
    records: impl Iterator<Item = (u32, u32, u32, u32)>,
    strings: Vec<ed_ebex::StringDefinition>,
    expected: usize,
) -> Result<std::collections::BTreeSet<u32>> {
    let strings = strings
        .into_iter()
        .map(|value| (value.id, value.value))
        .collect::<BTreeMap<_, _>>();
    let mut ids = std::collections::BTreeSet::new();
    let mut previous_symbol: Option<&str> = None;
    for (index, (id, symbol_id, name_id, category_id)) in records.enumerate() {
        ensure!(
            id == u32::try_from(index + 1)?,
            "catalog ids are not contiguous"
        );
        let symbol = strings
            .get(&symbol_id)
            .context("catalog symbol does not resolve")?;
        ensure!(
            previous_symbol.is_none_or(|previous| previous < symbol.as_str()),
            "catalog symbols are not strictly sorted"
        );
        previous_symbol = Some(symbol);
        for optional in [name_id, category_id] {
            ensure!(
                optional == 0 || strings.contains_key(&optional),
                "catalog metadata does not resolve"
            );
        }
        ids.insert(id);
    }
    ensure!(
        ids.len() == expected,
        "catalog count changed during encoding"
    );
    Ok(ids)
}

fn validate_availability(
    section: ed_ebex::SectionRef<'_>,
    stations: &BTreeMap<u64, StationRecord>,
    items: &std::collections::BTreeSet<u32>,
    expected_rows: usize,
    expected_snapshots: usize,
    outfitting: bool,
) -> Result<()> {
    let snapshots = ed_ebex::station_snapshots(section)?;
    ensure!(
        snapshots.len() == expected_snapshots,
        "availability snapshot count changed during encoding"
    );
    let snapshots = snapshots
        .into_iter()
        .map(|snapshot| {
            let station = stations
                .get(&snapshot.station_id)
                .context("availability snapshot references unknown station")?;
            let station_observed_at = if outfitting {
                station.outfitting_observed_at
            } else {
                station.shipyard_observed_at
            };
            ensure!(
                snapshot.observed_at == station_observed_at,
                "availability freshness differs from station section"
            );
            Ok((snapshot.station_id, snapshot.observed_at))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let records = if outfitting {
        ed_ebex::outfitting_records(section)?.collect::<Vec<_>>()
    } else {
        ed_ebex::shipyard_records(section)?.collect::<Vec<_>>()
    };
    let mut previous = None;
    for record in &records {
        ensure!(
            stations.contains_key(&record.station_id),
            "availability row references unknown station"
        );
        ensure!(
            snapshots.contains_key(&record.station_id),
            "availability row has no snapshot freshness"
        );
        ensure!(
            items.contains(&record.item_id),
            "availability row references unknown catalog item"
        );
        let key = (record.station_id, record.item_id);
        ensure!(
            previous.is_none_or(|value| value < key),
            "availability rows are not strictly sorted"
        );
        previous = Some(key);
    }
    ensure!(
        records.len() == expected_rows,
        "availability row count changed during encoding"
    );
    Ok(())
}

fn checked_u32(value: i64, field: &str) -> Result<u32> {
    u32::try_from(value).with_context(|| format!("{field} is outside EBEX u32 range: {value}"))
}

fn checked_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).with_context(|| format!("{field} is outside EBEX u64 range: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_ebex::{Section, COMMODITY_SCHEMA_V1, MARKET_SCHEMA_V1};

    /// The phase clock records marks in build order and serializes to
    /// the JSON object migration 0011 stores; on unix the CPU delta is
    /// present and can't run backwards.
    #[test]
    fn phase_clock_yields_ordered_marks_and_honest_cpu() {
        let mut clock = PhaseClock::start();
        clock.mark("identity");
        clock.mark("stream");
        let cost = clock.finish();
        assert_eq!(
            cost.phases.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
            vec!["identity", "stream"]
        );
        assert!(cost.phases.iter().all(|(_, seconds)| *seconds >= 0.0));
        let json: serde_json::Value = serde_json::from_str(&cost.phase_json()).unwrap();
        assert!(json.get("identity").and_then(|v| v.as_f64()).is_some());
        if cfg!(unix) {
            assert!(cost.cpu_seconds.is_some_and(|cpu| cpu >= 0.0));
        }
    }

    #[test]
    fn dictionary_is_deterministic_and_checked() {
        assert_eq!(
            encode_market_auxiliary(&[("gold".into(), "".into(), "".into())], &[]).unwrap(),
            vec![1, 0, 1, 0, 4, 0, 0, 0, 0, 0, b'g', b'o', b'l', b'd', 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert!(checked_u32(-1, "price").is_err());
        assert!(checked_u32(i64::from(u32::MAX) + 1, "price").is_err());
    }

    #[test]
    fn semantic_validation_accepts_empty_station_snapshots() {
        let bytes = ed_ebex::encode_snapshot(
            SnapshotHeader {
                sequence: 1,
                created_at: 2,
                watermark: 3,
            },
            vec![
                Section {
                    id: SECTION_COMMODITIES,
                    schema: COMMODITY_SCHEMA_V1,
                    required: true,
                    record_count: 0,
                    record_size: COMMODITY_RECORD_BYTES,
                    records: Vec::new(),
                    auxiliary: 0_u32.to_le_bytes().to_vec(),
                },
                Section {
                    id: SECTION_MARKETS,
                    schema: MARKET_SCHEMA_V1,
                    required: true,
                    record_count: 0,
                    record_size: MARKET_RECORD_BYTES,
                    records: Vec::new(),
                    auxiliary: encode_market_auxiliary(&[], &[(7, 3)]).unwrap(),
                },
            ],
        )
        .unwrap();
        validate_market_export(&bytes, 0, 1).unwrap();
    }

    #[test]
    fn only_market_is_published_as_required() {
        let required = ALL_V1_SECTIONS
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| ed_ebex::community_section_required(*id))
            .collect::<Vec<_>>();
        assert_eq!(required, vec![SECTION_MARKETS]);
    }

    /// The manifest this service writes must be the one a market-only client
    /// selects. This fails if either side changes its product key alone.
    #[test]
    fn published_manifest_is_selected_by_the_client() {
        let artifact = ArtifactFile::for_bytes("community/7/community-7.ebex.zst", b"zstd");
        let manifest = community_manifest(
            "2026-08-29T12:00:00Z",
            "2026-08-29T11:59:00Z".to_owned(),
            "7",
            artifact.clone(),
        );
        manifest.validate().unwrap();
        let json = serde_json::to_vec_pretty(&manifest).unwrap();
        let received: Manifest = serde_json::from_slice(&json).unwrap();
        let selected = received.community_baseline().unwrap();
        assert_eq!(selected.key, ed_sync::ProductKey::Community);
        assert_eq!(selected.product.version, "7");
        assert_eq!(selected.artifact, &artifact);
    }

    #[test]
    fn market_export_rejects_misordered_rows() {
        let mut records = Vec::new();
        for commodity_id in [2u16, 1u16] {
            MarketRecord {
                station_id: 7,
                commodity_id,
                buy_price: 1,
                sell_price: 2,
                demand: 3,
                supply: 4,
                observed_at: 3,
            }
            .encode_into(&mut records);
        }
        let bytes = ed_ebex::encode_snapshot(
            SnapshotHeader {
                sequence: 1,
                created_at: 2,
                watermark: 3,
            },
            vec![Section {
                id: SECTION_MARKETS,
                schema: MARKET_SCHEMA_V1,
                required: true,
                record_count: 2,
                record_size: MARKET_RECORD_BYTES,
                records,
                auxiliary: encode_market_auxiliary(&[("gold".into(), "".into(), "".into()), ("silver".into(), "".into(), "".into())], &[(7, 3)])
                    .unwrap(),
            }],
        )
        .unwrap();
        let error = validate_market_export(&bytes, 2, 1)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("not strictly sorted"),
            "unexpected error: {error}"
        );
    }
}
