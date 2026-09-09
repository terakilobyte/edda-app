//! The Spansh dump parser and the store that receives it are two modules
//! joined by `GalaxySink`. These tests drive the seam with two adapters --
//! the in-memory recorder and the SQLite importer -- so the parser can be
//! checked without a database and the database adapter can be checked for
//! parity against what the parser actually decoded.

use std::io::Write;
use std::path::Path;

use ed_store::galaxy::{
    self,
    sink::{RecordingSink, SinkEvent},
    spansh::{self, DumpKind},
    ImportStats,
};
use rusqlite::Connection;

const ALPHA: &str = r#"{"id64":1,"name":"Alpha","coords":{"x":1,"y":2,"z":3},"population":100,"date":"2026-08-24 01:00:00+00","controllingPower":"A. Lavigny-Duval","powerState":"Stronghold","stations":[{"id":10,"name":"Port A","type":"Outpost","updateTime":"2026-08-24 01:00:00+00","services":["Dock","Market","Outfitting"],"landingPads":{"large":0,"medium":1,"small":2},"market":{"updateTime":"2026-08-24 01:00:00+00","commodities":[{"symbol":"Gold","name":"Gold","category":"Metals","buyPrice":100,"sellPrice":90,"demand":5,"supply":7}]},"outfitting":{"updateTime":"2026-08-23 01:00:00+00","modules":[{"symbol":"Hpt_PulseLaser_Fixed_Small","name":"Pulse Laser","class":1,"rating":"F"}]}}],"factions":[{"name":"Alpha Dynasty","allegiance":"Empire","influence":0.5}]}"#;
const BETA: &str = r#"{"id64":2,"name":"Beta","coords":{"x":4,"y":5,"z":6},"bodies":[{"id64":200,"name":"Beta 1","type":"Planet","isLandable":true,"materials":{"Iron":20.5},"stations":[{"id":20,"name":"Carrier X","type":"Drake-Class Carrier","services":["Dock","Market"]}]}]}"#;

fn write_lines(path: &Path, gzip: bool) {
    let body = format!("[\n\t{ALPHA},\n\t{BETA}\n]\n").into_bytes();
    if gzip {
        let f = std::fs::File::create(path).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(&body).unwrap();
        enc.finish().unwrap();
    } else {
        std::fs::write(path, body).unwrap();
    }
}

fn fresh_store() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ed_store::schema::migrate(&conn).unwrap();
    ed_store::schema::attach_galaxy(&conn, None).unwrap();
    conn
}

/// Finding: the importer only ever gunzipped, so a plain `galaxy.json`
/// (what `ed-galaxy` already accepts) was rejected as "invalid gzip header".
#[test]
fn import_dump_accepts_uncompressed_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mini.json");
    write_lines(&path, false);

    let conn = fresh_store();
    let stats = galaxy::import_dump(&conn, &path, |_, _| {}).unwrap();
    assert_eq!(stats.systems, 2);
    assert_eq!(stats.stations, 2);
}

/// The parser decodes every record with its timestamps as epochs, without
/// any store involved. A body-hosted station and a system-hosted station
/// both reach the sink; timestamps arrive already parsed from Spansh's
/// `YYYY-MM-DD HH:MM:SS+00` form.
#[test]
fn recording_sink_receives_decoded_records_with_epoch_timestamps() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("galaxy_populated.json.gz");
    write_lines(&path, true);
    assert_eq!(spansh::dump_kind(&path), DumpKind::GalaxyPopulated);

    let mut sink = RecordingSink::default();
    let stats = spansh::stream_dump(&path, &mut sink, |_, _| {}).unwrap();
    assert_eq!(stats.parse_errors, 0);
    assert!(
        stats.bytes_in > 0,
        "compressed byte progress must be counted"
    );

    let epoch = 1_787_533_200i64;
    let systems: Vec<_> = sink
        .events
        .iter()
        .filter_map(|event| match event {
            SinkEvent::System { id64, updated, .. } => Some((*id64, *updated)),
            _ => None,
        })
        .collect();
    assert_eq!(systems, vec![(1, Some(epoch)), (2, None)]);

    let stations: Vec<_> = sink
        .events
        .iter()
        .filter_map(|event| match event {
            SinkEvent::Station {
                system_id64,
                id,
                body_name,
                times,
                ..
            } => Some((*system_id64, *id, body_name.clone(), times.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        stations.len(),
        2,
        "body-hosted stations must reach the sink"
    );
    assert_eq!(stations[0].0, 1);
    assert_eq!(stations[0].1, 10);
    assert_eq!(stations[0].3.station, Some(epoch));
    assert_eq!(stations[0].3.market, epoch);
    // Outfitting carries its own update time; shipyard falls back to the
    // station's.
    assert_eq!(stations[0].3.outfitting, Some(epoch - 86_400));
    assert_eq!(stations[0].3.shipyard, Some(epoch));
    assert_eq!(stations[1].2.as_deref(), Some("Beta 1"));
    // A board with no usable timestamp lands at epoch 0.
    assert_eq!(stations[1].3.market, 0);

    assert!(sink.events.iter().any(|event| matches!(
        event,
        SinkEvent::Body {
            system_id64: 2,
            id64: 200,
            ..
        }
    )));
    assert!(sink.events.iter().any(|event| matches!(
        event,
        SinkEvent::Factions {
            system_id64: 1,
            count: 1,
            ..
        }
    )));
    assert!(matches!(sink.events.last(), Some(SinkEvent::Finish)));
}

/// Two adapters on one seam: the SQLite importer must write exactly the
/// records the recorder saw, and the public entry points must still use it.
#[test]
fn sqlite_adapter_matches_the_recorded_stream() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("galaxy_stations.json.gz");
    write_lines(&path, true);

    let mut recorder = RecordingSink::default();
    spansh::stream_dump(&path, &mut recorder, |_, _| {}).unwrap();
    let recorded_stations = recorder
        .events
        .iter()
        .filter(|event| matches!(event, SinkEvent::Station { .. }))
        .count();

    let conn = fresh_store();
    let stats: ImportStats = galaxy::import_dump(&conn, &path, |_, _| {}).unwrap();
    assert_eq!(stats.stations as usize, recorded_stations);
    let stored: i64 = conn
        .query_row("SELECT count(*) FROM sys_stations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(stored as usize, recorded_stations);
    let market_updated: i64 = conn
        .query_row(
            "SELECT updated FROM sys_market WHERE station_id = 10",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(market_updated, 1_787_533_200);

    // Skip-unchanged survives the split: a second pass writes nothing.
    let again = galaxy::import_dump(&conn, &path, |_, _| {}).unwrap();
    assert_eq!(
        again.skipped_systems, 1,
        "Alpha carries a date and is unchanged"
    );
    assert_eq!(
        again.skipped_stations, 1,
        "Port A carries an updateTime and is unchanged"
    );
}
