//! Long-distance route plotting commands over the `ed-galaxy` star index.
//!
//! The index is opened lazily and cached: `.data/galaxy` (the full galaxy,
//! if the import has run) is preferred, `.data/galaxy_populated` (built
//! from the populated dump in ~90 s) is the fallback. Plotting runs on a
//! blocking worker with a cancel flag and progress events, like the trade
//! search, because a 20,000 ly plot can take a while.

use tauri::Manager as _;
use crate::exchange::SendApi;
use crate::state::AppState;
use ed_galaxy::router::{Control, RouteRequest};
use ed_galaxy::Galaxy;
use serde::Serialize;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, State};

pub struct RoutingState {
    galaxy: Mutex<Option<Arc<Galaxy>>>,
    /// Highway sub-index (neutrons and white dwarfs) for long-range
    /// planning; built lazily.
    neutrons: Mutex<Option<Arc<Galaxy>>>,
    /// Whether the last plot was allowed to boost off white dwarfs (opt-in;
    /// off until a plot asks); a re-plan of the followed route keeps the
    /// commander's choice.
    white_dwarfs: std::sync::atomic::AtomicBool,
    /// Bumped by every plot: a refinement watcher from an earlier plot
    /// sees the number move and stops polling.
    plot_generation: std::sync::atomic::AtomicU64,
    /// Whether the last plot minimised fuel stops (opt-in); a re-plan of
    /// the followed route keeps the commander's choice, or a min-fuel
    /// route would silently revert to eager scooping mid-flight.
    min_fuel: std::sync::atomic::AtomicBool,
    safe_margins: std::sync::atomic::AtomicBool,
    /// The app's job supervisor, once it exists (see `attach_jobs`).
    jobs: Mutex<Option<Arc<crate::jobs::Supervisor>>>,
}

/// The bubble index bundled with the installer: the inhabited galaxy's
/// coordinates and star classes, which never change enough to matter for
/// routing. ~3 MB compressed buys day-zero plotting; the community sync's
/// full routing index replaces it.
const BUNDLED_BUBBLE: [(&str, &[u8]); 4] = [
    ("stars.bin", include_bytes!("../assets/bubble/stars.bin.zst")),
    ("cells.bin", include_bytes!("../assets/bubble/cells.bin.zst")),
    ("names.bin", include_bytes!("../assets/bubble/names.bin.zst")),
    ("byname.bin", include_bytes!("../assets/bubble/byname.bin.zst")),
];

/// The pointer file naming the live highway sub-index directory. The
/// sub-index is versioned like the main galaxy index (item 47's Windows
/// lesson, round two: today's log showed rename-aside failing with
/// os error 5 on the LIVE mmap during a background rebuild — a mapped
/// directory on Windows can be neither removed NOR renamed). A rebuild
/// writes a fresh dir and flips this pointer; nothing live is touched.
fn neutron_pointer_path(galaxy_dir: &std::path::Path) -> std::path::PathBuf {
    let base = ed_galaxy::long_range::neutron_dir(galaxy_dir);
    base.with_file_name(format!(
        "{}.current",
        base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "boost".into())
    ))
}

/// The sub-index directory to open: the pointer's target when the file
/// names one, else the legacy fixed-name dir (existing installs).
fn current_neutron_dir(galaxy_dir: &std::path::Path) -> std::path::PathBuf {
    let legacy = ed_galaxy::long_range::neutron_dir(galaxy_dir);
    match std::fs::read_to_string(neutron_pointer_path(galaxy_dir)) {
        Ok(name) => {
            let name = name.trim();
            let base = legacy.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            // Only a sibling versioned dir is ever pointed at; anything
            // else in the file is corruption and falls back to legacy.
            if !name.is_empty() && name.starts_with(base.as_str()) && !name.contains(['/', '\\']) {
                galaxy_dir.join(name)
            } else {
                legacy
            }
        }
        Err(_) => legacy,
    }
}

/// A fresh, never-mapped directory name for a (re)build.
fn fresh_neutron_dir(galaxy_dir: &std::path::Path) -> std::path::PathBuf {
    let base = ed_galaxy::long_range::neutron_dir(galaxy_dir);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "boost".into());
    // A build and an immediate rebuild can share a millisecond; never
    // hand out a name that already exists.
    let mut tick = millis;
    loop {
        let candidate = base.with_file_name(format!("{name}-{tick}"));
        if !candidate.exists() {
            return candidate;
        }
        tick += 1;
    }
}

/// Atomically point the sub-index resolution at `dir` (write + rename).
fn point_neutrons_at(galaxy_dir: &std::path::Path, dir: &std::path::Path) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .context("sub-index dir has no name")?;
    let pointer = neutron_pointer_path(galaxy_dir);
    let pending = pointer.with_extension("current.pending");
    std::fs::write(&pending, name)?;
    std::fs::rename(&pending, &pointer)?;
    Ok(())
}

/// Remove every sub-index directory `current` does not name: earlier
/// layouts, pre-versioned rename-aside leftovers, superseded versions.
/// Best effort — a directory a plot still maps stays for a later pass.
fn collect_retired_neutron_dirs(galaxy_dir: &std::path::Path, current: &std::path::Path) {
    let base = ed_galaxy::long_range::neutron_dir(galaxy_dir);
    let base_name = base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut retired = vec![
        galaxy_dir.join("neutrons"),
        galaxy_dir.join(format!("neutrons{}", ed_galaxy::long_range::NEUTRON_CELL_LY as u32)),
    ];
    if let Ok(entries) = std::fs::read_dir(galaxy_dir) {
        for path in entries.flatten().map(|e| e.path()) {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
            let ours = name == base_name
                || name.starts_with(&format!("{base_name}.old-"))
                || name == format!("{base_name}.staging")
                || name.starts_with(&format!("{base_name}-"));
            if ours && path != current && path.is_dir() {
                retired.push(path);
            }
        }
    }
    for old in retired.into_iter().filter(|p| p.is_dir() && p != current) {
        match std::fs::remove_dir_all(&old) {
            Ok(()) => tracing::info!(dir = %old.display(), "retired highway sub-index removed"),
            Err(e) => tracing::debug!(dir = %old.display(), error = %e, "retired highway sub-index still mapped; removed later"),
        }
    }
}

/// The background rebuild, blocking: build a FRESH versioned directory,
/// flip the pointer, drop the cached handle so the next plot opens the
/// new files. The live directory is never renamed or removed — plots in
/// flight keep their mmap, and retired dirs are collected at the next
/// sub-index open.
fn rebuild_neutrons_blocking(
    routing: &Arc<RoutingState>,
    g: &Arc<Galaxy>,
    cancelled: &(dyn Fn() -> bool + Sync),
) {
    let fresh = fresh_neutron_dir(&g.dir);
    let started = std::time::Instant::now();
    let built = ed_galaxy::import::subset_cells_cancellable(
        g,
        &fresh,
        ed_galaxy::long_range::NEUTRON_CELL_LY,
        |r| ed_galaxy::long_range::highway_star(g.class(r)),
        cancelled,
    );
    let stats = match built {
        Ok(stats) => stats,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&fresh);
            if e.downcast_ref::<ed_galaxy::import::SubsetCancelled>().is_some() {
                tracing::info!("highway sub-index rebuild cancelled");
            } else {
                tracing::warn!(error = %e, "highway sub-index rebuild failed; keeping the current one");
            }
            return;
        }
    };
    if let Err(e) = point_neutrons_at(&g.dir, &fresh) {
        tracing::warn!(error = %e, "could not point at the rebuilt highway sub-index; keeping the current one");
        let _ = std::fs::remove_dir_all(&fresh);
        return;
    }
    *routing.neutrons.lock().unwrap_or_else(|e| e.into_inner()) = None;
    tracing::info!(stars = stats.systems, secs = started.elapsed().as_secs(), dir = %fresh.display(), "highway sub-index rebuilt with the learned stars");
}

/// Write the bundled bubble index into `data_dir/galaxy_populated` when
/// it is not there yet. Returns whether it installed one. Written beside
/// the target and renamed in, so a crash cannot leave a half index. This
/// is the only routing index the client has (B.4, 2026-09-09): the
/// downloadable full-galaxy index went with the local data; anything
/// past the bubble plots on the API.
pub fn install_bundled_bubble(data_dir: &std::path::Path) -> bool {
    let target = data_dir.join("galaxy_populated");
    if Galaxy::exists(&target) {
        return false;
    }
    let staging = data_dir.join("galaxy_populated.installing");
    let write = || -> anyhow::Result<()> {
        if staging.exists() {
            std::fs::remove_dir_all(&staging)?;
        }
        std::fs::create_dir_all(&staging)?;
        for (name, compressed) in BUNDLED_BUBBLE {
            std::fs::write(staging.join(name), ed_ebex::decompress(compressed)?)?;
        }
        Galaxy::validate_dir(&staging)?;
        if target.exists() {
            std::fs::remove_dir_all(&target)?;
        }
        std::fs::rename(&staging, &target)?;
        Ok(())
    };
    match write() {
        Ok(()) => {
            tracing::info!("bundled bubble index installed");
            true
        }
        Err(error) => {
            if cfg!(test) {
                tracing::warn!(%error, "bundled bubble install failed");
            }
            tracing::warn!(error = %format!("{error:#}"), "bundled bubble index not installed");
            let _ = std::fs::remove_dir_all(&staging);
            false
        }
    }
}

impl RoutingState {
    pub fn new() -> Self {
        RoutingState {
            galaxy: Mutex::new(None),
            neutrons: Mutex::new(None),
            white_dwarfs: std::sync::atomic::AtomicBool::new(false),
            plot_generation: std::sync::atomic::AtomicU64::new(0),
            min_fuel: std::sync::atomic::AtomicBool::new(true),
            safe_margins: std::sync::atomic::AtomicBool::new(false),
            jobs: Mutex::new(None),
        }
    }

    /// Open (once) the bundled bubble index, installing it first on a
    /// fresh data dir. `None` only if it could not be written or read.
    pub fn galaxy(&self, data_dir: &std::path::Path) -> Option<Arc<Galaxy>> {
        let mut g = self.galaxy.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            install_bundled_bubble(data_dir);
            let dir = data_dir.join("galaxy_populated");
            if Galaxy::exists(&dir) {
                match Galaxy::open(&dir) {
                    Ok(gal) => {
                        tracing::info!(dir = %dir.display(), systems = gal.count, "galaxy index opened");
                        *g = Some(Arc::new(gal));
                    }
                    Err(e) => tracing::warn!(error = %e, dir = %dir.display(), "galaxy index unreadable"),
                }
            }
        }
        g.clone()
    }

    /// The neutron sub-index for `g`, building it on first use (a single
    /// pass over the full index, ~30 s for the whole galaxy, then cached
    /// as `<index>/neutrons/`). `on_build` is told when a build starts.
    /// Rebuild the highway sub-index in the background after new highway
    /// stars were learned. Plots keep the index they have until the new one
    /// is complete: it is built beside the current one and swapped in with
    /// two renames, and the cached handle is dropped so the next plot opens
    /// the new files. A rebuild already running is left to finish; the
    /// supervisor's token cancels it (a build over the full galaxy is a
    /// minute of one core).
    pub fn rebuild_neutrons_in_background(self: &Arc<Self>, jobs: &crate::jobs::Supervisor, data_dir: &std::path::Path) {
        let Some(g) = self.galaxy(data_dir) else { return };
        self.rebuild_neutrons_in_background_with(jobs, g);
    }

    /// Remember the supervisor so background rebuilds can be scheduled from
    /// code that only holds the routing state.
    pub fn attach_jobs(&self, jobs: Arc<crate::jobs::Supervisor>) {
        *self.jobs.lock().unwrap_or_else(|e| e.into_inner()) = Some(jobs);
    }

    fn rebuild_neutrons_in_background_with(self: &Arc<Self>, jobs: &crate::jobs::Supervisor, g: Arc<Galaxy>) {
        let routing = self.clone();
        let spawned = jobs.spawn_blocking(crate::jobs::HIGHWAY_REBUILD, move |token| {
            rebuild_neutrons_blocking(&routing, &g, &|| token.is_cancelled());
        });
        match spawned {
            Ok(()) => tracing::info!("highway sub-index rebuild scheduled"),
            Err(_) => tracing::debug!("highway sub-index rebuild already running"),
        }
    }

    /// The highway sub-index for `g`, building it on first use (a single
    /// pass over the full index, ~1 min for the whole galaxy). `on_build` is
    /// told when a build starts; `cancelled` stops one (the plot's Stop).
    pub fn neutrons(&self, g: &Arc<Galaxy>, on_build: impl FnOnce(), cancelled: &(dyn Fn() -> bool + Sync)) -> anyhow::Result<Arc<Galaxy>> {
        let mut n = self.neutrons.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(n) = n.as_ref() {
            return Ok(n.clone());
        }
        let dir = current_neutron_dir(&g.dir);
        // Everything the current pointer does not name is retired: earlier
        // layouts ("neutrons", "neutrons250"), rename-aside leftovers
        // (".old-*", ".staging" from the pre-versioned swap), and versioned
        // dirs a later rebuild superseded. Best effort — a plot still
        // holding one keeps its mapping and the dir waits for a later open.
        collect_retired_neutron_dirs(&g.dir, &dir);
        if !Galaxy::exists(&dir) {
            on_build();
            let started = std::time::Instant::now();
            let fresh = fresh_neutron_dir(&g.dir);
            let built = ed_galaxy::import::subset_cells_cancellable(g, &fresh, ed_galaxy::long_range::NEUTRON_CELL_LY, |r| {
                // Learned classes count: a neutron or white dwarf found by
                // a scan or a knowledge sweep joins the highway on the next
                // rebuild.
                ed_galaxy::long_range::highway_star(g.class(r))
            }, cancelled);
            let stats = match built {
                Ok(stats) => stats,
                Err(e) => {
                    // A half-built directory is never pointed at, so it can
                    // only ever be collected, not opened.
                    let _ = std::fs::remove_dir_all(&fresh);
                    return Err(e);
                }
            };
            point_neutrons_at(&g.dir, &fresh)?;
            tracing::info!(stars = stats.systems, secs = started.elapsed().as_secs(), dir = %fresh.display(), "highway sub-index built (neutrons and white dwarfs)");
            let opened = Arc::new(Galaxy::open(&fresh)?);
            *n = Some(opened.clone());
            return Ok(opened);
        }
        let opened = Arc::new(Galaxy::open(&dir)?);
        *n = Some(opened.clone());
        Ok(opened)
    }

}

#[derive(Debug, Serialize)]
pub struct GalaxyStatus {
    pub available: bool,
    pub systems: usize,
    pub dir: Option<String>,
    /// True when only the populated-systems index is present.
    pub populated_only: bool,
}

#[tauri::command]
pub async fn galaxy_status(state: State<'_, AppState>, routing: State<'_, Arc<RoutingState>>) -> Result<GalaxyStatus, String> {
    Ok({
    let g = routing.galaxy(&state.data_dir);
    let bubble = state.data_dir.join("galaxy_populated");
    GalaxyStatus {
        available: g.is_some(),
        systems: g.as_ref().map(|g| g.count).unwrap_or(0),
        dir: g.as_ref().map(|g| g.dir.display().to_string()),
        populated_only: g.as_ref().is_some_and(|g| g.dir == bubble),
    }
    })
}

#[derive(Debug, Serialize)]
pub struct SystemHit {
    pub name: String,
    pub id64: u64,
    pub pos: [f32; 3],
    pub class: ed_galaxy::StarClass,
}

/// Name autocomplete over the index.
#[tauri::command]
pub async fn galaxy_complete(state: State<'_, AppState>, routing: State<'_, Arc<RoutingState>>, prefix: String) -> Result<Vec<SystemHit>, String> {
    Ok({
    let Some(g) = routing.galaxy(&state.data_dir) else { return Ok(Vec::new()) };
    g.complete(&prefix, 12)
        .into_iter()
        .map(|i| {
            let r = g.record(i);
            SystemHit { name: g.name(&r).to_string(), id64: r.id64, pos: r.pos(), class: g.class(&r) }
        })
        .collect()
    })
}

/// Exact system lookup in the compact route index. Unlike the populated
/// SQLite tables, this can resolve uninhabited systems from a full import.
#[tauri::command]
pub async fn galaxy_find(state: State<'_, AppState>, routing: State<'_, Arc<RoutingState>>, name: String) -> Result<Option<SystemHit>, String> {
    Ok({
        let Some(g) = routing.galaxy(&state.data_dir) else { return Ok(None) };
        g.find(&name).map(|i| {
            let r = g.record(i);
            SystemHit { name: g.name(&r).to_string(), id64: r.id64, pos: r.pos(), class: g.class(&r) }
        })
    })
}

#[derive(Debug, serde::Deserialize)]
struct EdsmCoords { x: f64, y: f64, z: f64 }

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct EdsmInformation {
    allegiance: Option<String>,
    government: Option<String>,
    population: Option<i64>,
    security: Option<String>,
    economy: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct EdsmPrimaryStar {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(rename = "isScoopable")]
    scoopable: Option<bool>,
}

#[derive(Debug, serde::Deserialize)]
struct EdsmSystemResponse {
    name: Option<String>,
    coords: Option<EdsmCoords>,
    // EDSM returns an empty array rather than an empty object when it has no
    // political/economic information for a system.
    information: Option<serde_json::Value>,
    #[serde(rename = "primaryStar")]
    primary_star: Option<EdsmPrimaryStar>,
}

#[derive(Debug, serde::Serialize)]
pub struct EdsmSystemInfo {
    name: String,
    coords: Option<[f64; 3]>,
    allegiance: Option<String>,
    government: Option<String>,
    population: Option<i64>,
    security: Option<String>,
    primary_economy: Option<String>,
    primary_star: Option<String>,
    scoopable: Option<bool>,
}

/// Network fallback for systems absent from the local inhabited database.
/// This async command runs outside the webview/UI thread.
#[tauri::command]
pub async fn edsm_system(state: State<'_, AppState>, name: String) -> Result<Option<EdsmSystemInfo>, String> {
    // Through OUR server only (maintainer, 2026-09-06: "route the EDSM calls
    // through our API"): the community proxy answers from its own galaxy
    // and asks EDSM upstream on our behalf, so a system name never leaves
    // the commander's machine for a third party. No API configured, or a
    // server that predates the endpoint, means "not known" — never a
    // direct call.
    let Some(api) = crate::exchange::endpoint(&state) else {
        return Ok(None);
    };
    let response = state
        .http
        .get(format!("{api}/v1/knowledge/system"))
        .timeout(std::time::Duration::from_secs(20))
        .query(&[("name", name.as_str())])
        .send_api().await.map_err(|e| e.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response: EdsmSystemResponse = response
        .error_for_status().map_err(|e| e.to_string())?
        .json().await.map_err(|e| e.to_string())?;
    let Some(found_name) = response.name.filter(|n| !n.is_empty()) else { return Ok(None) };
    let info = response.information
        .and_then(|v| serde_json::from_value::<EdsmInformation>(v).ok())
        .unwrap_or_default();
    Ok(Some(EdsmSystemInfo {
        name: found_name,
        coords: response.coords.map(|c| [c.x, c.y, c.z]),
        allegiance: info.allegiance,
        government: info.government,
        population: info.population,
        security: info.security,
        primary_economy: info.economy,
        primary_star: response.primary_star.as_ref().and_then(|s| s.kind.clone()),
        scoopable: response.primary_star.and_then(|s| s.scoopable),
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct NameHit {
    pub name: String,
    #[serde(default)]
    pub detail: Option<String>,
}

/// Name completion for any search box. Systems: the bundled bubble
/// index answers first (populated systems, no network), and when that
/// leaves the list short the API's `/v1/names/complete` fills it from
/// the full galaxy (maintainer, 2026-09-08: "beagle point not autocompleting
/// as a target for routing" - Beagle Point is unpopulated, so it is not
/// in the bundle). Stations: the API alone (B.4: no station table).
/// Local hits stay first; the API's are appended, deduped by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NameKind {
    System,
    Station,
    /// Ship hulls, from the bundled journal catalog: no API, no index.
    Ship,
    /// Module names, from the bundled outfitting table: no API, no index.
    Module,
    /// Minor faction names, from the server's precomputed list (0025):
    /// no local source (2026-10-01, the trade panel's "sell to faction").
    Faction,
}

#[tauri::command]
pub async fn name_complete(state: State<'_, AppState>, routing: State<'_, Arc<RoutingState>>, kind: NameKind, prefix: String) -> Result<Vec<NameHit>, String> {
    Ok({
    let p = prefix.trim();
    // Hulls are a bundled list of fifty, so one letter is enough and the
    // answer is local (maintainer, 2026-09-12: the Market tab offered no
    // completion at all when searching for a ship).
    if kind == NameKind::Ship {
        return Ok(ed_journal::ships::complete(p, NAME_HITS)
            .into_iter()
            .map(|name| NameHit { name: name.to_string(), detail: None })
            .collect());
    }
    if kind == NameKind::Module {
        return Ok(ed_journal::modules::complete_modules(p, NAME_HITS).into_iter().map(|name| NameHit { name, detail: None }).collect());
    }
    if kind == NameKind::Faction {
        if p.len() < 2 {
            return Ok(Vec::new());
        }
        return Ok(crate::remote_lookup::complete_names(&state, kind, p, NAME_HITS).await.unwrap_or_default());
    }
    if p.len() < 2 {
        return Ok(Vec::new());
    }
    let local: Vec<NameHit> = match (kind, routing.galaxy(&state.data_dir)) {
        (NameKind::System, Some(g)) => g
            .complete(p, NAME_HITS)
            .into_iter()
            .map(|i| {
                let r = g.record(i);
                NameHit { name: g.name(&r).to_string(), detail: None }
            })
            .collect(),
        _ => Vec::new(),
    };
    if local.len() >= NAME_HITS {
        return Ok(local);
    }
    let remote = crate::remote_lookup::complete_names(&state, kind, p, NAME_HITS).await.unwrap_or_default();
    merge_name_hits(local, remote, NAME_HITS)
    })
}

/// Rows per completion list, local and remote alike.
const NAME_HITS: usize = 12;

/// Local hits first, then the API's that are not already there (by name,
/// case-insensitive), cut to `limit`.
pub fn merge_name_hits(local: Vec<NameHit>, remote: Vec<NameHit>, limit: usize) -> Vec<NameHit> {
    let mut out = local;
    for hit in remote {
        if out.len() >= limit {
            break;
        }
        if !out.iter().any(|h| h.name.eq_ignore_ascii_case(&hit.name)) {
            out.push(hit);
        }
    }
    out.truncate(limit);
    out
}

#[cfg(test)]
mod name_hits_tests {
    use super::*;

    fn hit(name: &str) -> NameHit {
        NameHit { name: name.into(), detail: None }
    }

    /// The bundle knows the bubble; the API knows Beagle Point. Local
    /// stays first, a name both know appears once, the limit holds.
    #[test]
    fn remote_names_fill_what_the_bundle_lacks() {
        let merged = merge_name_hits(vec![hit("Beta Hydri")], vec![hit("beta hydri"), hit("Beagle Point"), hit("Bebia")], 2);
        assert_eq!(merged, vec![hit("Beta Hydri"), hit("Beagle Point")]);
        assert_eq!(merge_name_hits(vec![], vec![hit("Beagle Point")], 12), vec![hit("Beagle Point")], "an empty bundle answer is the API's");
        let full: Vec<NameHit> = (0..12).map(|i| hit(&format!("S{i}"))).collect();
        assert_eq!(merge_name_hits(full.clone(), vec![hit("Beagle Point")], 12), full, "a full local list is not touched");
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PlotQuery {
    pub from: Option<String>,
    pub to: String,
    pub range_ly: Option<f32>,
    pub supercharge: Option<bool>,
    pub max_dry_jumps: Option<u32>,
    pub weight: Option<f32>,
    pub thorough: Option<bool>,
    /// Plan with fuel physics (default true). Off = fixed range, no scoop stops.
    pub fuel: Option<bool>,
    /// Tonnes to keep in the tank at all times (default 0).
    pub reserve_t: Option<f32>,
    /// Plan for this ship (journal ShipID) instead of the one being flown.
    #[serde(default)]
    pub ship_id: Option<i64>,
    /// How long the planner may keep looking for a better route:
    /// "high" (default, 2 min), "medium" (1 min), "low" (30 s).
    #[serde(default)]
    pub effort: Option<String>,
    /// Use the FSD injections the commander can synthesise when nothing
    /// else crosses a gap (default true). They are never planned into a
    /// route that works without them.
    #[serde(default)]
    pub injections: Option<bool>,
    /// Boost off white dwarfs as well as neutrons. Opt-in (default false):
    /// a white-dwarf boost takes about twice as long to line up as a
    /// neutron's. Off plots with a x1.0 white-dwarf multiplier: the exact
    /// planner treats that as no boost and the coarse scan skips them.
    #[serde(default)]
    pub white_dwarfs: Option<bool>,
    /// Only stop for fuel when the tank requires it. Opt-in (default
    /// false): plans skip top-up scoops, refuelling at the latest
    /// scoopable star already passed only when the next jump would land
    /// under the floor (one max-fuel jump in hand). Fewer stops, same
    /// jump count or better; the tank rides lower.
    #[serde(default)]
    pub min_fuel: Option<bool>,
    /// Plan with the 2 t safety band on every reach (default false: the
    /// optimistic h0 era; the live monitor carries the safety burden).
    #[serde(default)]
    pub safe_margins: Option<bool>,
    /// Jumps-vs-refuels dial (default 1.0 = the journal-fit time model
    /// as measured). 0 judges by flying time alone; higher trades
    /// jumps away for fewer fuel stops.
    #[serde(default)]
    pub stop_weight: Option<f32>,
    /// Absolute fewest jumps, whatever it costs to find: thorough
    /// search, stops priced at zero, waves keep digging while the
    /// budget lasts, full 2-minute budget.
    #[serde(default)]
    pub try_hard: Option<bool>,
    /// Plan at THIS cargo load instead of what the hold carries right
    /// now (maintainer, 2026-09-05: "if edda is plotting the route we should
    /// be smart enough to factor in cargo for a planned trade route").
    /// The trade follower passes the departing stop's shopping list, so
    /// the outbound leg is laden-honest before a single ton is bought.
    #[serde(default)]
    pub cargo_t: Option<i64>,
}

/// The fuel model at a PLANNED load rather than the live hold: the
/// trade follower plots the leg out of a pad at the mass its shopping
/// list will create (maintainer, 2026-09-05). `None` keeps the live cargo.
pub fn with_planned_cargo(mut m: ed_galaxy::fuel::FuelModel, cargo_t: Option<i64>) -> ed_galaxy::fuel::FuelModel {
    if let Some(t) = cargo_t {
        m.cargo = t.max(0) as f32;
    }
    m
}

/// The plotter's time budget for an effort level.
pub fn effort_budget_ms(effort: Option<&str>) -> u64 {
    match effort.map(|e| e.trim().to_ascii_lowercase()).as_deref() {
        Some("low") => 30_000,
        Some("medium") => 60_000,
        _ => 120_000,
    }
}

/// The most fuel this ship has burned in one jump: FSDJump events flown
/// while it was the ship in the latest Loadout before them. (Taking every
/// jump after the ship's first Loadout once credited the Mandalay with a
/// Panther's 13 t hops and planned it as if it could barely jump twice.)
fn observed_max_fuel(conn: &rusqlite::Connection, ship: &str) -> Option<f32> {
    let mut st = conn.prepare("SELECT event, json_extract(raw,'$.Ship'), json_extract(raw,'$.FuelUsed') FROM events WHERE event IN ('Loadout','FSDJump') ORDER BY ts, file, offset").ok()?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<f64>>(2)?))).ok()?;
    let mut current: Option<String> = None;
    let mut best: Option<f32> = None;
    for (event, s, used) in rows.flatten() {
        if event == "Loadout" {
            current = s;
        } else if current.as_deref() == Some(ship) {
            if let Some(u) = used {
                best = Some(best.map_or(u as f32, |b| b.max(u as f32)));
            }
        }
    }
    best
}

/// The ship's fuel physics and supercharge multipliers from the latest
/// Loadout, plus the fuel aboard now. `None` without a Loadout.
/// Item 31 app wiring: the "safe margins" toggle. h0 (headroom_t 0.0)
/// is the default era; safe mode plans every reach with a 2 t phantom
/// band — twice the measured burn-model error, the h2 rung the matrix
/// crowned before the user chose optimism as the default.
impl RoutingState {
    /// The commander's current safe-margins choice, for plot paths that
    /// do not come through the panel (the ship computer's tool) — item
    /// 39's collision fix: one source of truth for plot options.
    pub fn sticky_safe_margins(&self) -> bool {
        self.safe_margins.load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub fn apply_safe_margins(m: &mut ed_galaxy::fuel::FuelModel, safe: bool) {
    m.headroom_t = if safe { 2.0 } else { 0.0 };
}

pub fn ship_fuel(conn: &rusqlite::Connection) -> Option<(ed_galaxy::fuel::FuelModel, ed_galaxy::fuel::BoostProfile, f32, String)> {
    ship_fuel_for(conn, None)
}

/// The best FSD injection grade the commander can synthesise now, with how
/// many: (range multiplier, grade, count). Recipes per Inara: basic carbon +
/// vanadium + germanium (+25 %); standard adds cadmium + niobium (+50 %);
/// premium carbon + germanium + arsenic + niobium + yttrium + polonium (+100 %).
pub fn injection_available(state: &AppState) -> Option<(f32, &'static str, u32)> {
    injections_status(state).into_iter().find(|g| g.can_make > 0).map(|g| (g.mult, g.grade, g.can_make))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InjectionMaterial {
    pub name: String,
    pub have: u32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct InjectionGrade {
    pub grade: &'static str,
    pub mult: f32,
    /// Injections of this grade the commander can synthesise now.
    pub can_make: u32,
    pub materials: Vec<InjectionMaterial>,
}

/// Every injection grade against the materials aboard, best first.
pub fn injections_status(state: &AppState) -> Vec<InjectionGrade> {
    let have = |sym: &str| -> u32 {
        state.with_read(|s| s.conn().query_row("SELECT count FROM materials WHERE symbol = ?1 COLLATE NOCASE", [sym], |r| r.get::<_, i64>(0)).unwrap_or(0)) as u32
    };
    ed_galaxy::router::INJECTION_RECIPES
        .iter()
        .map(|&(mult, grade, mats)| {
            let materials: Vec<InjectionMaterial> = mats.iter().map(|m| InjectionMaterial { name: (*m).to_string(), have: have(m) }).collect();
            let can_make = materials.iter().map(|m| m.have).min().unwrap_or(0);
            InjectionGrade { grade, mult, can_make, materials }
        })
        .collect()
}

/// The FSD injections the commander can synthesise now, by grade.
#[tauri::command]
pub async fn injections_available(state: State<'_, AppState>) -> Result<Vec<InjectionGrade>, String> {
    Ok(injections_status(&state))
}

/// The journal ShipID of the ship being flown: the newest row of the
/// derived `ships` table.
///
/// Until 2026-09-16 every Loadout read in this file took "the latest" as
/// the last row in FILE order. The game has written two file-name
/// formats side by side since 2022 and the old one sorts after the new
/// one as text, so on a veteran's journal the router planned with a 2021
/// ship's physics (maintainer, flying a donated seven-year journal: a
/// Caspian Explorer to Colonia in 365 plain jumps, no drive model). The
/// `ships` table holds one row per ship, newest Loadout by time, and
/// every reader below asks it instead.
pub fn current_ship_id(conn: &rusqlite::Connection) -> Option<i64> {
    let raw: String = conn.query_row("SELECT raw FROM ships ORDER BY loadout_ts DESC LIMIT 1", [], |r| r.get(0)).ok()?;
    serde_json::from_str::<serde_json::Value>(&raw).ok()?.get("ShipID")?.as_i64()
}

/// Fuel model for one ship by ShipID (its latest Loadout), or the current
/// ship. Another ship is assumed to start with a full tank -- it is not the
/// one whose tank the journal reports.
/// The FSD's health in a Loadout (0..1), if the drive is listed.
pub fn loadout_fsd_health(loadout: &serde_json::Value) -> Option<f32> {
    loadout
        .get("Modules")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find(|m| m.get("Slot").and_then(serde_json::Value::as_str) == Some("FrameShiftDrive"))
        .and_then(|m| m.get("Health"))
        .and_then(serde_json::Value::as_f64)
        .map(|h| h as f32)
}

/// The latest Loadout of `ship_id` (or the ship being flown), parsed.
fn latest_loadout(conn: &rusqlite::Connection, ship_id: Option<i64>) -> Option<serde_json::Value> {
    let current = current_ship_id(conn);
    let raw: String = match ship_id {
        Some(id) if Some(id) != current => conn
            .query_row("SELECT raw FROM ships WHERE ship_id = ?1", [id], |r| r.get(0))
            .ok()?,
        _ => conn.query_row("SELECT raw FROM ships ORDER BY loadout_ts DESC LIMIT 1", [], |r| r.get(0)).ok()?,
    };
    serde_json::from_str(&raw).ok()
}

/// The FSD integrity of the ship (`ship_id`, or the one being flown) from
/// its latest Loadout; `None` when no Loadout is known.
pub fn ship_fsd_integrity(conn: &rusqlite::Connection, ship_id: Option<i64>) -> Option<f32> {
    loadout_fsd_health(&latest_loadout(conn, ship_id)?)
}

/// Whether a Loadout carries an AFMU (`int_repairer_size…`).
pub fn loadout_has_afmu(loadout: &serde_json::Value) -> bool {
    loadout
        .get("Modules")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|ms| ms.iter().any(|m| m.get("Item").and_then(serde_json::Value::as_str).is_some_and(|i| i.to_ascii_lowercase().contains("int_repairer"))))
}

/// Whether the ship (`ship_id`, or the one being flown) has an AFMU, from
/// its latest Loadout; `None` when no Loadout is known.
pub fn ship_has_afmu(conn: &rusqlite::Connection, ship_id: Option<i64>) -> Option<bool> {
    Some(loadout_has_afmu(&latest_loadout(conn, ship_id)?))
}

/// Whether a Loadout carries a fuel scoop (`int_fuelscoop_size…`).
pub fn loadout_has_fuel_scoop(loadout: &serde_json::Value) -> bool {
    loadout
        .get("Modules")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|ms| ms.iter().any(|m| m.get("Item").and_then(serde_json::Value::as_str).is_some_and(|i| i.to_ascii_lowercase().contains("fuelscoop"))))
}

/// Fuel-scoop rate in tonnes per second from a Loadout's
/// `int_fuelscoop_size<N>_class<M>` item (class 5 = A … 1 = E). The game's
/// table is one ladder: an A scoop's rate per size, and each rating step
/// down is one seventh off (E = 3/7 of A).
pub fn loadout_scoop_rate_t_per_s(loadout: &serde_json::Value) -> Option<f32> {
    const A_RATE_KG_S: [f32; 8] = [42.0, 75.0, 176.0, 342.0, 577.0, 878.0, 1245.0, 1680.0];
    let module = loadout
        .get("Modules")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .find(|m| {
            m.get("Item")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|i| i.to_ascii_lowercase().contains("fuelscoop"))
        })?;
    let item = module.get("Item")?.as_str()?.to_ascii_lowercase();
    let size: usize = item.split("size").nth(1)?.split('_').next()?.parse().ok()?;
    let class: u32 = item.split("class").nth(1)?.parse().ok()?;
    if !(1..=8).contains(&size) || !(1..=5).contains(&class) {
        return None;
    }
    // class 1..=5 (E..A) -> 3/7 .. 7/7 of the A rate.
    let stock = A_RATE_KG_S[size - 1] * (class + 2) as f32 / 7.0 / 1000.0;
    // "Scoop rate enhanced" (G1-G5, +10-50 %) engineers the rate; the
    // Loadout modifier is the real value when present.
    //
    // Documented, not yet measured: no journal in the Commander's history
    // carries an engineered scoop (218 stock fuel-scoop slots scanned),
    // so both the "ScoopRate" label and the value's encoding are the
    // community's expectation, not our observation. Frontier writes
    // absolute stats elsewhere (MaxFuelPerJump in tonnes) but a bare
    // multiplier is also plausible, and 1.5x collides with a big scoop's
    // t/s reading — so instead of guessing a unit, score the encodings
    // against the module's own stock rate and take the first landing
    // inside the engineering band [stock, stock x 1.55], in order kg/s,
    // t/s, multiplier: absolute readings first (Frontier's convention
    // elsewhere), which in the ambiguous window is also the conservative
    // pick — the ETA overstates scoop time rather than understating it.
    // A value fitting none keeps the stock rate.
    let engineered = module
        .get("Engineering")
        .and_then(|e| e.get("Modifiers"))
        .and_then(serde_json::Value::as_array)
        .and_then(|mods| {
            mods.iter()
                .find(|m| {
                    m.get("Label")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|l| l.eq_ignore_ascii_case("scooprate"))
                })
                .and_then(|m| m.get("Value"))
                .and_then(serde_json::Value::as_f64)
        });
    if let Some(value) = engineered {
        let value = value as f32;
        let candidates = [value / 1000.0, value, stock * value];
        if let Some(rate) = candidates
            .into_iter()
            .find(|r| *r >= stock - 1e-4 && *r <= stock * 1.55)
        {
            return Some(rate);
        }
    }
    Some(stock)
}

/// The most recent main-tank reading the journal recorded: FSDJump's
/// post-jump `FuelLevel`, or FuelScoop's post-scoop `Total`. The
/// fallback when Status.json carries no live Fuel block.
fn last_journal_fuel(conn: &rusqlite::Connection) -> Option<f64> {
    conn.query_row(
        "SELECT COALESCE(json_extract(raw,'$.FuelLevel'), json_extract(raw,'$.Total')) \
         FROM events WHERE event IN ('FSDJump','FuelScoop') ORDER BY ts DESC, offset DESC LIMIT 1",
        [],
        |r| r.get::<_, Option<f64>>(0),
    )
    .ok()
    .flatten()
}

/// Whether the ship (`ship_id`, or the one being flown) has a fuel scoop,
/// from its latest Loadout; `None` when no Loadout is known.
pub fn ship_has_fuel_scoop(conn: &rusqlite::Connection, ship_id: Option<i64>) -> Option<bool> {
    let current = current_ship_id(conn);
    let raw: String = match ship_id {
        Some(id) if Some(id) != current => conn
            .query_row("SELECT raw FROM ships WHERE ship_id = ?1", [id], |r| r.get(0))
            .ok()?,
        _ => conn.query_row("SELECT raw FROM ships ORDER BY loadout_ts DESC LIMIT 1", [], |r| r.get(0)).ok()?,
    };
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(loadout_has_fuel_scoop(&v))
}

/// Scoop rate, drive cap and tank size for the ETA projection: display
/// inputs, not planning inputs.
#[derive(Debug, serde::Serialize)]
pub struct ScoopInfo {
    pub scoop_rate_t_per_s: Option<f32>,
    pub max_fuel_per_jump: f32,
    pub capacity: f32,
}

#[tauri::command]
pub async fn ship_scoop_info(state: State<'_, AppState>, ship_id: Option<i64>) -> Result<Option<ScoopInfo>, String> {
    Ok(state.with_read(|s| {
        let conn = s.conn();
        let (model, _, _, _) = ship_fuel_for(conn, ship_id)?;
        let rate = latest_loadout(conn, ship_id).and_then(|l| loadout_scoop_rate_t_per_s(&l));
        Some(ScoopInfo { scoop_rate_t_per_s: rate, max_fuel_per_jump: model.max_fuel_per_jump, capacity: model.capacity })
    }))
}

pub fn ship_fuel_for(conn: &rusqlite::Connection, ship_id: Option<i64>) -> Option<(ed_galaxy::fuel::FuelModel, ed_galaxy::fuel::BoostProfile, f32, String)> {
    let current = current_ship_id(conn);
    let raw: String = match ship_id {
        Some(id) if Some(id) != current => conn
            .query_row("SELECT raw FROM ships WHERE ship_id = ?1", [id], |r| r.get(0))
            .ok()?,
        _ => conn.query_row("SELECT raw FROM ships ORDER BY loadout_ts DESC LIMIT 1", [], |r| r.get(0)).ok()?,
    };
    let other_ship = matches!(ship_id, Some(id) if Some(id) != current);
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    // One fuel model for the app and the web router (ed_galaxy::loadout,
    // 2026-09-09): drive size/rating/SCO/Mk II, the engineered or table
    // cap, the Guardian booster and the laden mass come from there. The
    // journal adds what only it knows: the most this ship has actually
    // burned in one jump (first-hand beats the table), the hold, the
    // real scoop rate and the tank right now.
    let ship = v.get("Ship").and_then(serde_json::Value::as_str).unwrap_or("").to_string();
    let observed: Option<f32> = observed_max_fuel(conn, &ship);
    let cargo: f32 = conn.query_row("SELECT COALESCE(SUM(count),0) FROM cargo", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as f32;
    let physics = match ed_galaxy::loadout::physics_from_loadout(&v, cargo, observed) {
        Ok(p) => p,
        Err(error) => {
            tracing::debug!(%error, "loadout has no usable drive physics");
            return None;
        }
    };
    let capacity = physics.model.capacity;
    let mut model = physics.model;
    // Item 22: route_score prices a refuel as overhead + tonnes/rate,
    // with this ship's ACTUAL scoop (engineering included). No scoop
    // leaves 0.0 and the engine's conservative fallback applies.
    model.scoop_rate = loadout_scoop_rate_t_per_s(&v).unwrap_or(0.0);
    let boost = physics.boost;
    // Item 45: the game blanks Status.json's Fuel block outside the
    // cockpit (main menu, on foot, game closed), and assuming a full
    // tank there plans a route the real tank cannot fly. Fail closed:
    // the journal's last fuel reading is the truth we have; capacity
    // only when the journal has never seen this tank at all.
    // And in the SRV or on foot the block is the VEHICLE's, not the
    // ship's (FuelMain 0.0 beside the buggy's 0.37 t reservoir): a ship
    // planned from it has no first jump, and the server answered
    // no_route to a 220 ly hop the tank could fly four times over
    // (maintainer, 2026-09-27, plotting from the SRV at Synuefe NL-N
    // c23-4). Status fuel counts only with InMainShip set; otherwise
    // the journal's last reading of the ship's own tank.
    let status = conn
        .query_row(
            "SELECT json_extract(raw,'$.Fuel.FuelMain'), COALESCE(json_extract(raw,'$.Flags'), 0) FROM snapshots WHERE name = 'Status.json'",
            [],
            |r| Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok();
    let (now, source): (f32, &str) = match status {
        Some((Some(fuel), flags)) if flags & crate::status_flags::IN_MAIN_SHIP != 0 => (fuel as f32, "Status.json, in the ship"),
        Some((Some(_), _)) => match last_journal_fuel(conn) {
            Some(fuel) => (fuel as f32, "the journal's last reading (Status.json is the vehicle's, not the ship's)"),
            None => (capacity, "capacity (Status.json is the vehicle's and the journal has never seen the tank)"),
        },
        _ => match last_journal_fuel(conn) {
            Some(fuel) => (fuel as f32, "the journal's last reading (no live Status fuel)"),
            None => (capacity, "capacity (no reading anywhere)"),
        },
    };
    tracing::debug!(fuel = now, source, "start fuel");
    // The label's first " · " token is the display name (time_fit reads
    // it back); the rest is the shared summary, with the Mk II note the
    // app has always shown.
    let label = format!(
        "{} · {}",
        ed_journal::ships::display_name(&ship),
        physics.summary().replace(" Mk II", " Mk II (x6 neutron, 6.8 t cap)"),
    );
    Some((model, boost, if other_ship { capacity } else { now }, label))
}

#[tauri::command]
pub async fn plot_route(
    app: AppHandle,
    state: State<'_, AppState>,
    routing: State<'_, Arc<RoutingState>>,
    query: PlotQuery,
) -> Result<ed_galaxy::router::Route, String> {
    plot_inner(app, state.inner(), routing.inner().clone(), query).await
}

/// Re-plan the followed route from the current system with the real tank,
/// keep following it, and say what changed.
pub async fn replan_followed(app: AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let ar = state.with_read(|s| crate::follow::load(s.conn())).ok_or("no route is being followed")?;
    let dest = ar.route.hops.last().map(|h| h.name.clone()).ok_or("empty route")?;
    let routing = state.routing.clone();
    let white_dwarfs = routing.white_dwarfs.load(std::sync::atomic::Ordering::Relaxed);
    let min_fuel = routing.min_fuel.load(std::sync::atomic::Ordering::Relaxed);
    let safe_margins = routing.safe_margins.load(std::sync::atomic::Ordering::Relaxed);
    let query = PlotQuery { from: None, to: dest.clone(), range_ly: None, supercharge: Some(true), max_dry_jumps: None, weight: None, thorough: Some(false), fuel: Some(true), reserve_t: None, ship_id: None, effort: Some("medium".into()), injections: None, white_dwarfs: Some(white_dwarfs), min_fuel: Some(min_fuel), safe_margins: Some(safe_margins), stop_weight: None, try_hard: None, cargo_t: None };
    let route = plot_inner(app.clone(), state.inner(), routing, query).await?;
    let here = state.with_read(|s| ed_store::query::location(s.conn()).ok().flatten().and_then(|l| l.system_name));
    let next = here.as_deref().and_then(|h| route.hops.iter().position(|x| x.name.eq_ignore_ascii_case(h))).map(|i| i + 1).unwrap_or(1).min(route.hops.len());
    let jumps = route.jumps;
    let stops = route.refuel_stops;
    // Same hops ahead and the same stops: the tank changed nothing worth
    // saying (maintainer, 2026-09-09: "chatty voice about recalculating
    // routes" — a re-plan two minutes after the plot spoke a plan
    // identical to the one being flown).
    let unchanged = replan_is_same(&ar, &route, next);
    let new = crate::follow::ActiveRoute { route, next, source: "replan".into() };
    state.with_store(|s| crate::follow::save_pub(s.conn(), &new))?;
    use tauri::Emitter;
    let _ = app.emit(crate::events::ROUTE_FOLLOW, crate::follow::view(Some(&new)));
    let _ = app.emit(crate::events::ROUTE_REPLANNED, &new.route);
    if unchanged {
        tracing::info!(jumps, stops, "re-plan: same hops ahead and same stops; nothing said");
        return Ok(format!("Plan unchanged: {jumps} jumps to {dest}, {stops} fuel stop{}.", if stops == 1 { "" } else { "s" }));
    }
    let text = format!("Re-planned for the tank: {jumps} jumps to {dest}, {stops} fuel stop{}. {}", if stops == 1 { "" } else { "s" }, crate::follow::advance_text(&new));
    crate::watcher::deliver(&app, vec![(crate::callouts::Callout { kind: "route", text: text.clone(), priority: 1, speak: true, ts: String::new() }, None)]);
    Ok(text)
}

/// Does the re-planned route repeat what is already being flown: the
/// same systems from the cursor onward and the same number of stops?
pub fn replan_is_same(old: &crate::follow::ActiveRoute, new: &ed_galaxy::router::Route, new_next: usize) -> bool {
    let names = |hops: &[ed_galaxy::router::Hop]| -> Vec<String> { hops.iter().map(|h| h.name.to_ascii_lowercase()).collect() };
    same_plan_ahead(&names(&old.route.hops), old.next, old.route.refuel_stops, &names(&new.hops), new_next, new.refuel_stops)
}

/// The rule behind [`replan_is_same`], on names so it can be pinned.
pub fn same_plan_ahead(old: &[String], old_next: usize, old_stops: usize, new: &[String], new_next: usize, new_stops: usize) -> bool {
    old.iter().skip(old_next.saturating_sub(1)).eq(new.iter().skip(new_next.saturating_sub(1))) && old_stops == new_stops
}

#[cfg(test)]
mod replan_same_tests {
    use super::same_plan_ahead;
    fn n(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }
    /// The 02:07 re-plan: same three hops ahead, zero stops both times -
    /// silent. A different hop, a new stop, or a shifted cursor is worth saying.
    #[test]
    fn a_replan_that_repeats_the_plan_is_silent() {
        let old = n(&["ega", "a", "b", "crucis"]);
        assert!(same_plan_ahead(&old, 1, 0, &n(&["ega", "a", "b", "crucis"]), 1, 0));
        assert!(same_plan_ahead(&old, 2, 0, &n(&["a", "b", "crucis"]), 1, 0), "re-planned from the second hop: what is AHEAD still matches");
        assert!(!same_plan_ahead(&old, 1, 0, &n(&["a", "b", "crucis"]), 1, 0), "the old cursor still sees ega ahead");
        assert!(!same_plan_ahead(&old, 1, 0, &n(&["ega", "a", "c", "crucis"]), 1, 0));
        assert!(!same_plan_ahead(&old, 1, 0, &n(&["ega", "a", "b", "crucis"]), 1, 1));
    }
}

/// Plot a route to `dest` for the trade follower: the same quiet,
/// medium-effort shape as a replan, honoring the sticky toggles.
/// `cargo_t` is the load the leg will CARRY (the departing stop's
/// shopping list) so the plan is laden-honest before the buy happens.
pub(crate) async fn plot_for_trade(app: AppHandle, dest: String, cargo_t: Option<i64>) -> Result<ed_galaxy::router::Route, String> {
    let state = app.state::<AppState>();
    let routing = state.routing.clone();
    let white_dwarfs = routing.white_dwarfs.load(std::sync::atomic::Ordering::Relaxed);
    let min_fuel = routing.min_fuel.load(std::sync::atomic::Ordering::Relaxed);
    let safe_margins = routing.safe_margins.load(std::sync::atomic::Ordering::Relaxed);
    let query = PlotQuery { from: None, to: dest, range_ly: None, supercharge: Some(true), max_dry_jumps: None, weight: None, thorough: Some(false), fuel: Some(true), reserve_t: None, ship_id: None, effort: Some("medium".into()), injections: None, white_dwarfs: Some(white_dwarfs), min_fuel: Some(min_fuel), safe_margins: Some(safe_margins), stop_weight: None, try_hard: None, cargo_t };
    plot_inner(app.clone(), state.inner(), routing, query).await
}

/// Plot through POST /v1/route when no local index exists. The physics
/// still come from the LOCAL loadout (fuel model, boost, cargo,
/// margins) — the server contributes only the galaxy and the engine.
/// One honest divergence: the server can't know the start star's
/// scoopability, so the plan departs on the actual tank (its resolve
/// tops up to full only when we send no figure at all).
/// Why the API did not return a route: the server could not be reached
/// or answered badly (worth a retry, and `send_api` already made one),
/// or it answered and said no (a retry changes nothing until an input
/// does). The two read differently to the commander: "did not answer …
/// try again in a moment" was said of a no_route the server had decided
/// on an empty tank (maintainer, 2026-09-27: "why are we not auto
/// retrying? and why did it fail then succeed?").
#[derive(Debug)]
enum PlotFailure {
    /// The commander pressed Stop while the route server was working.
    /// Not a failure to recover from: no bubble-index fallback, no retry.
    Stopped,
    Transport(String),
    Refused(String),
}

impl PlotFailure {
    fn text(&self) -> &str {
        match self {
            PlotFailure::Transport(t) | PlotFailure::Refused(t) => t,
            PlotFailure::Stopped => "Plot stopped.",
        }
    }
}

async fn plot_via_api(state: &AppState, query: &PlotQuery, cancel: &tokio_util::sync::CancellationToken) -> Result<(ed_galaxy::router::Route, ServerAnswer), PlotFailure> {
    let api = crate::exchange::endpoint(state)
        .ok_or_else(|| PlotFailure::Refused("no galaxy index and no community API configured: install the index from Settings → System data, or set the API address there".into()))?;
    let (here, ship) = state.with_read(|s| {
        let conn = s.conn();
        let here = ed_store::query::location(conn).ok().flatten().and_then(|l| l.system_name);
        (here, ship_fuel_for(conn, query.ship_id))
    });
    let from = query
        .from
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or(here)
        .ok_or_else(|| PlotFailure::Refused("no origin: not in a known system".into()))?;
    let use_fuel = query.fuel.unwrap_or(true);
    let (fuel_model, boost, start_fuel) = match (&ship, use_fuel) {
        (Some((m, b, now, label)), true) => {
            let mut m = with_planned_cargo(*m, query.cargo_t);
            m.reserve = query.reserve_t.unwrap_or(0.0).max(0.0);
            apply_safe_margins(&mut m, query.safe_margins.unwrap_or(false));
            // Every input that can change the answer is in the line: two plots a
            // minute apart returned 311 and 59 hops for one ship and the log
            // could not say what differed (maintainer, 2026-09-21).
            tracing::info!(ship = %label, full_tank_range = m.range_at(m.capacity), from = %from, to = %query.to, supercharge = ?query.supercharge, white_dwarfs = ?query.white_dwarfs, start_fuel = now, cargo_t = ?query.cargo_t, reserve_t = ?query.reserve_t, safe_margins = ?query.safe_margins, "plotting via API with fuel model");
            (Some(m), Some(*b), Some(*now))
        }
        (Some((_, b, _, _)), false) => (None, Some(*b), None),
        (None, _) => (None, None, None),
    };
    let body = serde_json::json!({
        "from": from,
        "to": query.to,
        "range_ly": query.range_ly,
        "fuel_model": fuel_model,
        "boost": boost,
        "start_fuel": start_fuel,
        "supercharge": query.supercharge,
        "white_dwarfs": query.white_dwarfs,
        "min_fuel": query.min_fuel,
        "max_dry_jumps": query.max_dry_jumps,
        "weight": query.weight,
        "stop_weight": query.stop_weight,
        "thorough": query.thorough,
        // Last resort only, server-side as well (2026-10-09): the planner
        // never spends one on a route that works without it, and the
        // answer flags every hop that needs one so the Route tab can hold
        // it against the materials aboard.
        "injection": injection_wire(injection_available(state), query.injections != Some(false)),
    });
    let started = std::time::Instant::now();
    let mut body = body;
    // One retry: a system the server does not know by name (422
    // unknown_system) may be one this commander has visited — the
    // journal knows its coordinates, and the server plots to a position
    // (API-only spec: the no-EDMC path). Nothing leaves the machine that
    // the request did not already carry, plus three numbers.
    let mut retried_with_coords = false;
    loop {
        let request = state
            .http
            .post(format!("{api}/v1/route"))
            .json(&body)
            // The server's long lane budgets 120 s (a desert plot such as
            // Wongi → Beagle Point uses all of it); give it that plus
            // transit. At 40 s the client gave up on plots the server
            // was still going to answer (the assistant session, 2026-09-09).
            .timeout(std::time::Duration::from_secs(130))
            // Plain send, not send_api(): its gateway retry re-sends a plot
            // the server is still working on and doubles the wait (the boss,
            // 2026-10-07: 242 s for a far-rim plot, 504 at 120 s then the
            // retry's own 120 s). A 504 here means "still planning": the
            // server keeps going and caches the route, so the honest answer
            // is to say so and let the next replot collect it.
            .send();
        // Stop races the request: the commander's press wins at once, and
        // the server's answer, if it still comes, is dropped with the future.
        let response = tokio::select! {
            // Stop wins whenever both are ready: a press is a decision,
            // whatever the server was about to say.
            biased;
            _ = cancel.cancelled() => {
                tracing::info!(to = %query.to, ms = started.elapsed().as_millis() as u64, "plot stopped while the route server was working");
                return Err(PlotFailure::Stopped);
            }
            r = request => r.map_err(|error| PlotFailure::Transport(format!("route server unreachable: {error}")))?,
        };
        let status = response.status();
        let ms = started.elapsed().as_millis() as u64;
        if status.as_u16() == 429 {
            return Err(PlotFailure::Transport("the route server is busy — try again in a moment".into()));
        }
        if status.as_u16() == 504 {
            let detail = response.text().await.unwrap_or_default();
            return Err(budget_failure(&detail));
        }
        if status.is_server_error() {
            return Err(PlotFailure::Transport(format!("route server error ({status})")));
        }
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            let detail = detail.trim();
            if status.as_u16() == 422 && !retried_with_coords {
                if let Some(unknown) = serde_json::from_str::<serde_json::Value>(detail)
                    .ok()
                    .filter(|v| v.get("error").and_then(|e| e.as_str()) == Some("unknown_system"))
                    .and_then(|v| v.get("system").and_then(|n| n.as_str()).map(str::to_string))
                {
                    let coords = state.read_conn().ok().and_then(|conn| journal_coords(&conn, &unknown));
                    if let Some(pos) = coords {
                        let key = if body.get("to").and_then(|t| t.as_str()).is_some_and(|t| t.eq_ignore_ascii_case(&unknown)) { "to_coords" } else { "from_coords" };
                        tracing::info!(system = %unknown, key, "route: server does not know the system; retrying with the journal's coordinates");
                        body[key] = serde_json::json!(pos);
                        retried_with_coords = true;
                        continue;
                    }
                }
            }
            let code = serde_json::from_str::<serde_json::Value>(detail)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string));
            return Err(PlotFailure::Refused(match code.as_deref() {
                // Said with the inputs that decided it: the tank at
                // departure is the one a commander can change.
                Some("no_route") if detail.contains("\"island\"") => island_text(detail, &from, &query.to),
                Some("no_route") => {
                    let tank = match (start_fuel, &ship) {
                        (Some(f), Some((m, _, _, label))) => format!(
                            " for the {} departing with {f:.1} t of {:.0} t{}",
                            label.split(" · ").next().unwrap_or(label),
                            m.capacity,
                            if f < 1.0 { "; refuel first, or plot with the fuel model off" } else { "" }
                        ),
                        _ => String::new(),
                    };
                    format!("the route server found no route from {from} to {}{tank}", query.to)
                }
                Some(other) => format!("the route server refused the plot ({other}: {detail})"),
                None if detail.is_empty() => format!("the route server refused the plot ({status})"),
                None => format!("the route server refused the plot: {detail}"),
            }));
        }
        let answer: serde_json::Value = response
            .json()
            .await
            .map_err(|error| PlotFailure::Transport(format!("route server answer unreadable: {error}")))?;
        let served = ServerAnswer::of(&answer);
        let route: ed_galaxy::router::Route = serde_json::from_value(answer)
            .map_err(|error| PlotFailure::Transport(format!("route server answer unreadable: {error}")))?;
        let boosted = route.hops.iter().filter(|h| h.boosted).count();
        tracing::info!(hops = route.hops.len(), boosted, to = %query.to, ms, retried_with_coords, refining = served.refining, "route planned by API");
        if route.highway_pending {
            tracing::warn!(hops = route.hops.len(), to = %query.to, "route planned by API without the neutron highway (server rebuilding it): a bare-range route; replot in a minute");
        }
        return Ok((route, served));
    }
}

/// What the server said about its answer beside the route (ed-api
/// `augment`): an early answer still being refined, and the lane budget
/// that bounds how long the refinement can take.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ServerAnswer {
    pub refining: bool,
    pub budget_ms: u64,
}

impl ServerAnswer {
    pub fn of(answer: &serde_json::Value) -> Self {
        ServerAnswer {
            refining: answer.get("refining").and_then(|v| v.as_bool()).unwrap_or(false),
            budget_ms: answer.get("budget_ms").and_then(|v| v.as_u64()).unwrap_or(0),
        }
    }
}

/// How often the refinement watcher asks the server for the key, and how
/// long past the lane budget it keeps asking before giving up.
pub const REFINE_POLL: std::time::Duration = std::time::Duration::from_secs(5);
pub const REFINE_GRACE_MS: u64 = 15_000;

/// The server answered early and is refining under the same key (#198).
/// Poll the key: a better route so far is relayed to the Route tab as a
/// candidate (the numbers in the boxes, the line on the map), the
/// finished one as `done`. Stops on the plot's own cancel token (Stop),
/// when a newer plot starts, after three misses, or past the budget plus
/// a grace -- then says the first answer stands. (The boss, 2026-10-09,
/// after the 478-jump early answer hid a 373-jump refinement: "why was
/// it not shown initially?")
fn spawn_refine_watch(app: AppHandle, routing: Arc<RoutingState>, query: PlotQuery, cancel: tokio_util::sync::CancellationToken, generation: u64, early_jumps: usize, budget_ms: u64) {
    let _ = app.emit(crate::events::ROUTE_REFINING, serde_json::json!({ "early_jumps": early_jumps, "done": false, "route": null }));
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(budget_ms.max(10_000) + REFINE_GRACE_MS);
        let mut shown = early_jumps;
        let mut misses = 0u32;
        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(REFINE_POLL) => {}
            }
            if routing.plot_generation.load(std::sync::atomic::Ordering::Relaxed) != generation {
                return;
            }
            match plot_via_api(state.inner(), &query, &cancel).await {
                Ok((route, served)) if !served.refining => {
                    tracing::info!(early_jumps, final_jumps = route.jumps, to = %query.to, "route refinement finished on the server");
                    let _ = app.emit(crate::events::ROUTE_REFINING, serde_json::json!({ "early_jumps": early_jumps, "done": true, "route": route }));
                    return;
                }
                Ok((route, _)) => {
                    if route.jumps < shown {
                        shown = route.jumps;
                        tracing::info!(early_jumps, jumps = route.jumps, boosted = route.boosted_jumps, to = %query.to, "route refining: a better route so far");
                        let _ = app.emit(crate::events::ROUTE_REFINING, serde_json::json!({ "early_jumps": early_jumps, "done": false, "route": route }));
                    }
                }
                Err(PlotFailure::Stopped) => return,
                Err(failure) => {
                    misses += 1;
                    tracing::warn!(misses, error = %failure.text(), "route refinement poll failed");
                    if misses >= 3 {
                        break;
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                tracing::info!(early_jumps, budget_ms, "route refinement: nothing more from the server within the budget");
                break;
            }
        }
        let _ = app.emit(crate::events::ROUTE_REFINING, serde_json::json!({ "early_jumps": early_jumps, "done": true, "route": null }));
    });
}

/// The injection the plot may fall back on, as the server takes it:
/// the best grade the commander can synthesise and how many. With no
/// materials for any grade the request still names premium, capped, so
/// the answer can say "this needs injections you cannot make" instead
/// of a bare no-route (the boss, 2026-10-09). Off, nothing is sent.
pub fn injection_wire(available: Option<(f32, &'static str, u32)>, allow: bool) -> Option<serde_json::Value> {
    if !allow {
        return None;
    }
    let (grade, max) = match available {
        Some((_, grade, n)) if n > 0 => (grade, n),
        _ => ("premium", HYPOTHETICAL_INJECTIONS),
    };
    Some(serde_json::json!({ "grade": grade, "max": max }))
}

/// How many injections a plot may assume when the commander can make
/// none: enough to cross a rim's gaps, so the answer can name the bill.
pub const HYPOTHETICAL_INJECTIONS: u32 = 10;

/// An island refusal from the server (`why: "island"`), said with the
/// numbers that decided it: which end, the reach the plot was made with,
/// the nearest known star, and what a lighter tank or the injection would
/// have bought. Jongou XM-W d1-0 -> Byoi Fraae CQ-G d10-0 on a 72 ly
/// Caspian: both ends have their nearest known star 74-77 ly out.
pub fn island_text(detail: &str, from: &str, to: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(detail).unwrap_or_default();
    let f = |k: &str| v.get(k).and_then(|x| x.as_f64());
    let end = v.get("end").and_then(|x| x.as_str()).unwrap_or("destination");
    let system = v.get("system").and_then(|x| x.as_str()).unwrap_or(if end == "origin" { from } else { to });
    let reach = f("reach_ly").unwrap_or(0.0);
    let mut text = match f("nearest_ly") {
        Some(nearest) => format!(
            "no known star within {reach:.1} ly of {system} ({}): the nearest is {nearest:.1} ly away",
            if end == "origin" { "the origin" } else { "the destination" }
        ),
        None => format!("no known star anywhere near {system} ({}) at {reach:.1} ly", if end == "origin" { "the origin" } else { "the destination" }),
    };
    let nearest = f("nearest_ly");
    if let (Some(light), Some(n)) = (f("light_reach_ly"), nearest) {
        if light >= n && light > reach + 0.05 {
            text.push_str(&format!("; with one jump's fuel aboard the ship reaches {light:.1} ly, which would cross it"));
        }
    }
    match (f("injected_reach_ly"), nearest) {
        (Some(inj), Some(n)) if inj < n => text.push_str(&format!("; an FSD injection reaches {inj:.1} ly, still short")),
        (None, _) => text.push_str("; an FSD injection might cross it, but you can synthesise none"),
        _ => {}
    }
    text
}

/// A 504 from the route server: its own "budget" answer means the planner
/// ran its whole lane budget and found nothing in time (a refusal the
/// server remembers for two minutes, so a replot gets it back at once);
/// anything else in front of the server cutting the connection means the
/// planner may still be working and a replot collects the route.
pub(crate) fn budget_failure(detail: &str) -> PlotFailure {
    let v: serde_json::Value = serde_json::from_str(detail).unwrap_or_default();
    if v.get("error").and_then(|e| e.as_str()) == Some("budget") {
        let secs = v.get("budget_ms").and_then(|b| b.as_u64()).map(|ms| ms / 1000).unwrap_or(100);
        return PlotFailure::Refused(format!(
            "the route server ran out of its {secs} s budget without finding a route; a shorter leg, a different target, or the galaxy map's own plotter may get there"
        ));
    }
    PlotFailure::Transport("the route server is still working on this plot; replot in a moment and the route will be waiting".into())
}

/// Whether the bundled bubble index should have a go after the server
/// did not deliver: yes when the server could not be reached or errored,
/// no when it answered with a refusal -- its index holds everything the
/// bubble index holds.
pub(crate) fn bubble_worth_trying(remote: &PlotFailure) -> bool {
    matches!(remote, PlotFailure::Transport(_))
}

/// What the bundled bubble index can say about a system it does not
/// hold: it is outside the bubble, not unknown to EDDA (the boss,
/// 2026-10-09: "I'm not sure why this is an unknown system to us, it's a
/// well known system" -- the server knew it; the fallback did not).
pub fn bubble_text(local: &str) -> String {
    match local.strip_prefix("unknown system ") {
        Some(name) => format!("{name} is outside the bundled bubble index"),
        None => local.to_string(),
    }
}

/// Where the commander's own journal last saw a system: the StarPos of
/// the newest FSDJump / Location / CarrierJump naming it.
pub fn journal_coords(conn: &rusqlite::Connection, name: &str) -> Option<[f32; 3]> {
    let raw: String = conn
        .query_row(
            "SELECT raw FROM events WHERE event IN ('FSDJump', 'Location', 'CarrierJump') \
             AND json_extract(raw, '$.StarSystem') = ?1 COLLATE NOCASE \
             ORDER BY ts DESC LIMIT 1",
            [name.trim()],
            |r| r.get(0),
        )
        .ok()?;
    star_pos(&serde_json::from_str(&raw).ok()?)
}

/// A journal event's `StarPos`, as the planner's coordinates.
pub fn star_pos(event: &serde_json::Value) -> Option<[f32; 3]> {
    let p = event.get("StarPos")?.as_array()?;
    Some([p.first()?.as_f64()? as f32, p.get(1)?.as_f64()? as f32, p.get(2)?.as_f64()? as f32])
}

async fn plot_inner(app: AppHandle, state: &AppState, routing: Arc<RoutingState>, query: PlotQuery) -> Result<ed_galaxy::router::Route, String> {
    let started = std::time::Instant::now();
    let result = plot_inner_untimed(app, state, routing, query).await;
    crate::telemetry::record_timing("plot", started.elapsed().as_millis(), result.is_ok());
    result
}

async fn plot_inner_untimed(app: AppHandle, state: &AppState, routing: Arc<RoutingState>, query: PlotQuery) -> Result<ed_galaxy::router::Route, String> {
    // The plot runs on POST /v1/route — same engine, same Route type,
    // the server's full-galaxy index. When the server does not answer,
    // the bundled bubble index plots what it can (the inhabited galaxy)
    // and says why that is all it can do.
    // One token for the whole plot: the Route tab's Stop cancels it, and
    // until 2026-10-07 only the local planner looked at it -- an API plot
    // ran on for its full 35-50 s (the boss, HIP 90112 -> Beagle Point:
    // "stop button isn't working").
    let cancel = state.jobs.begin(crate::jobs::ROUTE_PLOT);
    let generation = routing.plot_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let remote = match plot_via_api(state, &query, &cancel).await {
        Ok((route, served)) => {
            if served.refining {
                spawn_refine_watch(app.clone(), routing.clone(), query.clone(), cancel.clone(), generation, route.jumps, served.budget_ms);
            }
            return Ok(route);
        }
        Err(PlotFailure::Stopped) => return Err(PlotFailure::Stopped.text().to_string()),
        Err(failure) => failure,
    };
    if !bubble_worth_trying(&remote) {
        // The server answered: it knows every system the bubble index
        // knows and more, so a refusal with its numbers is the answer,
        // not a reason to append "the bubble index cannot plot it either"
        // (the boss, 2026-10-09, reading exactly that).
        tracing::info!(error = %remote.text(), "remote plot refused; the server's answer stands");
        return Err(remote.text().to_string());
    }
    let transient = matches!(remote, PlotFailure::Transport(_));
    let remote = remote.text().to_string();
    tracing::warn!(error = %remote, transient, "remote plot failed; trying the bundled bubble index");
    if routing.galaxy(&state.data_dir).is_none() {
        return Err(remote);
    }
    plot_local(app, state, routing, &query).await.map_err(|local| {
        if local.starts_with("unknown system") || local.contains("no route") {
            let local = bubble_text(&local);
            if transient {
                format!("the community API did not answer ({remote}) and the bundled bubble index cannot plot this on its own ({local}); try again in a moment")
            } else {
                format!("{remote}; the bundled bubble index cannot plot it either ({local})")
            }
        } else {
            local
        }
    })
}

async fn plot_local(app: AppHandle, state: &AppState, routing: Arc<RoutingState>, query: &PlotQuery) -> Result<ed_galaxy::router::Route, String> {
    let query = query.clone();
    let Some(g) = routing.galaxy(&state.data_dir) else {
        return Err("no local routing index".into());
    };

    // Origin defaults to the current system; range to the ship's range at
    // its CURRENT mass -- the Loadout figure is the empty-hold maximum, and
    // the game plots with what is actually aboard.
    let (here, ship_range, ship, time_fit) = state.with_read(|s| {
        let conn = s.conn();
        let here = ed_store::query::location(conn).ok().flatten().and_then(|l| l.system_name);
        let ship = ship_fuel_for(conn, query.ship_id);
        // Item 28: this commander's own cadence, fitted per ship from
        // their journals; a fresh install has no fit and keeps defaults.
        let fit = ship.as_ref().and_then(|(_, _, _, label)| {
            let name = label.split(" \u{b7}").next().unwrap_or("").trim();
            crate::time_fit::fit_for_ship(conn, name)
        });
        (here, current_range(conn), ship, fit)
    });
    let from_name = query.from.clone().filter(|s| !s.trim().is_empty()).or(here).ok_or("no origin: not in a known system")?;
    let from = g.find(&from_name).ok_or_else(|| format!("unknown system {from_name:?}"))?;
    let to = g.find(&query.to).ok_or_else(|| format!("unknown system {:?}", query.to))?;
    // Fuel physics from the ship unless switched off; the planning range
    // is then the full-tank range (the honest one), not the Loadout maximum.
    let use_fuel = query.fuel.unwrap_or(true);
    // Under a scoopable star the plan starts from a full tank -- top up, then
    // go -- and the first hop is marked as the scoop. Elsewhere the tank is
    // what it is.
    let start_scoopable = g.class(&g.record(from)).scoopable();
    let (fuel_model, mut boost, start_fuel) = match (&ship, use_fuel) {
        (Some((m, b, now, label)), true) => {
            let mut m = with_planned_cargo(*m, query.cargo_t);
            m.reserve = query.reserve_t.unwrap_or(0.0).max(0.0);
            apply_safe_margins(&mut m, query.safe_margins.unwrap_or(false));
            let start = if start_scoopable { m.capacity } else { *now };
            tracing::info!(ship = %label, full_tank_range = m.range_at(m.capacity), fuel_now = now, start_fuel = start, "plotting with fuel model");
            (Some(m), *b, start)
        }
        (Some((_, b, _, _)), false) => (None, *b, 0.0),
        (None, _) => (None, ed_galaxy::fuel::BoostProfile::default(), 0.0),
    };
    let white_dwarfs = query.white_dwarfs.unwrap_or(false);
    // The planner minimizes time; lean stops are part of that (user,
    // 2026-09-03: "run it in minimize time by default, no knobs needed").
    // API callers may still pass false explicitly.
    let min_fuel = query.min_fuel.unwrap_or(true);
    let try_hard = query.try_hard.unwrap_or(false);
    // The dial multiplies the refuel term of the pilot-seconds judge;
    // try-hard is the preset that zeroes it and removes every early-out.
    let stop_weight = if try_hard { 0.0 } else { query.stop_weight.unwrap_or(1.0).clamp(0.0, 5.0) };
    tracing::info!(white_dwarfs, min_fuel, supercharge = query.supercharge.unwrap_or(true), injections = query.injections.unwrap_or(true), effort = ?query.effort, "plot options");
    routing.white_dwarfs.store(white_dwarfs, std::sync::atomic::Ordering::Relaxed);
    routing.min_fuel.store(min_fuel, std::sync::atomic::Ordering::Relaxed);
    routing.safe_margins.store(query.safe_margins.unwrap_or(false), std::sync::atomic::Ordering::Relaxed);
    if !white_dwarfs {
        // The commander's opt-out: a white dwarf is a plain star to this plot.
        boost.white_dwarf = 1.0;
    }
    let range_ly = query
        .range_ly
        .or(fuel_model.map(|m| m.range_at(m.capacity)))
        .or(ship_range.map(|r| r as f32))
        .unwrap_or(30.0)
        .max(1.0);
    let req = RouteRequest {
        from,
        to,
        range_ly,
        supercharge: query.supercharge.unwrap_or(true),
        max_dry_jumps: query.max_dry_jumps.unwrap_or(0),
        weight: query.weight.unwrap_or(1.3).max(1.0),
        max_expansions: 50_000_000,
        // Long routes always get the full portfolio: people plan trips on this.
        thorough: query.thorough.unwrap_or(true) || try_hard,
        injection: None,
        boost,
        fuel: fuel_model,
        start_fuel,
        time_budget_ms: if try_hard { effort_budget_ms(None) } else { effort_budget_ms(query.effort.as_deref()) },
        // Once a variant has a route the rest get a second to beat it.
        grace_ms: 1_000,
        min_fuel,
        stop_weight,
        prize_k: try_hard.then_some(0.0),
        t_jump_s: time_fit.and_then(|f| f.t_jump_s),
        stop_overhead_s: time_fit.and_then(|f| f.stop_overhead_s),
        secondary_boost_ls: 0.0,
    };

    let cancel = state.jobs.begin(crate::jobs::ROUTE_PLOT);
    let routing2 = routing.clone();
    // Star classes learned since the index was built.
    crate::spansh::load_star_overrides(state);
    // Only for the retry after "no route possible": a plan that works
    // without injections never gets one.
    let injection = if query.injections == Some(false) { None } else { injection_available(state) };
    let app_sweep = app.clone();
    let result: Result<ed_galaxy::router::Route, String> = tauri::async_runtime::spawn_blocking(move || {
        let cancelled = || cancel.is_cancelled();
        let stage_now = std::sync::Mutex::new(("exact", 0u32, 0u32));
        let progress = |n: u64, remaining: f32| {
            let (phase, leg, legs) = *stage_now.lock().unwrap_or_else(|e| e.into_inner());
            let _ = app.emit(crate::events::ROUTE_PROGRESS, serde_json::json!({ "expansions": n, "remaining_ly": remaining, "phase": phase, "leg": leg, "legs": legs }));
        };
        let stage = |phase: &'static str, leg: u32, legs: u32| {
            *stage_now.lock().unwrap_or_else(|e| e.into_inner()) = (phase, leg, legs);
            let _ = app.emit(crate::events::ROUTE_PROGRESS, serde_json::json!({ "expansions": 0, "remaining_ly": 0.0, "phase": phase, "leg": leg, "legs": legs }));
        };
        // Candidates for the map: a new best goes out at once; everything
        // else joins a pool, and a ticker shows one at random every two
        // seconds for as long as the search runs -- the variants finish in
        // bursts, and the map should keep moving between them.
        let pool: std::sync::Arc<std::sync::Mutex<(usize, Vec<ed_galaxy::router::Route>)>> = std::sync::Arc::new(std::sync::Mutex::new((usize::MAX, Vec::new())));
        let searching = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        {
            let pool = pool.clone();
            let searching = searching.clone();
            let app = app.clone();
            std::thread::spawn(move || {
                let mut seed: u64 = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
                while searching.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    if !searching.load(Ordering::Relaxed) {
                        break;
                    }
                    let pick = {
                        let p = pool.lock().unwrap_or_else(|e| e.into_inner());
                        if p.1.is_empty() {
                            None
                        } else {
                            seed ^= seed << 13;
                            seed ^= seed >> 7;
                            seed ^= seed << 17;
                            Some(p.1[(seed % p.1.len() as u64) as usize].clone())
                        }
                    };
                    if let Some(r) = pick {
                        let _ = app.emit(crate::events::ROUTE_CANDIDATE, &r);
                    }
                }
            });
        }
        let found = |route: &ed_galaxy::router::Route| {
            let mut p = pool.lock().unwrap_or_else(|e| e.into_inner());
            if route.jumps < p.0 {
                p.0 = route.jumps;
                let _ = app.emit(crate::events::ROUTE_CANDIDATE, route);
            } else {
                p.1.push(route.clone());
            }
        };
        let ctl = Control { cancelled: &cancelled, progress: &progress, stage: &stage, found: &found, trace: &|_, _, _| {} };
        let started = std::time::Instant::now();
        // `plan_best` decides: long routes go neutron-first (coarse plan
        // over neutron stars, legs refined exactly; the exact search alone
        // flood-fills the bubble at 22,000 ly), shorter ones exact -- with
        // the neutron-first planner alongside when a neutron is in the
        // corridor, so a mid-range plot does not walk past a x4 shortcut.
        // Thorough on a long route means the careful long-range planner,
        // not an exact search over the whole galaxy (that is hours).
        let neutrons = if req.supercharge {
            match routing2.neutrons(&g, || stage("neutrons", 0, 0), &cancelled) {
                Ok(n) => Some(n),
                Err(e) if e.downcast_ref::<ed_galaxy::import::SubsetCancelled>().is_some() => {
                    return Err(ed_galaxy::router::RouteError::Cancelled.to_string());
                }
                Err(e) => {
                    tracing::warn!(error = %e, "highway sub-index unavailable; exact search");
                    None
                }
            }
        } else {
            None
        };
        let r = ed_galaxy::long_range::plan_best(&g, neutrons.as_deref(), &req, &ctl).map_err(|e| e.to_string());
        // Nothing crosses at the ship's range: try again with the best FSD
        // injection the commander can synthesise, as sparingly as the
        // planner can manage.
        searching.store(false, Ordering::Relaxed);
        let r = match r {
            Err(e) if e.starts_with("Sorry, no route possible") => match injection {
                Some(inj) => {
                    tracing::info!(grade = inj.1, count = inj.2, "no plain route; retrying with FSD injections");
                    (stage)("injection", 0, 0);
                    let req2 = RouteRequest { injection: Some(inj), ..req.clone() };
                    let r2 = ed_galaxy::long_range::plan_best(&g, neutrons.as_deref(), &req2, &ctl).map_err(|e| e.to_string());
                    r2.map_err(|_| e)
                }
                None => Err(e),
            },
            other => other,
        };
        match &r {
            Ok(route) => tracing::info!(from = %from_name, to = %query.to, jumps = route.jumps, boosted = route.boosted_jumps, refuel = route.refuel_stops, expansions = route.expansions, ms = started.elapsed().as_millis() as u64, "route plotted"),
            Err(e) => tracing::info!(from = %from_name, to = %query.to, error = %e, ms = started.elapsed().as_millis() as u64, "route not plotted"),
        }
        // Same series names the server's routing endpoint will export;
        // collected locally, never transmitted (see metrics.rs).
        let outcome = match &r {
            Ok(_) => "ok",
            Err(e) if e == &ed_galaxy::router::RouteError::Cancelled.to_string() => "cancelled",
            Err(e) if e.starts_with("Sorry, no route possible") => "no_route",
            Err(_) => "error",
        };
        metrics::counter!("edda_route_requests_total", "outcome" => outcome).increment(1);
        metrics::histogram!("edda_route_wall_seconds").record(started.elapsed().as_secs_f64());
        if let Ok(route) = &r {
            metrics::histogram!("edda_route_jumps").record(route.jumps as f64);
        }
        r
    })
    .await
    .map_err(|e| e.to_string())?;
    // Hops whose star the index doesn't know: ask Spansh once and remember.
    let mut route = result?;
    // Which ship this plan belongs to.
    let (sid, slabel, has_scoop, integrity, has_afmu) = state.with_read(|s| {
        let id = query.ship_id.or_else(|| current_ship_id(s.conn()));
        (id, ship.as_ref().map(|(_, _, _, l)| l.split(" · ").next().unwrap_or(l).to_string()), ship_has_fuel_scoop(s.conn(), query.ship_id), ship_fsd_integrity(s.conn(), query.ship_id), ship_has_afmu(s.conn(), query.ship_id))
    });
    route.ship_id = sid;
    route.ship = slabel;
    // The plan assumes scoop stops; whether this ship can make them is the
    // follower's and the panel's business.
    route.ship_has_scoop = has_scoop;
    // Integrity is projected by the reader, not planned: 1 % per boost on
    // every drive but the Mk II SCO, repair due below 81 %.
    route.fsd_integrity = integrity;
    route.integrity_loss_per_boost = ship.as_ref().map(|(_, b, _, _)| b.integrity_loss_per_boost());
    route.ship_has_afmu = has_afmu;
    if let (Some((m, _, now, _)), Some(first)) = (&ship, route.hops.first_mut()) {
        if start_scoopable && use_fuel && *now < m.capacity - 0.5 {
            first.refuel = true;
            route.refuel_stops += 1;
        }
    }
    crate::spansh::resolve_unknown_stars(state, &mut route.hops).await;
    // In the background: star types around this route, from EDSM.
    crate::knowledge::sweep_route(app_sweep, route.hops.iter().map(|h| (h.name.clone(), h.pos)).collect());
    Ok(route)
}

#[tauri::command]
pub fn cancel_route(state: State<'_, AppState>) {
    state.jobs.cancel(crate::jobs::ROUTE_PLOT);
}

/// Systems within a radius, for the map background around a route.
#[tauri::command]
pub async fn galaxy_near(state: State<'_, AppState>, routing: State<'_, Arc<RoutingState>>, pos: [f32; 3], radius_ly: f32, limit: usize) -> Result<Vec<SystemHit>, String> {
    Ok({
    let Some(g) = routing.galaxy(&state.data_dir) else { return Ok(Vec::new()) };
    let mut hits = g.within(pos, radius_ly.clamp(1.0, 500.0));
    hits.sort_by(|a, b| a.1.total_cmp(&b.1));
    hits.truncate(limit.min(5000));
    hits.into_iter()
        .map(|(i, _)| {
            let r = g.record(i);
            SystemHit { name: g.name(&r).to_string(), id64: r.id64, pos: r.pos(), class: g.class(&r) }
        })
        .collect()
    })
}

/// Jump range at the ship's current mass: `MaxJumpRange` scaled by
/// (unladen + fuel-for-max-jump) / (unladen + current fuel + cargo aboard).
/// Falls back to `MaxJumpRange` when the masses are unknown.
pub fn current_range(conn: &rusqlite::Connection) -> Option<f64> {
    let (max_range, unladen, fuel_cap): (Option<f64>, Option<f64>, Option<f64>) = conn
        .query_row(
            "SELECT json_extract(raw,'$.MaxJumpRange'), json_extract(raw,'$.UnladenMass'), json_extract(raw,'$.FuelCapacity.Main')
             FROM events WHERE event = 'Loadout' ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()?;
    let max_range = max_range?;
    let cargo: f64 = conn.query_row("SELECT COALESCE(SUM(count),0) FROM cargo", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as f64;
    let fuel: f64 = conn
        .query_row("SELECT json_extract(raw,'$.Fuel.FuelMain') FROM snapshots WHERE name = 'Status.json'", [], |r| r.get::<_, Option<f64>>(0))
        .ok()
        .flatten()
        .or(fuel_cap)
        .unwrap_or(0.0);
    match unladen {
        // MaxJumpRange is quoted for a full tank and no cargo; the FSD range
        // scales with 1/mass at a fixed fuel burn.
        Some(u) if u > 0.0 => Some(max_range * (u + fuel_cap.unwrap_or(fuel)) / (u + fuel + cargo)),
        _ => Some(max_range),
    }
}

/// Item 39, game-route half: where along the GAME's own plotted route
/// will the pilot need fuel, and where can they get it? One entry per
/// hop that the burn model says must provide fuel: `via` is "scoop"
/// (scoopable star and the ship carries a scoop) or "station" (a
/// landable non-carrier station with a pad that fits this ship). The
/// HUD shows fuel icons ONLY on these hops — availability without need
/// stays silent (user, 2026-09-03).
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct GameFuelMark {
    pub index: usize,
    pub via: &'static str,
}

/// The pure simulation: `hops[i] = (leg leaving hop i in ly, fuel
/// available here as Some("scoop"|"station"))`, starting tank as given.
/// Mirrors the min-fuel rewrite's shape: fly without stopping; when a
/// leg would land under one max-effort escape jump, refuel to full at
/// the LATEST fuel-capable hop already passed, mark it, and re-simulate
/// from there. Unreachable legs stop the marking — the trap guard owns
/// live danger, this only decorates the plan.
pub fn mark_game_route_fuel(
    m: &ed_galaxy::fuel::FuelModel,
    start_fuel: f32,
    hops: &[(f64, Option<&'static str>)],
) -> Vec<GameFuelMark> {
    let floor = m.max_fuel_per_jump.max(m.reserve);
    let mut marks: Vec<GameFuelMark> = Vec::new();
    let mut fuel = start_fuel;
    let mut i = 0usize;
    while i < hops.len() {
        let leg = hops[i].0 as f32;
        if leg <= 0.0 {
            break;
        }
        let after = m.jump(leg, fuel, 1.0);
        let landing = after.unwrap_or(-1.0);
        if landing >= floor {
            fuel = landing;
            i += 1;
            continue;
        }
        // Need fuel before this leg: latest capable hop at or before i,
        // not already marked.
        let Some(j) = (0..=i)
            .rev()
            .find(|&j| hops[j].1.is_some() && !marks.iter().any(|mk| mk.index == j))
        else {
            break;
        };
        marks.push(GameFuelMark { index: j, via: hops[j].1.unwrap() });
        // Re-simulate from the refill.
        fuel = m.capacity;
        i = j;
        // The refill hop's own leg flies from a full tank.
        let leg = hops[i].0 as f32;
        match m.jump(leg, fuel, 1.0) {
            Some(left) if left >= floor || i + 1 == hops.len() => {
                fuel = left;
                i += 1;
            }
            _ => break,
        }
    }
    marks.sort_by_key(|mk| mk.index);
    marks
}

/// Every system on the game route, and the pad the ship needs there:
/// what [`game_route_fuel_marks_for`] wants the API asked about (one
/// `systems=` call, by the async caller). The whole route, not the hops
/// still ahead: the answer is memoised per route (`DockMemo`), and a key
/// that shrank with every jump would miss once per jump for nothing —
/// the marks only look up the hops ahead in the set.
pub fn game_route_dock_query(conn: &rusqlite::Connection, brief: &ed_store::route::RouteBrief) -> Option<(Vec<String>, ed_store::lookup::PadSize)> {
    let (ship_ident, _) = crate::trap::loadout_ship(conn)?;
    let pad = ed_store::lookup::PadSize::for_journal_ship(&ship_ident).unwrap_or(ed_store::lookup::PadSize::Large);
    Some((brief.hops.iter().map(|h| h.system.clone()).collect(), pad))
}

/// What a dock answer was fetched for: the route as plotted (its
/// timestamp and its hops, in order) and the pad the ship needs. A
/// re-plot to the same systems carries a new timestamp and misses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockKey {
    pub plotted: Option<String>,
    pub systems: Vec<String>,
    pub pad: ed_store::lookup::PadSize,
}

/// The last dock answer and what it was for. One entry: the pilot flies
/// one game route at a time, and the previous route's docks are of no use
/// once a new one is plotted.
#[derive(Debug, Default)]
pub struct DockMemo {
    entry: Option<(DockKey, std::collections::HashSet<String>)>,
}

impl DockMemo {
    pub fn get(&self, key: &DockKey) -> Option<std::collections::HashSet<String>> {
        self.entry.as_ref().filter(|(k, _)| k == key).map(|(_, docks)| docks.clone())
    }

    pub fn put(&mut self, key: DockKey, docks: std::collections::HashSet<String>) {
        self.entry = Some((key, docks));
    }
}

/// Which of the game route's systems hold a dock whose pads fit `pad`,
/// from the memo when the route has not changed, else from one
/// `/v1/stations?systems=` call at `api` (2026-10-07: `current_route`
/// runs on every journal tick from the HUD window and the Ship panel,
/// and every run asked the server the same question — 432 identical
/// calls in 19 min from one flying pilot). An unanswered call is not
/// remembered: the next tick asks again, as before.
pub async fn game_route_docks(state: &crate::state::AppState, api: Option<&str>, plotted: Option<&str>, systems: &[String], pad: ed_store::lookup::PadSize) -> std::collections::HashSet<String> {
    let key = DockKey { plotted: plotted.map(str::to_string), systems: systems.to_vec(), pad };
    if let Some(docks) = state.dock_memo.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return docks;
    }
    let Some(api) = api else { return Default::default() };
    match crate::remote_lookup::docks_by_systems_from(state, api, systems, pad).await {
        Some(docks) => {
            state.dock_memo.lock().unwrap_or_else(|e| e.into_inner()).put(key, docks.clone());
            docks
        }
        None => Default::default(),
    }
}

fn game_route_start(conn: &rusqlite::Connection, brief: &ed_store::route::RouteBrief) -> usize {
    // Simulate from the pilot's position on the route: the tank is what
    // it is NOW, and hops already behind need no decoration.
    let here = ed_store::query::location(conn).ok().flatten().and_then(|l| l.system_name);
    here.as_deref()
        .and_then(|h| brief.hops.iter().position(|hop| hop.system.eq_ignore_ascii_case(h)))
        .unwrap_or(0)
}

/// The journal-side assembly for [`mark_game_route_fuel`]: fuel
/// capability per game-route hop. Scoopable stars count when the ship
/// carries a scoop; a dock counts when `docks` (lower-cased system
/// names from `/v1/stations?systems=`, B.4 gap 3) names the hop's
/// system. No dock known reads as dry, which errs toward warning.
pub fn game_route_fuel_marks_for(conn: &rusqlite::Connection, brief: &ed_store::route::RouteBrief, docks: &std::collections::HashSet<String>) -> Vec<GameFuelMark> {
    let Some((m, _b, fuel_now, _label)) = ship_fuel(conn) else { return Vec::new() };
    let Some((_ship_ident, has_scoop)) = crate::trap::loadout_ship(conn) else { return Vec::new() };
    let start = game_route_start(conn, brief);
    let hops: Vec<(f64, Option<&'static str>)> = brief.hops[start..]
        .iter()
        .map(|h| {
            let via = if h.scoopable && has_scoop {
                Some("scoop")
            } else {
                docks.contains(&h.system.to_ascii_lowercase()).then_some("station")
            };
            (h.next_leg_ly, via)
        })
        .collect();
    let mut marks = mark_game_route_fuel(&m, fuel_now, &hops);
    for mk in &mut marks {
        mk.index += start;
    }
    marks
}

#[cfg(test)]
mod planned_cargo_tests {
    use super::*;

    /// The plan-laden fix, pinned with real drive physics: a Panther-
    /// class hold at 1,008 t must shrink the planning range, an
    /// explicit zero must beat a full live hold, None must change
    /// nothing, and a negative wire value clamps to empty.
    #[test]
    fn planned_cargo_reshapes_the_fuel_model_honestly() {
        // A big hauler: 1,790 t unladen, 128 t tank, ~37.6 ly empty.
        let live = ed_galaxy::fuel::FuelModel::from_loadout(
            1790.9, 128.0, 8.0, 8, true, true, 37.568, 0.0, 0.0,
        );
        let empty_range = live.range_at(live.capacity);
        let laden = with_planned_cargo(live, Some(1008));
        let laden_range = laden.range_at(laden.capacity);
        assert_eq!(laden.cargo, 1008.0);
        assert!(
            laden_range < empty_range * 0.75,
            "1,008 t must cost real range: {laden_range:.1} vs empty {empty_range:.1}"
        );
        // The maintainer's field numbers: ~37.6 empty, ~24.6 laden. The model
        // should land in that neighbourhood, not just "less".
        assert!((20.0..30.0).contains(&laden_range), "laden range {laden_range:.1} out of family");

        let mut full = live;
        full.cargo = 1008.0;
        let emptied = with_planned_cargo(full, Some(0));
        assert_eq!(emptied.cargo, 0.0, "an explicit empty plan overrides a full live hold");
        assert_eq!(with_planned_cargo(full, None).cargo, 1008.0, "None keeps the live hold");
        assert_eq!(with_planned_cargo(live, Some(-5)).cargo, 0.0, "negative wire values clamp");
    }
}

#[cfg(test)]
mod subindex_swap_tests {
    use super::*;
    use ed_galaxy::StarClassCode as _;
    use std::sync::Arc;

    fn tiny_galaxy(root: &std::path::Path) -> Arc<Galaxy> {
        let source = r#"[
{"id64":1,"name":"Jackson's Lighthouse","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"Neutron Star","mainStar":true}]},
{"id64":2,"name":"Sol","coords":{"x":10.0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]}
]"#;
        ed_galaxy::import::import_reader(Box::new(source.as_bytes()), root, &mut |_| {}).unwrap();
        Arc::new(Galaxy::open(root).unwrap())
    }

    /// Item 47's Windows family, round two (2026-09-04 log: rename-aside
    /// of boost250 failed with os error 5 under a live plot's mmap). A
    /// rebuild must NEVER rename or remove the live directory: the old
    /// dir survives while a handle holds it, the pointer flips to the
    /// fresh build, and the retired dir is collected at the next open.
    #[test]
    fn a_rebuild_never_touches_the_live_sub_index_directory() {
        let root = tempfile::tempdir().unwrap();
        let g = tiny_galaxy(root.path());
        let routing = Arc::new(RoutingState::new());
        let live = routing.neutrons(&g, || {}, &|| false).unwrap();
        let live_dir = live.dir.clone();
        assert!(Galaxy::exists(&live_dir));

        // A plot holds `live` mapped while the background rebuild runs.
        rebuild_neutrons_blocking(&routing, &g, &|| false);
        assert!(live_dir.is_dir(), "the live sub-index dir must never be renamed or removed under an open mmap");
        assert_eq!(live.name(&live.record(0)), "Jackson's Lighthouse", "the held handle still reads");
        let current = current_neutron_dir(&g.dir);
        assert_ne!(current, live_dir, "the pointer names the fresh rebuild");
        assert!(Galaxy::exists(&current));

        // The next open serves the fresh dir and collects the retired
        // one once nothing holds it.
        drop(live);
        let reopened = routing.neutrons(&g, || {}, &|| false).unwrap();
        assert_eq!(reopened.dir, current);
        assert!(!live_dir.exists(), "the retired dir is collected at the next open");
    }

    /// Existing installs: a legacy fixed-name boost250 with no pointer
    /// file keeps serving; the first rebuild moves the world to the
    /// versioned scheme and the legacy dir is collected afterwards.
    #[test]
    fn a_legacy_sub_index_serves_until_the_first_versioned_rebuild() {
        let root = tempfile::tempdir().unwrap();
        let g = tiny_galaxy(root.path());
        let legacy = ed_galaxy::long_range::neutron_dir(&g.dir);
        ed_galaxy::import::subset_cells(&g, &legacy, ed_galaxy::long_range::NEUTRON_CELL_LY, |r| {
            ed_galaxy::long_range::highway_star(ed_galaxy::StarClass::from_code(r.class))
        })
        .unwrap();
        let routing = Arc::new(RoutingState::new());
        let opened = routing.neutrons(&g, || panic!("legacy dir exists; no build"), &|| false).unwrap();
        assert_eq!(opened.dir, legacy, "no pointer: the legacy dir serves");

        rebuild_neutrons_blocking(&routing, &g, &|| false);
        drop(opened);
        let reopened = routing.neutrons(&g, || {}, &|| false).unwrap();
        assert_ne!(reopened.dir, legacy, "the pointer now names a versioned dir");
        assert!(!legacy.exists(), "the legacy dir is collected once unpinned");
    }
}

#[cfg(test)]
mod game_fuel_tests {
    use super::*;

    /// Item 39: fuel icons on the game route mean "you will need fuel by
    /// here, and here has it" — a scoopable star mid-route gets the
    /// mark, abundant availability without need gets nothing, and a
    /// route with no fuel anywhere marks nothing (the trap guard owns
    /// the danger case).
    #[test]
    fn game_route_marks_need_not_availability() {
        let m = ed_galaxy::fuel::FuelModel::from_loadout(100.0, 100.0, 8.0, 5, false, false, 75.0, 0.0, 0.0);
        // Five 50-ly legs from a 20 t tank burn ~5-6 t each; the floor is
        // 8 t, so fuel is needed mid-route. Hop 1 scoops, hop 3 has a
        // station.
        let hops = [
            (50.0, None),
            (50.0, Some("scoop")),
            (50.0, None),
            (50.0, Some("station")),
            (50.0, None),
            (0.0, None),
        ];
        let marks = mark_game_route_fuel(&m, 20.0, &hops);
        // Need arises before leg 3; the LATEST capable hop wins (the
        // min-fuel rewrite's rule), which is the station at 3 — not the
        // scoopable at 1 that a naive earliest-first would pick.
        assert_eq!(marks, vec![GameFuelMark { index: 3, via: "station" }], "{marks:?}");
        // With only the early scoop available, it gets the mark instead.
        let scoop_only = [
            (50.0, None),
            (50.0, Some("scoop")),
            (50.0, None),
            (50.0, None),
            (50.0, None),
            (0.0, None),
        ];
        let marks = mark_game_route_fuel(&m, 20.0, &scoop_only);
        assert_eq!(marks, vec![GameFuelMark { index: 1, via: "scoop" }], "{marks:?}");
        // From a FULL tank the same route needs nothing: no marks at all,
        // even though fuel is available at two hops.
        assert!(mark_game_route_fuel(&m, 100.0, &hops).is_empty(), "availability without need is silent");
        // No fuel anywhere: nothing to mark.
        let dry: Vec<(f64, Option<&'static str>)> = hops.iter().map(|(d, _)| (*d, None)).collect();
        assert!(mark_game_route_fuel(&m, 20.0, &dry).is_empty());
    }
}

#[cfg(test)]
mod safe_margin_tests {
    /// Item 31 app wiring: h0 is the default era — the plot's fuel model
    /// keeps headroom_t 0.0 unless the "safe margins" toggle asks for the
    /// 2 t band (2x the measured burn-model error). Absent means
    /// optimistic, exactly like absent-means-eager for min-fuel.
    #[test]
    fn safe_margins_toggle_sets_two_tonnes_and_absent_means_optimistic() {
        let mut m = ed_galaxy::fuel::FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
        assert_eq!(m.headroom_t, 0.0, "the h0 era default");
        super::apply_safe_margins(&mut m, false);
        assert_eq!(m.headroom_t, 0.0, "absent/false leaves optimistic");
        super::apply_safe_margins(&mut m, true);
        assert_eq!(m.headroom_t, 2.0, "safe mode plans with the 2 t band");
        super::apply_safe_margins(&mut m, false);
        assert_eq!(m.headroom_t, 0.0, "toggle off returns to optimistic");
    }
}

#[cfg(test)]
mod scoop_tests {

    /// A fresh data dir gets the bundled bubble index: valid, openable,
    /// and never written over an index that already exists.
    #[test]
    fn bundled_bubble_installs_into_an_empty_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        use super::install_bundled_bubble;
        use ed_galaxy::Galaxy;
        assert!(install_bundled_bubble(dir.path()), "an empty data dir gets the bundled index");
        let bubble = dir.path().join("galaxy_populated");
        assert!(Galaxy::exists(&bubble));
        let galaxy = Galaxy::open(&bubble).unwrap();
        assert!(galaxy.count > 100_000, "the bubble index knows the inhabited galaxy, got {}", galaxy.count);
        // Already present: nothing to do.
        assert!(!install_bundled_bubble(dir.path()));
        // A leftover downloaded full index (pre-B.4 installs kept one in
        // `galaxy`) is NOT an index any more: the bundle installs beside it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("galaxy")).unwrap();
        for name in ["stars.bin", "cells.bin", "names.bin", "byname.bin"] {
            std::fs::write(dir.path().join("galaxy").join(name), b"x").unwrap();
        }
        assert!(install_bundled_bubble(dir.path()));
        assert!(dir.path().join("galaxy_populated").exists());
    }

    use super::loadout_has_fuel_scoop;

    #[test]
    fn loadout_fsd_health_is_read_from_the_drive_slot() {
        let v: serde_json::Value = serde_json::json!({"Modules": [{"Slot": "PowerPlant", "Item": "int_powerplant_size5_class5", "Health": 0.5}, {"Slot": "FrameShiftDrive", "Item": "int_hyperdrive_size5_class5", "Health": 0.9123}]});
        assert_eq!(super::loadout_fsd_health(&v), Some(0.9123));
        assert_eq!(super::loadout_fsd_health(&serde_json::json!({"Modules": []})), None);
        let afmu: serde_json::Value = serde_json::json!({"Modules": [{"Item": "Int_Repairer_Size5_Class5"}]});
        assert!(super::loadout_has_afmu(&afmu));
        assert!(!super::loadout_has_afmu(&v));
    }

    #[test]
    fn loadout_scoop_is_read_from_the_modules() {
        let with: serde_json::Value = serde_json::json!({"Modules": [{"Item": "int_hyperdrive_size5_class5"}, {"Item": "Int_FuelScoop_Size6_Class5"}]});
        let without: serde_json::Value = serde_json::json!({"Modules": [{"Item": "int_hyperdrive_size5_class5"}, {"Item": "int_cargorack_size6_class1"}]});
        assert!(loadout_has_fuel_scoop(&with));
        assert!(!loadout_has_fuel_scoop(&without));
        assert!(!loadout_has_fuel_scoop(&serde_json::json!({})));
    }
}

#[cfg(test)]
mod scoop_rate_tests {
    /// Item 22 app wiring: the fuel model the planner receives carries
    /// the ship's real scoop rate, sourced from the same Loadout event
    /// everything else comes from.
    #[test]
    fn ship_fuel_for_carries_the_loadout_scoop_rate() {
        let store = ed_store::Store::open_in_memory(std::path::Path::new(".")).unwrap();
        let loadout = serde_json::json!({
            "ShipID": 7, "Ship": "mandalay",
            "UnladenMass": 319.2, "FuelCapacity": { "Main": 32.0 },
            "MaxJumpRange": 77.86,
            "Modules": [
                { "Item": "int_hyperdrive_overcharge_size5_class5" },
                { "Item": "int_fuelscoop_size5_class5" },
            ],
        });
        store
            .conn()
            .execute(
                "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 1, '2026-09-02T08:00:00Z', 'Loadout', ?1)",
                [loadout.to_string()],
            )
            .unwrap();
        // Readers go through the derived ships table, not the raw Loadout.
        ed_store::derive::derive_all(store.conn()).unwrap();
        let (model, _, _, _) = super::ship_fuel_for(store.conn(), None).expect("model from loadout");
        assert!((model.scoop_rate - 0.577).abs() < 1e-4, "5A scoop: {}", model.scoop_rate);
    }

    /// Item 45: with no live Status.json fuel (game closed, main menu),
    /// the plan starts from the journal's last recorded tank, never a
    /// silently assumed full one. A live Status reading still wins, and
    /// only a journal that has never seen the tank falls back to capacity.
    #[test]
    fn start_fuel_fails_closed_to_the_journals_last_reading() {
        let store = ed_store::Store::open_in_memory(std::path::Path::new(".")).unwrap();
        let loadout = serde_json::json!({
            "ShipID": 7, "Ship": "mandalay",
            "UnladenMass": 319.2, "FuelCapacity": { "Main": 32.0 },
            "MaxJumpRange": 77.86,
            "Modules": [{ "Item": "int_hyperdrive_overcharge_size5_class5" }],
        });
        store.conn().execute(
            "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 1, '2026-09-03T08:00:00Z', 'Loadout', ?1)",
            [loadout.to_string()],
        ).unwrap();
        // Readers go through the derived ships table, not the raw Loadout.
        ed_store::derive::derive_all(store.conn()).unwrap();
        let fuel_now = || super::ship_fuel_for(store.conn(), None).expect("model").2;
        assert!((fuel_now() - 32.0).abs() < 1e-4, "no reading anywhere: capacity is all we have");
        store.conn().execute(
            "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 2, '2026-09-03T09:00:00Z', 'FuelScoop', '{\"Scooped\":3.0,\"Total\":26.5}')",
            [],
        ).unwrap();
        store.conn().execute(
            "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 3, '2026-09-03T09:05:00Z', 'FSDJump', '{\"StarSystem\":\"Boeph UF-D d13-126\",\"FuelLevel\":19.7}')",
            [],
        ).unwrap();
        assert!((fuel_now() - 19.7).abs() < 1e-4, "latest journal reading, not capacity: {}", fuel_now());
        store.conn().execute(
            "INSERT INTO snapshots (name, ts, mtime, raw) VALUES ('Status.json', '2026-09-03T09:06:00Z', 0, '{\"Flags\":16777240,\"Fuel\":{\"FuelMain\":25.5}}')",
            [],
        ).unwrap();
        assert!((fuel_now() - 25.5).abs() < 1e-4, "a live Status reading in the ship wins: {}", fuel_now());
        // In the SRV the block is the buggy's (measured 2026-09-27: Flags with
        // InSRV, FuelMain 0.0, FuelReservoir 0.365): the ship's last journal
        // reading is the tank, not an empty one.
        store.conn().execute(
            "UPDATE snapshots SET raw = '{\"Flags\":73433934,\"Fuel\":{\"FuelMain\":0.0,\"FuelReservoir\":0.365339}}' WHERE name = 'Status.json'",
            [],
        ).unwrap();
        assert!((fuel_now() - 19.7).abs() < 1e-4, "the SRV's fuel is not the ship's: {}", fuel_now());
    }

    /// The ladder pinned against the game's table: A rates per size,
    /// each rating step down one seventh off.
    #[test]
    fn scoop_rates_match_the_games_table() {
        let rate = |size: u8, class: u8| {
            let loadout = serde_json::json!({ "Modules": [
                { "Slot": "Slot01_Size6", "Item": format!("int_fuelscoop_size{size}_class{class}") },
            ]});
            super::loadout_scoop_rate_t_per_s(&loadout).unwrap()
        };
        assert!((rate(5, 5) - 0.577).abs() < 1e-4, "5A");
        assert!((rate(8, 5) - 1.680).abs() < 1e-4, "8A");
        assert!((rate(1, 1) - 0.018).abs() < 1e-4, "1E");
        assert!((rate(6, 4) - 0.75257).abs() < 1e-4, "6B");
        assert!((rate(3, 2) - 0.10057).abs() < 1e-4, "3D");
        let no_scoop = serde_json::json!({ "Modules": [ { "Item": "int_cargorack_size6_class1" } ] });
        assert_eq!(super::loadout_scoop_rate_t_per_s(&no_scoop), None);
    }

    /// "Scoop rate enhanced" (G1-G5, +10-50%) engineers the rate; the
    /// Loadout modifier wins over the stock table, whichever unit the
    /// journal reports it in (the stock ranges of kg/s and t/s do not
    /// overlap, so the magnitude disambiguates).
    #[test]
    fn engineered_scoop_rate_comes_from_the_loadout_modifier() {
        let engineered = |value: f64| {
            serde_json::json!({ "Modules": [{
                "Item": "int_fuelscoop_size7_class5",
                "Engineering": { "BlueprintName": "FuelScoop_Enhanced",
                    "Modifiers": [
                        { "Label": "ScoopRate", "Value": value },
                        { "Label": "PowerDraw", "Value": 0.69 },
                    ] },
            }]})
        };
        // G5 7A: 1245 * 1.5, reported in kg/s...
        let rate = super::loadout_scoop_rate_t_per_s(&engineered(1867.5)).unwrap();
        assert!((rate - 1.8675).abs() < 1e-4, "kg/s modifier: {rate}");
        // ...or in t/s...
        let rate = super::loadout_scoop_rate_t_per_s(&engineered(1.8675)).unwrap();
        assert!((rate - 1.8675).abs() < 1e-4, "t/s modifier: {rate}");
        // On a 7A the value 1.5 is GENUINELY ambiguous — 1.5 t/s and
        // x1.5 = 1.8675 t/s both land in the engineering band — so the
        // absolute reading wins (Frontier's convention elsewhere), which
        // is also the conservative one: the ETA overstates scoop time
        // rather than understating it.
        let rate = super::loadout_scoop_rate_t_per_s(&engineered(1.5)).unwrap();
        assert!((rate - 1.5).abs() < 1e-4, "ambiguous prefers absolute: {rate}");
        // A bare multiplier decodes where it is unambiguous: on a 1E
        // (stock 0.018 t/s), 1.4 fits no absolute reading and only works
        // as x1.4.
        let small = serde_json::json!({ "Modules": [{
            "Item": "int_fuelscoop_size1_class1",
            "Engineering": { "Modifiers": [ { "Label": "ScoopRate", "Value": 1.4 } ] },
        }]});
        let rate = super::loadout_scoop_rate_t_per_s(&small).unwrap();
        assert!((rate - 0.018 * 1.4).abs() < 1e-5, "multiplier on a small scoop: {rate}");
        // A nonsense value never beats the stock table.
        let rate = super::loadout_scoop_rate_t_per_s(&engineered(400_000.0)).unwrap();
        assert!((rate - 1.245).abs() < 1e-4, "nonsense falls back to stock: {rate}");
        // An engineered scoop with no rate modifier keeps the table value.
        let other_mod = serde_json::json!({ "Modules": [{
            "Item": "int_fuelscoop_size7_class5",
            "Engineering": { "Modifiers": [ { "Label": "Mass", "Value": 2.5 } ] },
        }]});
        let rate = super::loadout_scoop_rate_t_per_s(&other_mod).unwrap();
        assert!((rate - 1.245).abs() < 1e-4, "stock fallback: {rate}");
    }
}

#[cfg(test)]
mod stop_tests {
    /// Stop pressed before the server answers: the plot returns "Plot
    /// stopped." at once, no 130 s wait, no bubble-index fallback. The
    /// API address is a port nothing listens on, so without the token the
    /// request would sit in connect until its timeout.
    #[test]
    fn a_stopped_api_plot_returns_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        // Under test the endpoint comes from EDDA_API_URL alone (exchange.rs):
        // a port nothing listens on, so a request would fail or hang, never answer.
        std::env::set_var("EDDA_API_URL", "http://127.0.0.1:9");
        let stopped = tauri::async_runtime::block_on(async {
            let cancel = state.jobs.begin(crate::jobs::ROUTE_PLOT);
            cancel.cancel();
            let query: super::PlotQuery = serde_json::from_value(serde_json::json!({ "from": "Sol", "to": "Colonia", "range_ly": 30.0, "supercharge": true, "fuel": false })).unwrap();
            let started = std::time::Instant::now();
            let result = super::plot_via_api(&state, &query, &cancel).await;
            let text = result.as_ref().map(|_| "a route".to_string()).unwrap_or_else(|e| e.text().to_string());
            (matches!(result, Err(super::PlotFailure::Stopped)), started.elapsed(), text)
        });
        std::env::remove_var("EDDA_API_URL");
        let (stopped, elapsed, text) = (stopped.0, stopped.1, stopped.2);
        assert!(stopped, "a cancelled token stops the plot before the request is waited on; got: {text}");
        assert!(elapsed < std::time::Duration::from_secs(5), "returned at once, not after the request timeout");
    }
}

#[cfg(test)]
mod dock_memo_tests {
    use super::{DockKey, DockMemo};
    use ed_store::lookup::PadSize;
    use std::collections::HashSet;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn key(plotted: &str, systems: &[&str], pad: PadSize) -> DockKey {
        DockKey { plotted: Some(plotted.into()), systems: systems.iter().map(|s| s.to_string()).collect(), pad }
    }

    #[test]
    fn the_memo_answers_the_same_route_and_misses_when_anything_moves() {
        let mut memo = DockMemo::default();
        let k = key("2026-10-07T18:00:00Z", &["Sol", "Alioth"], PadSize::Large);
        assert!(memo.get(&k).is_none(), "empty memo misses");
        memo.put(k.clone(), HashSet::from(["alioth".to_string()]));
        assert_eq!(memo.get(&k), Some(HashSet::from(["alioth".to_string()])));
        assert!(memo.get(&key("2026-10-07T18:00:00Z", &["Sol", "Alioth"], PadSize::Medium)).is_none(), "a smaller ship asks again");
        assert!(memo.get(&key("2026-10-07T18:05:00Z", &["Sol", "Alioth"], PadSize::Large)).is_none(), "a re-plot asks again");
        assert!(memo.get(&key("2026-10-07T18:00:00Z", &["Sol", "Alioth", "Colonia"], PadSize::Large)).is_none(), "a longer route asks again");
    }

    /// A loopback `/v1/stations` that answers `[]` and counts requests.
    fn stub_api(answers: usize) -> (String, Arc<AtomicUsize>) {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        std::thread::spawn(move || {
            for _ in 0..answers {
                let Ok((mut sock, _)) = listener.accept() else { return };
                let mut buf = [0u8; 4096];
                let mut got = Vec::new();
                while let Ok(n) = sock.read(&mut buf) {
                    if n == 0 { break }
                    got.extend_from_slice(&buf[..n]);
                    if got.windows(4).any(|w| w == b"\r\n\r\n") { break }
                }
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]");
            }
        });
        (url, hits)
    }

    /// The bug as measured on the box (2026-10-07, 18:17-18:36Z): one
    /// pilot's HUD and Ship panel re-read the route every journal tick and
    /// the server saw 432 identical `systems=` calls. Two reads of one
    /// route must cost one call; a re-plot costs one more; a server that
    /// did not answer is asked again next time.
    #[test]
    fn two_route_reads_on_one_route_make_one_stations_call() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let (api, hits) = stub_api(8);
        let systems: Vec<String> = ["Sol", "Alioth", "Colonia"].iter().map(|s| s.to_string()).collect();
        tauri::async_runtime::block_on(async {
            let first = super::game_route_docks(&state, Some(&api), Some("t1"), &systems, PadSize::Large).await;
            let second = super::game_route_docks(&state, Some(&api), Some("t1"), &systems, PadSize::Large).await;
            assert_eq!(first, second);
            assert_eq!(hits.load(Ordering::SeqCst), 1, "the second read of the same route is answered from the memo");

            super::game_route_docks(&state, Some(&api), Some("t2"), &systems, PadSize::Large).await;
            assert_eq!(hits.load(Ordering::SeqCst), 2, "a re-plot asks once more");
            super::game_route_docks(&state, Some(&api), Some("t2"), &systems, PadSize::Large).await;
            assert_eq!(hits.load(Ordering::SeqCst), 2);

            // A server that does not answer (a port nothing listens on) is
            // not remembered as "no docks": the next read asks again.
            let down = "http://127.0.0.1:9";
            let none = super::game_route_docks(&state, Some(down), Some("t3"), &systems, PadSize::Large).await;
            assert!(none.is_empty());
            super::game_route_docks(&state, Some(&api), Some("t3"), &systems, PadSize::Large).await;
            assert_eq!(hits.load(Ordering::SeqCst), 3, "the unanswered route was asked again, not served from the memo");

            // No endpoint at all: nothing asked, nothing remembered.
            super::game_route_docks(&state, None, Some("t4"), &systems, PadSize::Large).await;
            assert_eq!(hits.load(Ordering::SeqCst), 3);
        });
    }
}


#[cfg(test)]
mod injection_wire_tests {
        use super::{bubble_text, injection_wire, island_text, HYPOTHETICAL_INJECTIONS};

        #[test]
        fn the_wire_names_the_best_grade_the_commander_can_make() {
            let v = injection_wire(Some((1.5, "standard", 3)), true).unwrap();
            assert_eq!(v, serde_json::json!({ "grade": "standard", "max": 3 }));
        }

        #[test]
        fn no_materials_still_asks_so_the_answer_can_name_the_bill() {
            let v = injection_wire(None, true).unwrap();
            assert_eq!(v, serde_json::json!({ "grade": "premium", "max": HYPOTHETICAL_INJECTIONS }));
            let v = injection_wire(Some((2.0, "premium", 0)), true).unwrap();
            assert_eq!(v["grade"], "premium");
            assert_eq!(v["max"], HYPOTHETICAL_INJECTIONS);
        }

        #[test]
        fn injections_off_sends_nothing() {
            assert_eq!(injection_wire(Some((2.0, "premium", 4)), false), None);
        }

        #[test]
        fn an_island_is_said_with_its_numbers() {
            let detail = r#"{"error":"no_route","why":"island","end":"destination","system":"Byoi Fraae CQ-G d10-0","reach_ly":72.2,"nearest_ly":74.1,"light_reach_ly":77.5,"injected_reach_ly":null}"#;
            let text = island_text(detail, "Jongou XM-W d1-0", "Byoi Fraae CQ-G d10-0");
            assert_eq!(
                text,
                "no known star within 72.2 ly of Byoi Fraae CQ-G d10-0 (the destination): the nearest is 74.1 ly away; with one jump's fuel aboard the ship reaches 77.5 ly, which would cross it; an FSD injection might cross it, but you can synthesise none"
            );
        }

        #[test]
        fn a_short_injection_is_reported_and_a_lighter_tank_only_when_it_helps() {
            let detail = r#"{"error":"no_route","why":"island","end":"origin","system":"Lone","reach_ly":50.0,"nearest_ly":70.0,"light_reach_ly":50.0,"injected_reach_ly":62.5}"#;
            let text = island_text(detail, "Lone", "Home");
            assert_eq!(text, "no known star within 50.0 ly of Lone (the origin): the nearest is 70.0 ly away; an FSD injection reaches 62.5 ly, still short");
        }

        #[test]
        fn a_lost_star_has_no_nearest() {
            let detail = r#"{"error":"no_route","why":"island","end":"origin","system":"Rock","reach_ly":50.0,"nearest_ly":null,"light_reach_ly":50.0,"injected_reach_ly":100.0}"#;
            assert_eq!(island_text(detail, "Rock", "Home"), "no known star anywhere near Rock (the origin) at 50.0 ly");
        }

        #[test]
        fn the_server_answer_flags_are_read_beside_the_route() {
            use super::ServerAnswer;
            let early = serde_json::json!({ "jumps": 478, "refining": true, "budget_ms": 100000, "lane": "long" });
            assert_eq!(ServerAnswer::of(&early), ServerAnswer { refining: true, budget_ms: 100_000 });
            let plain = serde_json::json!({ "jumps": 5 });
            assert_eq!(ServerAnswer::of(&plain), ServerAnswer::default(), "an answer without the flags is a finished one");
        }

        #[test]
        fn a_budget_504_is_said_as_the_server_giving_up_not_still_working() {
            use super::{budget_failure, PlotFailure};
            match budget_failure(r#"{"error":"budget","budget_ms":100000,"lane":"long"}"#) {
                PlotFailure::Refused(text) => assert_eq!(text, "the route server ran out of its 100 s budget without finding a route; a shorter leg, a different target, or the galaxy map's own plotter may get there"),
                other => panic!("a refusal, got {other:?}"),
            }
            assert!(matches!(budget_failure(""), PlotFailure::Transport(_)), "a bare gateway 504: the planner may still be working");
            assert!(matches!(budget_failure("<html>Gateway Timeout</html>"), PlotFailure::Transport(_)));
        }

        #[test]
        fn the_bubble_index_only_follows_a_server_that_did_not_answer() {
            use super::{bubble_worth_trying, PlotFailure};
            assert!(bubble_worth_trying(&PlotFailure::Transport("route server unreachable".into())));
            assert!(!bubble_worth_trying(&PlotFailure::Refused("no known star within 37.1 ly".into())), "a refusal with its numbers is the answer");
            assert!(!bubble_worth_trying(&PlotFailure::Stopped));
        }

        #[test]
        fn the_bubble_index_says_outside_not_unknown() {
            assert_eq!(bubble_text("unknown system \"Jongou XM-W d1-0\""), "\"Jongou XM-W d1-0\" is outside the bundled bubble index");
            assert_eq!(bubble_text("no route"), "no route");
        }
}
