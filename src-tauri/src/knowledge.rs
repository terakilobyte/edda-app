//! Filling in star types the galaxy dump lacks, where routes actually go.
//!
//! The Spansh dump has no main-star type for most systems (never scanned,
//! or a partial record); EDSM often knows. After a plot, a background sweep
//! asks EDSM for the systems within 100 ly of each hop (`sphere-systems`,
//! one call per 100-ly cell, at most one call a second) and learns the
//! primary star of every system our index has as unknown. Learned neutrons
//! make the neutron sub-index stale; it is rebuilt on the next plot.
use crate::state::AppState;
use crate::exchange::SendApiBlocking;
use ed_galaxy::StarClassCode as _;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter, Manager};

const CELL_LY: f32 = 100.0;
const RADIUS_LY: f32 = 100.0;
/// A cell is not asked about again within this many days.
const REVISIT_DAYS: i64 = 30;
/// Consecutive EDSM failures after which a sweep gives up (EDSM is down,
/// not merely slow for one cell).
const MAX_FAILURES_IN_A_ROW: u32 = 3;

static RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct KnowledgeStatus {
    pub running: bool,
    pub cells_swept: u64,
    pub stars_learned: u64,
    pub neutrons_learned: u64,
    pub last_sweep: Option<String>,
}

fn cell_key(p: [f32; 3]) -> String {
    format!("{}:{}:{}", (p[0] / CELL_LY).floor() as i64, (p[1] / CELL_LY).floor() as i64, (p[2] / CELL_LY).floor() as i64)
}

fn unknown_stars_from_edsm(
    galaxy: &ed_galaxy::Galaxy,
    systems: &[serde_json::Value],
) -> Vec<(u64, String, String, ed_galaxy::StarClass)> {
    let mut rows = Vec::new();
    for system in systems {
        let (Some(name), Some(id64), Some(primary)) = (
            system["name"].as_str(),
            system["id64"].as_u64(),
            system.get("primaryStar"),
        ) else { continue };
        let Some(subtype) = primary["type"].as_str() else { continue };
        let class = ed_galaxy::StarClass::from_subtype(subtype);
        if class == ed_galaxy::StarClass::Unknown { continue; }
        let ours = galaxy.find(name).map(|i| galaxy.class(&galaxy.record(i)));
        if ours == Some(ed_galaxy::StarClass::Unknown) {
            rows.push((id64, name.to_string(), subtype.to_string(), class));
        }
    }
    rows
}

/// Sweep the systems around a route's hops. Returns at once; the work runs
/// on its own thread and reports as `knowledge-progress` events.
pub fn sweep_route(app: AppHandle, hops: Vec<(String, [f32; 3])>) {
    if hops.is_empty() || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sweep(&app, hops)));
        RUNNING.store(false, Ordering::SeqCst);
        if result.is_err() {
            tracing::warn!("knowledge sweep panicked");
        }
    });
}

fn sweep(app: &AppHandle, hops: Vec<(String, [f32; 3])>) {
    let state = app.state::<AppState>();
    let Some(galaxy) = state.routing.galaxy(&state.data_dir) else { return };
    // One cell per 100 ly of route, not yet swept recently.
    let mut cells: Vec<(String, [f32; 3])> = Vec::new();
    let mut skipped_known = 0usize;
    for (_, p) in &hops {
        let k = cell_key(*p);
        if cells.iter().any(|(c, _)| *c == k) {
            continue;
        }
        // Nothing to learn here (the bubble, Colonia): no call. Cheap: a
        // local sphere scan of the index counts its unknown-class stars.
        let mut unknown = 0u32;
        galaxy.for_each_within(*p, RADIUS_LY, |i, _| {
            if galaxy.class_code(i) == ed_galaxy::StarClass::Unknown.code() {
                unknown += 1;
            }
        });
        if unknown < 25 {
            skipped_known += 1;
            continue;
        }
        let recent: bool = state.with_read(|s| {
            s.conn()
                .query_row("SELECT 1 FROM edsm_sweeps WHERE cell = ?1 AND fetched_at > strftime('%Y-%m-%dT%H:%M:%SZ','now', ?2)", rusqlite::params![k, format!("-{REVISIT_DAYS} days")], |_| Ok(()))
                .is_ok()
        });
        if !recent {
            cells.push((k, *p));
        }
    }
    if cells.is_empty() {
        tracing::info!(skipped_known, "knowledge sweep: nothing to learn along this route");
        return;
    }
    tracing::info!(cells = cells.len(), skipped_known, hops = hops.len(), "knowledge sweep: asking about the route's surroundings");
    let client = app.state::<AppState>().http_blocking.clone();
    // The community server proxies EDSM (ledger 2026-09-06): a stale
    // cell costs ONE upstream fetch for the whole fleet, and a cell
    // anyone swept lately is answered from the server's own knowledge
    // instantly. EDSM direct stays as the fallback so a self-hosted or
    // offline-server install keeps learning.
    let proxy_base = crate::exchange::endpoint(&state);
    let (mut swept, mut learned_total, mut neutrons_total) = (0u64, 0u64, 0u64);
    // A sphere answer can take a while (the proxy's worst case is one
    // EDSM fetch; direct EDSM is ~10 s on a good day); one slow cell
    // must not cost the other hundred. A cell that fails is skipped
    // (and asked again on the next plot, since it is not recorded as
    // swept); the sweep only gives up after several failures in a row,
    // which means the upstream itself is down.
    let mut failures_in_a_row = 0u32;
    let mut last_via_proxy = proxy_base.is_some();
    for (i, (key, p)) in cells.iter().enumerate() {
        if i > 0 {
            // Toward our own server a short breath; toward EDSM the
            // classic 1.2 s politeness (the server paces its own EDSM
            // side regardless).
            let pace = if last_via_proxy { 300 } else { 1200 };
            std::thread::sleep(std::time::Duration::from_millis(pace));
        }
        let body = match fetch_sphere(&client, proxy_base.as_deref(), p) {
            Ok((b, via_proxy)) => {
                failures_in_a_row = 0;
                last_via_proxy = via_proxy;
                b
            }
            Err(e) => {
                failures_in_a_row += 1;
                last_via_proxy = false;
                if failures_in_a_row >= MAX_FAILURES_IN_A_ROW {
                    tracing::warn!(error = %e, failures = failures_in_a_row, "knowledge sweep: upstream keeps failing; stopping this sweep");
                    break;
                }
                tracing::warn!(error = %e, cell = %key, "knowledge sweep: request failed; skipping this cell");
                continue;
            }
        };
        let Ok(systems) = serde_json::from_str::<Vec<serde_json::Value>>(&body) else { continue };
        let mut learned = 0u64;
        let mut neutrons = 0u64;
        // Only what our index has as unknown: first-hand data stays first.
        let rows = unknown_stars_from_edsm(&galaxy, &systems);
        let _ = state.with_store(|st| {
            for (id64, name, t, class) in &rows {
                crate::spansh::learn_star_conn(&state, st.conn(), *id64, name, t, "edsm");
                learned += 1;
                if *class == ed_galaxy::StarClass::Neutron {
                    neutrons += 1;
                }
            }
            let _ = st.conn().execute(
                "INSERT OR REPLACE INTO edsm_sweeps (cell, fetched_at, systems, learned) VALUES (?1, strftime('%Y-%m-%dT%H:%M:%SZ','now'), ?2, ?3)",
                rusqlite::params![key, systems.len() as i64, learned as i64],
            );
            Ok::<(), String>(())
        });
        swept += 1;
        learned_total += learned;
        neutrons_total += neutrons;
        let _ = app.emit(crate::events::KNOWLEDGE_PROGRESS, serde_json::json!({ "cells": cells.len(), "swept": swept, "learned": learned_total, "neutrons": neutrons_total }));
    }
    tracing::info!(swept, learned = learned_total, neutrons = neutrons_total, "knowledge sweep finished");
    if neutrons_total > 0 {
        // The highway graph is built from the index's neutrons: it is stale
        // now. Rebuilt in the background; plots keep the current one until
        // the new one is ready.
        state.routing.rebuild_neutrons_in_background(&state.jobs, &state.data_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edsm_neutron_fills_unknown_routing_class_without_rewriting_index() {
        let dir = tempfile::tempdir().unwrap();
        let dump = br#"[
{"id64":42,"name":"Missing Neutron","coords":{"x":100,"y":0,"z":0},"bodies":[]},
{"id64":43,"name":"Known Main Star","coords":{"x":120,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]}
]"#;
        ed_galaxy::import::import_reader(Box::new(std::io::Cursor::new(dump)), dir.path(), &mut |_| {}).unwrap();
        let galaxy = ed_galaxy::Galaxy::open(dir.path()).unwrap();
        let response = serde_json::json!([
            {"id64":42,"name":"Missing Neutron","primaryStar":{"type":"Neutron Star"}},
            {"id64":43,"name":"Known Main Star","primaryStar":{"type":"Black Hole"}}
        ]);
        let rows = unknown_stars_from_edsm(&galaxy, response.as_array().unwrap());
        assert_eq!(rows.len(), 1, "EDSM must not replace a class already present in the dump");
        let (id64, _, _, class) = &rows[0];
        galaxy.learn_class(*id64, *class);

        let idx = galaxy.find("Missing Neutron").unwrap();
        let record = galaxy.record(idx);
        assert_eq!(galaxy.class_code(idx), ed_galaxy::StarClass::Unknown.code(), "base mmap stays immutable");
        assert_eq!(galaxy.class(&record), ed_galaxy::StarClass::Neutron);
        assert_eq!(galaxy.class(&record).boost(), 4.0, "routing sees the learned neutron boost");
        assert!(!galaxy.scoopable(idx), "a neutron is not itself fuel-scoopable");

        let neutron_dir = tempfile::tempdir().unwrap();
        ed_galaxy::import::subset(&galaxy, neutron_dir.path(), |r| {
            galaxy.class(r) == ed_galaxy::StarClass::Neutron
        }).unwrap();
        let neutrons = ed_galaxy::Galaxy::open(neutron_dir.path()).unwrap();
        assert!(neutrons.find("Missing Neutron").is_some(), "rebuilt routing accelerator includes the EDSM neutron");
        assert!(neutrons.find("Known Main Star").is_none());
    }
}

/// One cell's sphere: the community proxy when configured (the
/// fleet-wide EDSM dedup lives server-side); never EDSM direct.
/// Returns the body and whether the proxy answered — the caller paces
/// the next call accordingly.
fn fetch_sphere(
    client: &reqwest::blocking::Client,
    proxy: Option<&str>,
    p: &[f32; 3],
) -> Result<(String, bool), String> {
    // Through OUR server only (maintainer, 2026-09-06): the commander's
    // position never goes to a third party from this machine. No API,
    // or a proxy that fails, is "unavailable" — the caller paces and
    // skips the cell; it never asks EDSM directly.
    let Some(base) = proxy else {
        return Err("no community API configured for the knowledge sweep".into());
    };
    let url = format!(
        "{base}/v1/knowledge/sphere?x={:.2}&y={:.2}&z={:.2}&radius={RADIUS_LY}",
        p[0], p[1], p[2]
    );
    // Worst case the server is doing the EDSM fetch for everyone
    // (30 s budget over there); give it room.
    client
        .get(&url)
        .timeout(std::time::Duration::from_secs(45))
        .send_api()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.text())
        .map(|b| (b, true))
        .map_err(|e| e.to_string())
}

static COMPANIONS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();

/// At a neutron fuel stop: which star to scoop at, and how far -- from
/// EDSM's body list, once per system, spoken a moment after arrival.
pub fn announce_companion(app: AppHandle, system: String) {
    std::thread::spawn(move || {
        let cache = COMPANIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
        let line = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&system).cloned();
        let line = match line {
            Some(l) => l,
            None => {
                let state = app.state::<AppState>();
                let client = state.http_blocking.clone();
                let enc = system.replace(' ', "%20").replace('+', "%2B");
                // Community proxy first (cached server-side for the whole
                // fleet; its worst case is one EDSM fetch), EDSM direct
                // as the fallback.
                let fetch = |url: String, secs: u64| {
                    client
                        .get(&url)
                        .timeout(std::time::Duration::from_secs(secs))
                        .send_api()
                        .and_then(|r| r.error_for_status())
                        .and_then(|r| r.text())
                        .ok()
                };
                // Through OUR server only (maintainer, 2026-09-06): no direct EDSM.
                let body = crate::exchange::endpoint(&state)
                    .and_then(|base| fetch(format!("{base}/v1/knowledge/bodies?systemName={enc}"), 40));
                let Some(body) = body else { return };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { return };
                let mut best: Option<(f64, String)> = None;
                for b in v["bodies"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                    if b["type"].as_str() != Some("Star") || b["isScoopable"].as_bool() != Some(true) {
                        continue;
                    }
                    let d = b["distanceToArrival"].as_f64().unwrap_or(f64::INFINITY);
                    let name = b["name"].as_str().unwrap_or("").to_string();
                    if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                        best = Some((d, name));
                    }
                }
                let Some((d, name)) = best else { return };
                let short = name.strip_prefix(&system).map(|s| s.trim()).filter(|s| !s.is_empty()).map(|s| format!("star {s}")).unwrap_or(name.clone());
                let l = format!("Scoop at {short}, {:.0} light seconds.", d);
                cache.lock().unwrap_or_else(|e| e.into_inner()).insert(system.clone(), l.clone());
                l
            }
        };
        let state = app.state::<AppState>();
        let c = crate::callouts::Callout::new("arrival", "", 1, true, line);
        crate::watcher::deliver(&app, vec![(c, None)]);
        let _ = state;
    });
}

/// Totals for the Settings page.
#[tauri::command]
pub async fn knowledge_status(state: tauri::State<'_, AppState>) -> Result<KnowledgeStatus, String> {
    Ok(state.with_read(|s| {
        let cells: i64 = s.conn().query_row("SELECT count(*) FROM edsm_sweeps", [], |r| r.get(0)).unwrap_or(0);
        let last: Option<String> = s.conn().query_row("SELECT max(fetched_at) FROM edsm_sweeps", [], |r| r.get(0)).ok().flatten();
        let (stars, neutrons) = ed_store::stars::counts_for_source(s.conn(), "edsm").unwrap_or((0, 0));
        KnowledgeStatus { running: RUNNING.load(Ordering::Relaxed), cells_swept: cells as u64, stars_learned: stars as u64, neutrons_learned: neutrons as u64, last_sweep: last }
    }))
}
