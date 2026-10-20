//! The service-side adapters of the Spansh seam, tested without PostgreSQL:
//! record conversion into `ed_domain::Operation`, the routing product's
//! atomic publication, manifest merging across products, and the CLI
//! surface. The PostgreSQL round trip lives in `tests/postgres.rs`.

use std::io::Write;
use std::path::PathBuf;

use ed_api::{
    cli::{parse_command, Command},
    hydration::spansh::{source_system, station_operations},
    routing::{build_routing, ROUTING_FILES, ROUTING_SCHEMA},
};
use ed_domain::Operation;
use ed_store::galaxy::spansh;
use ed_sync::{ArtifactFile, Manifest, ProductKey};

const ALPHA: &str = r#"{"id64":1,"name":"Alpha","coords":{"x":1,"y":2,"z":3},"population":100,"date":"2026-08-24 01:00:00+00","security":"High","allegiance":"Empire","controllingPower":"A. Lavigny-Duval","powerState":"Stronghold","powers":["A. Lavigny-Duval"],"stations":[{"id":10,"name":"Port A","type":"Outpost","updateTime":"2026-08-24 01:00:00+00","services":["Dock","Market","Outfitting","Shipyard"],"market":{"updateTime":"2026-08-24 01:00:00+00","commodities":[{"symbol":"Gold","name":"Gold","category":"Metals","buyPrice":100,"sellPrice":90,"demand":5,"supply":7}]},"outfitting":{"updateTime":"2026-08-23 01:00:00+00","modules":[{"symbol":"Hpt_PulseLaser_Fixed_Small","name":"Pulse Laser"}]},"shipyard":{"ships":[{"symbol":"CobraMkIII","name":"Cobra Mk III"}]}},{"id":11,"name":"Bare Outpost","type":"Outpost","services":["Dock"]}]}"#;

/// Finding: the service could only seed from the synthetic fixture. A
/// Spansh station must become the same operations EDDN produces -- epoch
/// timestamps parsed from `YYYY-MM-DD HH:MM:SS+00`, lowercase symbols --
/// so the PostgreSQL write path (and its strictly-newer rule) is reused
/// rather than duplicated.
#[test]
fn spansh_records_become_domain_operations_with_epoch_timestamps() {
    let system = spansh::parse_line(ALPHA).unwrap().unwrap();
    let epoch = 1_787_533_200i64;

    let row = source_system(&system, "spansh:test").unwrap();
    assert_eq!(row.address, 1);
    assert_eq!(row.name, "Alpha");
    assert_eq!(row.position, Some([1.0, 2.0, 3.0]));
    assert_eq!(row.population, Some(100));
    assert_eq!(row.observed_at, Some(epoch));
    assert_eq!(row.controlling_power.as_deref(), Some("A. Lavigny-Duval"));
    assert_eq!(row.powers.as_deref(), Some("A. Lavigny-Duval"));
    assert_eq!(row.provenance, "spansh:test");

    let port = &system.stations[0];
    let operations = station_operations(&system, port, &port.times());
    assert_eq!(operations.len(), 4, "identity, then the three boards");
    // The identity comes first (API-only spec, 2026-09-07): what a
    // Docked event would say, from the dump, in the journal's vocabulary.
    let Operation::StationIdentity(identity) = &operations[0] else {
        panic!("first operation must be the station identity");
    };
    assert_eq!(identity.system_name, "Alpha");
    assert_eq!(identity.system_address, Some(1));
    assert_eq!(identity.station_name, "Port A");
    assert_eq!(identity.market_id, 10);
    assert_eq!(identity.station_type.as_deref(), Some("Outpost"));
    assert_eq!(identity.services, vec!["dock", "commodities", "outfitting", "shipyard"]);
    assert_eq!((identity.pad_small, identity.pad_medium, identity.pad_large), (None, None, None), "no landingPads in the record");
    assert_eq!(identity.observed_at.epoch_seconds, epoch, "the station's own updateTime");
    assert!(!identity.is_carrier());
    let Operation::Market(market) = &operations[1] else {
        panic!("second operation must be the market board");
    };
    assert_eq!(market.system_name, "Alpha");
    assert_eq!(market.market_id, Some(10));
    assert_eq!(market.station_name.as_deref(), Some("Port A"));
    assert_eq!(market.observed_at.epoch_seconds, epoch);
    assert_eq!(market.values[0].name, "gold");
    assert_eq!(market.values[0].stock, 7);
    assert_eq!(market.values[0].demand, 5);
    let Operation::Outfitting(outfitting) = &operations[2] else {
        panic!("third operation must be outfitting");
    };
    assert_eq!(outfitting.observed_at.epoch_seconds, epoch - 86_400);
    assert_eq!(outfitting.values, vec!["hpt_pulselaser_fixed_small"]);
    let Operation::Shipyard(shipyard) = &operations[3] else {
        panic!("fourth operation must be the shipyard");
    };
    assert_eq!(
        shipyard.observed_at.epoch_seconds, epoch,
        "falls back to the station time"
    );
    assert_eq!(shipyard.values, vec!["cobramkiii"]);

    // A station with no boards still has an identity (type, services)
    // and produces exactly that: no snapshot operations.
    let bare = &system.stations[1];
    let bare_ops = station_operations(&system, bare, &bare.times());
    assert_eq!(bare_ops.len(), 1, "{bare_ops:?}");
    let Operation::StationIdentity(identity) = &bare_ops[0] else {
        panic!("a boardless station's only operation is its identity");
    };
    assert_eq!(identity.station_name, "Bare Outpost");
    assert_eq!(identity.services, vec!["dock"]);
    assert_eq!(identity.observed_at.epoch_seconds, 0, "no updateTime: epoch 0, outranked by any dated identity");
}

/// Finding: an early community publisher built a manifest containing only
/// its own product, so publishing it after routing silently unpublished the
/// routing index. Every publisher goes through `Manifest::with_product`,
/// which carries the other products forward (the community product itself
/// was retired 2026-10-20; stars stands in for it here).
#[test]
fn publishing_one_product_keeps_the_others() {
    let routing = ed_sync::Product {
        version: "7".to_owned(),
        schema: ROUTING_SCHEMA,
        minimum_client: None,
        files: vec![ArtifactFile::for_bytes("routing/7/stars.bin", b"stars")],
        overlays: Vec::new(),
    };
    let stars = |version: &str, bytes: &[u8]| ed_sync::Product {
        version: version.to_owned(),
        schema: ed_sync::STARS_SCHEMA_V1,
        minimum_client: None,
        files: vec![ArtifactFile::for_bytes(
            format!("stars/{version}/stars-{version}.ebex.zst"),
            bytes,
        )],
        overlays: Vec::new(),
    };
    let existing = Manifest::with_product(
        None,
        "2026-08-29T10:00:00Z",
        Some("2026-08-29T09:00:00Z".to_owned()),
        ProductKey::Stars,
        stars("2", b"old"),
    );
    let existing = Manifest::with_product(
        Some(existing),
        "2026-08-29T11:00:00Z",
        None,
        ProductKey::Routing,
        routing.clone(),
    );
    assert_eq!(
        existing.eddn_watermark.as_deref(),
        Some("2026-08-29T09:00:00Z"),
        "a publication without a watermark keeps the existing one"
    );

    let published = Manifest::with_product(
        Some(existing),
        "2026-08-29T12:00:00Z",
        Some("2026-08-29T11:59:00Z".to_owned()),
        ProductKey::Stars,
        stars("3", b"new"),
    );
    published.validate().unwrap();
    assert_eq!(published.generated_at, "2026-08-29T12:00:00Z");
    assert_eq!(published.eddn_watermark.as_deref(), Some("2026-08-29T11:59:00Z"));
    assert_eq!(published.products[&ProductKey::Routing], routing);
    assert_eq!(published.products[&ProductKey::Stars].version, "3");
    assert!(published.artifact("routing/7/stars.bin").is_some());
    assert!(published.artifact("stars/3/stars-3.ebex.zst").is_some());
    assert!(published.artifact("stars/2/stars-2.ebex.zst").is_none());
}

const GALAXY_DUMP: &str = concat!(
    "[\n",
    r#"{"id64":10477373803,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},"#,
    "\n",
    r#"{"id64":1,"name":"Alpha Centauri","coords":{"x":3.03125,"y":-0.09375,"z":3.15625},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},"#,
    "\n",
    r#"{"id64":2,"name":"Jackson's Lighthouse","coords":{"x":-100,"y":10,"z":50},"bodies":[{"type":"Star","subType":"Neutron Star","mainStar":true}]}"#,
    "\n]\n",
);

fn write_galaxy_dump(path: &std::path::Path) {
    let f = std::fs::File::create(path).unwrap();
    let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
    enc.write_all(GALAXY_DUMP.as_bytes()).unwrap();
    enc.finish().unwrap();
}

/// Finding: there was no routing product. Building one must go through
/// staging, validation and an atomic rename, and land in the manifest next
/// to the community product with verifiable hashes.
#[test]
fn routing_publication_is_atomic_and_verifiable() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("galaxy.json.gz");
    write_galaxy_dump(&dump);
    let artifacts = dir.path().join("artifacts");
    std::fs::create_dir_all(&artifacts).unwrap();
    let stars = Manifest::with_product(
        None,
        "2026-08-29T10:00:00Z",
        Some("2026-08-29T09:00:00Z".to_owned()),
        ProductKey::Stars,
        ed_sync::Product {
            version: "2".to_owned(),
            schema: ed_sync::STARS_SCHEMA_V1,
            minimum_client: None,
            files: vec![ArtifactFile::for_bytes("stars/2/stars-2.ebex.zst", b"old")],
            overlays: Vec::new(),
        },
    );
    std::fs::write(
        artifacts.join("current.json"),
        serde_json::to_vec_pretty(&stars).unwrap(),
    )
    .unwrap();

    let publication = build_routing(&dump, &artifacts, "5", "2026-08-29T12:00:00Z").unwrap();
    assert_eq!(publication.version, "5");
    assert_eq!(publication.stats.systems, 3);
    // The four index files plus the chunk manifest (wire unification).
    assert_eq!(publication.files.len(), ROUTING_FILES.len() + 1);
    assert!(
        publication.files.iter().any(|f| f.path.ends_with("/chunks.json")),
        "the chunk manifest rides with the files"
    );

    let published_dir = artifacts.join("routing").join("5");
    ed_galaxy::Galaxy::validate_dir(&published_dir).unwrap();
    let galaxy = ed_galaxy::Galaxy::open(&published_dir).unwrap();
    assert!(galaxy.find("Sol").is_some());
    assert!(
        std::fs::read_dir(&artifacts).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging")),
        "staging directory must be renamed away"
    );

    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(artifacts.join("current.json")).unwrap()).unwrap();
    manifest.validate().unwrap();
    let routing = &manifest.products[&ProductKey::Routing];
    assert_eq!(routing.version, "5");
    assert_eq!(routing.schema, ROUTING_SCHEMA);
    assert_eq!(routing.schema, ed_galaxy::format::VERSION);
    for file in &routing.files {
        assert!(file.path.starts_with("routing/5/"), "{}", file.path);
        file.verify_file(&artifacts.join(&file.path)).unwrap();
    }
    assert_eq!(
        manifest.products[&ProductKey::Stars],
        stars.products[&ProductKey::Stars],
        "publishing routing keeps the stars product as it was"
    );
    assert_eq!(
        manifest.eddn_watermark.as_deref(),
        Some("2026-08-29T09:00:00Z")
    );
}

/// Finding: `ed-api hydrate` only took a fixture path.
#[test]
fn cli_accepts_spansh_hydration_and_routing_builds() {
    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(parse_command(args(&[])).unwrap(), Command::Serve);
    assert_eq!(
        parse_command(args(&["hydrate", "fixture.json"])).unwrap(),
        Command::HydrateFixture(PathBuf::from("fixture.json"))
    );
    assert_eq!(
        parse_command(args(&["hydrate", "--spansh", "galaxy_populated.json.gz"])).unwrap(),
        Command::HydrateSpansh(PathBuf::from("galaxy_populated.json.gz"))
    );
    assert_eq!(
        parse_command(args(&["build-routing", "galaxy.json.gz"])).unwrap(),
        Command::BuildRouting {
            source: PathBuf::from("galaxy.json.gz"),
            artifact_dir: None
        }
    );
    assert_eq!(
        parse_command(args(&["build-routing", "galaxy.json.gz", "out"])).unwrap(),
        Command::BuildRouting {
            source: PathBuf::from("galaxy.json.gz"),
            artifact_dir: Some(PathBuf::from("out"))
        }
    );
    assert_eq!(parse_command(args(&["publish-stars"])).unwrap(), Command::PublishStars);
    // Retired 2026-10-20 with the community baseline: unknown commands now.
    assert!(parse_command(args(&["publish-community"])).is_err());
    assert!(parse_command(args(&["publish-market-daily"])).is_err());
    assert!(parse_command(args(&["hydrate"])).is_err());
    assert!(parse_command(args(&["hydrate", "--spansh"])).is_err());
    assert!(parse_command(args(&["hydrate", "a", "b"])).is_err());
    assert!(parse_command(args(&["frobnicate"])).is_err());
}
