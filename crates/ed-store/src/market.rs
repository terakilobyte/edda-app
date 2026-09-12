//! The galaxy market interface: one writer, a few readers.
//!
//! Three sources feed `sys_market` -- live EDDN commodity messages, EBEX
//! community baselines and Spansh dumps -- and all three used to carry
//! their own copy of the snapshot rule. This module is the single seam
//! they now go through, so "snapshot complete; absent means delisted" and
//! the freshness decision (`ed_domain::freshness`) are implemented exactly
//! once.
//!
//! Readers exist so that callers outside `ed-store` (the profit finder in
//! `ed-route`, the desktop's status panels) never name a galaxy table.

use anyhow::{ensure, Context, Result};
use ed_domain::freshness;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// One line of a station's board, keyed on the interned commodity id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketRow {
    pub commodity_id: i64,
    pub buy_price: i64,
    pub sell_price: i64,
    pub demand: i64,
    pub supply: i64,
}

/// What [`write_snapshot`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// The snapshot was newer: old rows are gone, these rows are in.
    Applied { removed: u64, written: u64 },
    /// The station already holds a snapshot at least this new.
    Skipped,
}

/// The station's market watermark: when its last applied snapshot was
/// observed. Rows written before watermarks existed fall back to the newest
/// row -- that fallback is only ever taken for stations never written
/// through this module, so the empty-snapshot hole cannot reopen.
pub fn watermark(conn: &Connection, station_id: i64) -> Result<Option<i64>> {
    let recorded: Option<i64> = conn
        .prepare_cached("SELECT observed_at FROM sys_market_watermarks WHERE station_id = ?1")?
        .query_row([station_id], |r| r.get(0))
        .optional()?;
    if recorded.is_some() {
        return Ok(recorded);
    }
    Ok(conn
        .prepare_cached("SELECT MAX(updated) FROM sys_market WHERE station_id = ?1")?
        .query_row([station_id], |r| r.get(0))?)
}

/// Replace a station's board with a complete snapshot observed at
/// `observed_at`, if that is strictly newer than the station's watermark
/// (`ed_domain::freshness::accept`). A snapshot is the whole board: a
/// commodity absent from `rows` is delisted and its row removed, and an
/// empty snapshot legitimately clears the station. The watermark is
/// advanced even then, so an older replay cannot refill it.
pub fn write_snapshot(
    conn: &Connection,
    station_id: i64,
    observed_at: i64,
    rows: &[MarketRow],
) -> Result<SnapshotOutcome> {
    if !freshness::is_newer(watermark(conn, station_id)?, observed_at) {
        return Ok(SnapshotOutcome::Skipped);
    }
    let removed = conn
        .prepare_cached("DELETE FROM sys_market WHERE station_id = ?1")?
        .execute([station_id])? as u64;
    let mut insert = conn.prepare_cached(
        "INSERT INTO sys_market
             (station_id, commodity_id, buy_price, sell_price, demand, supply, updated)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(station_id, commodity_id) DO UPDATE SET
             buy_price = excluded.buy_price, sell_price = excluded.sell_price,
             demand = excluded.demand, supply = excluded.supply,
             updated = excluded.updated",
    )?;
    let mut written = 0u64;
    for row in rows {
        insert.execute(params![
            station_id,
            row.commodity_id,
            row.buy_price,
            row.sell_price,
            row.demand,
            row.supply,
            observed_at
        ])?;
        written += 1;
    }
    conn.prepare_cached(
        "INSERT INTO sys_market_watermarks (station_id, observed_at) VALUES (?1, ?2)
         ON CONFLICT(station_id) DO UPDATE SET observed_at = excluded.observed_at",
    )?
    .execute(params![station_id, observed_at])?;
    Ok(SnapshotOutcome::Applied { removed, written })
}

/// Intern a commodity symbol, returning its id. Display name and category
/// are filled in when known and never blanked by a source that lacks them.
/// Canonical commodity symbol: lowercase with the raw journal wrapper
/// stripped. `$magnesite_name;` and `magnesite` are ONE good — the
/// census of 2026-09-05 found 371 of 510 goods fragmented, one stranded
/// market row per good, because the wrapper interned as its own symbol.
/// The wrapper is the only variant that receives live writes; the
/// spaced display spellings (0 market rows) wait for the merge
/// migration rather than being folded blind here.
pub fn canonical_symbol(symbol: &str) -> String {
    let s = symbol.trim().to_lowercase();
    s.strip_prefix('$')
        .and_then(|s| s.strip_suffix("_name;").or_else(|| s.strip_suffix("_name")))
        .unwrap_or(&s)
        .to_string()
}

pub fn intern_commodity(
    conn: &Connection,
    symbol: &str,
    name: Option<&str>,
    category: Option<&str>,
) -> Result<i64> {
    let symbol = canonical_symbol(symbol);
    Ok(conn
        .prepare_cached(
            "INSERT INTO sys_commodities (symbol, name, category) VALUES (?1, ?2, ?3)
             ON CONFLICT(symbol) DO UPDATE SET
                 name = COALESCE(excluded.name, sys_commodities.name),
                 category = COALESCE(excluded.category, sys_commodities.category)
             RETURNING id",
        )?
        .query_row(
            params![
                symbol,
                name.filter(|s| !s.is_empty()),
                category.filter(|s| !s.is_empty())
            ],
            |r| r.get(0),
        )?)
}

// ── EBEX baselines ──────────────────────────────────────────────────

/// What an EBEX market hydration wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct HydrationStats {
    /// Market rows the artifact carried.
    pub rows: u64,
    pub commodities: u64,
    pub station_snapshots: u64,
    /// Local rows deleted because a newer snapshot no longer listed them.
    pub removed: u64,
    /// Station snapshots older than what was already stored (local EDDN
    /// had moved on); left untouched.
    pub stations_skipped: u64,
    /// Stations skipped without even a compare because a checkpoint from
    /// an interrupted run of the SAME artifact already covered them
    /// (maintainer, 2026-09-05: restarts must resume, not repeat).
    pub stations_fast_forwarded: u64,
}

/// The resume cursor, stamped inside every chunk commit (atomic with the
/// data it describes) and cleared on completion. Identity is the
/// snapshot's (sequence, watermark) — a different artifact never
/// fast-forwards.
fn stamp_checkpoint(
    tx: &Connection,
    station: i64,
    metadata: &ed_ebex::SnapshotMetadata,
) -> Result<()> {
    for (key, value) in [
        ("hydrate_ckpt_station", station),
        ("hydrate_ckpt_seq", i64::try_from(metadata.sequence)?),
        ("hydrate_ckpt_wm", metadata.watermark),
    ] {
        tx.execute(
            "INSERT INTO sys_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    Ok(())
}

fn read_checkpoint(conn: &Connection, metadata: &ed_ebex::SnapshotMetadata) -> Option<i64> {
    let read = |key: &str| -> Option<i64> {
        conn.query_row("SELECT value FROM sys_meta WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .ok()
    };
    match (
        read("hydrate_ckpt_station"),
        read("hydrate_ckpt_seq"),
        read("hydrate_ckpt_wm"),
    ) {
        (Some(station), Some(seq), Some(wm))
            if Ok(seq) == i64::try_from(metadata.sequence) && wm == metadata.watermark =>
        {
            Some(station)
        }
        _ => None,
    }
}

/// Merge an EBEX market section into the galaxy. Every station snapshot in
/// the artifact goes through [`write_snapshot`], so a baseline obeys the
/// same rule as a live EDDN message: strictly newer replaces the board,
/// otherwise the local data stands. Also records the artifact's sequence
/// and watermark in `sys_meta`.
///
/// Only `galaxy.sys_*` tables are written; journal-derived data is never
/// touched by hydration.
pub fn hydrate_ebex(conn: &Connection, bytes: &[u8]) -> Result<HydrationStats> {
    hydrate_ebex_with(conn, bytes, &mut |_| {})
}

/// A hydrate that stopped because the caller asked it to. Chunks already
/// committed stay (newer-wins makes the rerun a no-op over them); durable
/// PRAGMAs are restored before this is returned.
#[derive(Debug)]
pub struct HydrationCancelled;

impl std::fmt::Display for HydrationCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "hydration cancelled")
    }
}

impl std::error::Error for HydrationCancelled {}

/// Where a hydration is, reported as it goes: stations written so far out
/// of the snapshot's station count, and rows so far. Reported after every
/// [`HYDRATE_PROGRESS_EVERY`] stations and once at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HydrationProgress {
    pub stations_done: u64,
    pub stations_total: u64,
    pub rows: u64,
}

pub const HYDRATE_PROGRESS_EVERY: u64 = 256;
/// Boards per transaction. One transaction for a whole baseline meant a
/// write-ahead log the size of the database (a 99 M-row first sync wrote
/// 7 GB of WAL before it was interrupted) and everything written twice.
/// Committing every so many stations keeps the WAL small; an interrupted
/// hydration is safe to rerun because applied boards are equal-or-older
/// on the rerun and skipped by the freshness rule.
pub const HYDRATE_COMMIT_EVERY: u64 = 2_000;

/// [`hydrate_ebex`] with a progress callback. The market section is
/// grouped by station in the artifact (the publisher orders it), so boards
/// are applied one at a time as the records stream past: memory is one
/// board, not the 100 M rows of a full baseline, and the caller hears how
/// far along the write is.
pub fn hydrate_ebex_with(
    conn: &Connection,
    bytes: &[u8],
    progress: &mut dyn FnMut(HydrationProgress),
) -> Result<HydrationStats> {
    // This walks every market record; on a ~100M-row snapshot it is
    // minutes of CPU. Timed so the "Loading markets… starting" stretch
    // has a number on it (2026-09-04: ~18 min of silent 'starting' turned
    // out to be validate passes, PLURAL — exchange validated the mmap and
    // this validated it again; callers that already validated take
    // [`hydrate_prevalidated_with`] and pay once).
    let validate_started = std::time::Instant::now();
    let metadata = ed_ebex::validate_snapshot(bytes)?;
    tracing::info!(
        elapsed_s = validate_started.elapsed().as_secs_f64(),
        "hydrate: snapshot validated"
    );
    hydrate_prevalidated_with(conn, bytes, &metadata, progress, &|| false)
}

/// [`hydrate_ebex_with`] for bytes the caller has ALREADY passed through
/// `ed_ebex::validate_snapshot` (the same buffer, typically the mmap the
/// sync validated after decompressing to disk) — the second minutes-long
/// validate pass is the one this skips. `cancelled` is polled at chunk
/// commit boundaries: a cancelled hydrate returns [`HydrationCancelled`]
/// with its committed chunks kept (the rerun is a no-op over them by the
/// newer-wins rule), and durable PRAGMAs are restored on EVERY exit,
/// error and cancel included.
pub fn hydrate_prevalidated_with(
    conn: &Connection,
    bytes: &[u8],
    metadata: &ed_ebex::SnapshotMetadata,
    progress: &mut dyn FnMut(HydrationProgress),
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<HydrationStats> {
    // ONE directory parse: `sections()`/`section()` re-run the full
    // validate walk internally, which is exactly what this entry point
    // exists to avoid (review finding, 2026-09-04).
    let sections = ed_ebex::sections_prevalidated(bytes)?;
    for candidate in &sections {
        ensure!(
            candidate.id == ed_ebex::SECTION_MARKETS || !candidate.required,
            "this EDDA version does not support required EBEX section {} schema {}",
            candidate.id,
            candidate.schema
        );
    }
    // F4a (2026-09-04): the baseline has always CARRIED system and
    // station identity sections; the client never read them, leaving a
    // fresh install with boards for 657k stations and identity for 3.7k
    // (the measured split-brain). Identity goes in FIRST so the market
    // rows join to real stations from the first search.
    hydrate_identity(conn, &sections)?;
    hydrate_station_details(conn, &sections)?;
    hydrate_prohibited_section(conn, &sections)?;
    let section = sections
        .into_iter()
        .find(|section| section.id == ed_ebex::SECTION_MARKETS)
        .context("EBEX has no market section")?;
    let aux_started = std::time::Instant::now();
    let auxiliary = ed_ebex::market_auxiliary(section)?;
    tracing::info!(
        elapsed_s = aux_started.elapsed().as_secs_f64(),
        commodities = auxiliary.commodities.len(),
        "hydrate: auxiliary tables parsed"
    );

    // A baseline is a bulk load: durability per statement is not worth its
    // fsyncs, and an interrupted load is redone by the next sync anyway.
    // The default wal_autocheckpoint (1,000 pages ≈ 4 MB) checkpoints
    // mid-insert constantly — measured 2026-09-03 on a fresh install:
    // 200 MB/s of ~1-2 KB logical writes collapsing to 45 MB/s at the
    // disk, queue length 0, one core pegged. 25,000 pages (~100 MB)
    // keeps the WAL near the measured 390 MB baseline shape while
    // checkpointing in sequential bulk; the page cache stops index
    // maintenance from thrashing 4 KB reads. Both restored below.
    let _ = conn.execute_batch(
        "PRAGMA galaxy.synchronous = OFF;\n         PRAGMA wal_autocheckpoint = 25000;\n         PRAGMA galaxy.cache_size = -262144;",
    );
    let hydrate_started = std::time::Instant::now();
    // Everything between the durability PRAGMAs above and their restore
    // below runs in this closure, so an error or a cancellation can never
    // leave the connection on synchronous=OFF (a latent bug until the
    // cancel path made it reachable on purpose).
    let result = (|| -> Result<HydrationStats> {
        let mut tx = conn.unchecked_transaction()?;
        let mut commodity_ids = HashMap::with_capacity(auxiliary.commodities.len());
        for commodity in &auxiliary.commodities {
            let id = intern_commodity(
                &tx,
                &commodity.symbol,
                Some(&commodity.name),
                Some(&commodity.category),
            )?;
            commodity_ids.insert(commodity.id, id);
        }

        // Every station with a snapshot, in station order, with its freshness.
        // Records come grouped by station in the same order (the format
        // requires it and validate_snapshot checked it), so a board is
        // complete when the station id changes; stations with no rows at all
        // are empty snapshots and are written as the stream passes them.
        let snapshots: BTreeMap<i64, i64> = auxiliary
            .stations
            .iter()
            .map(|s| Ok((i64::try_from(s.station_id)?, s.observed_at)))
            .collect::<Result<_>>()?;
        let mut stats = HydrationStats {
            commodities: auxiliary.commodities.len() as u64,
            station_snapshots: auxiliary.stations.len() as u64,
            ..HydrationStats::default()
        };
        let stations_total = auxiliary.stations.len() as u64;
        let mut stations_done: u64 = 0;
        let mut pending = snapshots.iter().peekable();
        let mut current: Option<(i64, i64, Vec<MarketRow>)> = None;
        // Resume: everything at or below the checkpointed station id was
        // committed by an interrupted run of this exact artifact.
        let skip_through = read_checkpoint(conn, metadata);
        let mut high_water: i64 = i64::MIN;
        if let Some(skip) = skip_through {
            let skipped = snapshots.range(..=skip).count() as u64;
            stations_done = skipped;
            stats.stations_fast_forwarded = skipped;
            high_water = skip;
            while pending.peek().is_some_and(|(&id, _)| id <= skip) {
                pending.next();
            }
            tracing::info!(
                through = skip,
                stations = skipped,
                "hydrate: resuming from checkpoint"
            );
            progress(HydrationProgress {
                stations_done,
                stations_total,
                rows: 0,
            });
        }
        let report = |stations_done: u64,
                      rows: u64,
                      force: bool,
                      progress: &mut dyn FnMut(HydrationProgress)| {
            if force || stations_done.is_multiple_of(HYDRATE_PROGRESS_EVERY) {
                progress(HydrationProgress {
                    stations_done,
                    stations_total,
                    rows,
                });
            }
        };
        let flush = |tx: &Connection,
                     board: Option<(i64, i64, Vec<MarketRow>)>,
                     stats: &mut HydrationStats|
         -> Result<()> {
            if let Some((station_id, observed_at, rows)) = board {
                match write_snapshot(tx, station_id, observed_at, &rows)? {
                    SnapshotOutcome::Applied { removed, .. } => stats.removed += removed,
                    SnapshotOutcome::Skipped => stats.stations_skipped += 1,
                }
            }
            Ok(())
        };
        for record in ed_ebex::market_records(section)? {
            let station_id = i64::try_from(record.station_id)?;
            if skip_through.is_some_and(|skip| station_id <= skip) {
                continue;
            }
            let observed_at = *snapshots
                .get(&station_id)
                .context("EBEX market row references a station without snapshot freshness")?;
            ensure!(
                record.observed_at == observed_at,
                "EBEX market row freshness differs from its station snapshot"
            );
            if current.as_ref().is_some_and(|(id, _, _)| *id != station_id) {
                // The board before this one is complete, and so is every
                // station between the two that had no rows (empty boards).
                if let Some((flushed, _, _)) = current.as_ref() {
                    high_water = *flushed;
                }
                flush(&tx, current.take(), &mut stats)?;
                stations_done += 1;
                report(stations_done, stats.rows, false, progress);
                if stations_done.is_multiple_of(HYDRATE_COMMIT_EVERY) {
                    stamp_checkpoint(&tx, high_water, metadata)?;
                    tx.commit()?;
                    if cancelled() {
                        return Err(HydrationCancelled.into());
                    }
                    tx = conn.unchecked_transaction()?;
                }
            }
            while let Some((&id, &at)) = pending.peek() {
                if id >= station_id {
                    break;
                }
                pending.next();
                if current.as_ref().is_none_or(|(cur, _, _)| *cur != id) {
                    flush(&tx, Some((id, at, Vec::new())), &mut stats)?;
                    high_water = id;
                    stations_done += 1;
                    report(stations_done, stats.rows, false, progress);
                    if stations_done.is_multiple_of(HYDRATE_COMMIT_EVERY) {
                        stamp_checkpoint(&tx, high_water, metadata)?;
                        tx.commit()?;
                        if cancelled() {
                            return Err(HydrationCancelled.into());
                        }
                        tx = conn.unchecked_transaction()?;
                    }
                }
            }
            if pending.peek().is_some_and(|(&id, _)| id == station_id) {
                pending.next();
            }
            let commodity_id = *commodity_ids
                .get(&record.commodity_id)
                .context("EBEX market row references unknown commodity")?;
            let board = current.get_or_insert_with(|| (station_id, observed_at, Vec::new()));
            board.2.push(MarketRow {
                commodity_id,
                buy_price: i64::from(record.buy_price),
                sell_price: i64::from(record.sell_price),
                demand: i64::from(record.demand),
                supply: i64::from(record.supply),
            });
            stats.rows += 1;
        }
        if current.is_some() {
            flush(&tx, current.take(), &mut stats)?;
            stations_done += 1;
            report(stations_done, stats.rows, false, progress);
        }
        // Stations after the last row: empty boards.
        for (&id, &at) in pending {
            flush(&tx, Some((id, at, Vec::new())), &mut stats)?;
            stations_done += 1;
            report(stations_done, stats.rows, false, progress);
        }
        report(stations_done, stats.rows, true, progress);

        for (key, value) in [
            ("ebex_sequence", i64::try_from(metadata.sequence)?),
            ("ebex_watermark", metadata.watermark),
        ] {
            tx.execute(
                "INSERT INTO sys_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        // A completed pass owes no resume cursor; a stale one must never
        // fast-forward a FUTURE artifact that happens to share identity.
        tx.execute(
        "DELETE FROM sys_meta WHERE key IN ('hydrate_ckpt_station','hydrate_ckpt_seq','hydrate_ckpt_wm')",
        [],
    )?;
        tx.commit()?;
        Ok(stats)
    })();
    // Restore durable settings and fold the WAL back in one sequential
    // pass, so steady-state EDDN writes resume on safe defaults — on
    // success, on error, and on cancellation alike.
    let _ = conn.execute_batch(
        "PRAGMA galaxy.synchronous = NORMAL;\n         PRAGMA wal_autocheckpoint = 1000;\n         PRAGMA galaxy.cache_size = -2000;\n         PRAGMA galaxy.wal_checkpoint(TRUNCATE);",
    );
    let elapsed = hydrate_started.elapsed().as_secs_f64();
    let outcome = match &result {
        Ok(_) => "complete",
        Err(error) if error.downcast_ref::<HydrationCancelled>().is_some() => "cancelled",
        Err(_) => "error",
    };
    // The phase boundary is traced on EVERY exit — doctrine rule 2; a
    // cancelled or failed pass still says how long it ran.
    tracing::info!(elapsed_s = elapsed, outcome, "hydrate: write pass ended");
    let stats = result?;
    tracing::info!(
        elapsed_s = elapsed,
        stations = stats.station_snapshots,
        rows = stats.rows,
        rows_per_s = (stats.rows as f64 / elapsed.max(0.001)) as u64,
        "hydrate: write pass complete"
    );
    // Stats deliberately NOT refreshed here (maintainer ruling 2026-09-05):
    // the sweep scales with the whole table — 80 s at 6M rows, 10+ min
    // at 100M — and the finder tolerates minutes-stale stats. The sync
    // orchestrator defers it behind completion and skips small deltas.
    Ok(stats)
}

/// Addendum section 10 (2026-09-04): pads, carrier flag, black-market
/// presence, arrival distance and type for stations the server's
/// Docked-event ingest has learned. Applied over identity (which ran
/// first), never downgrading a locally learned carrier flag; a
/// malformed or newer-schema optional section is skipped with a warn,
/// never an error — old artifacts simply do not carry it.
fn hydrate_station_details(conn: &Connection, sections: &[ed_ebex::SectionRef<'_>]) -> Result<u64> {
    let Some(section) = sections
        .iter()
        .find(|s| s.id == ed_ebex::SECTION_STATION_DETAILS)
        .copied()
    else {
        return Ok(0);
    };
    let records = match ed_ebex::station_details_records(section) {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "station details section skipped");
            return Ok(0);
        }
    };
    let types: HashMap<u32, String> = match ed_ebex::string_table(section) {
        Ok(table) => table.into_iter().map(|s| (s.id, s.value)).collect(),
        Err(error) => {
            tracing::warn!(%error, "station details string table skipped");
            return Ok(0);
        }
    };
    let started = std::time::Instant::now();
    let tx = conn.unchecked_transaction()?;
    let mut applied = 0u64;
    {
        let mut update = tx.prepare_cached(
            "UPDATE sys_stations SET
                 pad_small = CASE WHEN ?2 THEN ?3 ELSE pad_small END,
                 pad_medium = CASE WHEN ?2 THEN ?4 ELSE pad_medium END,
                 pad_large = CASE WHEN ?2 THEN ?5 ELSE pad_large END,
                 is_carrier = MAX(?6, is_carrier),
                 distance_to_arrival = CASE WHEN ?7 THEN ?8 ELSE distance_to_arrival END,
                 type = COALESCE(?9, type)
             WHERE id = ?1",
        )?;
        let mut service = tx.prepare_cached(
            "INSERT OR IGNORE INTO sys_station_services (station_id, service) VALUES (?1, 'blackmarket')",
        )?;
        for record in records {
            let has_pads = record.flags & ed_ebex::StationDetailsRecord::HAS_PADS != 0;
            let has_arrival = record.flags & ed_ebex::StationDetailsRecord::HAS_ARRIVAL != 0;
            let is_carrier = record.flags & ed_ebex::StationDetailsRecord::IS_CARRIER != 0;
            let changed = update.execute(params![
                record.station_id as i64,
                has_pads,
                record.pad_small as i64,
                record.pad_medium as i64,
                record.pad_large as i64,
                is_carrier as i64,
                has_arrival,
                record.arrival_ls as f64,
                types.get(&record.type_id).map(String::as_str),
            ])?;
            if changed > 0 && record.flags & ed_ebex::StationDetailsRecord::HAS_BLACK_MARKET != 0 {
                service.execute([record.station_id as i64])?;
            }
            applied += changed as u64;
        }
    }
    tx.commit()?;
    tracing::info!(
        elapsed_s = started.elapsed().as_secs_f64(),
        stations = applied,
        "hydrate: station details applied"
    );
    Ok(applied)
}

/// Addendum section 11 (2026-09-04): confiscation pairs, resolved
/// through the artifact's own commodity catalog and stored as interned
/// symbols (readers match symbol OR name). Stations present in the
/// section have their list replaced wholesale; absent stations keep
/// what they have.
fn hydrate_prohibited_section(
    conn: &Connection,
    sections: &[ed_ebex::SectionRef<'_>],
) -> Result<u64> {
    let Some(section) = sections
        .iter()
        .find(|s| s.id == ed_ebex::SECTION_PROHIBITED)
        .copied()
    else {
        return Ok(0);
    };
    let records = match ed_ebex::prohibited_records(section) {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(%error, "prohibited section skipped");
            return Ok(0);
        }
    };
    let Some(catalog) = sections
        .iter()
        .find(|s| s.id == ed_ebex::SECTION_COMMODITIES)
        .copied()
    else {
        tracing::warn!("prohibited section without a commodity catalog; skipped");
        return Ok(0);
    };
    let strings: HashMap<u32, String> = ed_ebex::string_table(catalog)?
        .into_iter()
        .map(|s| (s.id, s.value))
        .collect();
    let symbols: HashMap<u32, &str> = ed_ebex::commodity_records(catalog)?
        .filter_map(|record| {
            Some((
                u32::from(record.id),
                strings.get(&record.symbol_id)?.as_str(),
            ))
        })
        .collect();
    let started = std::time::Instant::now();
    let tx = conn.unchecked_transaction()?;
    let mut pairs = 0u64;
    {
        let mut clear =
            tx.prepare_cached("DELETE FROM sys_market_prohibited WHERE station_id = ?1")?;
        let mut insert = tx.prepare_cached(
            "INSERT OR IGNORE INTO sys_market_prohibited (station_id, symbol) VALUES (?1, ?2)",
        )?;
        let mut current: Option<u64> = None;
        for record in records {
            if current != Some(record.station_id) {
                clear.execute([record.station_id as i64])?;
                current = Some(record.station_id);
            }
            if let Some(symbol) = symbols.get(&record.commodity_id) {
                insert.execute(params![record.station_id as i64, symbol])?;
                pairs += 1;
            }
        }
    }
    tx.commit()?;
    tracing::info!(
        elapsed_s = started.elapsed().as_secs_f64(),
        pairs,
        "hydrate: confiscation pairs applied"
    );
    Ok(pairs)
}

/// Apply the baseline's system and station identity sections when
/// present (a rolling daily may not carry them yet): names, coordinates,
/// population, power data, service flags. Pads, carrier flags, arrival
/// distances and station types are NOT in these sections — the ledgered
/// EBEX addendum adds them; until then `is_carrier` is seeded from the
/// callsign heuristic and never downgraded. Existing rows only gain:
/// identity fields update, better-known values are never blanked, and
/// `updated` never regresses.
fn hydrate_identity(conn: &Connection, sections: &[ed_ebex::SectionRef<'_>]) -> Result<(u64, u64)> {
    let find = |id: u16| sections.iter().find(|s| s.id == id).copied();
    let (Some(systems), Some(stations)) = (
        find(ed_ebex::SECTION_SYSTEMS),
        find(ed_ebex::SECTION_STATIONS),
    ) else {
        return Ok((0, 0));
    };
    let started = std::time::Instant::now();
    let tx = conn.unchecked_transaction()?;
    let mut n_systems = 0u64;
    {
        let strings: HashMap<u32, String> = ed_ebex::string_table(systems)?
            .into_iter()
            .map(|s| (s.id, s.value))
            .collect();
        let lookup = |id: u32| {
            if id == 0 {
                None
            } else {
                strings.get(&id).cloned()
            }
        };
        let mut upsert = tx.prepare_cached(
            "INSERT INTO sys_systems (id64, name, x, y, z, population, controlling_power, power_state, powers)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id64) DO UPDATE SET
                 name = excluded.name,
                 x = COALESCE(excluded.x, sys_systems.x),
                 y = COALESCE(excluded.y, sys_systems.y),
                 z = COALESCE(excluded.z, sys_systems.z),
                 population = COALESCE(excluded.population, sys_systems.population),
                 controlling_power = COALESCE(excluded.controlling_power, sys_systems.controlling_power),
                 power_state = COALESCE(excluded.power_state, sys_systems.power_state),
                 powers = COALESCE(excluded.powers, sys_systems.powers)",
        )?;
        for record in ed_ebex::system_records(systems)? {
            let name = lookup(record.name_id).context("system name does not resolve")?;
            let coords = record.flags & ed_ebex::SystemRecord::HAS_COORDINATES != 0;
            let population = (record.flags & ed_ebex::SystemRecord::HAS_POPULATION != 0)
                .then_some(record.population as i64);
            upsert.execute(params![
                record.address,
                name,
                coords.then_some(record.x),
                coords.then_some(record.y),
                coords.then_some(record.z),
                population,
                lookup(record.controlling_power_id),
                lookup(record.power_state_id),
                lookup(record.powers_id),
            ])?;
            n_systems += 1;
        }
    }
    let mut n_stations = 0u64;
    {
        let strings: HashMap<u32, String> = ed_ebex::string_table(stations)?
            .into_iter()
            .map(|s| (s.id, s.value))
            .collect();
        let mut upsert = tx.prepare_cached(
            "INSERT INTO sys_stations (id, system_id64, name, has_market, has_outfitting, has_shipyard, is_carrier, updated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 system_id64 = excluded.system_id64,
                 name = excluded.name,
                 has_market = MAX(excluded.has_market, sys_stations.has_market),
                 has_outfitting = MAX(excluded.has_outfitting, sys_stations.has_outfitting),
                 has_shipyard = MAX(excluded.has_shipyard, sys_stations.has_shipyard),
                 is_carrier = MAX(excluded.is_carrier, sys_stations.is_carrier),
                 updated = MAX(COALESCE(excluded.updated, 0), COALESCE(sys_stations.updated, 0))",
        )?;
        for record in ed_ebex::station_records(stations)? {
            let name = strings
                .get(&record.name_id)
                .context("station name does not resolve")?
                .clone();
            let is_carrier = ed_domain::station::is_carrier_callsign(&name);
            upsert.execute(params![
                record.id as i64,
                record.system_address,
                name,
                (record.flags & ed_ebex::StationRecord::HAS_MARKET != 0) as i64,
                (record.flags & ed_ebex::StationRecord::HAS_OUTFITTING != 0) as i64,
                (record.flags & ed_ebex::StationRecord::HAS_SHIPYARD != 0) as i64,
                is_carrier as i64,
                (record.market_observed_at != 0).then_some(record.market_observed_at),
            ])?;
            n_stations += 1;
        }
    }
    tx.commit()?;
    tracing::info!(
        elapsed_s = started.elapsed().as_secs_f64(),
        systems = n_systems,
        stations = n_stations,
        "hydrate: identity sections applied"
    );
    Ok((n_systems, n_stations))
}

/// Two passes over the market per hydrate, best effort — a failed
/// refresh keeps yesterday's stats and the guards keep working; missing
/// stats mean the guards never engage (the fail-open the readers handle
/// via the boards floors).
///
/// Pass 1: per-commodity mean sell over ALL boards. Pass 2: the STATION
/// price envelope — min/max/std of non-carrier boards, spikes included
/// (real demand spikes are real prices; see the 2026-09-04 retraction).
/// Carriers are then only considered within [min - k·std, max + k·std]
/// of these (maintainer rule 2026-09-04). On a fresh install with no station
/// identity every board counts as a station until the identity section
/// lands — the envelope inflates toward fail-open, never toward hiding
/// data.
pub fn refresh_commodity_stats(conn: &Connection) {
    let started = std::time::Instant::now();
    let refreshed = (|| -> Result<()> {
        conn.execute_batch(
            "INSERT OR REPLACE INTO sys_commodity_stats (commodity_id, mean_sell, boards)
             SELECT commodity_id, AVG(sell_price), COUNT(*)
             FROM sys_market WHERE sell_price > 0 GROUP BY commodity_id",
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT m.commodity_id,
                    MIN(CASE WHEN m.sell_price > 0 THEN m.sell_price END),
                    MAX(CASE WHEN m.sell_price > 0 THEN m.sell_price END),
                    AVG(CASE WHEN m.sell_price > 0 THEN CAST(m.sell_price AS REAL) END),
                    AVG(CASE WHEN m.sell_price > 0 THEN CAST(m.sell_price AS REAL) * m.sell_price END),
                    MIN(CASE WHEN m.buy_price > 0 THEN m.buy_price END),
                    MAX(CASE WHEN m.buy_price > 0 THEN m.buy_price END),
                    AVG(CASE WHEN m.buy_price > 0 THEN CAST(m.buy_price AS REAL) END),
                    AVG(CASE WHEN m.buy_price > 0 THEN CAST(m.buy_price AS REAL) * m.buy_price END),
                    COUNT(*)
             FROM sys_market m
             JOIN sys_stations st ON st.id = m.station_id AND {non_carrier}
             GROUP BY m.commodity_id",
            non_carrier = crate::lookup::SQL_ST_NON_CARRIER,
        ))?;
        let envelopes: Vec<(i64, [Option<f64>; 8], i64)> = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    [
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                        r.get(8)?,
                    ],
                    r.get::<_, i64>(9)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let std_of = |mean: Option<f64>, mean_sq: Option<f64>| -> Option<f64> {
            Some((mean_sq? - mean? * mean?).max(0.0).sqrt())
        };
        let mut update = conn.prepare_cached(
            "UPDATE sys_commodity_stats SET
                 station_sell_min = ?2, station_sell_max = ?3, station_sell_std = ?4,
                 station_buy_min = ?5, station_buy_max = ?6, station_buy_std = ?7,
                 station_boards = ?8
             WHERE commodity_id = ?1",
        )?;
        for (id, [sell_min, sell_max, sell_mean, sell_sq, buy_min, buy_max, buy_mean, buy_sq], n) in
            envelopes
        {
            update.execute(params![
                id,
                sell_min,
                sell_max,
                std_of(sell_mean, sell_sq),
                buy_min,
                buy_max,
                std_of(buy_mean, buy_sq),
                n
            ])?;
        }
        Ok(())
    })();
    match refreshed {
        Ok(()) => tracing::info!(
            elapsed_s = started.elapsed().as_secs_f64(),
            "hydrate: commodity price stats refreshed"
        ),
        Err(error) => {
            tracing::warn!(%error, "commodity price stats not refreshed; guards use prior values")
        }
    }
}

/// Statistics floors: a young install must not filter on a sample of
/// five. (A "poisoned board" silencer briefly lived here on 2026-09-04
/// — 999999 demand at >3x the mean — until the maintainer's Inara cross-check
/// proved its flagship match was a REAL demand spike the reference tool
/// also shows. Station prices are shown as reported; the ledger keeps
/// the retraction. This floor survives for the carrier envelope.)
pub const STATS_MIN_BOARDS: i64 = 100;

/// The carrier envelope rule (maintainer, 2026-09-04): a CARRIER price is only
/// considered within `CARRIER_ENVELOPE_STD` standard deviations of the
/// extreme STATION prices for the commodity — carriers set prices freely
/// (legally up to 10× galactic average), and anything outside the
/// station envelope is bait, a museum piece, or a restricted-access
/// listing no route should be built on. "Maybe less" than 1.0 is the
/// maintainer's own note: the factor is a knob to bench, not a law of nature.
pub const CARRIER_ENVELOPE_STD: f64 = 1.0;

/// Per-commodity price statistics as one row of `sys_commodity_stats`.
/// `station_boards == 0` (or below the poison floor) disables the
/// carrier envelope; `boards` below the floor disables the poison guard.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CommodityStats {
    pub mean_sell: f64,
    pub boards: i64,
    pub station_sell_min: f64,
    pub station_sell_max: f64,
    pub station_sell_std: f64,
    pub station_buy_min: f64,
    pub station_buy_max: f64,
    pub station_buy_std: f64,
    pub station_boards: i64,
}

impl CommodityStats {
    fn envelope_ready(&self) -> bool {
        self.station_boards >= STATS_MIN_BOARDS
    }
    /// Is a carrier's SELL price (what it pays the commander) outside
    /// the station sell envelope?
    pub fn carrier_sell_outside(&self, sell_price: i64) -> bool {
        self.envelope_ready() && {
            let p = sell_price as f64;
            p > self.station_sell_max + CARRIER_ENVELOPE_STD * self.station_sell_std
                || p < self.station_sell_min - CARRIER_ENVELOPE_STD * self.station_sell_std
        }
    }
    /// Is a carrier's BUY price (what the commander pays it) outside the
    /// station buy envelope?
    pub fn carrier_buy_outside(&self, buy_price: i64) -> bool {
        self.envelope_ready() && {
            let p = buy_price as f64;
            p > self.station_buy_max + CARRIER_ENVELOPE_STD * self.station_buy_std
                || p < self.station_buy_min - CARRIER_ENVELOPE_STD * self.station_buy_std
        }
    }
}

/// The guards' inputs, loaded once per search and keyed by the interned
/// SYMBOL (the profit finder's row key). Small — one row per commodity.
pub fn commodity_stats_by_symbol(
    conn: &Connection,
) -> Result<std::collections::HashMap<String, CommodityStats>> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.symbol, s.mean_sell, s.boards,
                COALESCE(s.station_sell_min, 0), COALESCE(s.station_sell_max, 0),
                COALESCE(s.station_sell_std, 0), COALESCE(s.station_buy_min, 0),
                COALESCE(s.station_buy_max, 0), COALESCE(s.station_buy_std, 0),
                COALESCE(s.station_boards, 0)
         FROM sys_commodity_stats s JOIN sys_commodities c ON c.id = s.commodity_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            CommodityStats {
                mean_sell: r.get(1)?,
                boards: r.get(2)?,
                station_sell_min: r.get(3)?,
                station_sell_max: r.get(4)?,
                station_sell_std: r.get(5)?,
                station_buy_min: r.get(6)?,
                station_buy_max: r.get(7)?,
                station_buy_std: r.get(8)?,
                station_boards: r.get(9)?,
            },
        ))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Commodities a station confiscates, resolved to interned SYMBOLS. The
/// table stores lowercased display names (the Spansh dump's format —
/// measured 2026-09-04: 19 of 19 distinct values join on LOWER(name),
/// only 7 join on symbol), so the join translates here and callers
/// compare like with like.
pub fn prohibited_symbols(
    conn: &Connection,
    station_id: i64,
) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT c.symbol FROM sys_market_prohibited p
         JOIN sys_commodities c ON LOWER(c.name) = p.symbol OR c.symbol = p.symbol
         WHERE p.station_id = ?1",
    )?;
    let rows = stmt.query_map([station_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Does the station offer a black-market contact? The gate for the
/// prohibited-goods opt-in: with one, selling confiscated goods is a
/// choice the commander can make (maintainer, 2026-09-04); without one the
/// sale is impossible and stays hidden regardless of the toggle.
pub fn has_black_market(conn: &Connection, station_id: i64) -> Result<bool> {
    // Two wire spellings, one meaning: the Spansh ingest stores the
    // display name, the Docked-event ingest stores the journal token.
    let mut stmt = conn.prepare_cached(
        "SELECT 1 FROM sys_station_services WHERE station_id = ?1 AND service IN (?2, 'blackmarket')",
    )?;
    Ok(stmt.exists(params![station_id, crate::lookup::BLACK_MARKET_SERVICE])?)
}

// ── Readers ─────────────────────────────────────────────────────────

/// A priced row as the profit finder wants it: symbol resolved, prices
/// defaulted, observation time as an epoch (age is the caller's, since it
/// knows its `now`).
#[derive(Debug, Clone, PartialEq)]
pub struct PricedRow {
    pub station_id: i64,
    pub symbol: String,
    pub name: Option<String>,
    pub buy_price: i64,
    pub sell_price: i64,
    pub demand: i64,
    pub supply: i64,
    pub updated: Option<i64>,
}

const PRICED_COLUMNS: &str = "m.station_id, c.symbol, c.name,
     COALESCE(m.buy_price, 0), COALESCE(m.sell_price, 0),
     COALESCE(m.demand, 0), COALESCE(m.supply, 0), m.updated";

fn priced_row(r: &rusqlite::Row) -> rusqlite::Result<PricedRow> {
    Ok(PricedRow {
        station_id: r.get(0)?,
        symbol: r.get(1)?,
        name: r.get(2)?,
        buy_price: r.get(3)?,
        sell_price: r.get(4)?,
        demand: r.get(5)?,
        supply: r.get(6)?,
        updated: r.get(7)?,
    })
}

/// Every row on one station's board.
pub fn rows_for_station(conn: &Connection, station_id: i64) -> Result<Vec<PricedRow>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {PRICED_COLUMNS} FROM sys_market m
         JOIN sys_commodities c ON c.id = m.commodity_id
         WHERE m.station_id = ?1"
    ))?;
    let rows = stmt.query_map([station_id], priced_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Whether [`scan_rows_since`] can use the `updated` index. Without it a
/// galaxy-wide scan is a full table walk; callers should stay per-station.
pub fn wide_scan_available(conn: &Connection) -> bool {
    crate::schema::has_market_index(conn)
}

/// Walk every market row observed at or after `cutoff` (epoch seconds),
/// galaxy-wide, in one indexed pass. `visit` returns `false` to stop early.
pub fn scan_rows_since(
    conn: &Connection,
    cutoff: i64,
    mut visit: impl FnMut(PricedRow) -> Result<bool>,
) -> Result<()> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {PRICED_COLUMNS} FROM sys_market m INDEXED BY idx_mkt_updated
         JOIN sys_commodities c ON c.id = m.commodity_id
         WHERE m.updated >= ?1"
    ))?;
    let rows = stmt.query_map([cutoff], priced_row)?;
    for row in rows {
        if !visit(row?)? {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO sys_systems (id64, name) VALUES (1, 'Alpha');
             INSERT INTO sys_stations (id, system_id64, name, has_market) VALUES (7, 1, 'Port', 1);
             INSERT INTO sys_commodities (id, symbol) VALUES (10, 'gold'), (11, 'silver');",
        )
        .unwrap();
        conn
    }

    fn string_table_aux(entries: &[(u32, &str)]) -> Vec<u8> {
        let mut aux = (entries.len() as u32).to_le_bytes().to_vec();
        for (id, value) in entries {
            aux.extend_from_slice(&id.to_le_bytes());
            aux.extend_from_slice(&(value.len() as u32).to_le_bytes());
            aux.extend_from_slice(value.as_bytes());
        }
        aux
    }

    /// F4a: a baseline carrying identity sections populates sys_systems
    /// and sys_stations — names, coords, service flags — and seeds
    /// is_carrier from the callsign heuristic, so the market rows join to
    /// real stations from the first search. A daily without the sections
    /// hydrates markets exactly as before (ebex_many carries none and
    /// every older test still passes).
    #[test]
    fn identity_sections_hydrate_systems_and_stations() {
        let conn = db();
        let mut sys_records = Vec::new();
        ed_ebex::SystemRecord {
            address: 42,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            population: 5000,
            observed_at: 100,
            name_id: 1,
            security_id: 0,
            allegiance_id: 0,
            controlling_power_id: 2,
            power_state_id: 0,
            powers_id: 0,
            flags: ed_ebex::SystemRecord::HAS_COORDINATES | ed_ebex::SystemRecord::HAS_POPULATION,
        }
        .encode_into(&mut sys_records);
        let mut st_records = Vec::new();
        for (id, name_id) in [(900u64, 1u32), (901, 2)] {
            ed_ebex::StationRecord {
                id,
                system_address: 42,
                name_id,
                flags: ed_ebex::StationRecord::HAS_MARKET,
                market_observed_at: 1_000,
                outfitting_observed_at: 0,
                shipyard_observed_at: 0,
            }
            .encode_into(&mut st_records);
        }
        let mut market_records = Vec::new();
        ed_ebex::MarketRecord {
            station_id: 900,
            commodity_id: 1,
            buy_price: 5,
            sell_price: 100,
            demand: 7,
            supply: 8,
            observed_at: 1_000,
        }
        .encode_into(&mut market_records);
        let mut market_aux = Vec::new();
        market_aux.extend_from_slice(&1u16.to_le_bytes());
        market_aux.extend_from_slice(&1u16.to_le_bytes());
        market_aux.extend_from_slice(&(4u16).to_le_bytes());
        market_aux.extend_from_slice(&0u16.to_le_bytes());
        market_aux.extend_from_slice(&0u16.to_le_bytes());
        market_aux.extend_from_slice(b"gold");
        market_aux.extend_from_slice(&1u64.to_le_bytes());
        market_aux.extend_from_slice(&900u64.to_le_bytes());
        market_aux.extend_from_slice(&1_000i64.to_le_bytes());
        // The addendum sections (10 + 11): details for both stations and
        // one confiscation pair, resolved through the commodity catalog.
        let mut catalog_records = Vec::new();
        ed_ebex::CommodityCatalogRecord {
            id: 1,
            symbol_id: 1,
            name_id: 2,
            category_id: 3,
        }
        .encode_into(&mut catalog_records);
        let mut details_records = Vec::new();
        ed_ebex::StationDetailsRecord {
            station_id: 900,
            flags: ed_ebex::StationDetailsRecord::HAS_PADS
                | ed_ebex::StationDetailsRecord::HAS_ARRIVAL
                | ed_ebex::StationDetailsRecord::HAS_BLACK_MARKET,
            pad_small: 4,
            pad_medium: 4,
            pad_large: 2,
            arrival_ls: 350.5,
            type_id: 1,
        }
        .encode_into(&mut details_records);
        ed_ebex::StationDetailsRecord {
            station_id: 901,
            flags: ed_ebex::StationDetailsRecord::IS_CARRIER,
            pad_small: 0,
            pad_medium: 0,
            pad_large: 0,
            arrival_ls: 0.0,
            type_id: 0,
        }
        .encode_into(&mut details_records);
        let mut prohibited_bytes = Vec::new();
        ed_ebex::ProhibitedRecord {
            station_id: 900,
            commodity_id: 1,
        }
        .encode_into(&mut prohibited_bytes);
        let bytes = ed_ebex::encode_snapshot(
            ed_ebex::SnapshotHeader {
                sequence: 1,
                created_at: 90,
                watermark: 1_000,
            },
            vec![
                ed_ebex::Section {
                    id: ed_ebex::SECTION_SYSTEMS,
                    schema: ed_ebex::SYSTEM_SCHEMA_V1,
                    required: false,
                    record_count: 1,
                    record_size: ed_ebex::SYSTEM_RECORD_BYTES,
                    records: sys_records,
                    auxiliary: string_table_aux(&[(1, "Wongi"), (2, "Aisling Duval")]),
                },
                ed_ebex::Section {
                    id: ed_ebex::SECTION_STATIONS,
                    schema: ed_ebex::STATION_SCHEMA_V1,
                    required: false,
                    record_count: 2,
                    record_size: ed_ebex::STATION_RECORD_BYTES,
                    records: st_records,
                    auxiliary: string_table_aux(&[(1, "New Port"), (2, "K7F-83H")]),
                },
                ed_ebex::Section {
                    id: ed_ebex::SECTION_COMMODITIES,
                    schema: ed_ebex::COMMODITY_SCHEMA_V1,
                    required: false,
                    record_count: 1,
                    record_size: ed_ebex::COMMODITY_RECORD_BYTES,
                    records: catalog_records,
                    auxiliary: string_table_aux(&[(1, "gold"), (2, "Gold"), (3, "Metals")]),
                },
                ed_ebex::Section {
                    id: ed_ebex::SECTION_MARKETS,
                    schema: ed_ebex::MARKET_SCHEMA_V1,
                    required: true,
                    record_count: 1,
                    record_size: ed_ebex::MARKET_RECORD_BYTES,
                    records: market_records,
                    auxiliary: market_aux,
                },
                ed_ebex::Section {
                    id: ed_ebex::SECTION_STATION_DETAILS,
                    schema: ed_ebex::STATION_DETAILS_SCHEMA_V1,
                    required: false,
                    record_count: 2,
                    record_size: ed_ebex::STATION_DETAILS_RECORD_BYTES,
                    records: details_records,
                    auxiliary: string_table_aux(&[(1, "Coriolis Starport")]),
                },
                ed_ebex::Section {
                    id: ed_ebex::SECTION_PROHIBITED,
                    schema: ed_ebex::PROHIBITED_SCHEMA_V1,
                    required: false,
                    record_count: 1,
                    record_size: ed_ebex::PROHIBITED_RECORD_BYTES,
                    records: prohibited_bytes,
                    auxiliary: string_table_aux(&[]),
                },
            ],
        )
        .unwrap();
        let metadata = ed_ebex::validate_snapshot(&bytes).unwrap();
        hydrate_prevalidated_with(&conn, &bytes, &metadata, &mut |_| {}, &|| false).unwrap();
        let (name, x, power): (String, f64, Option<String>) = conn
            .query_row(
                "SELECT name, x, controlling_power FROM sys_systems WHERE id64 = 42",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (name.as_str(), x, power.as_deref()),
            ("Wongi", 1.0, Some("Aisling Duval"))
        );
        let (st_name, has_market, is_carrier, updated): (String, i64, i64, i64) = conn
            .query_row(
                "SELECT name, has_market, is_carrier, updated FROM sys_stations WHERE id = 900",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (st_name.as_str(), has_market, is_carrier, updated),
            ("New Port", 1, 0, 1_000)
        );
        let carrier: i64 = conn
            .query_row(
                "SELECT is_carrier FROM sys_stations WHERE id = 901",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            carrier, 1,
            "the callsign seeds is_carrier where the sections cannot"
        );
        let joined: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sys_market m JOIN sys_stations st ON st.id = m.station_id WHERE st.name = 'New Port'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(joined, 1, "market rows join to hydrated identity");
        // Addendum sections: pads, type, arrival and black-market flow in;
        // the confiscation pair resolves through the catalog to a symbol.
        let (pl, kind, arrival): (i64, String, f64) = conn
            .query_row(
                "SELECT pad_large, type, distance_to_arrival FROM sys_stations WHERE id = 900",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((pl, kind.as_str()), (2, "Coriolis Starport"));
        assert!((arrival - 350.5).abs() < 0.01);
        assert!(has_black_market(&conn, 900).unwrap());
        let confiscated: String = conn
            .query_row(
                "SELECT symbol FROM sys_market_prohibited WHERE station_id = 900",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(confiscated, "gold");
    }

    /// The envelope is built from STATION boards only, and the floors
    /// gate both guards: with three boards the poison row stays in the
    /// envelope (floor unmet — a young install must not filter on a
    /// sample), and the carrier board never enters it at all.
    #[test]
    fn stats_refresh_builds_the_envelope_from_stations_only() {
        let conn = db();
        conn.execute_batch(
            "INSERT INTO sys_stations (id, system_id64, name, has_market, is_carrier)
                 VALUES (8, 1, 'K7F-83H', 1, 1);
             INSERT INTO sys_market (station_id, commodity_id, buy_price, sell_price, demand, supply, updated)
                 VALUES (7, 10, 8000, 9000, 100, 100, unixepoch()),
                        (7, 11, 0, 99000, 999999, 0, unixepoch()),
                        (8, 10, 0, 50000, 500, 0, unixepoch());",
        )
        .unwrap();
        refresh_commodity_stats(&conn);
        let stats = commodity_stats_by_symbol(&conn).unwrap();
        let gold = stats.get("gold").unwrap();
        assert_eq!(
            gold.station_boards, 1,
            "the carrier board is not a station board"
        );
        assert_eq!(
            gold.station_sell_max, 9000.0,
            "envelope max comes from the station"
        );
        assert_eq!(gold.station_buy_min, 8000.0);
        assert!(
            !gold.carrier_sell_outside(50_000),
            "below the boards floor the envelope must not engage"
        );
        let confident = CommodityStats {
            station_boards: 1000,
            ..*gold
        };
        assert!(
            confident.carrier_sell_outside(50_000),
            "with confidence, 50k is out"
        );
        assert!(
            !confident.carrier_sell_outside(9_000),
            "station-priced carrier is fine"
        );
    }

    fn row(commodity_id: i64, sell_price: i64) -> MarketRow {
        MarketRow {
            commodity_id,
            buy_price: 0,
            sell_price,
            demand: 1,
            supply: 0,
        }
    }

    fn board(conn: &Connection, station_id: i64) -> Vec<(String, i64, i64)> {
        conn.prepare(
            "SELECT c.symbol, m.sell_price, m.updated FROM sys_market m
             JOIN sys_commodities c ON c.id = m.commodity_id
             WHERE m.station_id = ?1 ORDER BY c.symbol",
        )
        .unwrap()
        .query_map([station_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    #[test]
    fn a_snapshot_replaces_the_board_and_absent_means_delisted() {
        let conn = db();
        assert_eq!(
            write_snapshot(&conn, 7, 100, &[row(10, 5), row(11, 6)]).unwrap(),
            SnapshotOutcome::Applied {
                removed: 0,
                written: 2
            }
        );
        assert_eq!(
            write_snapshot(&conn, 7, 200, &[row(10, 7)]).unwrap(),
            SnapshotOutcome::Applied {
                removed: 2,
                written: 1
            }
        );
        assert_eq!(board(&conn, 7), vec![("gold".into(), 7, 200)]);
        assert_eq!(watermark(&conn, 7).unwrap(), Some(200));
    }

    #[test]
    fn equal_or_older_snapshots_are_skipped() {
        let conn = db();
        write_snapshot(&conn, 7, 100, &[row(10, 5)]).unwrap();
        assert_eq!(
            write_snapshot(&conn, 7, 100, &[row(10, 9)]).unwrap(),
            SnapshotOutcome::Skipped
        );
        assert_eq!(
            write_snapshot(&conn, 7, 99, &[row(10, 9)]).unwrap(),
            SnapshotOutcome::Skipped
        );
        assert_eq!(board(&conn, 7), vec![("gold".into(), 5, 100)]);
    }

    #[test]
    fn an_empty_snapshot_clears_the_board_and_keeps_its_watermark() {
        let conn = db();
        write_snapshot(&conn, 7, 100, &[row(10, 5)]).unwrap();
        assert_eq!(
            write_snapshot(&conn, 7, 200, &[]).unwrap(),
            SnapshotOutcome::Applied {
                removed: 1,
                written: 0
            }
        );
        assert!(board(&conn, 7).is_empty());
        assert_eq!(watermark(&conn, 7).unwrap(), Some(200));
        assert_eq!(
            write_snapshot(&conn, 7, 150, &[row(11, 1)]).unwrap(),
            SnapshotOutcome::Skipped
        );
    }

    #[test]
    fn rows_written_before_watermarks_existed_still_count_as_stored() {
        let conn = db();
        conn.execute("INSERT INTO sys_market VALUES (7, 10, 0, 5, 1, 0, 100)", [])
            .unwrap();
        assert_eq!(watermark(&conn, 7).unwrap(), Some(100));
        assert_eq!(
            write_snapshot(&conn, 7, 100, &[row(11, 1)]).unwrap(),
            SnapshotOutcome::Skipped
        );
    }

    #[test]
    fn readers_return_epochs_not_strings() {
        let conn = db();
        write_snapshot(&conn, 7, 100, &[row(10, 5)]).unwrap();
        let rows = rows_for_station(&conn, 7).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].symbol, "gold");
        assert_eq!(rows[0].updated, Some(100));

        crate::schema::ensure_market_index(&conn).unwrap();
        let mut seen = Vec::new();
        scan_rows_since(&conn, 100, |r| {
            seen.push(r.station_id);
            Ok(true)
        })
        .unwrap();
        assert_eq!(seen, vec![7]);
        seen.clear();
        scan_rows_since(&conn, 101, |r| {
            seen.push(r.station_id);
            Ok(true)
        })
        .unwrap();
        assert!(seen.is_empty());
    }

    // ── EBEX ────────────────────────────────────────────────────────

    fn market_auxiliary(symbol: &str, station_id: u64, observed_at: i64) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&(symbol.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(symbol.as_bytes());
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&station_id.to_le_bytes());
        bytes.extend_from_slice(&observed_at.to_le_bytes());
        bytes
    }

    /// A snapshot over several stations (all sharing one observation
    /// time): `boards` is (station id, sell prices), rows sorted as the
    /// format requires. A station with no prices is an empty board.
    fn ebex_many(observed_at: i64, boards: &[(u64, &[u32])]) -> Vec<u8> {
        let mut records = Vec::new();
        let mut count = 0u64;
        for (station_id, prices) in boards {
            // Commodities 1 and 2 alternate so a two-price board is two rows.
            for (i, sell_price) in prices.iter().enumerate() {
                ed_ebex::MarketRecord {
                    station_id: *station_id,
                    commodity_id: (i as u16 % 2) + 1,
                    buy_price: 5,
                    sell_price: *sell_price,
                    demand: 7,
                    supply: 8,
                    observed_at,
                }
                .encode_into(&mut records);
                count += 1;
            }
        }
        let mut auxiliary = Vec::new();
        auxiliary.extend_from_slice(&2u16.to_le_bytes());
        for (id, symbol) in [(1u16, &b"gold"[..]), (2u16, &b"silver"[..])] {
            auxiliary.extend_from_slice(&id.to_le_bytes());
            auxiliary.extend_from_slice(&(symbol.len() as u16).to_le_bytes());
            auxiliary.extend_from_slice(&0u16.to_le_bytes());
            auxiliary.extend_from_slice(&0u16.to_le_bytes());
            auxiliary.extend_from_slice(symbol);
        }
        auxiliary.extend_from_slice(&(boards.len() as u64).to_le_bytes());
        for (station_id, _) in boards {
            auxiliary.extend_from_slice(&station_id.to_le_bytes());
            auxiliary.extend_from_slice(&observed_at.to_le_bytes());
        }
        ed_ebex::encode_snapshot(
            ed_ebex::SnapshotHeader {
                sequence: 1,
                created_at: 90,
                watermark: observed_at,
            },
            vec![ed_ebex::Section {
                id: ed_ebex::SECTION_MARKETS,
                schema: ed_ebex::MARKET_SCHEMA_V1,
                required: true,
                record_count: count,
                record_size: ed_ebex::MARKET_RECORD_BYTES,
                records,
                auxiliary,
            }],
        )
        .unwrap()
    }

    /// Cancellation (maintainer ruling 2026-09-04): a cancelled hydrate stops at
    /// a chunk boundary with its committed chunks kept, restores the
    /// durable PRAGMAs (the latent leave-synchronous-OFF bug on every
    /// early exit), and the rerun completes to the same end state.
    #[test]
    fn a_cancelled_hydrate_stops_cleanly_restores_pragmas_and_reruns() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let conn = db();
        let boards: Vec<(u64, &[u32])> = (1..=2_100u64).map(|id| (id, &[100u32][..])).collect();
        let bytes = ebex_many(1_000, &boards);
        let metadata = ed_ebex::validate_snapshot(&bytes).unwrap();
        let cancel = AtomicBool::new(true);
        let error = hydrate_prevalidated_with(&conn, &bytes, &metadata, &mut |_| {}, &|| {
            cancel.load(Ordering::Relaxed)
        })
        .unwrap_err();
        assert!(
            error.downcast_ref::<HydrationCancelled>().is_some(),
            "{error}"
        );
        // The first chunk (2,000 stations) committed before the cancel bit.
        assert_eq!(board(&conn, 1).len(), 1);
        assert_eq!(
            board(&conn, 2_050).len(),
            0,
            "past the cancel point: not written"
        );
        // Durable settings are restored even on the cancel path.
        let synchronous: i64 = conn
            .query_row("PRAGMA galaxy.synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            synchronous, 1,
            "synchronous must be back to NORMAL, never left OFF"
        );
        // The rerun is a plain completion over the kept chunks.
        cancel.store(false, Ordering::Relaxed);
        let stats = hydrate_prevalidated_with(&conn, &bytes, &metadata, &mut |_| {}, &|| {
            cancel.load(Ordering::Relaxed)
        })
        .unwrap();
        assert_eq!(stats.station_snapshots, 2_100);
        assert_eq!(board(&conn, 2_050).len(), 1);
    }

    #[test]
    fn journal_wrapped_symbols_intern_as_their_canonical_good() {
        assert_eq!(canonical_symbol("$magnesite_name;"), "magnesite");
        assert_eq!(canonical_symbol("$Helium3_name;"), "helium3");
        assert_eq!(canonical_symbol("magnesite"), "magnesite");
        assert_eq!(canonical_symbol("  Rutile "), "rutile");
        // Spaced display variants are NOT folded at the boundary — the
        // merge migration owns those (0 market rows today).
        assert_eq!(canonical_symbol("periclase dunite"), "periclase dunite");
        let conn = db();
        let a = intern_commodity(&conn, "$magnesite_name;", None, None).unwrap();
        let b = intern_commodity(&conn, "magnesite", Some("Magnesite"), None).unwrap();
        assert_eq!(a, b, "wrapper and compact symbol are one commodity id");
    }

    /// The checkpoint (maintainer, 2026-09-05: "restarts must resume, not
    /// repeat"): an interrupted hydrate stamps its last committed station
    /// inside the chunk commit; the rerun of the SAME artifact
    /// fast-forwards past the committed prefix without re-reading it,
    /// completes, and clears the cursor so later runs start fresh.
    #[test]
    fn an_interrupted_hydrate_resumes_from_its_checkpoint_not_from_zero() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let conn = db();
        let boards: Vec<(u64, &[u32])> = (1..=2_100u64).map(|id| (id, &[100u32][..])).collect();
        let bytes = ebex_many(1_000, &boards);
        let metadata = ed_ebex::validate_snapshot(&bytes).unwrap();
        let cancel = AtomicBool::new(true);
        hydrate_prevalidated_with(&conn, &bytes, &metadata, &mut |_| {}, &|| {
            cancel.load(Ordering::Relaxed)
        })
        .unwrap_err();
        let ckpt: i64 = conn
            .query_row(
                "SELECT value FROM sys_meta WHERE key = 'hydrate_ckpt_station'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ckpt, 2_000,
            "the cursor is the last station of the committed chunk"
        );
        cancel.store(false, Ordering::Relaxed);
        let stats =
            hydrate_prevalidated_with(&conn, &bytes, &metadata, &mut |_| {}, &|| false).unwrap();
        assert_eq!(
            stats.stations_fast_forwarded, 2_000,
            "the committed prefix is skipped, not re-compared"
        );
        assert_eq!(
            stats.rows, 100,
            "only the remaining stations' rows are read"
        );
        assert_eq!(board(&conn, 2_100).len(), 1, "the tail landed");
        assert!(
            conn.query_row(
                "SELECT value FROM sys_meta WHERE key = 'hydrate_ckpt_station'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .is_err(),
            "completion clears the cursor"
        );
        let again =
            hydrate_prevalidated_with(&conn, &bytes, &metadata, &mut |_| {}, &|| false).unwrap();
        assert_eq!(
            again.stations_fast_forwarded, 0,
            "no stale cursor survives a completed pass"
        );
    }

    /// A checkpoint must never fast-forward a DIFFERENT artifact: the
    /// cursor carries the snapshot's identity and a mismatch means a
    /// full pass.
    #[test]
    fn a_checkpoint_never_fast_forwards_a_different_artifact() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let conn = db();
        let boards: Vec<(u64, &[u32])> = (1..=2_100u64).map(|id| (id, &[100u32][..])).collect();
        let bytes_a = ebex_many(1_000, &boards);
        let metadata_a = ed_ebex::validate_snapshot(&bytes_a).unwrap();
        let cancel = AtomicBool::new(true);
        hydrate_prevalidated_with(&conn, &bytes_a, &metadata_a, &mut |_| {}, &|| {
            cancel.load(Ordering::Relaxed)
        })
        .unwrap_err();
        // A NEWER artifact (different watermark) arrives before the rerun.
        let bytes_b = ebex_many(2_000, &boards);
        let metadata_b = ed_ebex::validate_snapshot(&bytes_b).unwrap();
        let stats = hydrate_prevalidated_with(&conn, &bytes_b, &metadata_b, &mut |_| {}, &|| false)
            .unwrap();
        assert_eq!(
            stats.stations_fast_forwarded, 0,
            "identity mismatch: the whole artifact is walked"
        );
        assert_eq!(stats.station_snapshots, 2_100);
        assert_eq!(board(&conn, 2_100).len(), 1);
    }

    /// The prevalidated entry point lands the same rows and stats as the
    /// validating one — it only skips the SECOND walk over bytes the
    /// caller already validated (the 18-minute "starting" finding).
    #[test]
    fn prevalidated_hydration_matches_the_validating_path() {
        let bytes = ebex_many(1_000, &[(10, &[100, 101]), (11, &[]), (12, &[300])]);
        let a = db();
        let stats_a = hydrate_ebex_with(&a, &bytes, &mut |_| {}).unwrap();
        let b = db();
        let metadata = ed_ebex::validate_snapshot(&bytes).unwrap();
        let stats_b =
            hydrate_prevalidated_with(&b, &bytes, &metadata, &mut |_| {}, &|| false).unwrap();
        assert_eq!(stats_a.rows, stats_b.rows);
        assert_eq!(stats_a.station_snapshots, stats_b.station_snapshots);
        assert_eq!(board(&a, 10), board(&b, 10));
        assert_eq!(board(&a, 12), board(&b, 12));
    }

    /// Hydration reports its progress as boards are written -- station by
    /// station, not after grouping the whole artifact -- and empty boards
    /// (a station in the snapshot list with no rows) count too.
    #[test]
    fn ebex_hydration_reports_progress_per_station_including_empty_boards() {
        let conn = db();
        let bytes = ebex_many(
            1_000,
            &[(10, &[100, 101]), (11, &[]), (12, &[300]), (13, &[])],
        );
        let mut seen = Vec::new();
        let stats = hydrate_ebex_with(&conn, &bytes, &mut |p| seen.push(p)).unwrap();
        assert_eq!(stats.rows, 3);
        assert_eq!(stats.station_snapshots, 4);
        // The final report always arrives and says every station is done.
        assert_eq!(
            seen.last().copied(),
            Some(HydrationProgress {
                stations_done: 4,
                stations_total: 4,
                rows: 3
            })
        );
        assert!(seen.iter().all(|p| p.stations_total == 4));
        // Every board landed: prices where there were rows, empty boards with their watermark.
        assert_eq!(board(&conn, 10).len(), 2);
        assert_eq!(board(&conn, 11).len(), 0);
        assert_eq!(board(&conn, 12).len(), 1);
        let wm: i64 = conn
            .query_row(
                "SELECT observed_at FROM sys_market_watermarks WHERE station_id = 13",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(wm, 1_000);
        // Reports are periodic: a snapshot bigger than the interval reports before the end.
        let big: Vec<(u64, &[u32])> = (1..=(HYDRATE_PROGRESS_EVERY + 5))
            .map(|i| (100 + i, &[1u32][..]))
            .collect();
        let bytes = ebex_many(2_000, &big);
        let mut seen = Vec::new();
        hydrate_ebex_with(&db(), &bytes, &mut |p| seen.push(p)).unwrap();
        assert!(seen.len() >= 2, "{seen:?}");
        assert_eq!(seen[0].stations_done, HYDRATE_PROGRESS_EVERY);
    }

    /// A baseline bigger than the commit interval is committed in chunks
    /// and every board still lands; rerunning it applies nothing (every
    /// board is equal-aged) -- what makes an interrupted load safe to redo.
    #[test]
    fn ebex_hydration_commits_in_chunks_and_reruns_are_no_ops() {
        let conn = db();
        let boards: Vec<(u64, &[u32])> = (1..=(HYDRATE_COMMIT_EVERY + 7))
            .map(|i| (1_000 + i, &[1u32, 2][..]))
            .collect();
        let bytes = ebex_many(5_000, &boards);
        let stats = hydrate_ebex_with(&conn, &bytes, &mut |_| {}).unwrap();
        assert_eq!(stats.station_snapshots, HYDRATE_COMMIT_EVERY + 7);
        assert_eq!(stats.rows, 2 * (HYDRATE_COMMIT_EVERY + 7));
        let stations: i64 = conn
            .query_row(
                "SELECT count(DISTINCT station_id) FROM sys_market",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stations as u64, HYDRATE_COMMIT_EVERY + 7);
        let sync_mode: i64 = conn
            .query_row("PRAGMA galaxy.synchronous", [], |r| r.get(0))
            .unwrap();
        assert_ne!(sync_mode, 0, "synchronous is restored after the load");
        let again = hydrate_ebex_with(&conn, &bytes, &mut |_| {}).unwrap();
        assert_eq!(
            again.stations_skipped,
            HYDRATE_COMMIT_EVERY + 7,
            "a rerun applies nothing"
        );
    }

    fn ebex(symbol: &str, station_id: u64, observed_at: i64, rows: &[(u32, i64)]) -> Vec<u8> {
        let mut records = Vec::new();
        for (sell_price, at) in rows {
            ed_ebex::MarketRecord {
                station_id,
                commodity_id: 1,
                buy_price: 5,
                sell_price: *sell_price,
                demand: 7,
                supply: 8,
                observed_at: *at,
            }
            .encode_into(&mut records);
        }
        ed_ebex::encode_snapshot(
            ed_ebex::SnapshotHeader {
                sequence: 1,
                created_at: 90,
                watermark: observed_at,
            },
            vec![ed_ebex::Section {
                id: ed_ebex::SECTION_MARKETS,
                schema: ed_ebex::MARKET_SCHEMA_V1,
                required: true,
                record_count: rows.len() as u64,
                record_size: ed_ebex::MARKET_RECORD_BYTES,
                records,
                auxiliary: market_auxiliary(symbol, station_id, observed_at),
            }],
        )
        .unwrap()
    }

    /// A baseline is applied with the same rule as live EDDN: a station
    /// whose local board is newer keeps it whole; one that is older is
    /// replaced whole, delisted rows included.
    #[test]
    fn ebex_hydration_shares_the_snapshot_rule() {
        let conn = db();
        // Local EDDN already saw this station at t=100.
        write_snapshot(&conn, 7, 100, &[row(10, 200), row(11, 20)]).unwrap();
        let stats = hydrate_ebex(&conn, &ebex("gold", 7, 80, &[(6, 80)])).unwrap();
        assert_eq!((stats.stations_skipped, stats.removed), (1, 0));
        assert_eq!(
            board(&conn, 7),
            vec![("gold".into(), 200, 100), ("silver".into(), 20, 100)]
        );

        // A newer baseline replaces the board and drops silver.
        let stats = hydrate_ebex(&conn, &ebex("gold", 7, 120, &[(6, 120)])).unwrap();
        assert_eq!(
            (stats.stations_skipped, stats.removed, stats.rows),
            (0, 2, 1)
        );
        assert_eq!(board(&conn, 7), vec![("gold".into(), 6, 120)]);
        let (sequence, watermark_meta): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT value FROM sys_meta WHERE key='ebex_sequence'),
                        (SELECT value FROM sys_meta WHERE key='ebex_watermark')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((sequence, watermark_meta), (1, 120));
    }

    /// An EBEX station snapshot with no rows clears the board and leaves a
    /// watermark that an older EDDN replay cannot get past.
    #[test]
    fn ebex_station_watermark_rejects_an_older_eddn_snapshot() {
        let conn = db();
        write_snapshot(&conn, 7, 100, &[row(10, 200)]).unwrap();
        hydrate_ebex(&conn, &ebex("gold", 7, 300, &[])).unwrap();
        assert!(board(&conn, 7).is_empty());
        assert_eq!(watermark(&conn, 7).unwrap(), Some(300));
        assert_eq!(
            write_snapshot(&conn, 7, 250, &[row(10, 1)]).unwrap(),
            SnapshotOutcome::Skipped
        );
    }

    #[test]
    fn ebex_hydration_rejects_unknown_required_sections() {
        let conn = db();
        let bytes = ed_ebex::encode_snapshot(
            ed_ebex::SnapshotHeader {
                sequence: 1,
                created_at: 1,
                watermark: 1,
            },
            vec![ed_ebex::Section {
                id: 99,
                schema: 1,
                required: true,
                record_count: 0,
                record_size: 1,
                records: vec![],
                auxiliary: vec![],
            }],
        )
        .unwrap();
        assert!(hydrate_ebex(&conn, &bytes)
            .unwrap_err()
            .to_string()
            .contains("required EBEX section 99"));
    }
}
