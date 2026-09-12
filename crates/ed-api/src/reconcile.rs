//! Item 47 stage 1b: the nightly routing reconcile.
//!
//! Diff what the firehose has taught (EDDN journal systems with
//! coordinates, EDSM star-class teachings in the `stars` table) against
//! the currently published routing index, write the day's EDGO overlay,
//! apply it server-side, and publish the result as the next routing
//! version with the manifest's overlay chain extended.
//!
//! The determinism contract (ledger item 47, maintainer addendum c416128):
//! between rebases, published version N+1 IS `apply_overlays(N, day)` —
//! never a fresh import — so a client's applied bytes equal the server's
//! published bytes and every digest verifies. Fresh imports
//! (`build-routing`) remain the rebase path and reset the chain.
//!
//! Update sources split by what can locate a record:
//! - ADDS come from `systems` rows with coordinates (EDDN journal
//!   traffic): spatial membership against the base decides new vs known.
//! - UPDATES come from one sequential scan of the base index against the
//!   taught-class map (`stars` is keyed by id64, and so are the records),
//!   which closes the "teaching without coordinates" gap: a class lands
//!   the moment its system is in the index, coordinates known or not.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::{ensure, Context, Result};
use ed_galaxy::overlay::{
    add_at, apply_overlays, stars_sha256, update_at, AddRecord, Overlay, OverlayRecord,
};
use ed_galaxy::star::StarClassCode as _;
use ed_galaxy::{Galaxy, StarClass};
use ed_sync::{ArtifactFile, Manifest, OverlayLink, Product, ProductKey};
use serde::Serialize;
use sqlx::PgPool;

use crate::routing::{
    read_current_manifest, write_manifest, ROUTING_FILES, ROUTING_SCHEMA, ROUTING_SIDE_FILES,
};

/// A system the firehose knows by coordinates — an Add candidate.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub address: u64,
    pub name: String,
    pub pos: [f32; 3],
    /// Taught main-star class, when the stars table has one.
    pub class: Option<u8>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct DiffStats {
    /// Systems the index does not have: overlay adds.
    pub adds: u64,
    /// Records whose taught class differs: overlay updates.
    pub updates: u64,
    /// Candidates already present (spatially, within 0.1 ly).
    pub present: u64,
    /// Candidates whose id64 is already a record (position drifted or
    /// corrected upstream); skipped rather than double-added.
    pub present_by_id64: u64,
    /// Candidates dropped: non-finite position, empty or oversized name.
    pub invalid: u64,
    /// Candidates dropped for sharing an identity with an earlier one.
    pub duplicate_identity: u64,
    /// Teachings ignored because they teach Unknown (never downgrade).
    pub taught_unknown: u64,
    /// Records set back to Unknown by an explicit retraction: a `stars`
    /// row of class 0 from a `repair:` source, the one way a class the
    /// index should never have carried is taken out again (2026-09-10:
    /// navroute hops taught a secondary star's class into 17,000 blank
    /// records; a teaching cannot say "forget it", a retraction can).
    pub retracted: u64,
}

/// The overlay records for one reconcile day. Pure: index + inputs in,
/// records out, so the whole diff is testable without a database.
pub fn diff(
    base: &Galaxy,
    candidates: &[Candidate],
    taught: &HashMap<u64, u8>,
    retracted: &HashSet<u64>,
) -> Result<(Vec<OverlayRecord>, DiffStats)> {
    let mut stats = DiffStats::default();
    let mut records = Vec::new();

    // One sequential scan: updates for taught classes, and the id64s of
    // candidates that already exist under a (possibly moved) position.
    let candidate_ids: HashSet<u64> = candidates.iter().map(|c| c.address).collect();
    let mut present_ids: HashSet<u64> = HashSet::new();
    for index in 0..u32::try_from(base.count).context("index too large")? {
        let record = base.record(index);
        if candidate_ids.contains(&record.id64) {
            present_ids.insert(record.id64);
        }
        if let Some(&class) = taught.get(&record.id64) {
            if class == StarClass::Unknown.code() {
                if retracted.contains(&record.id64) && record.class != StarClass::Unknown.code() {
                    records.push(update_at(
                        record.pos(),
                        StarClass::Unknown.code(),
                        record.flags,
                    ));
                    stats.retracted += 1;
                } else {
                    stats.taught_unknown += 1;
                }
            } else if class != record.class {
                ensure!(class <= 0x0f, "taught class {class} exceeds the nibble");
                records.push(update_at(record.pos(), class, record.flags));
                stats.updates += 1;
            }
        }
    }

    let mut seen: BTreeSet<(u64, [u8; 12])> = BTreeSet::new();
    let pos_bytes = |pos: [f32; 3]| {
        let mut bytes = [0u8; 12];
        bytes[0..4].copy_from_slice(&pos[0].to_le_bytes());
        bytes[4..8].copy_from_slice(&pos[1].to_le_bytes());
        bytes[8..12].copy_from_slice(&pos[2].to_le_bytes());
        bytes
    };
    for candidate in candidates {
        let pos = candidate.pos;
        if pos.iter().any(|v| !v.is_finite())
            || candidate.name.is_empty()
            || candidate.name.len() > u16::MAX as usize
        {
            stats.invalid += 1;
            continue;
        }
        if present_ids.contains(&candidate.address) {
            stats.present_by_id64 += 1;
            continue;
        }
        if spatially_present(base, pos) {
            stats.present += 1;
            continue;
        }
        let record = add_at(
            pos,
            AddRecord {
                id64: candidate.address,
                class: candidate.class.unwrap_or_else(|| StarClass::Unknown.code()),
                flags: ed_galaxy::format::FLAG_MAIN_STAR,
                companion: 0,
                name: candidate.name.clone(),
            },
        );
        if !seen.insert((record.cell, pos_bytes(pos))) {
            stats.duplicate_identity += 1;
            continue;
        }
        records.push(record);
        stats.adds += 1;
    }
    Ok((records, stats))
}

/// Is there a record within 0.1 ly of `pos`? Searches the cell and its 26
/// neighbours — the same test the churn ground-truth measurement used.
fn spatially_present(base: &Galaxy, pos: [f32; 3]) -> bool {
    let (cx, cy, cz) = ed_galaxy::format::cell_of_with(pos, base.cell_ly);
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                let Some((start, count)) = base.cell_range(cx + dx, cy + dy, cz + dz) else {
                    continue;
                };
                for index in start..start + count {
                    let p = base.record(index).pos();
                    let d2 =
                        (p[0] - pos[0]).powi(2) + (p[1] - pos[1]).powi(2) + (p[2] - pos[2]).powi(2);
                    if d2 < 0.01 {
                        return true;
                    }
                }
            }
        }
    }
    false
}

#[derive(Debug, Clone, Serialize)]
pub struct OverlayPublication {
    pub version: String,
    pub from_version: String,
    pub overlay: ArtifactFile,
    pub files: Vec<ArtifactFile>,
    pub bytes: u64,
    pub diff: DiffStats,
    pub apply: ed_galaxy::overlay::ApplyStats,
}

/// Everything after the diff, blocking (the apply streams ~2x the index
/// through the disk): write the overlay artifact, apply it to the base,
/// hash and atomically publish the result as `version`, extend the chain.
///
/// The caller has already decided there is something to publish and
/// allocated `version`; `records` must be non-empty.
pub fn publish_overlay(
    artifact_dir: &Path,
    version: &str,
    generated_at: &str,
    records: Vec<OverlayRecord>,
    diff_stats: DiffStats,
) -> Result<OverlayPublication> {
    ensure!(!records.is_empty(), "nothing to publish");
    let started = std::time::Instant::now();
    let manifest = read_current_manifest(artifact_dir)?
        .context("no published manifest; build-routing must run before reconcile")?;
    let product = manifest
        .products
        .get(&ProductKey::Routing)
        .context("no routing product; build-routing must run before reconcile")?;
    let from_version = product.version.clone();
    ensure!(
        product.schema == ROUTING_SCHEMA,
        "published routing schema {} is not this build's {}",
        product.schema,
        ROUTING_SCHEMA
    );
    let base_dir = artifact_dir.join("routing").join(&from_version);

    let overlay = Overlay {
        base_stars_sha256: stars_sha256(&base_dir)?,
        created_at: u64::try_from(chrono_free_now()?)?,
        records,
    };
    let overlays_dir = artifact_dir.join("routing").join("overlays");
    std::fs::create_dir_all(&overlays_dir)?;
    let overlay_name = format!("{version}.edgo");
    let overlay_path = overlays_dir.join(&overlay_name);
    overlay.write(&overlay_path)?;
    let overlay_artifact = {
        let file = std::fs::File::open(&overlay_path)?;
        let bytes = file.metadata()?.len();
        ArtifactFile {
            path: format!("routing/overlays/{overlay_name}"),
            bytes,
            sha256: ed_sync::digest::sha256_hex_reader(file)?,
        }
    };
    overlay_artifact.validate()?;
    tracing::info!(
        version,
        records = overlay.records.len(),
        bytes = overlay_artifact.bytes,
        "reconcile: overlay written"
    );
    metrics::counter!("edda_reconcile_overlay_bytes_total").increment(overlay_artifact.bytes);

    // Apply server-side: the published N+1 is the apply output, which is
    // what makes the client's applied bytes verify against the manifest.
    let staging = artifact_dir.join(format!(".staging-routing-{version}"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    let published = artifact_dir.join("routing").join(version);
    ensure!(
        !published.exists(),
        "routing version {version} is already published"
    );
    let apply_started = std::time::Instant::now();
    let result = apply_and_hash(&base_dir, &overlay, &staging, version);
    let (apply_stats, files, bytes) = match result {
        Ok(ok) => ok,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staging);
            let _ = std::fs::remove_file(&overlay_path);
            return Err(error);
        }
    };
    let apply_seconds = apply_started.elapsed().as_secs_f64();
    metrics::histogram!("edda_reconcile_apply_seconds").record(apply_seconds);
    // Side files (the secondary-boost table and any other sidecar) are
    // not part of the overlay: carry them beside the applied index, or
    // the prune of the old version discards them (2026-09-11: the first
    // reconcile after an adopt left routing/164fcbd0 without boost.bin).
    for name in ROUTING_SIDE_FILES {
        let from = base_dir.join(name);
        if from.is_file() {
            let to = staging.join(name);
            if std::fs::hard_link(&from, &to).is_err() {
                if let Err(error) = std::fs::copy(&from, &to) {
                    let _ = std::fs::remove_dir_all(&staging);
                    let _ = std::fs::remove_file(&overlay_path);
                    return Err(error).with_context(|| format!("carrying side file {name}"));
                }
            }
        }
    }
    tracing::info!(
        version,
        apply_seconds,
        systems = apply_stats.systems,
        adds = apply_stats.adds,
        updates = apply_stats.updates,
        "reconcile: applied and hashed"
    );
    std::fs::rename(&staging, &published).context("atomically publishing applied index")?;

    let mut overlays = product.overlays.clone();
    overlays.push(OverlayLink {
        from: from_version.clone(),
        to: version.to_owned(),
        artifact: overlay_artifact.clone(),
    });
    let next = Manifest::with_product(
        Some(manifest),
        generated_at,
        None,
        ProductKey::Routing,
        Product {
            version: version.to_owned(),
            schema: ROUTING_SCHEMA,
            minimum_client: None,
            files: files.clone(),
            overlays,
            covers_from: None,
        },
    );
    write_manifest(artifact_dir, &next, &format!("routing-{version}"))?;
    // The manifest now points at `version`; `from_version` is the grace
    // copy for clients that fetched the manifest moments ago. Everything
    // older is unreachable and goes (11 GB per dir — ledger 04bd87a).
    let pruned =
        crate::routing::prune_routing_versions(artifact_dir, version, Some(from_version.as_str()))?;
    tracing::info!(
        version,
        pruned = pruned.len(),
        from_version,
        chain_length = next
            .products
            .get(&ProductKey::Routing)
            .map(|p| p.overlays.len())
            .unwrap_or(0),
        total_seconds = started.elapsed().as_secs_f64(),
        "reconcile: published"
    );

    Ok(OverlayPublication {
        version: version.to_owned(),
        from_version,
        overlay: overlay_artifact,
        files,
        bytes,
        diff: diff_stats,
        apply: apply_stats,
    })
}

fn apply_and_hash(
    base_dir: &Path,
    overlay: &Overlay,
    staging: &Path,
    version: &str,
) -> Result<(ed_galaxy::overlay::ApplyStats, Vec<ArtifactFile>, u64)> {
    let apply_stats = apply_overlays(base_dir, std::slice::from_ref(overlay), staging)?;
    Galaxy::validate_dir(staging).context("validating the applied index")?;
    let mut files = Vec::with_capacity(ROUTING_FILES.len() + 1);
    let mut bytes = 0;
    for name in ROUTING_FILES {
        let file = std::fs::File::open(staging.join(name))?;
        let length = file.metadata()?.len();
        let artifact = ArtifactFile {
            path: format!("routing/{version}/{name}"),
            bytes: length,
            sha256: ed_sync::digest::sha256_hex_reader(file)?,
        };
        artifact.validate()?;
        bytes += length;
        files.push(artifact);
    }
    // The chunk manifest for the wire unification rides with the files.
    let chunks = crate::routing::write_chunk_manifest(staging, version)?;
    bytes += chunks.bytes;
    files.push(chunks);
    Ok((apply_stats, files, bytes))
}

/// Unix seconds without pulling a clock crate in.
fn chrono_free_now() -> Result<i64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64)
}

/// The nightly entry point: query, diff, and — only when the day changed
/// something — allocate the next version and publish.
pub async fn reconcile_routing(
    pool: &PgPool,
    artifact_dir: &Path,
) -> Result<Option<OverlayPublication>> {
    let query_started = std::time::Instant::now();
    let candidates: Vec<Candidate> =
        sqlx::query_as::<_, (i64, String, f64, f64, f64, Option<i16>)>(
            "SELECT s.address, s.name, s.x, s.y, s.z, st.class \
         FROM systems s LEFT JOIN stars st ON st.address = s.address \
         WHERE s.address > 0 AND s.x IS NOT NULL AND s.y IS NOT NULL AND s.z IS NOT NULL",
        )
        .fetch_all(pool)
        .await
        .context("querying add candidates")?
        .into_iter()
        .map(|(address, name, x, y, z, class)| Candidate {
            address: address as u64,
            name,
            pos: [x as f32, y as f32, z as f32],
            class: class.and_then(|c| u8::try_from(c).ok()),
        })
        .collect();
    let taught: HashMap<u64, u8> =
        sqlx::query_as::<_, (i64, i16)>("SELECT address, class FROM stars WHERE address > 0")
            .fetch_all(pool)
            .await
            .context("querying taught classes")?
            .into_iter()
            .filter_map(|(address, class)| Some((address as u64, u8::try_from(class).ok()?)))
            .collect();
    // Retractions: class-0 rows only a repair writes (ingest never stores
    // Unknown), so a 0 here is a deliberate "this class was wrong".
    let retracted: HashSet<u64> = sqlx::query_scalar::<_, i64>(
        "SELECT address FROM stars WHERE address > 0 AND class = 0 AND source LIKE 'repair:%'",
    )
    .fetch_all(pool)
    .await
    .context("querying retractions")?
    .into_iter()
    .map(|a| a as u64)
    .collect();

    // Diff against the published base. Blocking: the update scan walks
    // the whole index.
    let manifest = read_current_manifest(artifact_dir)?
        .context("no published manifest; build-routing must run before reconcile")?;
    let product = manifest
        .products
        .get(&ProductKey::Routing)
        .context("no routing product; build-routing must run before reconcile")?;
    let base_dir = artifact_dir.join("routing").join(&product.version);
    tracing::info!(
        candidates = candidates.len(),
        taught = taught.len(),
        base_version = %product.version,
        query_seconds = query_started.elapsed().as_secs_f64(),
        "reconcile: inputs loaded"
    );
    let diff_started = std::time::Instant::now();
    let (records, diff_stats) = {
        let base_dir = base_dir.clone();
        tokio::task::spawn_blocking(move || -> Result<_> {
            let base = Galaxy::open(&base_dir).context("opening the published base index")?;
            diff(&base, &candidates, &taught, &retracted)
        })
        .await
        .context("reconcile diff panicked")??
    };
    let diff_seconds = diff_started.elapsed().as_secs_f64();
    metrics::histogram!("edda_reconcile_diff_seconds").record(diff_seconds);
    metrics::counter!("edda_reconcile_ops_total", "op" => "add").increment(diff_stats.adds);
    metrics::counter!("edda_reconcile_ops_total", "op" => "update").increment(diff_stats.updates);
    tracing::info!(?diff_stats, diff_seconds, "reconcile: diff complete");
    if records.is_empty() {
        metrics::counter!("edda_reconcile_runs_total", "outcome" => "quiet").increment(1);
        tracing::info!("reconcile: no changes today; nothing published");
        return Ok(None);
    }

    let (sequence, generated_at): (i64, String) = sqlx::query_as(
        "INSERT INTO artifact_publications (product, status) VALUES ('routing', 'building') \
         RETURNING id, to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')",
    )
    .fetch_one(pool)
    .await
    .context("starting overlay publication")?;
    let version = crate::version::short_version("routing", sequence, &generated_at);
    let dir = artifact_dir.to_owned();
    let built = tokio::task::spawn_blocking(move || {
        publish_overlay(&dir, &version, &generated_at, records, diff_stats)
    })
    .await
    .context("overlay publication panicked")?;
    match built {
        Ok(publication) => {
            sqlx::query(
                "UPDATE artifact_publications SET status = 'complete', artifact_path = $2, \
                 artifact_bytes = $3, artifact_sha256 = $4, completed_at = now() WHERE id = $1",
            )
            .bind(sequence)
            .bind(publication.overlay.path.as_str())
            .bind(i64::try_from(publication.overlay.bytes)?)
            .bind(publication.overlay.sha256.as_str())
            .execute(pool)
            .await?;
            metrics::counter!("edda_reconcile_runs_total", "outcome" => "published").increment(1);
            Ok(Some(publication))
        }
        Err(error) => {
            let _ = sqlx::query(
                "UPDATE artifact_publications SET status = 'failed', completed_at = now() WHERE id = $1",
            )
            .bind(sequence)
            .execute(pool)
            .await;
            metrics::counter!("edda_reconcile_runs_total", "outcome" => "failed").increment(1);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_galaxy::import::import_reader;

    const SAMPLE: &str = r#"[
{"id64":1,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":2,"name":"Jackson's Lighthouse","coords":{"x":-10.0,"y":5.0,"z":20.0},"bodies":[{"type":"Star","subType":"Neutron Star","mainStar":true}]},
{"id64":3,"name":"Far Away","coords":{"x":500.0,"y":0,"z":0},"bodies":[]}
]"#;

    fn base_index() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        import_reader(Box::new(SAMPLE.as_bytes()), dir.path(), &mut |_| {}).unwrap();
        dir
    }

    fn candidate(address: u64, name: &str, pos: [f32; 3], class: Option<u8>) -> Candidate {
        Candidate {
            address,
            name: name.to_owned(),
            pos,
            class,
        }
    }

    /// Updates come from the id64 scan: a taught class lands on its
    /// record, teaching Unknown never downgrades, and an unchanged class
    /// produces nothing.
    #[test]
    fn taught_classes_become_updates_and_unknown_never_downgrades() {
        let dir = base_index();
        let base = Galaxy::open(dir.path()).unwrap();
        let taught = HashMap::from([
            (3u64, StarClass::Neutron.code()), // Far Away learns its class
            (1u64, StarClass::G.code()),       // Sol: unchanged, no record
            (2u64, StarClass::Unknown.code()), // never downgrade
            (999u64, StarClass::K.code()),     // not in the index: ignored
        ]);
        let (records, stats) = diff(&base, &[], &taught, &HashSet::new()).unwrap();
        assert_eq!((stats.updates, stats.taught_unknown), (1, 1));
        assert_eq!(records.len(), 1);
        match &records[0].op {
            ed_galaxy::overlay::OverlayOp::Update { class, .. } => {
                assert_eq!(*class, StarClass::Neutron.code());
            }
            other => panic!("expected an update, got {other:?}"),
        }
        assert_eq!(records[0].pos, [500.0, 0.0, 0.0]);
    }

    /// A retraction is the one downgrade: a class-0 teaching for an address
    /// the repair named sets the record back to Unknown; the same class-0
    /// teaching without the retraction is ignored as before.
    #[test]
    fn a_retraction_downgrades_to_unknown_and_a_bare_unknown_still_does_not() {
        let dir = base_index();
        let base = Galaxy::open(dir.path()).unwrap();
        let taught = HashMap::from([
            (2u64, StarClass::Unknown.code()),
            (3u64, StarClass::Unknown.code()),
        ]);
        let retracted = HashSet::from([2u64]);
        let (records, stats) = diff(&base, &[], &taught, &retracted).unwrap();
        assert_eq!(
            (stats.retracted, stats.taught_unknown, stats.updates),
            (1, 1, 0)
        );
        assert_eq!(records.len(), 1);
        match &records[0].op {
            ed_galaxy::overlay::OverlayOp::Update { class, .. } => {
                assert_eq!(*class, StarClass::Unknown.code())
            }
            other => panic!("expected an update to Unknown, got {other:?}"),
        }
    }

    /// Adds come from spatial membership: absent candidates land (class
    /// taught or Unknown), present ones don't, a moved id64 doesn't
    /// double-add, and duplicate identities collapse to one.
    #[test]
    fn absent_candidates_become_adds_with_every_guard_applied() {
        let dir = base_index();
        let base = Galaxy::open(dir.path()).unwrap();
        let candidates = vec![
            candidate(
                100,
                "Gria Hypue AA-A h0",
                [1200.0, -40.0, 6000.0],
                Some(StarClass::Neutron.code()),
            ),
            candidate(101, "Untaught Newborn", [7000.0, 12.0, -30.0], None),
            candidate(200, "Sol Duplicate", [0.0, 0.0, 0.0], None), // new id64, known position: present spatially
            candidate(2, "Jackson's Moved", [-10.05, 5.0, 20.0], None), // same id64, drifted pos
            candidate(102, "Twin A", [42.0, 42.0, 42.0], None),
            candidate(103, "Twin A Copy", [42.0, 42.0, 42.0], None), // duplicate identity
            candidate(104, "", [1.0, 1.0, 1.0], None),               // invalid
            candidate(105, "NaN Land", [f32::NAN, 0.0, 0.0], None),  // invalid
        ];
        let (records, stats) = diff(&base, &candidates, &HashMap::new(), &HashSet::new()).unwrap();
        assert_eq!(stats.adds, 3, "{stats:?}");
        assert_eq!(stats.present, 1);
        assert_eq!(stats.present_by_id64, 1);
        assert_eq!(stats.duplicate_identity, 1);
        assert_eq!(stats.invalid, 2);
        let names: Vec<&str> = records
            .iter()
            .filter_map(|r| match &r.op {
                ed_galaxy::overlay::OverlayOp::Add(add) => Some(add.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, ["Gria Hypue AA-A h0", "Untaught Newborn", "Twin A"]);
    }

    /// The full publish: the applied index replaces the base as the next
    /// version, the manifest chain gains its link — and a CLIENT applying
    /// the served overlay to the served base produces byte-identical
    /// files. This is the determinism contract, tested end to end.
    #[test]
    fn publish_extends_the_chain_and_the_client_apply_byte_matches() {
        let artifact_dir = tempfile::tempdir().unwrap();
        let base_dir = artifact_dir.path().join("routing").join("1");
        std::fs::create_dir_all(&base_dir).unwrap();
        import_reader(Box::new(SAMPLE.as_bytes()), &base_dir, &mut |_| {}).unwrap();
        let mut files = Vec::new();
        for name in ROUTING_FILES {
            let bytes = std::fs::read(base_dir.join(name)).unwrap();
            files.push(ArtifactFile::for_bytes(format!("routing/1/{name}"), &bytes));
        }
        let manifest = Manifest::with_product(
            None,
            "2026-09-03T00:00:00Z",
            None,
            ProductKey::Routing,
            Product {
                version: "1".into(),
                schema: ROUTING_SCHEMA,
                minimum_client: None,
                files,
                overlays: Vec::new(),
                covers_from: None,
            },
        );
        write_manifest(artifact_dir.path(), &manifest, "test").unwrap();

        // A side file beside the base (the secondary-boost table) is not
        // part of the overlay and must ride along to the next version, or
        // the nightly prune of the old version silently discards it.
        let side = base_dir.join(ed_galaxy::boost_side::BOOST_SIDE_FILE);
        ed_galaxy::boost_side::write(
            &side,
            &mut vec![ed_galaxy::boost_side::SecondaryBoost {
                id64: 3,
                ls: 1234.5,
                class: StarClass::Neutron.code(),
            }],
        )
        .unwrap();
        let side_bytes = std::fs::read(&side).unwrap();

        let base = Galaxy::open(&base_dir).unwrap();
        let taught = HashMap::from([(3u64, StarClass::Neutron.code())]);
        let candidates = vec![candidate(
            100,
            "Gria Hypue AA-A h0",
            [1200.0, -40.0, 6000.0],
            None,
        )];
        let (records, stats) = diff(&base, &candidates, &taught, &HashSet::new()).unwrap();
        drop(base);
        let publication = publish_overlay(
            artifact_dir.path(),
            "2",
            "2026-09-03T01:00:00Z",
            records,
            stats,
        )
        .unwrap();
        assert_eq!((publication.diff.adds, publication.diff.updates), (1, 1));
        assert_eq!(publication.apply.systems, 4);
        assert_eq!(
            std::fs::read(
                artifact_dir
                    .path()
                    .join("routing")
                    .join("2")
                    .join(ed_galaxy::boost_side::BOOST_SIDE_FILE)
            )
            .ok(),
            Some(side_bytes),
            "the side file is carried beside the published version"
        );

        // The manifest: version 2 with a 1 -> 2 link, walkable by a client.
        let current = read_current_manifest(artifact_dir.path()).unwrap().unwrap();
        let product = current.products.get(&ProductKey::Routing).unwrap();
        assert_eq!(product.version, "2");
        let chain = product.overlay_chain("1").unwrap();
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].artifact.path, "routing/overlays/2.edgo");
        assert!(
            product.overlay_chain("0").is_none(),
            "unknown base: full download"
        );
        Galaxy::validate_dir(&artifact_dir.path().join("routing").join("2")).unwrap();

        // The client's side of the contract: read the served overlay,
        // apply it to the served base, compare every byte.
        let served = Overlay::read(
            &artifact_dir
                .path()
                .join("routing")
                .join("overlays")
                .join("2.edgo"),
        )
        .unwrap();
        let client_out = tempfile::tempdir().unwrap();
        apply_overlays(&base_dir, &[served], client_out.path()).unwrap();
        for name in ROUTING_FILES {
            assert_eq!(
                std::fs::read(client_out.path().join(name)).unwrap(),
                std::fs::read(artifact_dir.path().join("routing").join("2").join(name)).unwrap(),
                "{name}: client apply diverged from the published bytes"
            );
        }

        // A quiet day publishes nothing.
        let base = Galaxy::open(&artifact_dir.path().join("routing").join("2")).unwrap();
        let (records, _) = diff(&base, &candidates, &taught, &HashSet::new()).unwrap();
        assert!(
            records.is_empty(),
            "yesterday's changes are in the base now"
        );
    }
}
