//! Apply decoded EDDN messages to the galaxy tables.
//!
//! `ed-eddn` owns the wire format; this owns what the data means. The split
//! keeps the decoder testable without a database and the writer testable
//! without a socket.
//!
//! Two rules govern every write here:
//!
//! * **A commodity message is a full market snapshot, not a delta.** If a
//!   commodity vanished from a station's board it must vanish from ours, so
//!   the station's rows are replaced wholesale rather than upserted
//!   piecemeal. Upserting alone would leave a sold-out commodity sitting in
//!   the table at its last known price forever. The writer is
//!   `crate::market::write_snapshot`, shared with EBEX and dump imports;
//!   whether a snapshot is new enough is `ed_domain::freshness::accept`
//!   against the station's watermark.
//! * **Journal data from the commander's own machine outranks this.** EDDN
//!   is other players' second-hand reports; `powerplay_observations` is
//!   first-hand. Where they disagree about a system, the journal wins.

use crate::market;
use anyhow::Result;
use ed_domain::{freshness, ApplyStats, Operation};
use rusqlite::{params, Connection, OptionalExtension};

pub type Applied = ApplyStats;

/// Find a system id64 by name, or mint a name-only row.
///
/// EDDN commodity messages carry a system *name* but no id64. Rather than
/// drop the market data, an unknown system gets a placeholder row keyed on a
/// negative synthetic id, so the station and its prices are still queryable.
/// A later dump import or journal visit fills in the real id and coordinates.
fn system_id_for(conn: &Connection, name: &str) -> Result<i64> {
    // Prefer the REAL row when a provisional twin exists. Without the
    // ORDER BY, the scan returned whichever came first — the negative
    // provisional — and every station the message touched was then
    // re-parented onto a row with no coordinates, vanishing from every
    // radius query (field case 2026-09-05: docking at a large station
    // deleted it from the profit finder mid-run).
    if let Some(id) = conn
        .query_row(
            "SELECT id64 FROM sys_systems WHERE name = ?1 COLLATE NOCASE
             ORDER BY (id64 < 0), id64 LIMIT 1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
    {
        return Ok(id);
    }

    // Synthetic ids are negative so they can never collide with a real id64
    // and are trivially identifiable as provisional.
    let next: i64 = conn
        .query_row(
            "SELECT COALESCE(MIN(id64), 0) - 1 FROM sys_systems WHERE id64 < 0",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    conn.execute(
        "INSERT INTO sys_systems (id64, name) VALUES (?1, ?2)",
        params![next, name],
    )?;
    Ok(next)
}

/// Resolve a system for a message that carries the game's own
/// `SystemAddress` (journal Docked/FSDJump frames): the address is
/// authoritative — a known row wins outright, an unknown one creates the
/// REAL row (name only; coords arrive from feeds that carry them) rather
/// than a provisional ghost. Falls back to the name path when absent.
fn system_id_for_addressed(conn: &Connection, name: &str, address: Option<i64>) -> Result<i64> {
    if let Some(address) = address.filter(|a| *a > 0) {
        let known: bool = conn
            .query_row("SELECT 1 FROM sys_systems WHERE id64 = ?1", [address], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if !known {
            conn.execute(
                "INSERT INTO sys_systems (id64, name) VALUES (?1, ?2)",
                params![address, name],
            )?;
        }
        return Ok(address);
    }
    system_id_for(conn, name)
}

/// Ensure a station row exists for this market, returning its id.
///
/// Spansh keys stations on the game's own MarketID, which is exactly what
/// EDDN reports, so the two line up without a name match. Name lookup is
/// only the fallback for messages that omit `marketId`.
fn station_id_for(
    conn: &Connection,
    system_id: i64,
    market_id: Option<i64>,
    station_name: Option<&str>,
    stats: &mut Applied,
) -> Result<Option<i64>> {
    if let Some(id) = market_id {
        let exists: bool = conn
            .query_row("SELECT 1 FROM sys_stations WHERE id = ?1", [id], |_| {
                Ok(true)
            })
            .optional()?
            .unwrap_or(false);
        if !exists {
            conn.execute(
                "INSERT INTO sys_stations (id, system_id64, name, has_market)
                 VALUES (?1, ?2, ?3, 1)",
                params![id, system_id, station_name],
            )?;
            stats.stations += 1;
        } else if let Some(name) = station_name {
            // Re-parenting rule: a REAL system id always wins (it can
            // heal a station stuck on a ghost); a provisional id may
            // only replace another provisional — never drag a station
            // off a real system onto a coordinate-less row (the field
            // case above).
            conn.execute(
                "UPDATE sys_stations SET name = ?2,
                     system_id64 = CASE WHEN ?3 >= 0 OR system_id64 < 0 THEN ?3 ELSE system_id64 END
                 WHERE id = ?1",
                params![id, name, system_id],
            )?;
        }
        return Ok(Some(id));
    }

    let Some(name) = station_name else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT id FROM sys_stations WHERE system_id64 = ?1 AND name = ?2 COLLATE NOCASE",
            params![system_id, name],
            |r| r.get(0),
        )
        .optional()?)
}

pub fn apply_operation(conn: &Connection, operation: &Operation) -> Result<Applied> {
    let mut stats = Applied::default();
    let tx = conn.unchecked_transaction()?;

    match operation {
        // The service's prospecting and star teachings (2026-09-09) have
        // no local tables since B.4: the client keeps only its journal.
        Operation::Star(_) | Operation::Body(_) | Operation::RingHotspots(_) | Operation::BodySignals(_) => {
            stats.skipped += 1;
            tx.commit()?;
            return Ok(stats);
        }
        Operation::Market(m) => {
            let system_id = system_id_for(&tx, &m.system_name)?;
            let Some(station_id) = station_id_for(
                &tx,
                system_id,
                m.market_id,
                m.station_name.as_deref(),
                &mut stats,
            )?
            else {
                stats.skipped += 1;
                tx.commit()?;
                return Ok(stats);
            };

            let snapshot_time = m.observed_at.epoch_seconds;
            // The freshness check happens inside the writer, against the
            // station watermark. Commodities are interned first so a skip
            // still costs nothing but the catalog rows.
            let rows = m
                .values
                .iter()
                .map(|c| {
                    Ok(market::MarketRow {
                        commodity_id: market::intern_commodity(&tx, &c.name, None, None)?,
                        buy_price: c.buy_price,
                        sell_price: c.sell_price,
                        demand: c.demand,
                        supply: c.stock,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            match market::write_snapshot(&tx, station_id, snapshot_time, &rows)? {
                market::SnapshotOutcome::Skipped => {
                    stats.skipped += 1;
                    tx.commit()?;
                    return Ok(stats);
                }
                market::SnapshotOutcome::Applied { removed, written } => {
                    stats.market_rows_removed += removed;
                    stats.market_rows += written;
                }
            }
            tx.execute(
                "UPDATE sys_stations SET has_market = 1, updated = ?2 WHERE id = ?1",
                params![station_id, snapshot_time],
            )?;
            // Confiscated goods travel with the board (F2b, 2026-09-04):
            // a fresh snapshot replaces the prohibition list wholesale —
            // empty legitimately clears it. Values are lowercased wire
            // strings; readers match them against symbol OR name.
            tx.execute(
                "DELETE FROM sys_market_prohibited WHERE station_id = ?1",
                [station_id],
            )?;
            if !m.prohibited.is_empty() {
                let mut insert = tx.prepare_cached(
                    "INSERT OR IGNORE INTO sys_market_prohibited (station_id, symbol) VALUES (?1, ?2)",
                )?;
                for good in &m.prohibited {
                    insert.execute(params![station_id, good])?;
                }
            }
        }

        Operation::StationIdentity(identity) => {
            // A Docked event: pads, authoritative carrier flag, arrival
            // distance, type, services — gated on its own watermark so a
            // replay never regresses identity (station identity ingest,
            // 2026-09-04). Docked carries the game's SystemAddress:
            // resolve by it, never by a name a ghost row may shadow.
            let system_id =
                system_id_for_addressed(&tx, &identity.system_name, identity.system_address)?;
            let Some(station_id) = station_id_for(
                &tx,
                system_id,
                Some(identity.market_id),
                Some(&identity.station_name),
                &mut stats,
            )?
            else {
                stats.skipped += 1;
                tx.commit()?;
                return Ok(stats);
            };
            let observed = identity.observed_at.epoch_seconds;
            let fresh: bool = tx.query_row(
                "SELECT COALESCE(identity_updated, 0) < ?2 FROM sys_stations WHERE id = ?1",
                params![station_id, observed],
                |r| r.get(0),
            )?;
            if !fresh {
                stats.skipped += 1;
                tx.commit()?;
                return Ok(stats);
            }
            tx.execute(
                "UPDATE sys_stations SET
                     pad_small = COALESCE(?2, pad_small),
                     pad_medium = COALESCE(?3, pad_medium),
                     pad_large = COALESCE(?4, pad_large),
                     is_carrier = ?5,
                     distance_to_arrival = COALESCE(?6, distance_to_arrival),
                     type = COALESCE(?7, type),
                     identity_updated = ?8
                 WHERE id = ?1",
                params![
                    station_id,
                    identity.pad_small,
                    identity.pad_medium,
                    identity.pad_large,
                    identity.is_carrier() as i64,
                    identity.arrival_ls,
                    identity.station_type.as_deref(),
                    observed,
                ],
            )?;
            if !identity.services.is_empty() {
                tx.execute(
                    "DELETE FROM sys_station_services WHERE station_id = ?1",
                    [station_id],
                )?;
                let mut insert = tx.prepare_cached(
                    "INSERT OR IGNORE INTO sys_station_services (station_id, service) VALUES (?1, ?2)",
                )?;
                for service in &identity.services {
                    insert.execute(params![station_id, service.trim().to_lowercase()])?;
                }
            }
        }

        Operation::Outfitting(m) => {
            let system_id = system_id_for(&tx, &m.system_name)?;
            let Some(station_id) = station_id_for(
                &tx,
                system_id,
                m.market_id,
                m.station_name.as_deref(),
                &mut stats,
            )?
            else {
                stats.skipped += 1;
                tx.commit()?;
                return Ok(stats);
            };
            let newest_stored: Option<i64> = tx.query_row(
                "SELECT outfitting_updated FROM sys_stations WHERE id = ?1",
                [station_id],
                |row| row.get(0),
            )?;
            if !freshness::is_newer(newest_stored, m.observed_at.epoch_seconds) {
                stats.skipped += 1;
                tx.commit()?;
                return Ok(stats);
            }
            tx.execute(
                "DELETE FROM sys_outfitting WHERE station_id = ?1",
                [station_id],
            )?;
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO sys_outfitting (station_id, module_id) VALUES (?1, ?2)",
            )?;
            for module in &m.values {
                let symbol = module;
                let item_id: i64 = tx.query_row(
                    "INSERT INTO sys_modules(symbol) VALUES (?1)
                     ON CONFLICT(symbol) DO UPDATE SET symbol=excluded.symbol RETURNING id",
                    [&symbol],
                    |r| r.get(0),
                )?;
                ins.execute(params![station_id, item_id])?;
                stats.outfitting_rows += 1;
            }
            tx.execute(
                "UPDATE sys_stations SET has_outfitting = 1, outfitting_updated = ?2 WHERE id = ?1",
                params![station_id, m.observed_at.epoch_seconds],
            )?;
        }

        Operation::Shipyard(m) => {
            let system_id = system_id_for(&tx, &m.system_name)?;
            let Some(station_id) = station_id_for(
                &tx,
                system_id,
                m.market_id,
                m.station_name.as_deref(),
                &mut stats,
            )?
            else {
                stats.skipped += 1;
                tx.commit()?;
                return Ok(stats);
            };
            let newest_stored: Option<i64> = tx.query_row(
                "SELECT shipyard_updated FROM sys_stations WHERE id = ?1",
                [station_id],
                |row| row.get(0),
            )?;
            if !freshness::is_newer(newest_stored, m.observed_at.epoch_seconds) {
                stats.skipped += 1;
                tx.commit()?;
                return Ok(stats);
            }
            tx.execute(
                "DELETE FROM sys_shipyard WHERE station_id = ?1",
                [station_id],
            )?;
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO sys_shipyard (station_id, ship_id) VALUES (?1, ?2)",
            )?;
            for ship in &m.values {
                let symbol = ship;
                let item_id: i64 = tx.query_row(
                    "INSERT INTO sys_ships(symbol) VALUES (?1)
                     ON CONFLICT(symbol) DO UPDATE SET symbol=excluded.symbol RETURNING id",
                    [&symbol],
                    |r| r.get(0),
                )?;
                ins.execute(params![station_id, item_id])?;
                stats.shipyard_rows += 1;
            }
            tx.execute(
                "UPDATE sys_stations SET has_shipyard = 1, shipyard_updated = ?2 WHERE id = ?1",
                params![station_id, m.observed_at.epoch_seconds],
            )?;
        }

        Operation::System(m) => {
            let name = &m.system_name;
            let pos = m.position;
            // id64 when the message has it, otherwise fall back to the name.
            let id = match m.system_address {
                Some(addr) => addr,
                None => system_id_for(&tx, name)?,
            };
            let changed = tx.execute(
                "INSERT INTO sys_systems
                     (id64, name, x, y, z, security, allegiance, population,
                      controlling_power, power_state, powers, updated, eddn_updated)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                 ON CONFLICT(id64) DO UPDATE SET
                     name=excluded.name,
                     x=COALESCE(excluded.x, sys_systems.x),
                     y=COALESCE(excluded.y, sys_systems.y),
                     z=COALESCE(excluded.z, sys_systems.z),
                     security=COALESCE(excluded.security, sys_systems.security),
                     allegiance=COALESCE(excluded.allegiance, sys_systems.allegiance),
                     population=COALESCE(excluded.population, sys_systems.population),
                     controlling_power=COALESCE(excluded.controlling_power, sys_systems.controlling_power),
                     power_state=COALESCE(excluded.power_state, sys_systems.power_state),
                     powers=COALESCE(excluded.powers, sys_systems.powers),
                     updated=excluded.updated,
                     eddn_updated=excluded.eddn_updated
                 -- ed_domain::freshness::accept: strictly newer wins.
                 WHERE sys_systems.eddn_updated IS NULL OR sys_systems.eddn_updated < excluded.eddn_updated",
                params![
                    id,
                    name,
                    pos.map(|p| p[0]),
                    pos.map(|p| p[1]),
                    pos.map(|p| p[2]),
                    m.security,
                    m.allegiance,
                    m.population,
                    m.controlling_power,
                    m.powerplay_state,
                    m.powers.as_ref().map(|p| p.join(", ")),
                    m.observed_at.epoch_seconds,
                    m.observed_at.epoch_seconds,
                ],
            )?;
            if changed == 0 {
                stats.skipped += 1;
            } else {
                stats.systems += 1;
            }
        }
    }

    tx.commit()?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn apply(conn: &Connection, envelope: &ed_eddn::Envelope) -> Result<Applied> {
        let Some(operation) = envelope.operation() else {
            return Ok(Applied {
                skipped: 1,
                ..Applied::default()
            });
        };
        apply_operation(conn, &operation)
    }

    fn zlib(s: &str) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(s.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    fn commodity_msg(system: &str, market_id: i64, entries: &str) -> Vec<u8> {
        commodity_msg_at(system, market_id, "2026-08-24T01:00:00Z", entries)
    }

    fn commodity_msg_at(system: &str, market_id: i64, timestamp: &str, entries: &str) -> Vec<u8> {
        zlib(&format!(
            r#"{{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{{}},
                "message":{{"systemName":"{system}","stationName":"Test Port","marketId":{market_id},
                "timestamp":"{timestamp}","commodities":[{entries}]}}}}"#
        ))
    }

    /// The vanishing-station spiral (field case 2026-09-05: docking at
    /// a large station deleted it from the profit finder). With a ghost
    /// row (negative id64, no coords) shadowing the real system by name:
    /// (1) name resolution must prefer the REAL row, (2) an existing
    /// station must never be re-parented onto a ghost, and (3) a Docked
    /// frame's SystemAddress must heal a station already stuck on one.
    #[test]
    fn a_ghost_system_never_steals_a_station() {
        let conn = db();
        conn.execute_batch(
            "INSERT INTO sys_systems (id64, name, x, y, z) VALUES (500, 'Ega', 1.0, 2.0, 3.0);
             INSERT INTO sys_systems (id64, name) VALUES (-7, 'Ega');",
        )
        .unwrap();
        // (1)+(2): a board for a station the store does not know yet
        // lands on the REAL Ega, ghost notwithstanding.
        let env = ed_eddn::decode(&commodity_msg(
            "Ega",
            900,
            r#"{"name":"palladium","buyPrice":0,"sellPrice":302136,"demand":999999,"stock":0}"#,
        ))
        .unwrap();
        apply(&conn, &env).unwrap();
        let parent: i64 = conn
            .query_row("SELECT system_id64 FROM sys_stations WHERE id = 900", [], |r| r.get(0))
            .unwrap();
        assert_eq!(parent, 500, "the real system wins over the ghost");

        // (3): a station stuck on the ghost (pre-fix databases) is healed
        // by the next Docked frame, which carries the game's address.
        conn.execute("UPDATE sys_stations SET system_id64 = -7 WHERE id = 900", []).unwrap();
        let docked = zlib(
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{},
                "message":{"timestamp":"2026-09-05T13:00:00Z","event":"Docked","StarSystem":"Ega",
                "SystemAddress":500,"StationName":"Metz Enterprise","MarketID":900,
                "StationType":"Ocellus","DistFromStarLS":5393.9,
                "LandingPads":{"Small":10,"Medium":14,"Large":7},
                "StationServices":["dock","commodities"]}}"#,
        );
        let env = ed_eddn::decode(&docked).unwrap();
        apply(&conn, &env).unwrap();
        let parent: i64 = conn
            .query_row("SELECT system_id64 FROM sys_stations WHERE id = 900", [], |r| r.get(0))
            .unwrap();
        assert_eq!(parent, 500, "the address re-parents the station off the ghost");

        // A message naming an UNKNOWN system mints a ghost for it but may
        // not drag a really-parented station onto it.
        let env = ed_eddn::decode(&commodity_msg(
            "Egg Nebula Prime",
            900,
            r#"{"name":"gold","buyPrice":0,"sellPrice":100,"demand":1,"stock":0}"#,
        ))
        .unwrap();
        apply(&conn, &env).unwrap();
        let parent: i64 = conn
            .query_row("SELECT system_id64 FROM sys_stations WHERE id = 900", [], |r| r.get(0))
            .unwrap();
        assert_eq!(parent, 500, "a fresh ghost never steals from a real parent");
    }

    /// Station identity ingest (2026-09-04): a Docked event fills pads,
    /// the authoritative carrier flag, arrival distance, type and
    /// services; a replay older than the identity watermark is skipped;
    /// and a commodity message's prohibited[] replaces the confiscation
    /// list with the board.
    #[test]
    fn docked_and_prohibited_fill_station_identity() {
        let conn = db();
        let docked = |ts: &str, station_type: &str| {
            zlib(&format!(
                r#"{{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{{}},
                    "message":{{"timestamp":"{ts}","event":"Docked","StarSystem":"Ega",
                    "SystemAddress":123,"StationName":"Metz Enterprise","MarketID":900,
                    "StationType":"{station_type}","DistFromStarLS":346.5,
                    "LandingPads":{{"Small":4,"Medium":4,"Large":2}},
                    "StationServices":["dock","commodities","blackmarket"]}}}}"#
            ))
        };
        let env = ed_eddn::decode(&docked("2026-09-04T16:00:00Z", "Coriolis")).unwrap();
        apply(&conn, &env).unwrap();
        let (pads, carrier, arrival, kind): (i64, i64, f64, String) = conn
            .query_row(
                "SELECT pad_large, is_carrier, distance_to_arrival, type FROM sys_stations WHERE id = 900",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((pads, carrier, arrival, kind.as_str()), (2, 0, 346.5, "Coriolis"));
        assert!(
            crate::market::has_black_market(&conn, 900).unwrap(),
            "the journal's blackmarket token counts"
        );
        // An older replay claiming FleetCarrier must not regress identity.
        let stale = ed_eddn::decode(&docked("2026-09-04T15:00:00Z", "FleetCarrier")).unwrap();
        let s = apply(&conn, &stale).unwrap();
        assert_eq!(s.skipped, 1);
        let carrier: i64 = conn
            .query_row("SELECT is_carrier FROM sys_stations WHERE id = 900", [], |r| r.get(0))
            .unwrap();
        assert_eq!(carrier, 0, "stale identity is refused");

        // Prohibited goods arrive with the board and replace wholesale.
        let board = zlib(
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},
                "message":{"systemName":"Ega","stationName":"Metz Enterprise","marketId":900,
                "timestamp":"2026-09-04T16:05:00Z",
                "commodities":[{"name":"gold","buyPrice":0,"sellPrice":100,"demand":10,"stock":0}],
                "prohibited":["Slaves","Battle Weapons"]}}"#,
        );
        let env = ed_eddn::decode(&board).unwrap();
        apply(&conn, &env).unwrap();
        let goods: Vec<String> = conn
            .prepare("SELECT symbol FROM sys_market_prohibited WHERE station_id = 900 ORDER BY symbol")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(goods, vec!["battle weapons", "slaves"], "lowercased wire values");
    }

    #[test]
    fn a_commodity_message_creates_system_station_and_prices() {
        let conn = db();
        let env = ed_eddn::decode(&commodity_msg(
            "Deciat",
            3229332736,
            r#"{"name":"gold","buyPrice":0,"sellPrice":48549,"demand":1200,"stock":0}"#,
        ))
        .unwrap();
        let s = apply(&conn, &env).unwrap();
        assert_eq!(s.market_rows, 1);
        assert_eq!(s.stations, 1);

        let (symbol, sell): (String, i64) = conn
            .query_row(
                "SELECT c.symbol, m.sell_price FROM sys_market m
                 JOIN sys_commodities c ON c.id=m.commodity_id WHERE m.station_id = 3229332736",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(symbol, "gold");
        assert_eq!(sell, 48549);
    }

    #[test]
    fn a_delisted_commodity_disappears_rather_than_lingering() {
        let conn = db();
        let first = ed_eddn::decode(&commodity_msg(
            "Deciat",
            1,
            r#"{"name":"gold","buyPrice":0,"sellPrice":48549,"demand":10,"stock":0},
               {"name":"silver","buyPrice":0,"sellPrice":100,"demand":5,"stock":0}"#,
        ))
        .unwrap();
        apply(&conn, &first).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sys_market", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );

        // Silver is gone from the board. A pure upsert would leave it behind
        // at its last known price -- which is a wrong answer, not a stale one.
        let second = ed_eddn::decode(&commodity_msg_at(
            "Deciat",
            1,
            "2026-08-24T01:01:00Z",
            r#"{"name":"gold","buyPrice":0,"sellPrice":50000,"demand":20,"stock":0}"#,
        ))
        .unwrap();
        apply(&conn, &second).unwrap();

        let rows: Vec<(String, i64)> = conn
            .prepare(
                "SELECT c.symbol, m.sell_price FROM sys_market m
                      JOIN sys_commodities c ON c.id=m.commodity_id ORDER BY c.symbol",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows, vec![("gold".to_string(), 50000)]);
    }

    #[test]
    fn an_older_market_snapshot_cannot_replace_newer_data() {
        let conn = db();
        let newer = ed_eddn::decode(&commodity_msg_at(
            "Deciat",
            1,
            "2026-08-24T02:00:00Z",
            r#"{"name":"gold","buyPrice":0,"sellPrice":50000,"demand":20,"stock":0}"#,
        ))
        .unwrap();
        apply(&conn, &newer).unwrap();

        let older = ed_eddn::decode(&commodity_msg_at(
            "Deciat",
            1,
            "2026-08-24T01:00:00Z",
            r#"{"name":"silver","buyPrice":0,"sellPrice":100,"demand":5,"stock":0}"#,
        ))
        .unwrap();
        assert_eq!(apply(&conn, &older).unwrap().skipped, 1);

        let rows: Vec<(String, i64)> = conn
            .prepare(
                "SELECT c.symbol, m.sell_price FROM sys_market m
                 JOIN sys_commodities c ON c.id=m.commodity_id ORDER BY c.symbol",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(rows, vec![("gold".to_string(), 50000)]);
    }

    /// Freshness must come from a station-level watermark, not `MAX(updated)`
    /// over the rows that happen to remain: once an empty snapshot deletes
    /// every row, MAX is NULL and an older snapshot would be accepted.
    #[test]
    fn older_snapshot_is_rejected_after_an_empty_snapshot_deleted_rows() {
        let conn = db();
        let stocked = ed_eddn::decode(&commodity_msg_at(
            "Deciat",
            1,
            "2026-08-24T01:00:00Z",
            r#"{"name":"gold","buyPrice":0,"sellPrice":50000,"demand":20,"stock":0}"#,
        ))
        .unwrap();
        apply(&conn, &stocked).unwrap();

        // The board is empty now -- every row goes.
        let empty =
            ed_eddn::decode(&commodity_msg_at("Deciat", 1, "2026-08-24T02:00:00Z", "")).unwrap();
        let s = apply(&conn, &empty).unwrap();
        assert_eq!(s.market_rows_removed, 1);

        // A replayed message from before the empty board must not resurrect it.
        let older = ed_eddn::decode(&commodity_msg_at(
            "Deciat",
            1,
            "2026-08-24T01:30:00Z",
            r#"{"name":"silver","buyPrice":0,"sellPrice":100,"demand":5,"stock":0}"#,
        ))
        .unwrap();
        assert_eq!(apply(&conn, &older).unwrap().skipped, 1);
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sys_market WHERE station_id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 0);
    }

    /// Every `*updated` column is an integer epoch; the station one was a
    /// timestamp string while its siblings were integers.
    #[test]
    fn station_updated_is_stored_as_an_epoch() {
        let conn = db();
        let env = ed_eddn::decode(&commodity_msg_at(
            "Deciat",
            1,
            "2026-08-24T01:00:00Z",
            r#"{"name":"gold","buyPrice":0,"sellPrice":1,"demand":1,"stock":0}"#,
        ))
        .unwrap();
        apply(&conn, &env).unwrap();
        let (kind, value): (String, i64) = conn
            .query_row(
                "SELECT typeof(updated), updated FROM sys_stations WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(kind, "integer");
        assert_eq!(value, ed_eddn::epoch_secs("2026-08-24T01:00:00Z").unwrap());
    }

    #[test]
    fn a_journal_message_records_powerplay_for_an_unvisited_system() {
        let conn = db();
        let raw = r#"{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{},
          "message":{"event":"FSDJump","StarSystem":"Sol","SystemAddress":10477373803,
          "StarPos":[0,0,0],"timestamp":"2026-08-24T01:00:00Z",
          "ControllingPower":"Zachary Hudson","PowerplayState":"Fortified","Population":22780919531}}"#;
        let env = ed_eddn::decode(&zlib(raw)).unwrap();
        assert_eq!(apply(&conn, &env).unwrap().systems, 1);

        let (name, power, state): (String, String, String) = conn
            .query_row(
                "SELECT name, controlling_power, power_state FROM sys_systems WHERE id64=10477373803",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(name, "Sol");
        assert_eq!(power, "Zachary Hudson");
        assert_eq!(state, "Fortified");
    }

    #[test]
    fn a_later_message_without_powerplay_does_not_erase_it() {
        let conn = db();
        for raw in [
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{},
               "message":{"event":"FSDJump","StarSystem":"Sol","SystemAddress":1,
               "timestamp":"2026-08-24T01:00:00Z",
               "ControllingPower":"Zachary Hudson","PowerplayState":"Fortified"}}"#,
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{},
               "message":{"event":"Location","StarSystem":"Sol","SystemAddress":1,
               "timestamp":"2026-08-24T01:01:00Z",
               "StarPos":[0,0,0],"Population":100}}"#,
        ] {
            let env = ed_eddn::decode(&zlib(raw)).unwrap();
            apply(&conn, &env).unwrap();
        }
        let power: Option<String> = conn
            .query_row(
                "SELECT controlling_power FROM sys_systems WHERE id64=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(power.as_deref(), Some("Zachary Hudson"));
    }

    #[test]
    fn older_catalog_and_system_operations_are_rejected() {
        let conn = db();
        for raw in [
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/outfitting/2","header":{},
               "message":{"systemName":"Sol","stationName":"Galileo","marketId":10,
               "timestamp":"2026-08-24T02:00:00Z","modules":["int_hyperdrive_size2_class1"]}}"#,
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/outfitting/2","header":{},
               "message":{"systemName":"Sol","stationName":"Galileo","marketId":10,
               "timestamp":"2026-08-24T01:00:00Z","modules":["int_hyperdrive_size3_class1"]}}"#,
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{},
               "message":{"event":"FSDJump","StarSystem":"Achenar","SystemAddress":20,
               "timestamp":"2026-08-24T02:00:00Z","ControllingPower":"Dent\u2019on Patreus"}}"#,
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{},
               "message":{"event":"FSDJump","StarSystem":"Achenar","SystemAddress":20,
               "timestamp":"2026-08-24T01:00:00Z","ControllingPower":"Someone Older"}}"#,
        ] {
            let envelope = ed_eddn::decode(&zlib(raw)).unwrap();
            apply(&conn, &envelope).unwrap();
        }

        let module: String = conn
            .query_row(
                "SELECT m.symbol FROM sys_outfitting o JOIN sys_modules m ON m.id = o.module_id",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let power: String = conn
            .query_row(
                "SELECT controlling_power FROM sys_systems WHERE id64 = 20",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(module, "int_hyperdrive_size2_class1");
        assert_eq!(power, "Dent’on Patreus");
    }

    #[test]
    fn an_unknown_system_gets_a_provisional_negative_id() {
        let conn = db();
        let env = ed_eddn::decode(&commodity_msg(
            "Nowhere Ever Seen",
            42,
            r#"{"name":"gold","buyPrice":1,"sellPrice":2,"demand":3,"stock":4}"#,
        ))
        .unwrap();
        apply(&conn, &env).unwrap();

        let id: i64 = conn
            .query_row(
                "SELECT id64 FROM sys_systems WHERE name = 'Nowhere Ever Seen'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(id < 0, "provisional ids must be negative, got {id}");
        // The market data still landed, which is the point of not dropping it.
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sys_market", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn unhandled_schemas_are_counted_as_skipped_not_errors() {
        let conn = db();
        let raw =
            r#"{"$schemaRef":"https://eddn.edcd.io/schemas/navroute/1","header":{},"message":{}}"#;
        let env = ed_eddn::decode(&zlib(raw)).unwrap();
        assert_eq!(apply(&conn, &env).unwrap().skipped, 1);
    }
}
