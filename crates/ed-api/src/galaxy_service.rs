//! The serving process's resident galaxy (item 48): the published
//! routing product, memory-mapped once and shared by `/v1/route` and the
//! knowledge proxy. The daily reconcile republishes under a new version;
//! [`GalaxyService::current`] notices the manifest change and reopens.
//!
//! The neutron/white-dwarf highway sub-index (`plan_best` wants it for
//! supercharged plots) is a second, much smaller Galaxy built by
//! filtering the main one — ~30-60 s over the full galaxy, done once per
//! routing version in a background task into a dot-prefixed dir the
//! artifact route can never serve. The publishers (the daily reconcile, an adopt) build it for the new
//! version BEFORE the manifest points there, so the first plot after a
//! rebuild finds both indexes together (2026-09-21: the maintainer's
//! Colonia plot in the minute after the daily rebuild came back as 311
//! bare-range jumps instead of 59). The lazy build here is the belt for a
//! version published without one; the route handler waits a bounded
//! while for it, and a plot that still had to run without it says so
//! (`Route::highway_pending`) and is never cached.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use ed_galaxy::Galaxy;
use ed_sync::ProductKey;

use crate::routing::read_current_manifest;

struct Loaded {
    version: String,
    galaxy: Arc<Galaxy>,
    neutrons: Option<Arc<Galaxy>>,
    /// A build for `version` is running; do not start another.
    neutrons_building: bool,
}

pub struct GalaxyService {
    artifact_dir: PathBuf,
    state: tokio::sync::Mutex<Option<Loaded>>,
}

/// What a caller gets: the index pair for the current published version.
#[derive(Clone)]
pub struct GalaxyHandle {
    pub version: String,
    pub galaxy: Arc<Galaxy>,
    pub neutrons: Option<Arc<Galaxy>>,
}

impl GalaxyService {
    pub fn new(artifact_dir: PathBuf) -> Self {
        GalaxyService { artifact_dir, state: tokio::sync::Mutex::new(None) }
    }

    fn neutron_dir(&self, version: &str) -> PathBuf {
        // Dot-prefixed: `safe_relative_path` + the manifest gate keep it
        // out of /v1/artifacts either way, but never serving what was
        // never published is worth belt and braces.
        self.artifact_dir.join(".highway").join(version)
    }

    /// The current pair, reopening if the published version moved. `None`
    /// when no routing product has ever been published (a fresh box).
    pub async fn current(self: &Arc<Self>) -> Result<Option<GalaxyHandle>> {
        let manifest = read_current_manifest(&self.artifact_dir)?;
        let Some(product) = manifest.as_ref().and_then(|m| m.products.get(&ProductKey::Routing)) else {
            return Ok(None);
        };
        let version = product.version.clone();
        let mut state = self.state.lock().await;
        if state.as_ref().map(|l| l.version.as_str()) != Some(version.as_str()) {
            let dir = self.artifact_dir.join("routing").join(&version);
            let galaxy = tokio::task::spawn_blocking(move || Galaxy::open(&dir))
                .await
                .context("galaxy open panicked")??;
            tracing::info!(%version, systems = galaxy.count, "serve: routing index mapped");
            *state = Some(Loaded {
                version: version.clone(),
                galaxy: Arc::new(galaxy),
                neutrons: None,
                neutrons_building: false,
            });
        }
        let loaded = state.as_mut().expect("just loaded");
        if loaded.neutrons.is_none() && !loaded.neutrons_building {
            let dir = self.neutron_dir(&version);
            if Galaxy::exists(&dir) {
                match Galaxy::open(&dir) {
                    Ok(n) => loaded.neutrons = Some(Arc::new(n)),
                    Err(error) => {
                        tracing::warn!(%error, "serve: highway sub-index unreadable; rebuilding");
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                }
            }
            if loaded.neutrons.is_none() {
                loaded.neutrons_building = true;
                let service = Arc::clone(self);
                let galaxy = Arc::clone(&loaded.galaxy);
                let for_version = version.clone();
                tokio::spawn(async move {
                    service.build_neutrons(galaxy, for_version).await;
                });
            }
        }
        Ok(Some(GalaxyHandle {
            version: loaded.version.clone(),
            galaxy: Arc::clone(&loaded.galaxy),
            neutrons: loaded.neutrons.clone(),
        }))
    }

    async fn build_neutrons(self: Arc<Self>, galaxy: Arc<Galaxy>, version: String) {
        let dir = self.neutron_dir(&version);
        let started = std::time::Instant::now();
        let build = {
            let artifact_dir = self.artifact_dir.clone();
            let version = version.clone();
            tokio::task::spawn_blocking(move || build_highway_blocking(&galaxy, &artifact_dir, &version)).await
        };
        let outcome = match build {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(error)) => Err(error),
            Err(join) => Err(anyhow::anyhow!("highway build panicked: {join}")),
        };
        let mut state = self.state.lock().await;
        if let Some(loaded) = state.as_mut() {
            if loaded.version == version {
                loaded.neutrons_building = false;
                match outcome.and_then(|()| Galaxy::open(&dir)) {
                    Ok(n) => {
                        metrics::histogram!("edda_highway_build_seconds").record(started.elapsed().as_secs_f64());
                        tracing::info!(%version, secs = started.elapsed().as_secs(), "serve: highway sub-index built");
                        loaded.neutrons = Some(Arc::new(n));
                    }
                    Err(error) => {
                        // Next current() call retries; plots keep running
                        // highway-less meanwhile.
                        tracing::warn!(%error, "serve: highway sub-index build failed");
                    }
                }
            }
        }
        // Old versions' highways are dead weight once the pointer moves.
        if let Ok(entries) = std::fs::read_dir(self.artifact_dir.join(".highway")) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().as_ref() != version {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }
}

/// Where a routing version's highway sub-index lives (dot-prefixed: never
/// served as an artifact).
pub fn highway_dir(artifact_dir: &Path, version: &str) -> PathBuf {
    artifact_dir.join(".highway").join(version)
}

/// Build the neutron/white-dwarf highway sub-index for `galaxy` under
/// `.highway/<version>`, staged and renamed into place. Blocking (30-60 s
/// over the full galaxy). Shared by the serving process (lazy, as the
/// belt) and by the publishers, which call it BEFORE the manifest points
/// at the version, so a plot never meets a version without its highway.
pub fn build_highway_blocking(galaxy: &Galaxy, artifact_dir: &Path, version: &str) -> Result<PathBuf> {
    let dir = highway_dir(artifact_dir, version);
    if Galaxy::exists(&dir) {
        return Ok(dir);
    }
    let staging = artifact_dir.join(".highway").join(format!(".staging-{version}"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    let started = std::time::Instant::now();
    let built = ed_galaxy::import::subset_cells_cancellable(
        galaxy,
        &staging,
        ed_galaxy::long_range::NEUTRON_CELL_LY,
        |r| ed_galaxy::long_range::highway_star(galaxy.class(r)),
        &|| false,
    );
    if let Err(error) = built {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(error).context("building highway sub-index");
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::rename(&staging, &dir).context("publishing highway sub-index")?;
    metrics::histogram!("edda_highway_build_seconds").record(started.elapsed().as_secs_f64());
    tracing::info!(version, secs = started.elapsed().as_secs(), "highway sub-index built");
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The builder makes an openable sub-index and is idempotent; the
    /// serving path then finds it by `Galaxy::exists` and never has to
    /// build lazily.
    #[test]
    fn the_highway_is_built_where_the_serving_process_looks() {
        let dir = tempfile::tempdir().unwrap();
        let source = r#"[
{"id64":1,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":2,"name":"Jackson's Lighthouse","coords":{"x":-30,"y":0,"z":0},"bodies":[{"type":"Star","subType":"Neutron Star","mainStar":true}]}
]"#;
        let galaxy_dir = dir.path().join("routing").join("v1");
        ed_galaxy::import::import_reader(Box::new(source.as_bytes()), &galaxy_dir, &mut |_| {}).unwrap();
        let galaxy = Galaxy::open(&galaxy_dir).unwrap();
        let built = build_highway_blocking(&galaxy, dir.path(), "v1").unwrap();
        assert_eq!(built, highway_dir(dir.path(), "v1"));
        assert!(Galaxy::exists(&built), "openable where current() looks");
        let highway = Galaxy::open(&built).unwrap();
        assert_eq!(highway.count, 1, "the neutron only");
        assert_eq!(build_highway_blocking(&galaxy, dir.path(), "v1").unwrap(), built, "a second call is a no-op");
        assert!(!dir.path().join(".highway").join(".staging-v1").exists(), "no staging left behind");
    }
}
