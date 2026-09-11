//! The `routing` product: a versioned EDGX index built from a Spansh galaxy
//! dump and published next to the community baseline.
//!
//! Publication is atomic in the same way as `snapshot::publish_community`:
//! build into a staging directory, validate by opening the index, hash
//! every file, rename the directory into place, then replace the manifest
//! pointer. A failed build never touches the published manifest.

use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use ed_galaxy::Galaxy;
use ed_sync::{ArtifactFile, Manifest, Product, ProductKey};
use serde::Serialize;
use sqlx::PgPool;

/// The manifest `schema` of the routing product is the EDGX format version
/// the files carry, which is what a client must understand to open them.
pub const ROUTING_SCHEMA: u32 = ed_galaxy::format::VERSION;

/// The files one EDGX index consists of, in publication order.
pub const ROUTING_FILES: [&str; 4] = ["stars.bin", "cells.bin", "names.bin", "byname.bin"];

#[derive(Clone, Debug, Serialize)]
pub struct RoutingPublication {
    pub version: String,
    /// Published directory, relative to the artifact root.
    pub directory: PathBuf,
    pub files: Vec<ArtifactFile>,
    pub bytes: u64,
    pub stats: ed_galaxy::import::ImportStats,
}

/// Read the currently published manifest, if any, so a publication can
/// carry the other products forward.
pub fn read_current_manifest(artifact_dir: &Path) -> Result<Option<Manifest>> {
    let path = artifact_dir.join("current.json");
    match std::fs::read(&path) {
        Ok(bytes) => {
            let manifest: Manifest = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing published manifest {}", path.display()))?;
            Ok(Some(manifest))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Atomically replace `current.json` with `manifest`.
pub fn write_manifest(artifact_dir: &Path, manifest: &Manifest, token: &str) -> Result<()> {
    manifest.validate()?;
    let pending = artifact_dir.join(format!(".current-{token}.json"));
    std::fs::write(&pending, serde_json::to_vec_pretty(manifest)?)?;
    std::fs::rename(&pending, artifact_dir.join("current.json"))
        .context("atomically publishing manifest")
}

/// Where a routing base comes from: a galaxy dump imported here, or a
/// directory already built elsewhere (2026-09-10: the maintainer's PC
/// imports the dump with the flag-and-sidecar importer and pushes the
/// result; the box adopts it rather than pulling 116 GB to rebuild what
/// exists). Either way the publish half is the same: validate, chunk,
/// rename into place, start a fresh overlay chain, prune.
#[derive(Clone, Debug)]
pub enum RoutingSource {
    Import(PathBuf),
    Prebuilt(PathBuf),
}

/// Build the index from `source` and publish it as `routing/<version>/`
/// under `artifact_dir`, merging the product into the current manifest.
/// Blocking: the import is CPU- and memory-bound (the full galaxy needs
/// ~8 GB), so callers on an async runtime use `spawn_blocking`.
pub fn build_routing(
    source: &Path,
    artifact_dir: &Path,
    version: &str,
    generated_at: &str,
) -> Result<RoutingPublication> {
    publish_from(&RoutingSource::Import(source.to_owned()), artifact_dir, version, generated_at)
}

/// Adopt an index built elsewhere as the next routing version: a rebase
/// without an import. The four EDGX files are required and validated;
/// side files beside them (`boost.bin`, `agg250.bin`, `alt250.bin`,
/// `graph250.bin`) travel with them and are not part of the product.
pub fn adopt_routing(
    prebuilt: &Path,
    artifact_dir: &Path,
    version: &str,
    generated_at: &str,
) -> Result<RoutingPublication> {
    publish_from(&RoutingSource::Prebuilt(prebuilt.to_owned()), artifact_dir, version, generated_at)
}

/// Side files an index may carry beside the four product files. Copied
/// on adopt when present, never listed in the manifest.
pub const ROUTING_SIDE_FILES: [&str; 4] = [
    ed_galaxy::boost_side::BOOST_SIDE_FILE,
    "agg250.bin",
    "alt250.bin",
    "graph250.bin",
];

fn publish_from(
    source: &RoutingSource,
    artifact_dir: &Path,
    version: &str,
    generated_at: &str,
) -> Result<RoutingPublication> {
    ensure!(!version.trim().is_empty(), "routing version is empty");
    let staging = artifact_dir.join(format!(".staging-routing-{version}"));
    let published = artifact_dir.join("routing").join(version);
    ensure!(
        !published.exists(),
        "routing version {version} is already published at {}",
        published.display()
    );
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;

    let result = match source {
        RoutingSource::Import(dump) => build_into(dump, &staging, artifact_dir, version),
        RoutingSource::Prebuilt(dir) => adopt_into(dir, &staging, artifact_dir, version),
    };
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    let (stats, files, bytes) = result?;

    std::fs::create_dir_all(artifact_dir.join("routing"))?;
    std::fs::rename(&staging, &published).context("atomically publishing routing index")?;

    let existing = read_current_manifest(artifact_dir)?;
    let grace = existing
        .as_ref()
        .and_then(|manifest| manifest.products.get(&ProductKey::Routing))
        .map(|product| product.version.clone());
    let manifest = Manifest::with_product(
        existing,
        generated_at,
        None,
        ProductKey::Routing,
        Product {
            version: version.to_owned(),
            schema: ROUTING_SCHEMA,
            minimum_client: None,
            files: files.clone(),
            // A full build is a rebase: the fresh version starts a fresh
            // overlay chain (Item 47 — the overlay publisher extends it).
            overlays: Vec::new(),
        covers_from: None,
        },
    );
    write_manifest(artifact_dir, &manifest, &format!("routing-{version}"))?;
    // A rebase also orphans the old chain's overlay files along with the
    // old versions; version dirs prune to current + grace here, and the
    // overlay pruning question (delete .edgo files the fresh chain no
    // longer references) rides the same policy at the next reconcile.
    let pruned = prune_routing_versions(artifact_dir, version, grace.as_deref())?;
    if !pruned.is_empty() {
        tracing::info!(?pruned, "rebase pruned retired routing versions");
    }

    Ok(RoutingPublication {
        version: version.to_owned(),
        directory: PathBuf::from("routing").join(version),
        files,
        bytes,
        stats,
    })
}

/// Bring a prebuilt index into staging: the four product files (a
/// rename where the two sit on one filesystem, a copy otherwise), any
/// side files present, then the same validation and chunking an import
/// gets.
fn adopt_into(
    prebuilt: &Path,
    staging: &Path,
    artifact_dir: &Path,
    version: &str,
) -> Result<(ed_galaxy::import::ImportStats, Vec<ArtifactFile>, u64)> {
    ensure!(
        ed_galaxy::Galaxy::exists(prebuilt),
        "{} does not hold the four EDGX files",
        prebuilt.display()
    );
    Galaxy::validate_dir(prebuilt).with_context(|| format!("validating {}", prebuilt.display()))?;
    let take = |name: &str| -> Result<()> {
        let from = prebuilt.join(name);
        let to = staging.join(name);
        if std::fs::rename(&from, &to).is_err() {
            std::fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
        }
        Ok(())
    };
    for name in ROUTING_FILES {
        take(name)?;
    }
    for name in ROUTING_SIDE_FILES {
        if prebuilt.join(name).is_file() {
            take(name)?;
        }
    }
    let galaxy = Galaxy::open(staging).context("opening the adopted index")?;
    ensure!(galaxy.count > 0, "{} holds no systems", prebuilt.display());
    let stats = ed_galaxy::import::ImportStats {
        phase: "adopted".into(),
        systems: galaxy.count as u64,
        ..Default::default()
    };
    tracing::info!(systems = galaxy.count, from = %prebuilt.display(), "adopting prebuilt routing index");
    let (files, bytes) = chunk_staging(staging, artifact_dir, version)?;
    Ok((stats, files, bytes))
}

fn build_into(
    source: &Path,
    staging: &Path,
    artifact_dir: &Path,
    version: &str,
) -> Result<(ed_galaxy::import::ImportStats, Vec<ArtifactFile>, u64)> {
    let mut last = std::time::Instant::now();
    let mut progress = |stats: &ed_galaxy::import::ImportStats| {
        if last.elapsed().as_secs() >= 5 {
            last = std::time::Instant::now();
            tracing::info!(phase = %stats.phase, systems = stats.systems, bytes_in = stats.bytes_in, "building routing index");
        }
    };
    let stats = ed_galaxy::import::import(source, staging, &mut progress)
        .with_context(|| format!("importing {}", source.display()))?;
    ensure!(
        stats.systems > 0,
        "dump {} contained no systems",
        source.display()
    );
    Galaxy::validate_dir(staging).context("validating built routing index")?;
    let (files, bytes) = chunk_staging(staging, artifact_dir, version)?;
    Ok((stats, files, bytes))
}

/// Digest and chunk the four product files in `staging`: the artifact
/// list and the chunk manifest a client verifies against.
fn chunk_staging(staging: &Path, artifact_dir: &Path, version: &str) -> Result<(Vec<ArtifactFile>, u64)> {
    let mut files = Vec::with_capacity(ROUTING_FILES.len() + 1);
    let mut bytes = 0;
    for name in ROUTING_FILES {
        let path = staging.join(name);
        let file = std::fs::File::open(&path)?;
        let length = file.metadata()?.len();
        let sha256 = ed_sync::digest::sha256_hex_reader(file)?;
        let relative = format!("routing/{version}/{name}");
        let artifact = ArtifactFile {
            path: relative,
            bytes: length,
            sha256,
        };
        artifact.validate()?;
        bytes += length;
        files.push(artifact);
    }
    // The chunk manifest for the wire unification rides with the files
    // (old clients ignore the extra entry).
    let chunks = write_chunk_manifest(staging, version)?;
    bytes += chunks.bytes;
    files.push(chunks);
    // The manifest must be readable before anything is renamed into place.
    let _ = read_current_manifest(artifact_dir)?;
    Ok((files, bytes))
}

/// CDC chunk boundaries for the wire unification (ledger 04bd87a /
/// 5fed853): average 1 MiB, min a quarter, max four times — the tradeoff
/// the delta bench priced. Content-defined so a rebase's unchanged
/// regions keep their hashes across versions.
const CHUNK_AVG: u32 = 1 << 20;

/// Chunk one published file: stream it once, hashing each CDC chunk and
/// the whole file in the same pass.
pub fn chunk_file(path: &Path, name: &str) -> Result<ed_sync::ChunkedFile> {
    use sha2::Digest as _;
    let file = std::fs::File::open(path).with_context(|| format!("chunking {}", path.display()))?;
    let mut whole = sha2::Sha256::new();
    let mut chunks = Vec::new();
    let mut offset = 0u64;
    for chunk in fastcdc::v2020::StreamCDC::new(
        std::io::BufReader::with_capacity(1 << 20, file),
        CHUNK_AVG / 4,
        CHUNK_AVG,
        CHUNK_AVG * 4,
    ) {
        let chunk = chunk.context("reading a CDC chunk")?;
        whole.update(&chunk.data);
        chunks.push(ed_sync::Chunk {
            offset,
            len: u32::try_from(chunk.data.len()).context("chunk larger than u32")?,
            sha256: ed_sync::sha256_hex(&chunk.data),
        });
        offset += chunk.data.len() as u64;
    }
    Ok(ed_sync::ChunkedFile {
        name: name.to_owned(),
        bytes: offset,
        sha256: format!("{:x}", whole.finalize()),
    	chunks,
    })
}

/// Write `chunks.json` for the four index files of `staging`, returning
/// the artifact entry to append to the product's files (it is served,
/// listed, and verified like any other file; old clients ignore it).
pub fn write_chunk_manifest(staging: &Path, version: &str) -> Result<ArtifactFile> {
    let mut files = Vec::with_capacity(ROUTING_FILES.len());
    for name in ROUTING_FILES {
        files.push(chunk_file(&staging.join(name), name)?);
    }
    let manifest = ed_sync::ChunkManifest { version: version.to_owned(), files };
    manifest.validate()?;
    let bytes = serde_json::to_vec(&manifest)?;
    let path = staging.join(ed_sync::CHUNKS_FILE);
    std::fs::write(&path, &bytes)?;
    let artifact = ArtifactFile::for_bytes(
        format!("routing/{version}/{}", ed_sync::CHUNKS_FILE),
        &bytes,
    );
    artifact.validate()?;
    Ok(artifact)
}

/// Item 47 prune policy (maintainer-ruled, ledger 04bd87a): full version dirs
/// pile up 11 GB per publish, and the manifest only ever points clients
/// at CURRENT — so keep current plus one grace version (a client that
/// fetched the manifest just before a publish may be mid-download), and
/// delete the rest. Overlay artifacts live in `routing/overlays/` and
/// are never touched: the chain needs every link until a rebase resets
/// it. Returns the versions removed.
pub fn prune_routing_versions(
    artifact_dir: &Path,
    current: &str,
    grace: Option<&str>,
) -> Result<Vec<String>> {
    prune_version_dirs(artifact_dir, "routing", current, grace, &["overlays"])
}

/// The general form: prune `<artifact_dir>/<product_dir>/<numeric>` to
/// current + grace. `protected` names (and anything non-numeric) always
/// survive.
pub fn prune_version_dirs(
    artifact_dir: &Path,
    product_dir: &str,
    current: &str,
    grace: Option<&str>,
    protected: &[&str],
) -> Result<Vec<String>> {
    let root = artifact_dir.join(product_dir);
    let mut removed = Vec::new();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(removed),
        Err(error) => return Err(error).context("listing product versions"),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        // Only version-shaped dirs (numeric legacy, or short-hash) are
        // publications; protected names and anything unexpected (a
        // sub-index dir, a human's scratch) stay.
        if !crate::version::is_version_dir_name(&name)
            || name == current
            || Some(name.as_str()) == grace
            || protected.contains(&name.as_str())
        {
            continue;
        }
        std::fs::remove_dir_all(entry.path())
            .with_context(|| format!("pruning {product_dir} version {name}"))?;
        tracing::info!(version = %name, product = product_dir, "pruned retired version");
        removed.push(name);
    }
    removed.sort();
    Ok(removed)
}

/// Run [`build_routing`] as a recorded `routing` publication.
pub async fn publish_routing(
    pool: &PgPool,
    artifact_dir: &Path,
    source: &Path,
) -> Result<RoutingPublication> {
    publish_recorded(pool, artifact_dir, RoutingSource::Import(source.to_owned())).await
}

/// Run [`adopt_routing`] as a recorded `routing` publication.
pub async fn adopt_routing_recorded(
    pool: &PgPool,
    artifact_dir: &Path,
    prebuilt: &Path,
) -> Result<RoutingPublication> {
    publish_recorded(pool, artifact_dir, RoutingSource::Prebuilt(prebuilt.to_owned())).await
}

async fn publish_recorded(
    pool: &PgPool,
    artifact_dir: &Path,
    source: RoutingSource,
) -> Result<RoutingPublication> {
    let (sequence, generated_at): (i64, String) = sqlx::query_as(
        "INSERT INTO artifact_publications (product, status) VALUES ('routing', 'building') \
         RETURNING id, to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')",
    )
    .fetch_one(pool)
    .await
    .context("starting routing publication")?;
    let version = crate::version::short_version("routing", sequence, &generated_at);
    let artifact_dir = artifact_dir.to_owned();
    let built = tokio::task::spawn_blocking(move || {
        publish_from(&source, &artifact_dir, &version, &generated_at)
    })
    .await
    .context("routing build panicked")?;
    match built {
        Ok(publication) => {
            let digest = ed_sync::digest::sha256_hex(
                publication
                    .files
                    .iter()
                    .flat_map(|file| file.sha256.bytes())
                    .collect::<Vec<u8>>()
                    .as_slice(),
            );
            sqlx::query(
                "UPDATE artifact_publications SET status = 'complete', artifact_path = $2, \
                 artifact_bytes = $3, artifact_sha256 = $4, completed_at = now() WHERE id = $1",
            )
            .bind(sequence)
            .bind(publication.directory.to_string_lossy().as_ref())
            .bind(i64::try_from(publication.bytes)?)
            .bind(digest)
            .execute(pool)
            .await?;
            Ok(publication)
        }
        Err(error) => {
            let _ = sqlx::query(
                "UPDATE artifact_publications SET status = 'failed', completed_at = now() WHERE id = $1",
            )
            .bind(sequence)
            .execute(pool)
            .await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chunking covers the file exactly, the chunk hashes reassemble to
    /// the whole-file hash, and boundaries are content-defined (a big
    /// enough file yields more than one chunk).
    #[test]
    fn chunked_files_cover_exactly_and_validate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stars.bin");
        // 5 MiB of varied bytes so CDC finds boundaries.
        let data: Vec<u8> = (0..5 * 1024 * 1024u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        std::fs::write(&path, &data).unwrap();
        let chunked = chunk_file(&path, "stars.bin").unwrap();
        chunked.validate().unwrap();
        assert_eq!(chunked.bytes, data.len() as u64);
        assert_eq!(chunked.sha256, ed_sync::sha256_hex(&data));
        assert!(chunked.chunks.len() > 1, "5 MiB should cut into several ~1 MiB chunks");
        for chunk in &chunked.chunks {
            let slice = &data[chunk.offset as usize..chunk.offset as usize + chunk.len as usize];
            assert_eq!(chunk.sha256, ed_sync::sha256_hex(slice));
        }
    }

    /// An index built elsewhere is adopted as a rebase: the four files
    /// validated, chunked and renamed into place, the side file carried,
    /// the manifest's routing product pointing at it with an empty chain.
    #[test]
    fn a_prebuilt_index_is_adopted_with_its_side_file_and_a_fresh_chain() {
        let json = r#"[
{"id64":1,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":2,"name":"Twin","coords":{"x":30,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true},{"type":"Star","subType":"Neutron Star","mainStar":false,"distanceToArrival":4000.0}]}
]"#;
        let built = tempfile::tempdir().unwrap();
        ed_galaxy::import::import_reader(Box::new(std::io::Cursor::new(json.as_bytes().to_vec())), built.path(), &mut |_| {}).unwrap();
        assert!(built.path().join(ed_galaxy::boost_side::BOOST_SIDE_FILE).is_file());
        let artifacts = tempfile::tempdir().unwrap();
        let publication = adopt_routing(built.path(), artifacts.path(), "77", "2026-09-10T12:00:00Z").unwrap();
        assert_eq!(publication.stats.systems, 2);
        let published = artifacts.path().join("routing").join("77");
        for name in ROUTING_FILES {
            assert!(published.join(name).is_file(), "{name} published");
        }
        assert!(published.join(ed_sync::CHUNKS_FILE).is_file(), "chunks.json written");
        assert!(published.join(ed_galaxy::boost_side::BOOST_SIDE_FILE).is_file(), "side file carried");
        assert_eq!(publication.files.len(), ROUTING_FILES.len() + 1, "the side file is not a product file");
        let g = Galaxy::open(&published).unwrap();
        assert_eq!(g.boost_secondary(g.find("Twin").unwrap()), Some((ed_galaxy::StarClass::Neutron, 4000.0)));
        let manifest = read_current_manifest(artifacts.path()).unwrap().unwrap();
        let routing = manifest.products.get(&ProductKey::Routing).unwrap();
        assert_eq!(routing.version, "77");
        assert!(routing.overlays.is_empty(), "a rebase starts a fresh chain");
        // A directory without the four files is refused before anything moves.
        let empty = tempfile::tempdir().unwrap();
        assert!(adopt_routing(empty.path(), artifacts.path(), "78", "2026-09-10T12:00:00Z").is_err());
        assert!(!artifacts.path().join("routing").join("78").exists());
    }

    /// The prune keeps current + grace + `overlays` and anything that is
    /// not a numeric version dir; everything else goes.
    #[test]
    fn prune_keeps_current_grace_overlays_and_strangers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("routing");
        for name in ["8", "45", "49", "50", "51", "overlays", "scratch-notes"] {
            std::fs::create_dir_all(root.join(name)).unwrap();
            std::fs::write(root.join(name).join("marker"), b"x").unwrap();
        }
        let removed = prune_routing_versions(dir.path(), "51", Some("50")).unwrap();
        assert_eq!(removed, ["45", "49", "8"]);
        for kept in ["50", "51", "overlays", "scratch-notes"] {
            assert!(root.join(kept).is_dir(), "{kept} should survive");
        }
        for gone in ["8", "45", "49"] {
            assert!(!root.join(gone).exists(), "{gone} should be pruned");
        }
        // No grace (fresh install), missing root: both fine.
        assert_eq!(prune_routing_versions(dir.path(), "51", None).unwrap(), ["50"]);
        let empty = tempfile::tempdir().unwrap();
        assert!(prune_routing_versions(empty.path(), "1", None).unwrap().is_empty());
    }
}
