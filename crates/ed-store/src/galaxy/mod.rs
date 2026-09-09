//! Bootstrap the galaxy tables from a Spansh bulk dump.
//!
//! Two modules on one seam: [`spansh`] decodes and streams dump records,
//! [`sink::GalaxySink`] receives them. [`SqliteSink`] is the desktop
//! adapter; the functions below are its convenience entry points and keep
//! the signatures the app has always called. Any other store implements
//! the trait and calls [`spansh::stream_dump`] directly.

pub mod sink;
pub mod spansh;
mod sqlite;

use std::io::Read;
use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

pub use sink::{GalaxySink, RecordingSink, SinkEvent, SystemVisit};
pub use spansh::{dump_kind, DumpKind};
pub use sqlite::SqliteSink;

#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct ImportStats {
    pub systems: u64,
    pub stations: u64,
    pub market_rows: u64,
    pub outfitting_rows: u64,
    pub shipyard_rows: u64,
    pub factions: u64,
    pub bodies: u64,
    pub body_material_rows: u64,
    pub hotspots: u64,
    pub parse_errors: u64,
    /// Stations whose `updateTime` matched what was stored: nothing written.
    pub skipped_stations: u64,
    /// Systems whose `date` matched: bodies and factions left as they were.
    pub skipped_systems: u64,
    /// Source bytes consumed so far (compressed for a `.gz`), for a
    /// progress fraction.
    pub bytes_in: u64,
}

/// Stream one Spansh dump (`.json.gz` or plain `.json`) into the galaxy
/// tables.
///
/// `progress` receives `(stats, total_bytes)` periodically. Safe to re-run
/// and safe to run over several dumps: every write is an upsert keyed on
/// the game's own ids.
pub fn import_dump(
    conn: &Connection,
    path: &Path,
    progress: impl FnMut(&ImportStats, u64),
) -> Result<ImportStats> {
    crate::schema::with_galaxy_indexes_dropped(conn, || import_dump_unindexed(conn, path, progress))
}

/// Import one dump while the caller owns the bulk-load index lifecycle.
/// Multi-dump imports use this to avoid rebuilding every galaxy index
/// between sources. Prefer [`import_dump`] for a standalone import.
pub fn import_dump_unindexed(
    conn: &Connection,
    path: &Path,
    progress: impl FnMut(&ImportStats, u64),
) -> Result<ImportStats> {
    spansh::stream_dump(path, &mut SqliteSink::new(conn), progress)
}

/// Import a gzip-compressed dump from any byte source -- a file or an HTTP
/// body -- so the archive never has to touch the disk. `total_compressed`
/// is a hint for progress (0 = unknown).
pub fn import_gz(
    conn: &Connection,
    source: impl Read,
    total_compressed: u64,
    progress: impl FnMut(&ImportStats, u64),
) -> Result<ImportStats> {
    crate::schema::with_galaxy_indexes_dropped(conn, || {
        import_gz_unindexed(conn, source, total_compressed, progress)
    })
}

/// Import a gzip stream while the caller owns the bulk-load index lifecycle.
/// Prefer [`import_gz`] unless several dumps are being loaded as one session.
pub fn import_gz_unindexed(
    conn: &Connection,
    source: impl Read,
    total_compressed: u64,
    progress: impl FnMut(&ImportStats, u64),
) -> Result<ImportStats> {
    spansh::stream_gz(
        source,
        total_compressed,
        &mut SqliteSink::new(conn),
        progress,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    /// Push one already-parsed system through the SQLite adapter, as the
    /// streamer would, without the transaction wrapper.
    fn deliver(conn: &Connection, line: &str) -> ImportStats {
        let sys = spansh::parse_line(line).unwrap().unwrap();
        let mut stats = ImportStats::default();
        spansh::deliver(&sys, &mut SqliteSink::new(conn), &mut stats).unwrap();
        stats
    }

    #[test]
    fn import_round_trips_a_two_system_dump() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mini.json.gz");
        {
            use std::io::Write;
            let f = std::fs::File::create(&path).unwrap();
            let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            writeln!(enc, "[").unwrap();
            writeln!(enc, "\t{{\"id64\":1,\"name\":\"Alpha\",\"coords\":{{\"x\":1,\"y\":2,\"z\":3}},\"population\":100,\"controllingPower\":\"A. Lavigny-Duval\",\"powerState\":\"Stronghold\",\"stations\":[{{\"id\":10,\"name\":\"Port A\",\"type\":\"Outpost\",\"services\":[\"Dock\",\"Market\"],\"landingPads\":{{\"large\":0,\"medium\":1,\"small\":2}},\"market\":{{\"commodities\":[{{\"symbol\":\"gold\",\"name\":\"Gold\",\"category\":\"Metals\",\"buyPrice\":100,\"sellPrice\":90,\"demand\":5,\"supply\":7}}]}}}}]}},").unwrap();
            writeln!(enc, "\t{{\"id64\":2,\"name\":\"Beta\",\"coords\":{{\"x\":4,\"y\":5,\"z\":6}},\"bodies\":[{{\"stations\":[{{\"id\":20,\"name\":\"Carrier X\",\"type\":\"Drake-Class Carrier\",\"services\":[\"Dock\",\"Market\"]}}]}}]}}").unwrap();
            writeln!(enc, "]").unwrap();
            enc.finish().unwrap();
        }

        let conn = store();
        let stats = import_dump(&conn, &path, |_, _| {}).unwrap();

        assert_eq!(stats.systems, 2);
        assert_eq!(stats.stations, 2, "body-hosted stations must be picked up");
        assert_eq!(stats.market_rows, 1);
        assert_eq!(stats.parse_errors, 0);

        let power: String = conn
            .query_row(
                "SELECT controlling_power FROM sys_systems WHERE id64=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(power, "A. Lavigny-Duval");

        let carrier: i64 = conn
            .query_row("SELECT is_carrier FROM sys_stations WHERE id=20", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(carrier, 1);

        // Re-importing the same dump must not duplicate anything.
        let again = import_dump(&conn, &path, |_, _| {}).unwrap();
        assert_eq!(again.systems, 2);
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM sys_stations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn dump_market_cannot_overwrite_a_newer_live_price() {
        let conn = store();
        conn.execute_batch(
            "INSERT INTO sys_systems (id64, name) VALUES (1, 'Alpha');
             INSERT INTO sys_stations (id, system_id64, name, has_market)
                 VALUES (10, 1, 'Port A', 1);
             INSERT INTO sys_commodities (id, symbol) VALUES (1, 'gold');
             INSERT INTO sys_market
                 (station_id, commodity_id, buy_price, sell_price, demand, supply, updated)
                 VALUES (10, 1, 0, 50000, 20, 0, 9000000000);",
        )
        .unwrap();

        deliver(
            &conn,
            r#"{"id64":1,"name":"Alpha","stations":[{"id":10,"name":"Port A","type":"Outpost","services":["Market"],"market":{"updateTime":"2026-08-29T08:00:00Z","commodities":[{"symbol":"gold","sellPrice":100}]}}]}"#,
        );

        let price: i64 = conn
            .query_row(
                "SELECT sell_price FROM sys_market WHERE station_id=10 AND commodity_id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(price, 50000);
    }

    /// Spansh writes `YYYY-MM-DD HH:MM:SS+00`, not RFC-3339. A strict
    /// parser left every dump market row with a NULL `updated`, which reads
    /// as infinitely old.
    #[test]
    fn spansh_dump_timestamps_are_stored_as_epochs() {
        let conn = store();
        let line = r#"{"id64":1,"name":"Alpha","stations":[{"id":10,"name":"Port A","type":"Outpost","services":["Market"],"updateTime":"2026-08-24 01:00:00+00","market":{"updateTime":"2026-08-24 01:00:00+00","commodities":[{"symbol":"gold","sellPrice":100}]}}]}"#;
        deliver(&conn, line);

        let epoch = 1_787_533_200i64;
        let market: Option<i64> = conn
            .query_row(
                "SELECT updated FROM sys_market WHERE station_id=10",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(market, Some(epoch));
        let station: Option<i64> = conn
            .query_row("SELECT updated FROM sys_stations WHERE id=10", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(station, Some(epoch));

        // Re-importing the same station is recognised as unchanged.
        let again = deliver(&conn, line);
        assert_eq!(again.skipped_stations, 1);
    }

    #[test]
    fn a_null_power_from_the_stations_dump_does_not_erase_a_known_one() {
        let conn = store();
        conn.execute(
            "INSERT INTO sys_systems (id64, name, controlling_power, power_state, population)
             VALUES (1, 'Alpha', 'A. Lavigny-Duval', 'Stronghold', 500)",
            [],
        )
        .unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stations.json.gz");
        {
            use std::io::Write;
            let f = std::fs::File::create(&path).unwrap();
            let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
            writeln!(enc, "[").unwrap();
            writeln!(enc, "\t{{\"id64\":1,\"name\":\"Alpha\",\"controllingPower\":null,\"powerState\":null,\"population\":null}}").unwrap();
            writeln!(enc, "]").unwrap();
            enc.finish().unwrap();
        }
        import_dump(&conn, &path, |_, _| {}).unwrap();

        let (power, pop): (Option<String>, i64) = conn
            .query_row(
                "SELECT controlling_power, population FROM sys_systems WHERE id64=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(power.as_deref(), Some("A. Lavigny-Duval"));
        assert_eq!(
            pop, 500,
            "population must not be zeroed by the stations dump"
        );
    }
}
