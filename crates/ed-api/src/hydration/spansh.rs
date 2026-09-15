//! The PostgreSQL adapter of `ed_store::galaxy::GalaxySink`.
//!
//! Decoding runs on a blocking thread (the parser is CPU-bound and uses
//! rayon); the sink turns each system into a [`SourceSystem`] row, its
//! stations into rows plus `ed_domain::Operation` boards, and ships
//! batches over a bounded channel to an async writer that applies each
//! batch in one transaction through `ed_store::postgres`. Boards therefore
//! obey exactly the rule the EDDN feed obeys -- a station-level watermark,
//! strictly newer wins -- so re-hydrating from an older dump cannot regress
//! a fresher live observation.
//!
//! Bodies and factions have no service tables yet and are not delivered.

use std::path::Path;

use anyhow::{Context, Result};
use ed_domain::{Commodity, ObservedAt, Operation, Snapshot, StationIdentity};
use ed_store::{
    galaxy::{
        spansh::{self, Station, StationTimes, System},
        GalaxySink, ImportStats, SystemVisit,
    },
    postgres::{
        apply_in_transaction, apply_source_system, ensure_station, SourceSystem, SystemWrite,
    },
};
use sqlx::PgPool;
use tokio::sync::mpsc;

use super::{complete_job, fail_job, start_job, HydrationResult};

/// Systems per transaction. Populated systems carry a few stations with a
/// hundred-row board each, so this keeps a transaction to tens of
/// thousands of rows.
const BATCH_SYSTEMS: usize = 500;
/// Cap on board rows queued in one batch, whichever limit comes first.
const BATCH_ROWS: usize = 50_000;

/// The system row a Spansh record describes.
pub fn source_system(system: &System, provenance: &str) -> Option<SourceSystem> {
    Some(SourceSystem {
        address: system.id64?,
        name: system.name.clone().filter(|name| !name.trim().is_empty())?,
        position: system.position(),
        population: system.population,
        security: system.security.clone(),
        allegiance: system.allegiance.clone(),
        controlling_power: system.controlling_power.clone(),
        power_state: system.power_state.clone(),
        powers: system.powers.as_ref().map(|powers| powers.join(", ")),
        observed_at: system.updated(),
        provenance: provenance.to_owned(),
    })
}

fn observed(epoch: i64) -> ObservedAt {
    ObservedAt {
        timestamp: ed_store::session::iso_from_epoch(epoch),
        epoch_seconds: epoch,
    }
}

/// The operations a station carries: its identity (pads, type, arrival
/// distance, services) first, then the boards — market, outfitting,
/// shipyard — each stamped with its own observation time. A station
/// with none of those yields nothing.
///
/// The identity was missing until 2026-09-07: only EDDN `Docked` events
/// produced one, so 634,192 of prod's 659,282 market stations (96 %)
/// had unknown pads and the server-side profit finder dropped them all
/// (`excluded.pad_unknown`). The dump has carried every one of these
/// fields all along; the sink's freshness rule keeps a newer EDDN
/// identity ahead of an older dump one.
pub fn station_operations(
    system: &System,
    station: &Station,
    times: &StationTimes,
) -> Vec<Operation> {
    let mut operations = Vec::with_capacity(4);
    let (Some(system_name), Some(market_id)) = (system.name.clone(), station.id) else {
        return operations;
    };
    if let Some(identity) = station_identity(&system_name, system.id64, station, times, market_id) {
        operations.push(Operation::StationIdentity(identity));
    }
    fn snapshot<T>(
        system_name: &str,
        station: &Station,
        market_id: i64,
        epoch: i64,
        values: Vec<T>,
    ) -> Snapshot<T> {
        Snapshot {
            system_name: system_name.to_owned(),
            station_name: station.name.clone(),
            market_id: Some(market_id),
            observed_at: observed(epoch),
            values,
            // The dump carries prohibitedCommodities but this parser does
            // not read them yet; the live EDDN commodity/3 capture fills
            // station_prohibited in the meantime (2026-09-04 ingest).
            prohibited: Vec::new(),
        }
    }
    if let Some(market) = &station.market {
        let values = market
            .commodities
            .iter()
            .filter_map(|c| {
                Some(Commodity {
                    name: c.key()?,
                    buy_price: c.buy_price.unwrap_or(0),
                    sell_price: c.sell_price.unwrap_or(0),
                    demand: c.demand.unwrap_or(0),
                    stock: c.supply.unwrap_or(0),
                })
            })
            .collect();
        operations.push(Operation::Market(snapshot(
            &system_name,
            station,
            market_id,
            times.market,
            values,
        )));
    }
    if let (Some(outfitting), Some(epoch)) = (&station.outfitting, times.outfitting) {
        let values = outfitting.modules.iter().filter_map(|m| m.key()).collect();
        operations.push(Operation::Outfitting(snapshot(
            &system_name,
            station,
            market_id,
            epoch,
            values,
        )));
    }
    if let (Some(shipyard), Some(epoch)) = (&station.shipyard, times.shipyard) {
        let values = shipyard.ships.iter().filter_map(|s| s.key()).collect();
        operations.push(Operation::Shipyard(snapshot(
            &system_name,
            station,
            market_id,
            epoch,
            values,
        )));
    }
    operations
}

/// The dump station's identity, when it says anything a `Docked` event
/// would: type, pads, arrival distance or services. Stamped with the
/// station's own `updateTime` (else the market's), so a later EDDN
/// docking still wins.
fn station_identity(
    system_name: &str,
    system_address: Option<i64>,
    station: &Station,
    times: &StationTimes,
    market_id: i64,
) -> Option<StationIdentity> {
    let pads = station.landing_pads.as_ref();
    let services: Vec<String> = station
        .services
        .iter()
        .map(|s| ed_domain::station::journal_service_key(s))
        .collect();
    if station.kind.is_none()
        && pads.is_none()
        && station.distance_to_arrival.is_none()
        && services.is_empty()
        && station.primary_economy.is_none()
        && station.government.is_none()
        && station.controlling_faction.is_none()
    {
        return None;
    }
    let epoch = times.station.unwrap_or(times.market);
    Some(StationIdentity {
        system_name: system_name.to_owned(),
        system_address,
        station_name: station.name.clone().unwrap_or_default(),
        market_id,
        observed_at: observed(epoch),
        station_type: station.kind.clone(),
        arrival_ls: station.distance_to_arrival,
        pad_small: pads.and_then(|p| p.small),
        pad_medium: pads.and_then(|p| p.medium),
        pad_large: pads.and_then(|p| p.large),
        primary_economy: station.primary_economy.clone(),
        government: station.government.clone(),
        controlling_faction: station.controlling_faction.clone(),
        services,
    })
}

#[derive(Default)]
struct Batch {
    systems: Vec<SourceSystem>,
    /// `(station id, system address, name)`.
    stations: Vec<(i64, i64, Option<String>)>,
    operations: Vec<Operation>,
    /// Main-star classes the dump carries (item 47 stage 1c): taught to
    /// the stars table so the nightly reconcile folds them into the
    /// routing index.
    stars: Vec<crate::stars::StarObservation>,
    /// Prospecting bodies with their rings, hotspots and surface
    /// materials (the mining search, B.4).
    bodies: Vec<ed_domain::BodyTeaching>,
    rows: usize,
    /// Newest system `date` seen, for the job's source watermark.
    newest: Option<i64>,
}

/// Collects decoded records into batches and hands them to the writer.
struct PostgresSink {
    provenance: String,
    batch: Batch,
    sender: mpsc::Sender<Batch>,
}

impl PostgresSink {
    fn flush(&mut self) -> Result<()> {
        if self.batch.systems.is_empty()
            && self.batch.operations.is_empty()
            && self.batch.stars.is_empty()
            && self.batch.bodies.is_empty()
        {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.batch);
        self.sender
            .blocking_send(batch)
            .map_err(|_| anyhow::anyhow!("hydration writer stopped"))
    }
}

impl GalaxySink for PostgresSink {
    fn system(
        &mut self,
        system: &System,
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<SystemVisit> {
        let Some(row) = source_system(system, &self.provenance) else {
            return Ok(SystemVisit::Skip);
        };
        stats.systems += 1;
        self.batch.newest = self.batch.newest.max(updated);
        self.batch.systems.push(row);
        // Bodies now feed the stars table (item 47 stage 1c); factions
        // still have nowhere to go and are dropped in `body`'s sibling.
        Ok(SystemVisit::Full)
    }

    fn station(
        &mut self,
        system: &System,
        station: &Station,
        _body_name: Option<&str>,
        times: &StationTimes,
        stats: &mut ImportStats,
    ) -> Result<()> {
        let (Some(id), Some(address)) = (station.id, system.id64) else {
            return Ok(());
        };
        stats.stations += 1;
        self.batch
            .stations
            .push((id, address, station.name.clone()));
        for operation in station_operations(system, station, times) {
            self.batch.rows += match &operation {
                Operation::Market(s) => s.values.len(),
                Operation::Outfitting(s) | Operation::Shipyard(s) => s.values.len(),
                Operation::System(_)
                | Operation::StationIdentity(_)
                | Operation::Star(_)
                | Operation::Body(_)
                | Operation::RingHotspots(_)
                | Operation::BodySignals(_) => 0,
            };
            self.batch.operations.push(operation);
        }
        if self.batch.systems.len() >= BATCH_SYSTEMS || self.batch.rows >= BATCH_ROWS {
            self.flush()?;
        }
        Ok(())
    }

    fn body(
        &mut self,
        system_id64: i64,
        body: &spansh::Body,
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<()> {
        if let Some(star) =
            crate::stars::spansh_body_main_star(system_id64, body, updated, &self.provenance)
        {
            self.batch.stars.push(star);
        }
        if let Some(row) = crate::mining::body_row(system_id64, body, updated, &self.provenance) {
            stats.bodies += 1;
            stats.hotspots += row.hotspots.len() as u64;
            stats.body_material_rows += row.materials.len() as u64;
            self.batch.bodies.push(row);
        }
        Ok(())
    }

    fn factions(
        &mut self,
        _: i64,
        _: &[spansh::Faction],
        _: Option<i64>,
        _: &mut ImportStats,
    ) -> Result<()> {
        Ok(())
    }

    fn checkpoint(&mut self, _stats: &ImportStats) -> Result<()> {
        self.flush()
    }

    fn finish(&mut self, _stats: &ImportStats) -> Result<()> {
        self.flush()
    }
}

#[derive(Default)]
struct Written {
    systems: u64,
    stars: u64,
    bodies: u64,
    hotspots: u64,
    /// Systems whose name another address already holds; left out with
    /// their stations and boards. Kept for the whole run: a system's
    /// stations can straddle two batches (the flush check runs inside
    /// `station`), and the later batch must still know to drop them.
    unfiled: std::collections::HashSet<i64>,
    snapshots_applied: u64,
    snapshots_skipped: u64,
    identities_applied: u64,
    identities_skipped: u64,
    market_rows: u64,
    newest: Option<i64>,
}

async fn apply_batch(pool: &PgPool, batch: &Batch, written: &mut Written) -> Result<()> {
    let mut transaction = pool.begin().await?;
    // The galaxy has systems that share a name (case-insensitively) under
    // different addresses; `systems.name` is unique. A system that could
    // not be filed has no row, so its stations cannot reference it and
    // its boards, keyed by system name, would land on the namesake.
    for system in &batch.systems {
        match apply_source_system(&mut transaction, system).await? {
            SystemWrite::Written => written.systems += 1,
            SystemWrite::Unchanged => {}
            SystemWrite::NameTaken => {
                written.unfiled.insert(system.address);
            }
        }
    }
    let mut unfiled_stations: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (id, address, name) in &batch.stations {
        if written.unfiled.contains(address) {
            unfiled_stations.insert(*id);
            continue;
        }
        ensure_station(&mut transaction, *id, *address, name.as_deref()).await?;
    }
    let operations: Vec<Operation> = if unfiled_stations.is_empty() {
        batch.operations.clone()
    } else {
        batch
            .operations
            .iter()
            .filter(|operation| match operation {
                Operation::Market(s) => {
                    s.market_id.is_none_or(|id| !unfiled_stations.contains(&id))
                }
                Operation::Outfitting(s) | Operation::Shipyard(s) => {
                    s.market_id.is_none_or(|id| !unfiled_stations.contains(&id))
                }
                Operation::System(_)
                | Operation::Star(_)
                | Operation::Body(_)
                | Operation::RingHotspots(_)
                | Operation::BodySignals(_) => true,
                Operation::StationIdentity(i) => !unfiled_stations.contains(&i.market_id),
            })
            .cloned()
            .collect()
    };
    // Identities and boards are counted apart: the report says how many
    // stations learned their pads, not how many "snapshots" landed.
    let (identities, boards): (Vec<Operation>, Vec<Operation>) = operations
        .into_iter()
        .partition(|op| matches!(op, Operation::StationIdentity(_)));
    let identity_applied = apply_in_transaction(&mut transaction, &identities).await?;
    let applied = apply_in_transaction(&mut transaction, &boards).await?;
    let bodies =
        ed_store::postgres::apply_bodies(&mut transaction, &batch.bodies, &written.unfiled).await?;
    transaction.commit().await?;
    written.bodies += bodies.bodies;
    written.hotspots += bodies.hotspots;
    // Star teachings ride outside the transaction: apply_stars is its own
    // newer-wins upsert, so a replay is idempotent.
    written.stars += crate::stars::apply_stars(pool, &batch.stars).await?;
    written.identities_applied += identity_applied.messages;
    written.identities_skipped += identity_applied.skipped;
    written.snapshots_applied += applied.messages;
    written.snapshots_skipped += applied.skipped;
    written.market_rows += applied.market_rows;
    written.newest = written.newest.max(batch.newest);
    Ok(())
}

/// The source identity recorded for a dump: its kind and file name.
pub fn source_identity(path: &Path) -> String {
    format!(
        "spansh:{}:{}",
        spansh::dump_kind(path).as_str(),
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    )
}

/// Stream a Spansh dump into the service tables as one recorded hydration
/// job. The job's `source_observed_at` starts as the file's modification
/// time and ends as the newest system `date` the dump carried.
pub async fn hydrate_spansh(pool: &PgPool, path: &Path) -> Result<HydrationResult> {
    let metadata = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("failed to read dump {}", path.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| elapsed.as_secs() as i64);
    let source = source_identity(path);

    let mut transaction = pool.begin().await?;
    let run_id = start_job(
        &mut transaction,
        &source,
        modified,
        i64::try_from(metadata.len()).context("dump is too large")?,
    )
    .await?;
    transaction.commit().await?;

    let (sender, mut receiver) = mpsc::channel::<Batch>(2);
    let reader_path = path.to_owned();
    let provenance = source.clone();
    let reader = tokio::task::spawn_blocking(move || {
        let mut sink = PostgresSink {
            provenance,
            batch: Batch::default(),
            sender,
        };
        spansh::stream_dump(&reader_path, &mut sink, |stats, total| {
            tracing::info!(
                systems = stats.systems,
                stations = stats.stations,
                bytes_in = stats.bytes_in,
                total,
                "hydrating from Spansh dump"
            );
        })
    });

    let mut written = Written::default();
    let mut failure = None;
    while let Some(batch) = receiver.recv().await {
        if let Err(error) = apply_batch(pool, &batch, &mut written).await {
            failure = Some(error);
            break;
        }
    }
    // Dropping the receiver makes the reader's next send fail, which ends
    // the stream; its own error then reports the writer's.
    drop(receiver);
    let stats = match (failure, reader.await.context("hydration reader panicked")?) {
        (Some(error), _) => {
            fail_job(pool, run_id, written.systems).await;
            return Err(error.context("applying hydration batch"));
        }
        (None, Err(error)) => {
            fail_job(pool, run_id, written.systems).await;
            return Err(error);
        }
        (None, Ok(stats)) => stats,
    };

    let mut transaction = pool.begin().await?;
    complete_job(&mut transaction, run_id, written.systems, written.newest).await?;
    transaction.commit().await?;
    if !written.unfiled.is_empty() {
        tracing::warn!(
            systems = written.unfiled.len(),
            "systems left out because another address holds their name"
        );
    }

    Ok(HydrationResult {
        source,
        systems_seen: usize::try_from(stats.systems).unwrap_or(usize::MAX),
        systems_applied: written.systems,
        systems_unfiled: written.unfiled.len() as u64,
        stations_seen: stats.stations,
        snapshots_applied: written.snapshots_applied,
        snapshots_skipped: written.snapshots_skipped,
        identities_applied: written.identities_applied,
        identities_skipped: written.identities_skipped,
        market_rows: written.market_rows,
        stars_taught: written.stars,
        bodies_applied: written.bodies,
        hotspots_applied: written.hotspots,
        parse_errors: stats.parse_errors,
    })
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    fn times(station: Option<i64>, market: i64) -> StationTimes {
        StationTimes {
            station,
            market,
            outfitting: None,
            shipyard: None,
        }
    }

    /// A dump station with pads, a type, an arrival distance and
    /// services yields an identity FIRST, in the journal's vocabulary,
    /// stamped with the station's updateTime.
    #[test]
    fn a_dump_station_yields_its_identity_first() {
        let line = r#"{"id64":819519,"name":"Test Sys","stations":[{"id":100,"name":"Sys Port","type":"Coriolis Starport","distanceToArrival":118.1,"services":["Dock","Market","Material Trader","Interstellar Factors Contact"],"landingPads":{"large":8,"medium":11,"small":9},"market":{"commodities":[{"symbol":"gold","name":"Gold","category":"Metals","demand":5,"supply":7,"buyPrice":100,"sellPrice":90}]}}]}"#;
        let system: System = serde_json::from_str(line).unwrap();
        let station = &system.stations[0];
        let ops = station_operations(&system, station, &times(Some(1_700_000_000), 1_700_000_100));
        assert_eq!(ops.len(), 2, "identity + market: {ops:?}");
        let Operation::StationIdentity(id) = &ops[0] else {
            panic!("identity first: {:?}", ops[0])
        };
        assert_eq!(id.market_id, 100);
        assert_eq!(id.station_name, "Sys Port");
        assert_eq!(id.station_type.as_deref(), Some("Coriolis Starport"));
        assert_eq!(
            (id.pad_small, id.pad_medium, id.pad_large),
            (Some(9), Some(11), Some(8))
        );
        assert_eq!(id.arrival_ls, Some(118.1));
        assert_eq!(
            id.services,
            vec!["dock", "commodities", "materialtrader", "facilitator"]
        );
        assert_eq!(
            id.observed_at.epoch_seconds, 1_700_000_000,
            "the station's own updateTime"
        );
        assert!(!id.is_carrier());
        assert!(matches!(ops[1], Operation::Market(_)));
    }

    /// 2026-09-12: the dump carries primaryEconomy, government and
    /// controllingFaction on every station, the shared parser already
    /// deserializes all three, and this function dropped them — so
    /// `/v1/stations` returned a hard-coded null for each and the
    /// client's material-trader type filter discarded every row. The
    /// dump is also what backfills them, so this is the path that
    /// matters.
    #[test]
    fn a_dump_station_carries_its_economy_government_and_faction() {
        let line = r#"{"id64":819519,"name":"Test Sys","stations":[{"id":100,"name":"Sys Port","type":"Coriolis Starport","primaryEconomy":"High Tech","government":"Corporate","controllingFaction":"The Dark Wheel","services":["Material Trader"]}]}"#;
        let system: System = serde_json::from_str(line).unwrap();
        let ops = station_operations(&system, &system.stations[0], &times(Some(1_700_000_000), 0));
        let Operation::StationIdentity(id) = &ops[0] else {
            panic!("identity: {ops:?}")
        };
        assert_eq!(id.primary_economy.as_deref(), Some("High Tech"));
        assert_eq!(id.government.as_deref(), Some("Corporate"));
        assert_eq!(id.controlling_faction.as_deref(), Some("The Dark Wheel"));
    }

    /// An economy alone is enough to teach: a station record that
    /// carries nothing else still has something worth writing, and the
    /// guard must not throw it away.
    #[test]
    fn an_economy_alone_still_yields_an_identity() {
        let line = r#"{"id64":1,"name":"Sys","stations":[{"id":7,"name":"Nameplate","primaryEconomy":"Refinery"}]}"#;
        let system: System = serde_json::from_str(line).unwrap();
        let ops = station_operations(&system, &system.stations[0], &times(None, 0));
        let Operation::StationIdentity(id) = &ops[0] else {
            panic!("identity: {ops:?}")
        };
        assert_eq!(id.primary_economy.as_deref(), Some("Refinery"));
    }

    /// A carrier from the dump is a carrier to the sink even though the
    /// dump spells it "Drake-Class Carrier", not "FleetCarrier".
    #[test]
    fn a_dump_carrier_is_a_carrier() {
        let line = r#"{"id64":1,"name":"Sys","stations":[{"id":7,"name":"T2X-02X","type":"Drake-Class Carrier","landingPads":{"large":8,"medium":4,"small":4}}]}"#;
        let system: System = serde_json::from_str(line).unwrap();
        let ops = station_operations(&system, &system.stations[0], &times(None, 0));
        let Operation::StationIdentity(id) = &ops[0] else {
            panic!()
        };
        assert!(id.is_carrier());
        assert_eq!(
            id.observed_at.epoch_seconds, 0,
            "no updateTime: epoch 0, outranked by any dated identity"
        );
    }

    /// A station record with nothing an identity could carry yields no
    /// identity (and, boardless, nothing at all).
    #[test]
    fn a_bare_station_yields_nothing() {
        let line = r#"{"id64":1,"name":"Sys","stations":[{"id":7,"name":"Nameplate"}]}"#;
        let system: System = serde_json::from_str(line).unwrap();
        assert!(station_operations(&system, &system.stations[0], &times(None, 0)).is_empty());
    }
}
