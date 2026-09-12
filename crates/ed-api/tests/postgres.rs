use ed_galaxy::star::StarClassCode as _;
use std::{collections::BTreeMap, path::Path};

use axum::{body::Body, http::Request};
use ed_api::{
    config::ServiceConfig,
    database_pool,
    http::{self, AppState},
    hydration::hydrate_fixture,
};
use ed_domain::Operation;
use ed_sync::{ArtifactFile, Manifest, Product, ProductKey, MANIFEST_PROTOCOL_V1};
use rusqlite::Connection;
use serde_json::json;
use sqlx::PgPool;
use tempfile::TempDir;
use tower::ServiceExt;

/// Both ignored tests reset the same database; they must not interleave.
static DATABASE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn postgres_hydration_and_readiness_contract() {
    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let artifact_dir = TempDir::new().unwrap();
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: artifact_dir.path().to_owned(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };

    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;
    database_pool(&config).await.unwrap();

    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/synthetic-galaxy.json");
    let initial = hydrate_fixture(&pool, &fixture_path).await.unwrap();
    assert_eq!(initial.systems_seen, 3);
    assert_eq!(initial.systems_applied, 3);

    let fixture_dir = TempDir::new().unwrap();
    let stale_path = fixture_dir.path().join("stale.json");
    write_fixture(&stale_path, "2020-01-01T00:00:00Z", 1).await;
    let stale = hydrate_fixture(&pool, &stale_path).await.unwrap();
    assert_eq!(stale.systems_applied, 0);
    assert_eq!(sol_population(&pool).await, 22_781_091_954);

    let newer_path = fixture_dir.path().join("newer.json");
    write_fixture(&newer_path, "2026-08-30T00:00:00Z", 42).await;
    let newer = hydrate_fixture(&pool, &newer_path).await.unwrap();
    assert_eq!(newer.systems_applied, 1);
    assert_eq!(sol_population(&pool).await, 42);

    let runs_before_failure: i64 = sqlx::query_scalar("SELECT count(*) FROM service_hydrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    let invalid_path = fixture_dir.path().join("invalid-timestamp.json");
    write_fixture(&invalid_path, "not-a-timestamp", 100).await;
    assert!(hydrate_fixture(&pool, &invalid_path).await.is_err());
    let runs_after_failure: i64 = sqlx::query_scalar("SELECT count(*) FROM service_hydrations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs_after_failure, runs_before_failure);
    assert_eq!(sol_population(&pool).await, 42);

    let response = readiness(&pool, artifact_dir.path()).await;
    assert_eq!(response.status(), 503);

    write_manifest(artifact_dir.path(), MANIFEST_PROTOCOL_V1).await;
    let response = readiness(&pool, artifact_dir.path()).await;
    assert_eq!(response.status(), 200);

    write_manifest(artifact_dir.path(), MANIFEST_PROTOCOL_V1 + 1).await;
    let response = readiness(&pool, artifact_dir.path()).await;
    assert_eq!(response.status(), 503);

    reset_database(&pool).await;
    cross_database_contract(&pool).await;
    reset_database(&pool).await;
}

/// `GET /v1/stations?systems=` (B.4 gap 3): the docks of several systems
/// in one call, in the order the names were given, with unknown names
/// contributing nothing — the game-route fuel marks count from this.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn stations_of_several_systems_come_back_in_route_order() {
    use ed_api::hydration::hydrate_spansh;
    use ed_api::stations::{in_systems, StationsQuery};
    use std::io::Write as _;

    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: std::env::temp_dir(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;

    let dir = TempDir::new().unwrap();
    let dump = dir.path().join("galaxy_stations.json.gz");
    const DUMP: &str = concat!(
        "[\n",
        r#"{"id64":1,"name":"Alpha","coords":{"x":0,"y":0,"z":0},"date":"2026-08-25 00:00:00+00","stations":[{"id":10,"name":"Port A","type":"Coriolis Starport","updateTime":"2026-08-25 00:00:00+00","services":["Dock"]},{"id":11,"name":"Camp A","type":"Settlement","updateTime":"2026-08-25 00:00:00+00","services":["Dock"]}]},"#,
        "\n",
        r#"{"id64":2,"name":"Beta","coords":{"x":10,"y":0,"z":0},"date":"2026-08-25 00:00:00+00","stations":[{"id":20,"name":"Port B","type":"Outpost","updateTime":"2026-08-25 00:00:00+00","services":["Dock"]}]}"#,
        "\n]\n",
    );
    {
        let f = std::fs::File::create(&dump).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(DUMP.as_bytes()).unwrap();
        enc.finish().unwrap();
    }
    hydrate_spansh(&pool, &dump).await.unwrap();

    let q = serde_urlencoded::from_str::<StationsQuery>("systems=beta,Nowhere,ALPHA").unwrap();
    let ed_api::stations::Mode::InSystems(names) = q.mode().unwrap() else {
        panic!("list mode")
    };
    let rows = in_systems(&pool, &names, &q).await.unwrap();
    let got: Vec<(&str, &str)> = rows
        .iter()
        .map(|v| {
            (
                v["system_name"].as_str().unwrap(),
                v["name"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        got,
        vec![("Beta", "Port B"), ("Alpha", "Port A"), ("Alpha", "Camp A")],
        "route order, then class rank; Nowhere contributes nothing"
    );

    let q =
        serde_urlencoded::from_str::<StationsQuery>("systems=Alpha&include_minor=false").unwrap();
    let ed_api::stations::Mode::InSystems(names) = q.mode().unwrap() else {
        panic!("list mode")
    };
    let rows = in_systems(&pool, &names, &q).await.unwrap();
    assert_eq!(rows.len(), 1, "the settlement is minor");
    reset_database(&pool).await;
}

/// The feed's teachings land in the same tables the dump fills (2026-09-09):
/// a Scan's star class reaches `stars`, a planet Scan `bodies` with its
/// materials and rings, a ring's SAASignalsFound `ring_hotspots` under the
/// parent found by name, body signals onto the body — and the mining
/// search answers from them without any dump having run.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn feed_scans_teach_stars_bodies_and_hotspots() {
    use ed_api::mining::{search, MiningSearchRequest};
    use ed_domain::{
        body_id64, BodySignals, BodyTeaching, ObservedAt, Operation, RingHotspots, RingTeaching,
        StarTeaching, SystemObservation,
    };

    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: std::env::temp_dir(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;

    let at = |secs: i64| ObservedAt {
        timestamp: "2026-09-09T10:00:00Z".into(),
        epoch_seconds: 1_788_000_000 + secs,
    };
    let address = 6_681_123_623_626i64;
    let ops = vec![
        Operation::System(SystemObservation {
            system_name: "Deciat".into(),
            system_address: Some(address),
            position: Some([0.0, 0.0, 0.0]),
            observed_at: at(0),
            controlling_power: None,
            powerplay_state: None,
            powers: None,
            population: None,
            security: None,
            allegiance: None,
        }),
        Operation::Star(StarTeaching {
            system_address: address,
            system_name: Some("Deciat".into()),
            position: Some([0.0, 0.0, 0.0]),
            star_type: "K".into(),
            observed_at: at(1),
            source: "eddn:scan".into(),
        }),
        // A ring's hotspots BEFORE its parent body exists: skipped, not an error.
        Operation::RingHotspots(RingHotspots {
            system_address: address,
            ring_name: "Deciat 6 a A Ring".into(),
            signals: vec![("Painite".into(), 2)],
            observed_at: at(2),
        }),
        Operation::Body(BodyTeaching {
            id64: body_id64(address, 7),
            system_address: address,
            body_id: Some(7),
            name: Some("Deciat 6 a".into()),
            kind: Some("Planet".into()),
            sub_type: Some("Rocky body".into()),
            is_landable: true,
            distance_to_arrival: Some(1510.0),
            gravity: Some(0.12),
            atmosphere: None,
            volcanism: None,
            bio_signals: None,
            geo_signals: None,
            observed_at: at(3),
            provenance: "eddn:scan".into(),
            materials: vec![("Iron".into(), 21.3)],
            rings: vec![RingTeaching {
                name: "Deciat 6 a A Ring".into(),
                kind: Some("Metallic".into()),
                mass: None,
                inner_radius: None,
                outer_radius: None,
            }],
            hotspots: Vec::new(),
        }),
        Operation::RingHotspots(RingHotspots {
            system_address: address,
            ring_name: "Deciat 6 a A Ring".into(),
            signals: vec![("Painite".into(), 2), ("Platinum".into(), 1)],
            observed_at: at(4),
        }),
        Operation::BodySignals(BodySignals {
            id64: body_id64(address, 7),
            system_address: address,
            body_id: Some(7),
            name: Some("Deciat 6 a".into()),
            bio_signals: Some(3),
            geo_signals: None,
            observed_at: at(5),
        }),
        // Signals for a body no Scan has described yet: a stub row.
        Operation::BodySignals(BodySignals {
            id64: body_id64(address, 9),
            system_address: address,
            body_id: Some(9),
            name: Some("Deciat 6 b".into()),
            bio_signals: Some(1),
            geo_signals: None,
            observed_at: at(6),
        }),
    ];
    let stats = ed_api::eddn::apply_operations(&pool, &ops).await.unwrap();
    assert_eq!(
        (
            stats.stars,
            stats.bodies,
            stats.hotspots,
            stats.body_signals,
            stats.skipped
        ),
        (1, 1, 2, 2, 1),
        "{stats:?}"
    );

    let star: (i16, bool, String) =
        sqlx::query_as("SELECT class, scoopable, source FROM stars WHERE address = $1")
            .bind(address)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        star,
        (6, true, "eddn:scan".to_string()),
        "K is code 6, scoopable"
    );
    let hotspots: Vec<(String, i32)> = sqlx::query_as(
        "SELECT material, count FROM ring_hotspots WHERE body_id64 = $1 ORDER BY material",
    )
    .bind(body_id64(address, 7))
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        hotspots,
        vec![("Painite".to_string(), 2), ("Platinum".to_string(), 1)]
    );
    let signals: (Option<i32>, Option<i32>) =
        sqlx::query_as("SELECT bio_signals, geo_signals FROM bodies WHERE id64 = $1")
            .bind(body_id64(address, 7))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(signals, (Some(3), None));
    let stub: (Option<String>, Option<i32>) =
        sqlx::query_as("SELECT name, bio_signals FROM bodies WHERE id64 = $1")
            .bind(body_id64(address, 9))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stub, (Some("Deciat 6 b".to_string()), Some(1)));

    // An older star observation cannot regress the class.
    let older = vec![Operation::Star(StarTeaching {
        system_address: address,
        system_name: None,
        position: None,
        star_type: "M".into(),
        observed_at: at(-100),
        source: "eddn:navroute".into(),
    })];
    let stats = ed_api::eddn::apply_operations(&pool, &older).await.unwrap();
    assert_eq!((stats.stars, stats.skipped), (0, 1));

    // And the mining search sees what the feed taught.
    let req = MiningSearchRequest {
        text: "painite".into(),
        system: Some("Deciat".into()),
        coords: None,
        radius_ly: Some(10.0),
        limit: None,
    };
    let answer = search(&pool, &req).await.unwrap();
    assert_eq!(
        answer["hotspots"][0]["ring"], "Deciat 6 a A Ring",
        "{answer}"
    );
    assert_eq!(answer["hotspots"][0]["ring_type"], "Metallic");
    reset_database(&pool).await;
}

/// The mining search (B.4): a dump's ringed giant and landable rock
/// become hotspot, ring and body rows through the same hydration job as
/// the stations; the three lists answer inside the sphere in the page's
/// shape; an older dump cannot regress a body's hotspots.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn mining_search_answers_from_hydrated_bodies() {
    use ed_api::hydration::hydrate_spansh;
    use ed_api::mining::{search, vocabulary, MiningSearchRequest};
    use std::io::Write as _;

    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: std::env::temp_dir(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;

    let dir = TempDir::new().unwrap();
    let write_dump = |name: &str, text: &str| {
        let path = dir.path().join(name);
        let f = std::fs::File::create(&path).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(text.as_bytes()).unwrap();
        enc.finish().unwrap();
        path
    };
    const DUMP: &str = concat!(
        "[\n",
        r#"{"id64":5,"name":"Deciat","coords":{"x":0,"y":0,"z":0},"date":"2026-08-25 00:00:00+00","stations":[],"bodies":["#,
        r#"{"id64":500,"bodyId":0,"name":"Deciat","type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true},"#,
        r#"{"id64":506,"bodyId":6,"name":"Deciat 6","type":"Planet","subType":"Class II gas giant","distanceToArrival":1500,"updateTime":"2026-08-25 00:00:00+00","rings":[{"name":"Deciat 6 A Ring","type":"Metallic","signals":{"signals":{"Platinum":2,"Painite":1}}}]},"#,
        r#"{"id64":507,"bodyId":7,"name":"Deciat 6 a","type":"Planet","subType":"Rocky body","isLandable":true,"gravity":0.1,"distanceToArrival":1510,"materials":{"Iron":21.3}}]},"#,
        "\n",
        r#"{"id64":6,"name":"Far","coords":{"x":400,"y":0,"z":0},"date":"2026-08-25 00:00:00+00","stations":[],"bodies":[{"id64":600,"bodyId":1,"name":"Far 1","type":"Planet","subType":"Class I gas giant","rings":[{"name":"Far 1 A Ring","type":"Metallic","signals":{"signals":{"Platinum":3}}}]}]}"#,
        "\n]\n",
    );
    let result = hydrate_spansh(&pool, &write_dump("galaxy_1day.json.gz", DUMP))
        .await
        .unwrap();
    assert_eq!(result.systems_applied, 2);
    assert_eq!(
        result.bodies_applied, 3,
        "the two giants and the rock; the bare star is the stars table's"
    );
    assert_eq!(result.hotspots_applied, 3);
    assert_eq!(
        result.stars_taught, 1,
        "the main star still reaches the routing teacher"
    );

    // A hotspot material by system name, 100 ly: Deciat's ring, not Far's at 400 ly.
    let req = MiningSearchRequest {
        text: "platinum".into(),
        system: Some("deciat".into()),
        coords: None,
        radius_ly: Some(100.0),
        limit: None,
    };
    let answer = search(&pool, &req).await.unwrap();
    assert_eq!(answer["origin"], "deciat");
    assert_eq!(answer["known_hotspot"], true);
    assert_eq!(answer["hotspots"].as_array().unwrap().len(), 1, "{answer}");
    assert_eq!(answer["hotspots"][0]["ring"], "Deciat 6 A Ring");
    assert_eq!(answer["hotspots"][0]["ring_type"], "Metallic");
    assert_eq!(answer["hotspots"][0]["count"], 2);
    assert_eq!(answer["hotspots"][0]["distance_to_arrival"], 1500.0);
    assert_eq!(
        answer["rings"].as_array().unwrap().len(),
        0,
        "a hotspot material lists hotspots, not rings"
    );
    assert_eq!(answer["data_installed"], true);

    // A laser-mined good by the commander's own coordinates, 500 ly:
    // rings of the type, nearest first.
    let req = MiningSearchRequest {
        text: "Gold".into(),
        system: None,
        coords: Some([0.0, 0.0, 0.0]),
        radius_ly: Some(500.0),
        limit: None,
    };
    let answer = search(&pool, &req).await.unwrap();
    assert_eq!(answer["known_hotspot"], false);
    assert_eq!(answer["ring_hint"]["type"], "Metallic");
    let rings = answer["rings"].as_array().unwrap();
    assert_eq!(rings.len(), 2, "{answer}");
    assert_eq!(rings[0]["system"], "Deciat");
    assert_eq!(rings[1]["system"], "Far");
    assert_eq!(rings[1]["distance_ly"], 400.0);

    // A surface material: the landable rock, richest first.
    let req = MiningSearchRequest {
        text: "iron".into(),
        system: Some("Deciat".into()),
        coords: None,
        radius_ly: None,
        limit: None,
    };
    let answer = search(&pool, &req).await.unwrap();
    assert_eq!(answer["known_surface"], true);
    assert_eq!(answer["bodies"][0]["body"], "Deciat 6 a");
    assert_eq!(answer["bodies"][0]["percent"], 21.3);
    assert_eq!(answer["bodies"][0]["gravity"], 0.1);

    // An unknown system is the same refusal the market search gives.
    let req = MiningSearchRequest {
        text: "iron".into(),
        system: Some("Nowhere".into()),
        coords: None,
        radius_ly: None,
        limit: None,
    };
    assert!(matches!(
        search(&pool, &req).await,
        Err(ed_api::market_search::Refusal::UnknownSystem(_))
    ));

    // An older dump of the giant, with a different hotspot count, cannot
    // regress the newer row; a newer one rewrites the children with it.
    const OLDER: &str = concat!(
        "[\n",
        r#"{"id64":5,"name":"Deciat","coords":{"x":0,"y":0,"z":0},"date":"2026-08-25 00:00:00+00","stations":[],"bodies":[{"id64":506,"bodyId":6,"name":"Deciat 6","type":"Planet","subType":"Class II gas giant","updateTime":"2026-08-20 00:00:00+00","rings":[{"name":"Deciat 6 A Ring","type":"Metallic","signals":{"signals":{"Platinum":9}}}]}]}"#,
        "\n]\n",
    );
    let older = hydrate_spansh(&pool, &write_dump("older.json.gz", OLDER))
        .await
        .unwrap();
    assert_eq!(older.bodies_applied, 0, "older than the stored body");
    let count: (i32,) = sqlx::query_as(
        "SELECT count FROM ring_hotspots WHERE body_id64 = 506 AND material = 'Platinum'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count.0, 2);
    const NEWER: &str = concat!(
        "[\n",
        r#"{"id64":5,"name":"Deciat","coords":{"x":0,"y":0,"z":0},"date":"2026-08-25 00:00:00+00","stations":[],"bodies":[{"id64":506,"bodyId":6,"name":"Deciat 6","type":"Planet","subType":"Class II gas giant","updateTime":"2026-08-30 00:00:00+00","rings":[{"name":"Deciat 6 A Ring","type":"Metallic","signals":{"signals":{"Platinum":4}}}]}]}"#,
        "\n]\n",
    );
    let newer = hydrate_spansh(&pool, &write_dump("newer.json.gz", NEWER))
        .await
        .unwrap();
    assert_eq!(newer.bodies_applied, 1);
    let rows: Vec<(String, i32)> = sqlx::query_as(
        "SELECT material, count FROM ring_hotspots WHERE body_id64 = 506 ORDER BY material",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![("Platinum".to_string(), 4)],
        "Painite is gone with the rewrite; Platinum carries the new count"
    );

    let vocab = vocabulary(&pool).await.unwrap();
    let names: Vec<(&str, &str)> = vocab
        .iter()
        .map(|e| (e.stored.as_str(), e.kind.as_str()))
        .collect();
    assert_eq!(names, vec![("Platinum", "hotspot"), ("Iron", "surface")]);
    reset_database(&pool).await;
}

async fn reset_database(pool: &PgPool) {
    sqlx::raw_sql(
        "TRUNCATE market, outfitting, shipyard, stations, commodities, modules, ships, \
         systems, bodies, stars, service_hydrations, artifact_publications RESTART IDENTITY CASCADE; \
         UPDATE eddn_ingestion SET received = 0, applied = 0, skipped = 0, errors = 0, \
         last_message_at = NULL, updated_at = now() WHERE singleton;",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn cross_database_contract(pool: &PgPool) {
    let operations = contract_operations();
    let postgres_stats = ed_api::eddn::apply_operations(pool, &operations)
        .await
        .unwrap();

    let sqlite = Connection::open_in_memory().unwrap();
    ed_store::schema::migrate(&sqlite).unwrap();
    ed_store::schema::attach_galaxy(&sqlite, None).unwrap();
    let mut sqlite_skipped = 0;
    for operation in &operations {
        sqlite_skipped += ed_store::eddn::apply_operation(&sqlite, operation)
            .unwrap()
            .skipped;
    }
    assert_eq!(postgres_stats.skipped, sqlite_skipped);
    assert_eq!(postgres_stats.skipped, 5);

    let postgres_market: Vec<(String, i64)> =
        sqlx::query_as("SELECT commodity_symbol, sell_price FROM market ORDER BY commodity_symbol")
            .fetch_all(pool)
            .await
            .unwrap();
    let sqlite_market = sqlite
        .prepare(
            "SELECT c.symbol, m.sell_price FROM sys_market m \
             JOIN sys_commodities c ON c.id = m.commodity_id ORDER BY c.symbol",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<(String, i64)>>>()
        .unwrap();
    assert_eq!(postgres_market, sqlite_market);
    // Galileo keeps gold; Empty Port's board was cleared at 04:00 and the
    // 03:30 replay must not have refilled it -- in either store.
    assert_eq!(postgres_market, vec![("gold".to_owned(), 60_000)]);
    let postgres_empty_port: i64 =
        sqlx::query_scalar("SELECT count(*) FROM market WHERE station_id = 11")
            .fetch_one(pool)
            .await
            .unwrap();
    let sqlite_empty_port: i64 = sqlite
        .query_row(
            "SELECT count(*) FROM sys_market WHERE station_id = 11",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!((postgres_empty_port, sqlite_empty_port), (0, 0));

    let postgres_module: String =
        sqlx::query_scalar("SELECT module_symbol FROM outfitting LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap();
    let sqlite_module: String = sqlite
        .query_row(
            "SELECT m.symbol FROM sys_outfitting o JOIN sys_modules m ON m.id = o.module_id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(postgres_module, sqlite_module);

    let postgres_ship: String = sqlx::query_scalar("SELECT ship_symbol FROM shipyard LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap();
    let sqlite_ship: String = sqlite
        .query_row(
            "SELECT s.symbol FROM sys_shipyard y JOIN sys_ships s ON s.id = y.ship_id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(postgres_ship, sqlite_ship);

    let postgres_power: String =
        sqlx::query_scalar("SELECT controlling_power FROM systems WHERE address = 20")
            .fetch_one(pool)
            .await
            .unwrap();
    let sqlite_power: String = sqlite
        .query_row(
            "SELECT controlling_power FROM sys_systems WHERE id64 = 20",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(postgres_power, sqlite_power);
    assert_eq!(postgres_power, "Aisling Duval");

    verify_market_publication(pool).await;
}

async fn verify_market_publication(pool: &PgPool) {
    // Display metadata rides once hydrated: FDevIDs names/categories must
    // reach both the market dictionary and section 4. The CSV symbol is
    // CamelCase exactly as FDevIDs publishes it; the metadata must land
    // on the lowercase row the market ingest created, not beside it
    // (the first real hydrate grew the catalog 399 -> 656 that way).
    let fdev = TempDir::new().unwrap();
    let csv = fdev.path().join("commodity.csv");
    std::fs::write(&csv, "id,symbol,category,name\n1,Gold,Metals,Gold\n").unwrap();
    assert_eq!(
        ed_api::fdev_ids::hydrate_commodities(pool, &csv)
            .await
            .unwrap(),
        1
    );

    let artifacts = TempDir::new().unwrap();
    let publication = ed_api::snapshot::publish_market(pool, artifacts.path())
        .await
        .unwrap();
    assert_eq!(publication.rows, 1);
    assert!(publication
        .artifact
        .to_string_lossy()
        .ends_with(".ebex.zst"));
    let compressed = tokio::fs::read(artifacts.path().join(&publication.artifact))
        .await
        .unwrap();
    assert_eq!(compressed.len() as u64, publication.bytes);
    let decoded = ed_ebex::decompress(&compressed).unwrap();
    let metadata = ed_ebex::validate_snapshot(&decoded).unwrap();
    assert_eq!(metadata.sequence, publication.sequence as u64);
    // 8 original sections + station details + prohibited (the
    // 2026-09-04 addendum; this pin lagged until the full ignored
    // suite ran again on 2026-09-06).
    assert_eq!(metadata.section_count, 10);
    for section in ed_ebex::sections(&decoded).unwrap() {
        assert_eq!(
            section.required,
            section.id == ed_ebex::SECTION_MARKETS,
            "section {} required flag",
            section.id
        );
    }
    ed_ebex::validate_market_baseline(&decoded, ed_ebex::MARKET_BASELINE_SECTIONS).unwrap();
    assert_eq!(&decoded[..8], b"EBEX\0\0\0\0");
    let systems = ed_ebex::section(&decoded, ed_ebex::SECTION_SYSTEMS)
        .unwrap()
        .unwrap();
    let stations = ed_ebex::section(&decoded, ed_ebex::SECTION_STATIONS)
        .unwrap()
        .unwrap();
    assert_eq!(ed_ebex::system_records(systems).unwrap().count(), 2);
    // Galileo and Empty Port; the latter has a market watermark but no rows.
    assert_eq!(ed_ebex::station_records(stations).unwrap().count(), 2);
    let commodities = ed_ebex::section(&decoded, ed_ebex::SECTION_COMMODITIES)
        .unwrap()
        .unwrap();
    let commodity_strings: std::collections::BTreeMap<u32, String> =
        ed_ebex::string_table(commodities)
            .unwrap()
            .into_iter()
            .map(|s| (s.id, s.value))
            .collect();
    let catalog: Vec<(String, String, String)> = ed_ebex::commodity_records(commodities)
        .unwrap()
        .map(|r| {
            let resolve = |id: u32| commodity_strings.get(&id).cloned().unwrap_or_default();
            (
                resolve(r.symbol_id),
                resolve(r.name_id),
                resolve(r.category_id),
            )
        })
        .collect();
    assert_eq!(
        catalog,
        vec![
            ("gold".into(), "Gold".into(), "Metals".into()),
            ("silver".into(), String::new(), String::new()),
        ],
        "hydrated metadata reaches section 4; unhydrated symbols stay absent"
    );
    let dictionary = ed_ebex::market_auxiliary(
        ed_ebex::section(&decoded, ed_ebex::SECTION_MARKETS)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        dictionary
            .commodities
            .iter()
            .map(|c| (c.symbol.as_str(), c.name.as_str(), c.category.as_str()))
            .collect::<Vec<_>>(),
        vec![("gold", "Gold", "Metals"), ("silver", "", "")],
        "the market dictionary the client hydrates carries the same names"
    );
    assert_eq!(
        ed_ebex::module_records(
            ed_ebex::section(&decoded, ed_ebex::SECTION_MODULES)
                .unwrap()
                .unwrap()
        )
        .unwrap()
        .count(),
        1
    );
    let outfitting = ed_ebex::section(&decoded, ed_ebex::SECTION_OUTFITTING)
        .unwrap()
        .unwrap();
    assert_eq!(ed_ebex::outfitting_records(outfitting).unwrap().count(), 1);
    assert_eq!(ed_ebex::station_snapshots(outfitting).unwrap().len(), 1);
    assert_eq!(
        ed_ebex::ship_records(
            ed_ebex::section(&decoded, ed_ebex::SECTION_SHIPS)
                .unwrap()
                .unwrap()
        )
        .unwrap()
        .count(),
        1
    );
    let shipyard = ed_ebex::section(&decoded, ed_ebex::SECTION_SHIPYARDS)
        .unwrap()
        .unwrap();
    assert_eq!(ed_ebex::shipyard_records(shipyard).unwrap().count(), 1);
    assert_eq!(ed_ebex::station_snapshots(shipyard).unwrap().len(), 1);

    let app = http::router(AppState::new(
        pool.clone(),
        artifacts.path().to_owned(),
        test_metrics(),
    ));
    let manifest = app
        .clone()
        .oneshot(Request::get("/v1/manifest").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(manifest.status(), 200);
    let artifact = app
        .oneshot(
            Request::get(format!("/v1/artifacts/{}", publication.artifact.display()))
                .header("range", "bytes=0-7")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(artifact.status(), 206);
    assert_eq!(artifact.headers()["accept-ranges"], "bytes");
}

fn contract_operations() -> Vec<Operation> {
    [
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},"message":{"systemName":"Sol","stationName":"Galileo","marketId":10,"timestamp":"2026-08-24T02:00:00Z","commodities":[{"name":"gold","sellPrice":50000},{"name":"silver","sellPrice":100}]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},"message":{"systemName":"Sol","stationName":"Galileo","marketId":10,"timestamp":"2026-08-24T03:00:00Z","commodities":[{"name":"gold","sellPrice":60000}]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},"message":{"systemName":"Sol","stationName":"Galileo","marketId":10,"timestamp":"2026-08-24T01:00:00Z","commodities":[{"name":"silver","sellPrice":1}]}}"#,
        // Empty Port: stocked, then an empty board deletes every row, then an
        // older replay arrives. The station watermark -- not MAX over the
        // (now absent) rows -- must reject the replay in both stores.
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},"message":{"systemName":"Sol","stationName":"Empty Port","marketId":11,"timestamp":"2026-08-24T02:00:00Z","commodities":[{"name":"gold","sellPrice":1000}]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},"message":{"systemName":"Sol","stationName":"Empty Port","marketId":11,"timestamp":"2026-08-24T04:00:00Z","commodities":[]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},"message":{"systemName":"Sol","stationName":"Empty Port","marketId":11,"timestamp":"2026-08-24T03:30:00Z","commodities":[{"name":"silver","sellPrice":5}]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/outfitting/2","header":{},"message":{"systemName":"Sol","stationName":"Galileo","marketId":10,"timestamp":"2026-08-24T03:00:00Z","modules":["int_hyperdrive_size2_class1"]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/outfitting/2","header":{},"message":{"systemName":"Sol","stationName":"Galileo","marketId":10,"timestamp":"2026-08-24T01:00:00Z","modules":["stale_module"]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/shipyard/2","header":{},"message":{"systemName":"Sol","stationName":"Galileo","marketId":10,"timestamp":"2026-08-24T03:00:00Z","ships":["cobramkiii"]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/shipyard/2","header":{},"message":{"systemName":"Sol","stationName":"Galileo","marketId":10,"timestamp":"2026-08-24T01:00:00Z","ships":["sidewinder"]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{},"message":{"event":"FSDJump","StarSystem":"Achenar","SystemAddress":20,"timestamp":"2026-08-24T03:00:00Z","ControllingPower":"Aisling Duval"}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{},"message":{"event":"FSDJump","StarSystem":"Achenar","SystemAddress":20,"timestamp":"2026-08-24T01:00:00Z","ControllingPower":"Someone Older"}}"#,
    ]
    .into_iter()
    .map(|raw| ed_eddn::decode(raw.as_bytes()).unwrap().operation().unwrap())
    .collect()
}

async fn sol_population(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT population FROM systems WHERE name = 'Sol'")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn write_fixture(path: &Path, observed_at: &str, population: i64) {
    let fixture = json!({
        "source": "integration-test",
        "observed_at": observed_at,
        "systems": [{
            "address": 10477373803_i64,
            "name": "Sol",
            "x": 0.0,
            "y": 0.0,
            "z": 0.0,
            "population": population
        }]
    });
    tokio::fs::write(path, serde_json::to_vec(&fixture).unwrap())
        .await
        .unwrap();
}

async fn write_manifest(artifact_dir: &Path, protocol: u32) {
    let manifest = Manifest {
        protocol,
        generated_at: "2026-08-30T00:00:00Z".to_owned(),
        eddn_watermark: Some("2026-08-29T23:59:59Z".to_owned()),
        products: BTreeMap::from([(
            ProductKey::Bootstrap,
            Product {
                version: "2026-08-30.1".to_owned(),
                schema: 1,
                minimum_client: Some("0.1.0".to_owned()),
                files: vec![ArtifactFile {
                    path: "community-1.ebex.zst".to_owned(),
                    bytes: 123,
                    sha256: "0".repeat(64),
                }],
                overlays: Vec::new(),
                covers_from: None,
            },
        )]),
    };
    tokio::fs::write(
        artifact_dir.join("current.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .await
    .unwrap();
}

async fn readiness(pool: &PgPool, artifact_dir: &Path) -> axum::response::Response {
    http::router(AppState::new(
        pool.clone(),
        artifact_dir.to_owned(),
        test_metrics(),
    ))
    .oneshot(Request::get("/readyz").body(Body::empty()).unwrap())
    .await
    .unwrap()
}

/// Finding: a board listing one symbol twice violated the relation's
/// primary key, which failed the whole batch -- and the live EDDN writer
/// retries a failed batch forever. `ed_eddn::decode` happens to
/// deduplicate, but the write path is shared with adapters that build
/// operations directly (Spansh hydration), so it must cope on its own.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn a_board_listing_a_symbol_twice_is_applied_once() {
    use ed_domain::{Commodity, ObservedAt, Snapshot};

    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: std::env::temp_dir(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;

    fn board<T>(values: Vec<T>) -> Snapshot<T> {
        Snapshot {
            system_name: "Sol".to_owned(),
            station_name: Some("Galileo".to_owned()),
            market_id: Some(10),
            observed_at: ObservedAt {
                timestamp: "2026-08-24T03:00:00Z".to_owned(),
                epoch_seconds: 1_787_540_400,
            },
            values,
            prohibited: Vec::new(),
        }
    }
    let gold = |sell_price| Commodity {
        name: "gold".to_owned(),
        buy_price: 0,
        sell_price,
        demand: 0,
        stock: 0,
    };
    let operations = vec![
        Operation::Outfitting(board(vec![
            "int_hyperdrive_size2_class1".to_owned(),
            "int_hyperdrive_size2_class1".to_owned(),
            "int_fueltank_size2".to_owned(),
        ])),
        Operation::Shipyard(board(vec![
            "cobramkiii".to_owned(),
            "cobramkiii".to_owned(),
        ])),
        Operation::Market(board(vec![gold(1), gold(2)])),
    ];
    let stats = ed_api::eddn::apply_operations(&pool, &operations)
        .await
        .expect("duplicate symbols within one board must not fail the batch");
    assert_eq!(stats.skipped, 0);
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM outfitting WHERE station_id = 10), \
                (SELECT count(*) FROM shipyard WHERE station_id = 10), \
                (SELECT count(*) FROM market WHERE station_id = 10)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (2, 1, 1));
    reset_database(&pool).await;
}

/// Finding: the service could only be seeded from the synthetic fixture.
/// A Spansh dump must stream into the service tables as a recorded job,
/// through the same write path as EDDN, so an older dump board never
/// regresses a fresher live one, and a provisional (name-only) EDDN
/// system is promoted to its real address with its stations.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn a_system_whose_name_another_address_holds_is_left_out_with_its_stations() {
    use ed_api::hydration::hydrate_spansh;
    use std::io::Write as _;

    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: std::env::temp_dir(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;

    // The real populated dump carries twenty such pairs (HIP 60025,
    // 88 G. Carinae, ...). Before this was handled the second system's
    // station violated the foreign key and the whole hydration died.
    let dir = TempDir::new().unwrap();
    let dump = dir.path().join("galaxy_populated.json.gz");
    const DUMP: &str = concat!(
        "[\n",
        r#"{"id64":5,"name":"HIP 60025","coords":{"x":1,"y":2,"z":3},"population":100,"date":"2026-08-25 00:00:00+00","stations":[{"id":30,"name":"Port A","type":"Outpost","updateTime":"2026-08-25 00:00:00+00","services":["Dock","Market"],"market":{"updateTime":"2026-08-25 00:00:00+00","commodities":[{"symbol":"Silver","sellPrice":9}]}}]},"#,
        "\n",
        r#"{"id64":6,"name":"Hip 60025","coords":{"x":4,"y":5,"z":6},"population":7,"date":"2026-08-25 00:00:00+00","stations":[{"id":31,"name":"Port B","type":"Outpost","updateTime":"2026-08-25 00:00:00+00","services":["Dock","Market"],"market":{"updateTime":"2026-08-25 00:00:00+00","commodities":[{"symbol":"Gold","sellPrice":1}]}}]},"#,
        "\n",
        r#"{"id64":7,"name":"Alpha","coords":{"x":7,"y":8,"z":9},"population":1,"date":"2026-08-25 00:00:00+00","stations":[]}"#,
        "\n]\n",
    );
    {
        let f = std::fs::File::create(&dump).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(DUMP.as_bytes()).unwrap();
        enc.finish().unwrap();
    }

    let result = hydrate_spansh(&pool, &dump).await.unwrap();
    assert_eq!(result.systems_seen, 3);
    assert_eq!(result.systems_applied, 2);
    assert_eq!(result.systems_unfiled, 1);
    assert_eq!(result.stations_seen, 2);
    assert_eq!(result.snapshots_applied, 1, "only Port A's board");
    assert_eq!(result.parse_errors, 0);

    // The same pair again, but the namesake's first board is large enough
    // to end a batch (BATCH_ROWS), so its second station reaches the
    // writer in the next batch -- which must still know the system was
    // never filed.
    reset_database(&pool).await;
    let big_board: Vec<String> = (0..50_001)
        .map(|i| format!(r#"{{"symbol":"c{i}","sellPrice":1}}"#))
        .collect();
    let straddle = format!(
        concat!(
            "[
",
            r#"{{"id64":5,"name":"HIP 60025","coords":{{"x":1,"y":2,"z":3}},"population":100,"date":"2026-08-25 00:00:00+00","stations":[{{"id":30,"name":"Port A","type":"Outpost","updateTime":"2026-08-25 00:00:00+00","services":["Dock","Market"],"market":{{"updateTime":"2026-08-25 00:00:00+00","commodities":[{{"symbol":"Silver","sellPrice":9}}]}}}}]}},"#,
            "
",
            r#"{{"id64":6,"name":"Hip 60025","coords":{{"x":4,"y":5,"z":6}},"population":7,"date":"2026-08-25 00:00:00+00","stations":[{{"id":31,"name":"Port B","type":"Outpost","updateTime":"2026-08-25 00:00:00+00","services":["Dock","Market"],"market":{{"updateTime":"2026-08-25 00:00:00+00","commodities":[{}]}}}},{{"id":32,"name":"Port C","type":"Outpost","updateTime":"2026-08-25 00:00:00+00","services":["Dock","Market"],"market":{{"updateTime":"2026-08-25 00:00:00+00","commodities":[{{"symbol":"Gold","sellPrice":1}}]}}}}]}}"#,
            "
]
",
        ),
        big_board.join(",")
    );
    {
        let f = std::fs::File::create(&dump).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(straddle.as_bytes()).unwrap();
        enc.finish().unwrap();
    }
    let result = hydrate_spansh(&pool, &dump).await.unwrap();
    assert_eq!(result.systems_applied, 1);
    assert_eq!(result.systems_unfiled, 1);
    assert_eq!(result.stations_seen, 3);
    assert_eq!(result.snapshots_applied, 1, "only Port A's board");
    let stations: Vec<i64> = sqlx::query_scalar("SELECT id FROM stations ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        stations,
        vec![30],
        "Port C arrived in a later batch and must still be dropped"
    );
    reset_database(&pool).await;
    {
        let f = std::fs::File::create(&dump).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(DUMP.as_bytes()).unwrap();
        enc.finish().unwrap();
    }
    let result = hydrate_spansh(&pool, &dump).await.unwrap();
    assert_eq!(result.parse_errors, 0);

    let addresses: Vec<i64> = sqlx::query_scalar("SELECT address FROM systems ORDER BY address")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(addresses, vec![5, 7]);
    let stations: Vec<(i64, i64)> =
        sqlx::query_as("SELECT id, system_address FROM stations ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        stations,
        vec![(30, 5)],
        "Port B must not land on the namesake"
    );
    let boards: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT station_id FROM market ORDER BY 1")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(boards, vec![30]);
}

#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn spansh_hydration_never_regresses_fresher_eddn_rows() {
    use ed_api::{hydration::hydrate_spansh, routing, snapshot};
    use std::io::Write as _;

    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: std::env::temp_dir(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;

    // Live observations first: Galileo's board at 03:00 and a provisional
    // system the feed only knows by name.
    let live: Vec<Operation> = [
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},"message":{"systemName":"Sol","stationName":"Galileo","marketId":10,"timestamp":"2026-08-24T03:00:00Z","commodities":[{"name":"gold","sellPrice":60000}]}}"#,
        r#"{"$schemaRef":"https://eddn.edcd.io/schemas/commodity/3","header":{},"message":{"systemName":"Alpha","stationName":"Port A","marketId":30,"timestamp":"2026-08-24T03:00:00Z","commodities":[{"name":"silver","sellPrice":5}]}}"#,
    ]
    .into_iter()
    .map(|raw| ed_eddn::decode(raw.as_bytes()).unwrap().operation().unwrap())
    .collect();
    ed_api::eddn::apply_operations(&pool, &live).await.unwrap();
    let provisional: i64 = sqlx::query_scalar("SELECT address FROM systems WHERE name = 'Alpha'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(provisional < 0);

    let dir = TempDir::new().unwrap();
    let dump = dir.path().join("galaxy_populated.json.gz");
    // Sol: Galileo's dump board is older than the live one (skipped);
    // Daedalus is new (applied). Alpha: real address 5, and a newer board
    // for Port A (applied).
    const DUMP: &str = concat!(
        "[\n",
        r#"{"id64":10477373803,"name":"Sol","coords":{"x":0,"y":0,"z":0},"population":22781091954,"date":"2026-08-24 02:00:00+00","security":"High","allegiance":"Federation","stations":[{"id":10,"name":"Galileo","type":"Ocellus Starport","updateTime":"2026-08-24 02:00:00+00","services":["Dock","Market"],"market":{"updateTime":"2026-08-24 02:00:00+00","commodities":[{"symbol":"Gold","sellPrice":1}]}},{"id":12,"name":"Daedalus","type":"Orbis Starport","updateTime":"2026-08-24 01:00:00+00","services":["Dock","Market","Shipyard"],"market":{"commodities":[{"symbol":"Gold","sellPrice":700},{"symbol":"Silver","sellPrice":300}]},"shipyard":{"ships":[{"symbol":"CobraMkIII"}]}}]},"#,
        "\n",
        r#"{"id64":5,"name":"Alpha","coords":{"x":1,"y":2,"z":3},"population":100,"date":"2026-08-25 00:00:00+00","stations":[{"id":30,"name":"Port A","type":"Outpost","updateTime":"2026-08-25 00:00:00+00","services":["Dock","Market"],"market":{"updateTime":"2026-08-25 00:00:00+00","commodities":[{"symbol":"Silver","sellPrice":9}]}}]}"#,
        "\n]\n",
    );
    {
        let f = std::fs::File::create(&dump).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(DUMP.as_bytes()).unwrap();
        enc.finish().unwrap();
    }

    let result = hydrate_spansh(&pool, &dump).await.unwrap();
    assert_eq!(
        result.source,
        "spansh:galaxy_populated:galaxy_populated.json.gz"
    );
    assert_eq!(result.systems_seen, 2);
    assert_eq!(result.systems_applied, 2);
    assert_eq!(result.stations_seen, 3);
    assert_eq!(
        result.snapshots_applied, 3,
        "Daedalus market+shipyard, Port A market"
    );
    assert_eq!(result.snapshots_skipped, 1, "Galileo's older dump board");
    assert_eq!(result.parse_errors, 0);
    assert_eq!(
        result.identities_applied, 3,
        "every dump station carries its type, pads or services"
    );
    // The identity landed: Daedalus knows its type and services in the
    // journal's vocabulary (API-only spec: the server knows what the dump knows).
    let daedalus_type: Option<String> =
        sqlx::query_scalar("SELECT station_type FROM stations WHERE id = 12")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(daedalus_type.as_deref(), Some("Orbis Starport"));
    let mut daedalus_services: Vec<String> =
        sqlx::query_scalar("SELECT service FROM station_services WHERE station_id = 12")
            .fetch_all(&pool)
            .await
            .unwrap();
    daedalus_services.sort();
    assert_eq!(daedalus_services, vec!["commodities", "dock", "shipyard"]);

    let galileo_gold: i64 = sqlx::query_scalar(
        "SELECT sell_price FROM market WHERE station_id = 10 AND commodity_symbol = 'gold'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(galileo_gold, 60_000, "the fresher live board stands");
    let daedalus: Vec<(String, i64)> = sqlx::query_as(
        "SELECT commodity_symbol, sell_price FROM market WHERE station_id = 12 ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        daedalus,
        vec![("gold".to_owned(), 700), ("silver".to_owned(), 300)]
    );
    let (alpha_address, alpha_provenance): (i64, String) =
        sqlx::query_as("SELECT address, provenance FROM systems WHERE name = 'Alpha'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        alpha_address, 5,
        "provisional system promoted to its real address"
    );
    assert!(alpha_provenance.starts_with("spansh:"));
    let port_a: (i64, i64) = sqlx::query_as(
        "SELECT system_address, sell_price FROM stations JOIN market ON market.station_id = stations.id WHERE stations.id = 30",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        port_a,
        (5, 9),
        "stations follow the promoted system; newer dump board applied"
    );
    assert_eq!(sol_population(&pool).await, 22_781_091_954);

    let job: (String, String, i64, i64, i64) = sqlx::query_as(
        "SELECT source, status, source_bytes, rows_applied, EXTRACT(EPOCH FROM source_observed_at)::BIGINT \
         FROM service_hydrations ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(job.0, result.source);
    assert_eq!(job.1, "complete");
    assert!(job.2 > 0, "byte count recorded");
    assert_eq!(job.3, 2);
    assert_eq!(
        job.4, 1_787_616_000,
        "watermark is the newest system date in the dump"
    );

    // Re-hydrating the same dump changes nothing: equal is not newer.
    let again = hydrate_spansh(&pool, &dump).await.unwrap();
    assert_eq!(again.systems_applied, 0);
    assert_eq!(again.snapshots_applied, 0);
    assert_eq!(again.snapshots_skipped, 4);

    // Both products publish from what was hydrated, and each keeps the other.
    let artifacts = TempDir::new().unwrap();
    let community = snapshot::publish_community(&pool, artifacts.path())
        .await
        .unwrap();
    assert!(community.rows >= 4);
    let routing = routing::publish_routing(&pool, artifacts.path(), &dump)
        .await
        .unwrap();
    assert_eq!(routing.stats.systems, 2);
    let manifest: Manifest = serde_json::from_slice(
        &tokio::fs::read(artifacts.path().join("current.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    manifest.validate().unwrap();
    assert!(manifest.products.contains_key(&ProductKey::Community));
    assert_eq!(
        manifest.products[&ProductKey::Routing].version,
        routing.version
    );
    let publications: Vec<(String, String)> =
        sqlx::query_as("SELECT product, status FROM artifact_publications ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(publications.contains(&("routing".to_owned(), "complete".to_owned())));
    let response = readiness(&pool, artifacts.path()).await;
    assert_eq!(response.status(), 200);

    reset_database(&pool).await;
}

/// The stars feed end to end on a database: an EDSM bodies dump teaches
/// main-star classes (strictly newer wins), the lookup endpoint answers
/// what is known and queues the rest, and publish-stars writes them as
/// the `stars` product a client can decode.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn stars_are_learned_published_and_answered() {
    use ed_api::stars::{answer_stars, hydrate_edsm_bodies, publish_stars};
    use std::io::Write as _;

    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let dir = TempDir::new().unwrap();
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: dir.path().to_path_buf(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;
    sqlx::query("TRUNCATE stars, star_lookups")
        .execute(&pool)
        .await
        .unwrap();

    let dump = dir.path().join("bodies7days.json.gz");
    const BODIES: &str = concat!(
        "[\n",
        r#"{"type":"Star","subType":"Neutron Star","isMainStar":true,"isScoopable":false,"updateTime":"2026-08-23 06:53:55","systemId64":100,"systemName":"A"},"#,
        "\n",
        r#"{"type":"Star","subType":"K (Yellow-Orange) Star","isMainStar":true,"isScoopable":true,"updateTime":"2026-08-24 00:00:00","systemId64":200,"systemName":"B"},"#,
        "\n",
        r#"{"type":"Star","subType":"M (Red dwarf) Star","isMainStar":false,"updateTime":"2026-08-24 00:00:00","systemId64":200,"systemName":"B"},"#,
        "\n",
        r#"{"type":"Planet","subType":"Rocky body","isMainStar":false,"systemId64":200,"systemName":"B"}"#,
        "\n",
        "]\n",
    );
    {
        let f = std::fs::File::create(&dump).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        enc.write_all(BODIES.as_bytes()).unwrap();
        enc.finish().unwrap();
    }
    let result = hydrate_edsm_bodies(&pool, &dump).await.unwrap();
    assert_eq!(
        (result.bodies, result.main_stars, result.written),
        (4, 2, 2)
    );
    // An older observation for A does not regress it; a newer one replaces it.
    let older = ed_api::stars::StarObservation {
        address: 100,
        subtype: "K (Yellow-Orange) Star".into(),
        class: ed_galaxy::StarClass::K,
        scoopable: true,
        observed_at: 1,
        source: "test".into(),
    };
    assert_eq!(
        ed_api::stars::apply_stars(&pool, &[older]).await.unwrap(),
        0
    );
    let newer = ed_api::stars::StarObservation {
        address: 100,
        subtype: "White Dwarf (DA) Star".into(),
        class: ed_galaxy::StarClass::WhiteDwarf,
        scoopable: false,
        observed_at: 2_000_000_000,
        source: "test".into(),
    };
    assert_eq!(
        ed_api::stars::apply_stars(&pool, &[newer]).await.unwrap(),
        1
    );

    // The endpoint: known answered, unknown queued once.
    let answer = answer_stars(&pool, &[100, 200, 300, 300]).await.unwrap();
    assert_eq!(answer.known.len(), 2);
    assert_eq!(answer.known[0].id64, 100);
    assert_eq!(
        answer.known[0].class,
        ed_galaxy::StarClass::WhiteDwarf.code()
    );
    assert_eq!(answer.queued, vec![300, 300]);
    let queued: i64 = sqlx::query_scalar("SELECT count(*) FROM star_lookups")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(queued, 1);

    // Published as the stars product, decodable and in address order.
    let publication = publish_stars(&pool, dir.path()).await.unwrap();
    assert_eq!(publication.stars, 2);
    let path = dir.path().join(&publication.artifact.path);
    let bytes = ed_ebex::decompress(&std::fs::read(&path).unwrap()).unwrap();
    let section = ed_ebex::section(&bytes, ed_ebex::SECTION_STARS)
        .unwrap()
        .unwrap();
    ed_ebex::validate_stars_section(section).unwrap();
    let records: Vec<ed_ebex::StarRecord> = ed_ebex::star_records(section).unwrap().collect();
    assert_eq!(
        records.iter().map(|r| r.address).collect::<Vec<_>>(),
        vec![100, 200]
    );
    assert_eq!(records[1].class, ed_galaxy::StarClass::K.code());
    let manifest = ed_api::routing::read_current_manifest(dir.path())
        .unwrap()
        .unwrap();
    assert!(manifest.products.contains_key(&ed_sync::ProductKey::Stars));
}

/// A detached recorder per call: tests must not fight over the one
/// global recorder slot.
fn test_metrics() -> metrics_exporter_prometheus::PrometheusHandle {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .build_recorder()
        .handle()
}

/// /v1/trade/search: legs pair the cheapest fresh buy boards with the
/// best fresh sell boards, confiscating destinations are excluded, and
/// the second identical request is answered from cache.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn trade_search_pairs_legs_and_caches() {
    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let artifact_dir = TempDir::new().unwrap();
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: artifact_dir.path().to_owned(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;
    sqlx::raw_sql(
        "INSERT INTO systems (address, name, x, y, z, provenance) VALUES \
             (20001, 'Sol', 0, 0, 0, 'test'), (20002, 'Near', 10, 0, 0, 'test'); \
         INSERT INTO commodities (symbol, name, category) VALUES ('gold', 'Gold', 'Metals'); \
         INSERT INTO stations (id, system_address, name, has_market, market_observed_at, pad_large, is_carrier) VALUES \
             (11, 20001, 'Cheap Source', true, now(), 2, false), \
             (12, 20002, 'Rich Sink',    true, now(), 2, false), \
             (13, 20002, 'Confiscator',  true, now(), 2, false); \
         INSERT INTO market (station_id, commodity_symbol, buy_price, sell_price, demand, supply, observed_at) VALUES \
             (11, 'gold', 10000, 9000,  0,    5000, now()), \
             (12, 'gold', 0,     60000, 5000, 0,    now()), \
             (13, 'gold', 0,     90000, 5000, 0,    now()); \
         INSERT INTO station_prohibited (station_id, symbol) VALUES (13, 'gold');",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = http::router(AppState::new(
        pool.clone(),
        artifact_dir.path().to_owned(),
        test_metrics(),
    ));
    let post = || {
        Request::post("/v1/trade/search")
            .header("content-type", "application/json")
            .body(Body::from(json!({"system": "Sol"}).to_string()))
            .unwrap()
    };
    let response = app.clone().oneshot(post()).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let legs = value["legs"].as_array().unwrap();
    assert_eq!(
        legs.len(),
        1,
        "the confiscating sink must not appear: {value}"
    );
    let leg = &legs[0];
    assert_eq!(leg["commodity"], "Gold");
    assert_eq!(leg["profit_t"], 50_000);
    assert_eq!(leg["from"]["station"], "Cheap Source");
    assert_eq!(leg["to"]["station"], "Rich Sink");
    assert_eq!(leg["to"]["max_pad"], "large");
    // Identical request: served from cache (same body, warm service).
    let again = app.oneshot(post()).await.unwrap();
    assert_eq!(again.status(), axum::http::StatusCode::OK);
}

/// /v1/market/search (design (b)): the server answers with the local
/// hit shape, and every exclusion the local search enforces — demand
/// sentinel, confiscation with the black-market opt-in, carriers,
/// radius, pad floor, freshness — holds on the wire. Refusals carry
/// suggestions.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn market_search_mirrors_the_local_contract() {
    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let artifact_dir = TempDir::new().unwrap();
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: artifact_dir.path().to_owned(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;

    sqlx::raw_sql(
        "INSERT INTO systems (address, name, x, y, z, provenance) VALUES \
             (10001, 'Sol', 0, 0, 0, 'test'), \
             (10002, 'Near', 12, 0, 0, 'test'), \
             (10003, 'Far', 600, 0, 0, 'test'); \
         INSERT INTO commodities (symbol, name, category) VALUES \
             ('palladium', 'Palladium', 'Metals'), \
             ('gold', 'Gold', 'Metals'); \
         INSERT INTO modules (symbol) VALUES ('int_fsd_size5_class5'); \
         INSERT INTO ships (symbol) VALUES ('panthermkii'); \
         INSERT INTO stations \
             (id, system_address, name, has_market, market_observed_at, outfitting_observed_at, shipyard_observed_at, \
              pad_small, pad_medium, pad_large, is_carrier, arrival_ls) VALUES \
             (1, 10001, 'Good Port',       true, now(),                      now(), now(), 4, 4, 2, false, 430), \
             (2, 10002, 'Sentinel Rest',   true, now(),                      NULL, NULL, 0, 0, 2, false, 10), \
             (3, 10002, 'Stale Hold',      true, now() - interval '10 days', NULL, NULL, 0, 0, 2, false, 10), \
             (4, 10002, 'Confiscator',     true, now(),                      NULL, NULL, 0, 0, 2, false, 10), \
             (5, 10002, 'Blackmarket Bay', true, now(),                      NULL, NULL, 0, 0, 2, false, 10), \
             (6, 10002, 'X9Z-42',          true, now(),                      NULL, NULL, 0, 0, 2, true,  10), \
             (7, 10003, 'Far Depot',       true, now(),                      NULL, NULL, 0, 0, 2, false, 10), \
             (8, 10002, 'Small Outpost',   true, now(),                      NULL, NULL, 4, 0, 0, false, 10); \
         INSERT INTO market (station_id, commodity_symbol, buy_price, sell_price, demand, supply, observed_at) VALUES \
             (1, 'palladium', 180000, 200000, 5000,   300, now()), \
             (2, 'palladium', 0,      210000, 999999, 0,   now()), \
             (3, 'palladium', 0,      220000, 5000,   0,   now() - interval '10 days'), \
             (4, 'palladium', 0,      230000, 5000,   0,   now()), \
             (5, 'palladium', 0,      240000, 5000,   0,   now()), \
             (6, 'palladium', 0,      250000, 5000,   0,   now()), \
             (7, 'palladium', 0,      260000, 5000,   0,   now()), \
             (8, 'palladium', 0,      190000, 5000,   0,   now()); \
         INSERT INTO station_prohibited (station_id, symbol) VALUES \
             (4, 'palladium'), (5, 'palladium'); \
         INSERT INTO station_services (station_id, service) VALUES (5, 'blackmarket'); \
         INSERT INTO outfitting (station_id, module_symbol) VALUES (1, 'int_fsd_size5_class5'); \
         INSERT INTO shipyard (station_id, ship_symbol) VALUES (1, 'panthermkii');",
    )
    .execute(&pool)
    .await
    .unwrap();

    let app = http::router(AppState::new(
        pool.clone(),
        artifact_dir.path().to_owned(),
        test_metrics(),
    ));
    let post = |body: serde_json::Value| {
        Request::post("/v1/market/search")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    async fn read(response: axum::response::Response) -> serde_json::Value {
        let bytes = http_body_util::BodyExt::collect(response.into_body())
            .await
            .unwrap()
            .to_bytes();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
    }
    let stations = |value: &serde_json::Value| -> Vec<String> {
        value["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["station"].as_str().unwrap().to_owned())
            .collect()
    };

    // Sell, defaults: the stale board, both confiscators, the carrier
    // and the out-of-radius depot stay hidden; no pad floor was
    // requested so the outpost shows. The 999999-demand board stays
    // VISIBLE on purpose — F1's "poison sentinel" was retracted
    // 2026-09-04 and field-confirmed 2026-09-06 (Metz Enterprise, Ega:
    // the game's unlimited-demand convention on a real station); this
    // pin keeps the dead filter from being resurrected a second time.
    let response = app
        .clone()
        .oneshot(post(json!({
            "kind": "commodity", "text": "Palladium", "system": "Sol", "side": "sell",
        })))
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let value = read(response).await;
    assert_eq!(
        stations(&value),
        vec!["Sentinel Rest", "Good Port", "Small Outpost"],
        "{value}"
    );
    assert_eq!(value["symbol"], "palladium");
    assert_eq!(value["provenance"], "server");
    assert!(value["as_of"].as_str().unwrap().ends_with('Z'));
    let hit = &value["results"][0];
    for key in [
        "station_id",
        "station",
        "system",
        "distance_ly",
        "distance_to_arrival",
        "max_pad",
        "is_carrier",
        "price",
        "quantity",
        "updated",
        "age_hours",
    ] {
        assert!(hit.get(key).is_some(), "missing {key}: {hit}");
    }
    assert_eq!(
        hit["price"], 210000,
        "price order leads with the unlimited-demand board"
    );
    assert_eq!(hit["quantity"], 999999);
    assert_eq!(hit["max_pad"], "large");

    // Opt-ins: prohibited-with-black-market appears, the plain
    // confiscator never does; carriers come back only when asked; the
    // large-pad floor hides the outpost.
    let value = read(
        app.clone()
            .oneshot(post(json!({
                "kind": "commodity", "text": "palladium", "system": "Sol", "side": "sell",
                "include_prohibited": true, "include_carriers": true, "min_pad": "l",
            })))
            .await
            .unwrap(),
    )
    .await;
    let names = stations(&value);
    assert!(names.contains(&"Blackmarket Bay".to_owned()), "{names:?}");
    assert!(names.contains(&"X9Z-42".to_owned()), "{names:?}");
    assert!(!names.contains(&"Confiscator".to_owned()), "{names:?}");
    assert!(
        !names.contains(&"Small Outpost".to_owned()),
        "pad floor: {names:?}"
    );

    // Buy side reads supply and the min-quantity floor.
    let value = read(
        app.clone()
            .oneshot(post(json!({
                "kind": "commodity", "text": "Palladium", "system": "Sol", "side": "buy",
                "min_quantity": 400,
            })))
            .await
            .unwrap(),
    )
    .await;
    assert!(stations(&value).is_empty(), "{value}");
    let value = read(
        app.clone()
            .oneshot(post(json!({
                "kind": "commodity", "text": "Palladium", "system": "Sol", "side": "buy",
                "min_quantity": 100,
            })))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(stations(&value), vec!["Good Port"]);

    // Modules and ships come back symbol-only with nulled metadata for
    // the client catalog to fill.
    let value = read(
        app.clone()
            .oneshot(post(json!({
                "kind": "module", "text": "size5_class5", "system": "Sol",
            })))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(stations(&value), vec!["Good Port"]);
    assert_eq!(value["results"][0]["symbol"], "int_fsd_size5_class5");
    assert!(value["results"][0]["class"].is_null());
    let value = read(
        app.clone()
            .oneshot(post(json!({
                "kind": "ship", "text": "panther", "system": "Sol",
            })))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(stations(&value), vec!["Good Port"]);

    // Refusals: unknown commodity suggests, unknown system 404s.
    let response = app
        .clone()
        .oneshot(post(json!({
            "kind": "commodity", "text": "Palladiu", "system": "Sol", "side": "sell",
        })))
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
    let value = read(response).await;
    assert!(
        value["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m == "Palladium"),
        "{value}"
    );
    let response = app
        .oneshot(post(json!({
            "kind": "commodity", "text": "Palladium", "system": "Nowhere Real", "side": "sell",
        })))
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// The by-name system proxy (maintainer, 2026-09-06: "route the EDSM calls
/// through our API. This was supposed to be done."). Two invariants,
/// neither needing the network:
///
/// 1. A system WE know is answered from our own tables, in EDSM's shape,
///    so the client's existing parser needs no change.
/// 2. An upstream answer, once learned, is answerable LOCALLY — that
///    round trip is the entire reason to proxy rather than relay, and
///    without it every client would still cost EDSM a fetch.
///
/// The upstream leg itself is deliberately not exercised here: it is one
/// `reqwest` call whose parse is covered by `parse_upstream_system`.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn knowledge_system_answers_locally_and_learns_what_it_fetches() {
    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let artifact_dir = TempDir::new().unwrap();
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: artifact_dir.path().to_owned(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;

    // A system we already hold, with a primary star from the same table
    // the sphere proxy fills.
    sqlx::raw_sql(
        "INSERT INTO systems (address, name, x, y, z, population, security, allegiance, \
                              government, economy, source_observed_at, provenance) \
         VALUES (77001, 'Wongi', 1.5, -2.5, 3.5, 4200, 'High', 'Federation', \
                 'Democracy', 'Agriculture', now(), 'test'); \
         INSERT INTO stars (address, class, scoopable, subtype, source, observed_at) \
         VALUES (77001, 5, true, 'K (Yellow-Orange) Star', 'test', now());",
    )
    .execute(&pool)
    .await
    .unwrap();

    let answer = ed_api::knowledge::local_system(&pool, "wongi")
        .await
        .unwrap()
        .expect("a system we hold is answered locally, case-insensitively");
    assert_eq!(answer.name, "Wongi");
    assert_eq!(answer.id64, Some(77001));
    let value = serde_json::to_value(&answer).unwrap();
    // EDSM's shape, because the client parses EDSM's shape.
    assert_eq!(value["coords"]["x"], 1.5);
    assert_eq!(value["information"]["allegiance"], "Federation");
    assert_eq!(value["information"]["government"], "Democracy");
    assert_eq!(value["information"]["economy"], "Agriculture");
    assert_eq!(value["information"]["population"], 4200);
    assert_eq!(value["primaryStar"]["type"], "K (Yellow-Orange) Star");
    assert_eq!(value["primaryStar"]["isScoopable"], true);

    // A name we do not hold is not invented.
    assert!(ed_api::knowledge::local_system(&pool, "Nowhere At All")
        .await
        .unwrap()
        .is_none());

    // An upstream reply, parsed then learned, becomes a LOCAL answer --
    // the proxy's whole bargain. EDSM sends `information` as [] when it
    // has nothing political to say, which must not become an error.
    let upstream = serde_json::json!({
        "name": "Deciat",
        "id64": 77002,
        "coords": { "x": 122.0, "y": -0.5, "z": -47.0 },
        "information": [],
        "primaryStar": { "type": "K (Yellow-Orange) Star", "isScoopable": true }
    });
    let parsed =
        ed_api::knowledge::parse_upstream_system(&upstream).expect("a named system parses");
    assert_eq!(
        parsed.information,
        serde_json::json!({}),
        "[] becomes an empty object"
    );
    ed_api::knowledge::learn_system(&pool, &parsed)
        .await
        .unwrap();

    let relearned = ed_api::knowledge::local_system(&pool, "DECIAT")
        .await
        .unwrap()
        .expect("what we fetched once, we now answer ourselves");
    assert_eq!(relearned.id64, Some(77002));
    let value = serde_json::to_value(&relearned).unwrap();
    assert_eq!(value["coords"]["z"], -47.0);
    assert_eq!(
        value["primaryStar"]["isScoopable"], true,
        "the star landed in the shared table"
    );

    // An unknown name upstream is `{}` -- absence, not an error.
    assert!(ed_api::knowledge::parse_upstream_system(&serde_json::json!({})).is_none());
    assert!(
        ed_api::knowledge::parse_upstream_system(&serde_json::json!({ "name": "" })).is_none(),
        "an empty name is no name"
    );
}

/// API-only spec, first defect: round trips must come from the server.
/// Two stations, gold one way and silver the other: one loop, found by
/// `trade_report::prepare` + `ed_route::profit::assemble` — the same
/// pipeline the local finder runs.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn trade_report_finds_the_round_trip_the_legacy_query_could_not() {
    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let artifact_dir = TempDir::new().unwrap();
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: artifact_dir.path().to_owned(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;
    seed_two_station_loop(&pool).await;
    let c = ed_route::profit::Constraints {
        radius_ly: 50.0,
        max_age_hours: 48.0,
        ..Default::default()
    };
    let prepared = ed_api::trade_report::prepare(&pool, (0.0, 0.0, 0.0), &c)
        .await
        .unwrap();
    assert_eq!(prepared.stations.len(), 2);
    assert_eq!(prepared.rows.len(), 4);
    assert_eq!(prepared.excluded.no_market_data, 0);
    let ship = ed_route::cost::Ship {
        cargo_capacity: 100,
        jump_range_ly: 30.0,
        laden_range_ly: 25.0,
    };
    let report = ed_route::profit::assemble(
        "Alpha",
        (0.0, 0.0, 0.0),
        None,
        &ship,
        &c,
        10,
        prepared,
        &ed_route::profit::SearchControl::none(),
    );
    assert_eq!(report.legs.len(), 2);
    assert_eq!(report.round_trips.len(), 1, "gold out, silver back");
    assert_eq!(report.stations_considered, 2);
}

/// Alpha (0,0,0) and Beta (10,0,0), one market each: gold is cheap at
/// Alpha and dear at Beta, silver the reverse. Beta has a material
/// trader (for the stations test).
async fn seed_two_station_loop(pool: &PgPool) {
    sqlx::raw_sql(
        "INSERT INTO systems (address, name, x, y, z, provenance) VALUES \
             (30001, 'Alpha', 0, 0, 0, 'test'), (30002, 'Beta', 10, 0, 0, 'test'); \
         INSERT INTO commodities (symbol, name, category) VALUES ('gold', 'Gold', 'Metals'), ('silver', 'Silver', 'Metals'); \
         INSERT INTO stations (id, system_address, name, has_market, market_observed_at, pad_large, is_carrier, station_type, identity_observed_at) VALUES \
             (11, 30001, 'A Dock', true, now(), 4, false, 'Coriolis', now()), \
             (22, 30002, 'B Dock', true, now(), 4, false, 'Coriolis', now()); \
         INSERT INTO market (station_id, commodity_symbol, buy_price, sell_price, demand, supply, observed_at) VALUES \
             (11, 'gold',   100, 0,   0,    1000, now()), \
             (22, 'gold',   0,   200, 1000, 0,    now()), \
             (22, 'silver', 50,  0,   0,    1000, now()), \
             (11, 'silver', 0,   150, 1000, 0,    now()); \
         INSERT INTO station_services (station_id, service) VALUES (22, 'materialtrader');",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// The v2 path end to end through `TradeService::report`: a ship in the
/// request, the report back, and the second call a cache hit.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn trade_search_with_a_ship_returns_the_report() {
    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let artifact_dir = TempDir::new().unwrap();
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: artifact_dir.path().to_owned(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;
    seed_two_station_loop(&pool).await;
    let service = ed_api::trade_search::TradeService::default();
    let req = ed_api::trade_search::ReportRequest {
        system: "Alpha".into(),
        ship: ed_route::cost::Ship {
            cargo_capacity: 100,
            jump_range_ly: 30.0,
            laden_range_ly: 25.0,
        },
        constraints: ed_route::profit::Constraints {
            radius_ly: 50.0,
            max_age_hours: 48.0,
            ..Default::default()
        },
        from_station_id: None,
        board: None,
        limit: Some(10),
    };
    let ed_api::trade_search::TradeOutcome::Legs(value, verdict) =
        service.report(&pool, &req).await.unwrap()
    else {
        panic!("saturated")
    };
    assert_eq!(verdict, "miss");
    assert_eq!(value["round_trips"].as_array().unwrap().len(), 1, "{value}");
    assert_eq!(value["legs"].as_array().unwrap().len(), 2);
    assert_eq!(value["provenance"], "server");
    assert_eq!(value["stations_considered"], 2);
    let ed_api::trade_search::TradeOutcome::Legs(_, verdict) =
        service.report(&pool, &req).await.unwrap()
    else {
        panic!("saturated")
    };
    assert_eq!(verdict, "hit");

    // The docked board (B.4 gap 2): newer than the stored A Dock board,
    // its gold price is the one the leg is priced at; the answer is not
    // cached; an older board is ignored and says so.
    let board = |observed_at: String, gold_buy: i64| ed_api::trade_report::ClientBoard {
        station_id: 11,
        observed_at,
        rows: vec![
            ed_api::trade_report::ClientBoardRow {
                symbol: "gold".into(),
                buy_price: gold_buy,
                sell_price: 0,
                demand: 0,
                supply: 1000,
            },
            ed_api::trade_report::ClientBoardRow {
                symbol: "silver".into(),
                buy_price: 0,
                sell_price: 150,
                demand: 1000,
                supply: 0,
            },
        ],
    };
    let fresh = ed_store::session::iso_from_epoch(ed_route::profit::now_epoch_secs() + 60);
    let docked = ed_api::trade_search::ReportRequest {
        from_station_id: Some(11),
        board: Some(board(fresh, 80)),
        ..req.clone()
    };
    let ed_api::trade_search::TradeOutcome::Legs(value, verdict) =
        service.report(&pool, &docked).await.unwrap()
    else {
        panic!("saturated")
    };
    assert_eq!(verdict, "bypass", "a fused report is one commander's");
    assert_eq!(
        value["board"],
        serde_json::json!({"used": true, "reason": "newer", "rows": 2}),
        "{value}"
    );
    let legs = value["legs"].as_array().unwrap();
    assert_eq!(legs.len(), 1, "sourced from A Dock only: {value}");
    assert_eq!(
        (legs[0]["symbol"].as_str(), legs[0]["buy_price"].as_i64()),
        (Some("gold"), Some(80))
    );
    assert_eq!(value["round_trips"].as_array().unwrap().len(), 1);
    let stale = ed_api::trade_search::ReportRequest {
        from_station_id: Some(11),
        board: Some(board("2020-01-01T00:00:00Z".into(), 80)),
        ..req.clone()
    };
    let ed_api::trade_search::TradeOutcome::Legs(value, _) =
        service.report(&pool, &stale).await.unwrap()
    else {
        panic!("saturated")
    };
    assert_eq!(
        value["board"],
        serde_json::json!({"used": false, "reason": "older", "rows": 0})
    );
    assert_eq!(
        value["legs"][0]["buy_price"].as_i64(),
        Some(100),
        "the stored board priced the leg"
    );
    let ed_api::trade_search::TradeOutcome::Legs(_, verdict) =
        service.report(&pool, &req).await.unwrap()
    else {
        panic!("saturated")
    };
    assert_eq!(
        verdict, "hit",
        "the plain request's cache entry survived the fused ones"
    );
    // And over the wire: the same body through the router is the report.
    let app = http::router(AppState::new(
        pool.clone(),
        artifact_dir.path().to_owned(),
        test_metrics(),
    ));
    let response = app
        .oneshot(
            Request::post("/v1/trade/search")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&req).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire["round_trips"].as_array().unwrap().len(), 1);
}

/// /v1/stations, the three lookups on the same two-station fixture:
/// in-system order and shape, nearest-with-service (and the empty
/// sphere as a valid answer), and by name prefix.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn stations_answers_the_three_lookups() {
    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL")
        .expect("EDDA_API_TEST_DATABASE_URL must be set for this ignored test");
    let artifact_dir = TempDir::new().unwrap();
    let config = ServiceConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url,
        artifact_dir: artifact_dir.path().to_owned(),
        eddn_relay: ed_eddn::EDDN_RELAY.to_owned(),
        eddn_queue_capacity: 100,
        ingest_bind: "127.0.0.1:0".parse().unwrap(),
        eddn_in_serve: true,
    };
    let pool = database_pool(&config).await.unwrap();
    reset_database(&pool).await;
    seed_two_station_loop(&pool).await;
    let q = ed_api::stations::StationsQuery::default();
    let in_alpha = ed_api::stations::in_system(&pool, "alpha", &q)
        .await
        .unwrap();
    assert_eq!(in_alpha.len(), 1, "{in_alpha:?}");
    assert_eq!(in_alpha[0]["name"], "A Dock");
    assert_eq!(in_alpha[0]["max_pad"], "large");
    assert_eq!(in_alpha[0]["class"], "starport");
    assert!(in_alpha[0]["updated"].is_string());
    let traders = ed_api::stations::near(
        &pool,
        (0.0, 0.0, 0.0),
        Some("materialtrader"),
        50.0,
        None,
        &q,
    )
    .await
    .unwrap();
    assert_eq!(traders.len(), 1, "{traders:?}");
    assert_eq!(traders[0]["id"], 22);
    assert!((traders[0]["distance_ly"].as_f64().unwrap() - 10.0).abs() < 1e-6);
    let none = ed_api::stations::near(&pool, (0.0, 0.0, 0.0), Some("techbroker"), 50.0, None, &q)
        .await
        .unwrap();
    assert!(none.is_empty(), "an empty sphere is a valid empty answer");
    let large_only = ed_api::stations::near(
        &pool,
        (0.0, 0.0, 0.0),
        None,
        50.0,
        Some(ed_domain::station::PadSize::Large),
        &q,
    )
    .await
    .unwrap();
    assert_eq!(large_only.len(), 2);
    let by_name = ed_api::stations::by_name(&pool, "b d", &q).await.unwrap();
    assert_eq!(by_name.len(), 1, "{by_name:?}");
    assert_eq!(by_name[0]["system_name"], "Beta");
    // Over the wire: a bad service is a 400 that names the accepted keys.
    let app = http::router(AppState::new(
        pool.clone(),
        artifact_dir.path().to_owned(),
        test_metrics(),
    ));
    let response = app
        .clone()
        .oneshot(
            Request::get("/v1/stations?near=Alpha&service=teleporter")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    let response = app
        .oneshot(
            Request::get("/v1/stations?system=Beta")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = http_body_util::BodyExt::collect(response.into_body())
        .await
        .unwrap()
        .to_bytes();
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(wire[0]["name"], "B Dock");
}
