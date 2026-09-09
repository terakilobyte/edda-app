//! The serving process's resident galaxy (item 48): the published
//! routing product, memory-mapped once and shared by `/v1/route` and the
//! knowledge proxy. The daily reconcile republishes under a new version;
//! [`GalaxyService::current`] notices the manifest change and reopens.
//!
//! The neutron/white-dwarf highway sub-index (`plan_best` wants it for
//! supercharged plots) is a second, much smaller Galaxy built by
//! filtering the main one — ~30-60 s over the full galaxy, done once per
//! routing version in a background task into a dot-prefixed dir the
//! artifact route can never serve. Plots that arrive before it is ready
//! run without the highway rather than waiting: a correct route now
//! beats a boosted route later.

use std::path::PathBuf;
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
        let staging = self.artifact_dir.join(".highway").join(format!(".staging-{version}"));
        let started = std::time::Instant::now();
        let build = {
            let staging = staging.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                if staging.exists() {
                    std::fs::remove_dir_all(&staging)?;
                }
                std::fs::create_dir_all(&staging)?;
                ed_galaxy::import::subset_cells_cancellable(
                    &galaxy,
                    &staging,
                    ed_galaxy::long_range::NEUTRON_CELL_LY,
                    |r| ed_galaxy::long_range::highway_star(galaxy.class(r)),
                    &|| false,
                )?;
                Ok(())
            })
            .await
        };
        let outcome = match build {
            Ok(Ok(())) => std::fs::rename(&staging, &dir).context("publishing highway sub-index"),
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
                        let _ = std::fs::remove_dir_all(&staging);
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
