//! Following a route in the game.
//!
//! The app keeps one **active route** -- plotted here or imported from
//! Spansh -- with a cursor. Every `FSDJump` the watcher sees is matched
//! against the hops ahead of the cursor (any of them, so a detour or a
//! skipped hop is fine) and the cursor moves on, with a callout: "96 jumps
//! left. Next: Skaudai MS-K d8-55, neutron; scoop at the one after."
//!
//! **Target next** puts the next system into the game: a key macro built
//! from the commander's own `Custom.binds` opens the galaxy map, types the
//! name into the search box and confirms. The galaxy map's keyboard
//! behaviour varies by version and settings, so the steps are editable in
//! Settings, and the name is also placed on the clipboard so a paste works
//! when the macro does not. Nothing is sent unless the game window has
//! focus.

use crate::state::AppState;
use ed_galaxy::router::{Hop, Route};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

static MAP_SETUP_TESTING: AtomicBool = AtomicBool::new(false);
const MAP_SETUP_SYSTEM: &str = "Diso";

/// The system the map tests search for: the nearest populated neighbor
/// of wherever the commander is, so the test plot is one short jump. A
/// Colonia resident cannot plot 22,000 ly to the bubble's Diso (maintainer,
/// 2026-09-03); Diso remains the fallback when position is unknown.
/// The neighbors come from the bundled bubble index (populated systems
/// with positions, no network), the position from the journal.
fn map_setup_system(state: &AppState) -> String {
    let nearest = crate::capabilities::commander::current_system_name(state).and_then(|here| {
        let pos = state.read_conn().ok().and_then(|conn| crate::routing::journal_coords(&conn, &here))?;
        let g = state.routing.galaxy(&state.data_dir)?;
        let mut near = g.within(pos, 30.0);
        near.sort_by(|a, b| a.1.total_cmp(&b.1));
        let names: Vec<String> = near
            .into_iter()
            .map(|(i, _)| g.name(&g.record(i)).to_string())
            .filter(|name| !name.eq_ignore_ascii_case(&here))
            .collect();
        // A properly-named neighbor beats a nearer procedural one: Elite's
        // map search is fuzzy, and "Crucis Sector WU-P b5-2" returns a
        // list of near-identical siblings whose FIRST row is often the
        // wrong one — the recipe then targets a sibling and the test
        // fails on the name (field case 2026-09-05, three systems in a
        // row). Proper names return themselves first.
        names
            .iter()
            .find(|name| !is_procedural_name(name))
            .or_else(|| names.first())
            .cloned()
    });
    nearest.unwrap_or_else(|| MAP_SETUP_SYSTEM.into())
}

/// "Crucis Sector WU-P b5-2"-shaped: a AA-A mass-code pair anywhere in
/// the name marks StellarForge procedural naming.
fn is_procedural_name(name: &str) -> bool {
    let tokens: Vec<&str> = name.split_whitespace().collect();
    tokens.windows(2).any(|w| {
        let code = w[0].as_bytes();
        let mass = w[1].as_bytes();
        code.len() == 4
            && code[2] == b'-'
            && code[0].is_ascii_uppercase()
            && code[1].is_ascii_uppercase()
            && code[3].is_ascii_uppercase()
            && mass.len() >= 2
            && mass[0].is_ascii_lowercase()
            && mass[1..].iter().all(|b| b.is_ascii_digit() || *b == b'-')
    })
}

/// The frontend asks which system the teach/test flow should use, so its
/// spoken prompts and captions name the same nearby system the macros do.
/// The name also lands on the clipboard (maintainer, 2026-09-03: nobody should
/// have to type it — paste and go); clipboard failure is not worth
/// failing the flow over.
#[tauri::command]
pub async fn map_setup_target(state: State<'_, AppState>) -> Result<String, String> {
    let name = map_setup_system(&state);
    if let Err(error) = set_clipboard(&name) {
        tracing::warn!(%error, "map setup: clipboard not set; the commander types the name");
    }
    Ok(name)
}

pub fn map_setup_testing() -> bool { MAP_SETUP_TESTING.load(Ordering::SeqCst) }

struct MapSetupGuard;
impl Drop for MapSetupGuard {
    fn drop(&mut self) { MAP_SETUP_TESTING.store(false, Ordering::SeqCst); }
}

fn map_setup_prompt(app: &AppHandle, state: &AppState, text: &str) {
    let _ = app.emit(crate::events::CALLOUT,
        serde_json::json!({
            "kind": "setup",
            "text": text,
            "priority": 1,
            "speak": false,
            "ts": chrono::Utc::now().to_rfc3339(),
        }),
    );
    state.voice.say_wait(text);
}

#[tauri::command]
pub async fn map_setup_say(
    app: AppHandle,
    state: State<'_, AppState>,
    text: String,
) -> Result<(), String> {
    map_setup_prompt(&app, &state, &text);
    Ok(())
}

/// One step of the targeting macro.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum MacroStep {
    /// Press the chord bound to this action in Custom.binds (e.g. `GalaxyMapOpen`).
    Bind { action: String },
    /// Ctrl+V: paste the system name put on the clipboard by `Clipboard`.
    Paste,
    /// Press a key by its binds name (e.g. `Key_Enter`, `Key_Escape`).
    Key { key: String },
    /// Hold the chord bound to an action for a while (camera zoom etc.).
    Hold { action: String, ms: u64 },
    /// Type text; `{system}` is replaced by the next system's name.
    Type { text: String },
    /// Pause.
    Wait { ms: u64 },
    /// Pause for the map to pan to the selected system: `base_ms` plus
    /// `per_ly` milliseconds for every light-year of the next jump.
    WaitPan { base_ms: u64, per_ly: f32 },
    /// Put the next system's name on the clipboard.
    Clipboard,
    /// Park the mouse at a fraction (0..1) of the game window: the galaxy
    /// map focuses whatever the cursor is over, so UI keys need a known spot.
    Mouse { x: f32, y: f32 },
    /// Left-click at a fraction (0..1) of the game window -- the map's own
    /// buttons, learned by recording.
    Click { x: f32, y: f32 },
    /// Wait until the game's `Status.json` reports this GUI focus
    /// (6 = galaxy map, 7 = system map, 0 = none); abort on timeout. The
    /// guard that stops keystrokes landing in the cockpit.
    WaitGui { focus: u8, timeout_ms: u64 },
}

/// `Status.json` GuiFocus values.
pub const GUI_GALAXY_MAP: u8 = 6;

/// Default: the commander's own recipe for the current game build. Once the
/// map reports open and has settled, UI_Up then UI_Select puts focus in the
/// search box, the name is typed, a raw Down-arrow (the one key a focused
/// text field honours) moves to the results, and UI_Select selects the
/// system -- targeting it. A second UI_Select would plot the route; add
/// `{"do":"wait","ms":1000},{"do":"bind","action":"UI_Select"}` for that.
/// The commander's verified recipe (2026-08-26, in space, 4.3): open the
/// map, wait for it, a held zoom as the settle, park the mouse, UI_Up +
/// UI_Select into search, type, Down + Select picks the result, a pause and
/// a camera nudge so the map re-focuses on it, Select, Right + Down x8
/// walks the info panel to the targeting control, Select, then the map key
/// again to close.
/// The four places on the galaxy map the targeting recipes click, as
/// fractions of the game window: taught by clicking each once.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MapPoints {
    pub search: Option<(f32, f32)>,
    pub result: Option<(f32, f32)>,
    pub target: Option<(f32, f32)>,
    #[serde(default)]
    pub plot: Option<(f32, f32)>,
    /// Waits, in ms, tuned per machine: after the map opens before the
    /// search box is clicked (default 500); for the search to list results
    /// (1,800); after the result is clicked before the target button (1,200).
    #[serde(default)]
    pub open_delay_ms: Option<u64>,
    #[serde(default)]
    pub search_delay_ms: Option<u64>,
    #[serde(default)]
    pub target_delay_ms: Option<u64>,
}

/// The recipe from the three points used to target one hop: open the map, click the search
/// box, type the name, click the first result, click the target button,
/// close the map.
pub fn point_macro(p: &MapPoints) -> Option<Vec<MacroStep>> {
    let (s, r, t) = (p.search?, p.result?, p.target?);
    Some(vec![
        MacroStep::Clipboard,
        MacroStep::Bind {
            action: "GalaxyMapOpen".into(),
        },
        MacroStep::WaitGui {
            focus: GUI_GALAXY_MAP,
            timeout_ms: 10000,
        },
        MacroStep::Wait {
            ms: p.open_delay_ms.unwrap_or(500).clamp(0, 10000),
        },
        MacroStep::Click { x: s.0, y: s.1 },
        MacroStep::Wait { ms: 250 },
        MacroStep::Paste,
        MacroStep::Wait {
            ms: p.search_delay_ms.unwrap_or(1800).clamp(200, 10000),
        },
        MacroStep::Click { x: r.0, y: r.1 },
        // The map pans to the system and the info panel settles; then the
        // cursor sweeps in from the left so the button sees a hover first.
        MacroStep::Wait {
            ms: p.target_delay_ms.unwrap_or(1200).clamp(0, 10000),
        },
        MacroStep::Mouse {
            x: t.0 - 0.08,
            y: t.1,
        },
        MacroStep::Wait { ms: 60 },
        MacroStep::Mouse {
            x: t.0 - 0.05,
            y: t.1,
        },
        MacroStep::Wait { ms: 60 },
        MacroStep::Mouse {
            x: t.0 - 0.025,
            y: t.1,
        },
        MacroStep::Wait { ms: 60 },
        MacroStep::Mouse {
            x: t.0 - 0.01,
            y: t.1,
        },
        MacroStep::Wait { ms: 120 },
        MacroStep::Click { x: t.0, y: t.1 },
        // Let the game take the target before the map goes away.
        MacroStep::Wait { ms: 200 },
        MacroStep::Bind {
            action: "GalaxyMapOpen".into(),
        },
        MacroStep::WaitGui {
            focus: 0,
            timeout_ms: 4000,
        },
    ])
}

/// The taught search flow ending at Elite's Plot Route button rather than
/// the single-system Target button.
pub fn point_plot_macro(p: &MapPoints) -> Option<Vec<MacroStep>> {
    let plot = p.plot?;
    let mut steps = point_macro(p)?;
    let click = steps.iter_mut().rfind(|s| matches!(s, MacroStep::Click { .. }))?;
    *click = MacroStep::Click { x: plot.0, y: plot.1 };
    Some(steps)
}

/// Wait for the next click in the game window and remember it as one of
/// the four map points ("search", "result", "target", "plot"). The first
/// three build the single-hop target recipe; all four enable route plotting.
#[tauri::command]
pub async fn map_point_capture(
    state: State<'_, AppState>,
    which: String,
) -> Result<MapPoints, String> {
    if !matches!(which.as_str(), "search" | "result" | "target" | "plot") {
        return Err(format!("unknown map point {which:?}"));
    }
    // Plotting the route is part of the teaching flow. Mute the normal
    // NavRoute announcement before the commander clicks it, and keep it
    // muted across the short frontend handoff into `map_setup_test`.
    let plot_capture = which == "plot";
    if plot_capture {
        MAP_SETUP_TESTING.store(true, Ordering::SeqCst);
    }
    if let Err(error) = ed_input::record::start() {
        if plot_capture {
            MAP_SETUP_TESTING.store(false, Ordering::SeqCst);
        }
        return Err(error);
    }
    let click = tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let found = loop {
            if let Some(c) = ed_input::record::peek_clicks().into_iter().find(|c| c.game) {
                break Some(c);
            }
            if started.elapsed().as_secs() >= 30 {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        let _ = ed_input::record::stop_with_clicks();
        found
    })
    .await
    .map_err(|e| {
        if plot_capture {
            MAP_SETUP_TESTING.store(false, Ordering::SeqCst);
        }
        e.to_string()
    })?;
    let Some(c) = click else {
        if plot_capture {
            MAP_SETUP_TESTING.store(false, Ordering::SeqCst);
        }
        return Err("no click in the game window within 30 s".into());
    };
    let pt = (
        (c.xf * 1000.0).round() / 1000.0,
        (c.yf * 1000.0).round() / 1000.0,
    );
    let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let mut pts = cfg.map_points.clone().unwrap_or_default();
    match which.as_str() {
        "search" => pts.search = Some(pt),
        "result" => pts.result = Some(pt),
        "target" => pts.target = Some(pt),
        _ => pts.plot = Some(pt),
    }
    if let Some(m) = point_macro(&pts) {
        cfg.target_macro = Some(m);
    }
    cfg.map_points = Some(pts.clone());
    cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    Ok(pts)
}

/// End a teaching session that did not proceed into the guided test.
#[tauri::command]
pub async fn map_setup_cancel() {
    MAP_SETUP_TESTING.store(false, Ordering::SeqCst);
}

/// Set one of the recipe's waits ("open", "search", "target"), in ms.
#[tauri::command]
pub async fn map_delay_set(
    state: State<'_, AppState>,
    which: String,
    ms: u64,
) -> Result<MapPoints, String> {
    let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    let mut pts = cfg.map_points.clone().unwrap_or_default();
    let v = Some(ms.clamp(0, 10000));
    match which.as_str() {
        "open" => pts.open_delay_ms = v,
        "search" => pts.search_delay_ms = v,
        "target" => pts.target_delay_ms = v,
        _ => return Err(format!("unknown delay {which:?}")),
    }
    cfg.map_points = Some(pts.clone());
    cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    Ok(pts)
}

#[tauri::command]
pub async fn map_points_get(state: State<'_, AppState>) -> Result<MapPoints, String> {
    Ok(state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map_points
        .clone()
        .unwrap_or_default())
}

/// Forget the taught points; the macro goes back to the default recipe.
#[tauri::command]
pub async fn map_points_clear(state: State<'_, AppState>) -> Result<MapPoints, String> {
    let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    cfg.map_points = None;
    cfg.target_macro = None;
    cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    Ok(MapPoints::default())
}

/// The recipe in force: built from the taught map points when all three
/// are known (so timing changes apply without re-teaching), else the saved
/// steps, else the default.
pub fn configured_macro(cfg: &crate::state::AppConfig) -> Vec<MacroStep> {
    cfg.map_points
        .as_ref()
        .and_then(point_macro)
        .or_else(|| cfg.target_macro.clone())
        .unwrap_or_else(default_macro)
}

pub fn default_macro() -> Vec<MacroStep> {
    map_macro(8)
}

/// The galaxy-map recipe with the info-panel walk length as a parameter:
/// 8 downs reaches the targeting control, 7 the plot/clear-route control.
pub fn map_macro(downs: usize) -> Vec<MacroStep> {
    let mut v = vec![
        MacroStep::Clipboard,
        MacroStep::Bind {
            action: "GalaxyMapOpen".into(),
        },
        MacroStep::WaitGui {
            focus: 6,
            timeout_ms: 10000,
        },
        MacroStep::Hold {
            action: "CamZoomIn".into(),
            ms: 500,
        },
        MacroStep::Mouse { x: 0.5, y: 0.5 },
        MacroStep::Wait { ms: 60 },
        MacroStep::Bind {
            action: "UI_Up".into(),
        },
        MacroStep::Wait { ms: 120 },
        MacroStep::Bind {
            action: "UI_Select".into(),
        },
        MacroStep::Wait { ms: 150 },
        MacroStep::Type {
            text: "{system}".into(),
        },
        MacroStep::Wait { ms: 120 },
        MacroStep::Key {
            key: "Key_DownArrow".into(),
        },
        MacroStep::Wait { ms: 200 },
        MacroStep::Bind {
            action: "UI_Select".into(),
        },
        MacroStep::WaitPan {
            base_ms: 700,
            per_ly: 4.0,
        },
        MacroStep::Hold {
            action: "CamTranslateLeft".into(),
            ms: 30,
        },
        MacroStep::Wait { ms: 60 },
        MacroStep::Hold {
            action: "CamTranslateRight".into(),
            ms: 30,
        },
        MacroStep::Wait { ms: 150 },
        MacroStep::Bind {
            action: "UI_Select".into(),
        },
        MacroStep::Wait { ms: 250 },
        MacroStep::Bind {
            action: "UI_Right".into(),
        },
        MacroStep::Wait { ms: 80 },
    ];
    for _ in 0..downs {
        v.push(MacroStep::Bind {
            action: "UI_Down".into(),
        });
        v.push(MacroStep::Wait { ms: 60 });
    }
    v.extend([
        MacroStep::Wait { ms: 40 },
        MacroStep::Bind {
            action: "UI_Select".into(),
        },
        MacroStep::Wait { ms: 300 },
        MacroStep::Bind {
            action: "GalaxyMapOpen".into(),
        },
        MacroStep::WaitGui {
            focus: 0,
            timeout_ms: 4000,
        },
    ]);
    v
}

/// EliteIntel's RoutePlotter recipe (its source documents why each step
/// exists): a held camera zoom plus UI left/right lands focus on the search
/// field on the builds it was written against.
pub fn eliteintel_macro() -> Vec<MacroStep> {
    vec![
        MacroStep::Clipboard,
        MacroStep::Bind {
            action: "GalaxyMapOpen".into(),
        },
        MacroStep::WaitGui {
            focus: GUI_GALAXY_MAP,
            timeout_ms: 15000,
        },
        MacroStep::Wait { ms: 1000 },
        MacroStep::Hold {
            action: "CamZoomIn".into(),
            ms: 500,
        },
        MacroStep::Bind {
            action: "UI_Left".into(),
        },
        MacroStep::Wait { ms: 200 },
        MacroStep::Bind {
            action: "UI_Right".into(),
        },
        MacroStep::Wait { ms: 200 },
        MacroStep::Bind {
            action: "UI_Select".into(),
        },
        MacroStep::Wait { ms: 200 },
        MacroStep::Type {
            text: "{system}".into(),
        },
        MacroStep::Wait { ms: 250 },
        MacroStep::Key {
            key: "Key_DownArrow".into(),
        },
        MacroStep::Bind {
            action: "UI_Select".into(),
        },
    ]
}

#[derive(Debug, Serialize)]
pub struct MacroPreset {
    pub name: String,
    pub steps: Vec<MacroStep>,
}

#[tauri::command]
pub async fn target_macro_presets() -> Vec<MacroPreset> {
    vec![
        MacroPreset {
            name: "Default (Up, Select, type)".into(),
            steps: default_macro(),
        },
        MacroPreset {
            name: "Zoom first (zoom, Left, Right, Select, type)".into(),
            steps: eliteintel_macro(),
        },
    ]
}

/// Current `GuiFocus` from the latest Status.json snapshot.
fn gui_focus(state: &AppState) -> Option<u8> {
    state.with_read(|s| {
        s.conn()
            .query_row(
                "SELECT json_extract(raw,'$.GuiFocus') FROM snapshots WHERE name = 'Status.json'",
                [],
                |r| r.get::<_, Option<i64>>(0),
            )
            .ok()
            .flatten()
            .map(|v| v as u8)
    })
}

/// Why a route cannot be followed by a ship without a fuel scoop: it
/// relies on scoop stops. `None` when it can (no stops, a scoop fitted,
/// or no Loadout to judge by).
pub fn scoop_refusal(refuel_stops: usize, has_scoop: Option<bool>) -> Option<String> {
    match (refuel_stops, has_scoop) {
        (0, _) | (_, None) | (_, Some(true)) => None,
        (n, Some(false)) => Some(format!(
            "no fuel scoop fitted: this route relies on {n} scoop stop{}. Fit a scoop, or plot a route within one tank.",
            if n == 1 { "" } else { "s" }
        )),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveRoute {
    pub route: Route,
    /// Index into `route.hops` of the next system to jump to.
    pub next: usize,
    pub source: String,
}

/// The worst remaining jump a followed route still expects, when it no
/// longer fits the ship — the mass-change blind spot (maintainer, 2026-09-05:
/// bought 1,008 t at a stop, laden range fell under hop one, and the
/// jump-triggered replanner could never fire because the impossible
/// jump never happened). Boost-aware: a supercharged hop is allowed the
/// multiplier its departure star grants, so neutron highways are not
/// false alarms.
pub fn infeasible_hop(
    ar: &ActiveRoute,
    range_ly: f64,
    boost: &ed_galaxy::fuel::BoostProfile,
) -> Option<(String, f64)> {
    let hops = &ar.route.hops;
    let mut worst: Option<(String, f64)> = None;
    for i in ar.next..hops.len() {
        let hop = &hops[i];
        let allowed = if hop.boosted {
            // The multiplier comes from the star being DEPARTED.
            let mult = match i.checked_sub(1).map(|p| hops[p].class) {
                Some(ed_galaxy::StarClass::Neutron) => f64::from(boost.neutron),
                Some(ed_galaxy::StarClass::WhiteDwarf) => f64::from(boost.white_dwarf),
                _ => f64::from(boost.neutron.max(boost.white_dwarf)),
            };
            range_ly * mult.max(1.0)
        } else {
            range_ly
        };
        let d = f64::from(hop.distance_ly);
        if d > allowed && worst.as_ref().is_none_or(|(_, w)| d > *w) {
            worst = Some((hop.name.clone(), d));
        }
    }
    worst
}

/// FSDTarget for a system that is not on the EDDA-planned route: say so
/// NOW, while the commander can still change their mind, rather than at
/// arrival where the only remaining move is to re-plan (maintainer,
/// 2026-09-06: "if on a route we could say they aren't on the route and
/// we'll recalculate if they're following an edda route and not a game
/// planned route").
///
/// EDDA routes ONLY, by design. With no followed route the GAME owns the
/// plot, its own route line is the commander's guide, and there is no
/// "our route" to be off — warning against a plan we did not make would
/// be pure noise. A trade leg handed to the game's plotter is exactly
/// that case and stays silent here.
///
/// Warns; does not re-plan. The re-plan still belongs to arrival in
/// [`on_jump`], where "from here" names a system the ship is in.
pub fn on_target_off_route(
    conn: &Connection,
    v: &serde_json::Value,
    warned: &mut Option<i64>,
) -> Option<String> {
    let name = v.get("Name").and_then(serde_json::Value::as_str)?;
    let address = v.get("SystemAddress").and_then(serde_json::Value::as_i64);
    // One line per targeted system, not one per FSDTarget event.
    if address.is_some() && *warned == address {
        return None;
    }
    let ar = load(conn)?;
    // ANY hop counts as on-route, not just the next one: targeting a hop
    // further along the plan is a legitimate thing to do, and so is
    // re-targeting one already passed. Only a system the plan never
    // mentions is off it.
    if ar
        .route
        .hops
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case(name))
    {
        return None;
    }
    *warned = address;
    let dest = ar.route.hops.last()?.name.clone();
    tracing::info!(target = %name, %dest, "targeted off the followed route");
    Some(format!(
        "{name} is not on the route to {dest}. Jump there and I'll re-plan from it."
    ))
}

/// FSDTarget beyond any possible jump: the game only whispers "exceeds
/// fuel reserves" on the HUD (no journal event exists for the refusal),
/// but target + fuel model make it computable at targeting time. Only
/// fires while following, only for the route's own next hop, and only
/// when the distance beats even a max-boosted laden full-tank jump —
/// then replans and says why.
pub fn on_target_beyond_range(
    app: &tauri::AppHandle,
    conn: &rusqlite::Connection,
    v: &serde_json::Value,
    replanned: &mut Option<i64>,
) {
    use tauri::Manager as _;
    let Some(name) = v.get("Name").and_then(serde_json::Value::as_str) else { return };
    let address = v.get("SystemAddress").and_then(serde_json::Value::as_i64);
    if address.is_some() && *replanned == address {
        return;
    }
    let Some(ar) = load(conn) else { return };
    let Some(next_hop) = ar.route.hops.get(ar.next) else { return };
    if !next_hop.name.eq_ignore_ascii_case(name) {
        return;
    }
    let Some((m, boost, _, _)) = crate::routing::ship_fuel(conn) else { return };
    let state = app.state::<crate::state::AppState>();
    let Some(g) = state.routing.galaxy(&state.data_dir) else { return };
    let here = ed_store::query::location(conn).ok().flatten().and_then(|l| l.system_name);
    let (Some(here), Some(target)) = (here.and_then(|h| g.find(&h)), g.find(name)) else { return };
    let a = g.record(here).pos();
    let b = g.record(target).pos();
    let d = f64::from(((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt());
    let max_possible =
        f64::from(m.range_at(m.capacity)) * f64::from(boost.neutron.max(boost.white_dwarf).max(1.0));
    if d > max_possible {
        *replanned = address;
        tracing::info!(target = "redacted", needed = d, max_possible, "targeted hop beyond any possible jump; replanning");
        state.voice.say(format!(
            "That hop needs {d:.0} light years and this ship can't make it at this weight. Replotting."
        ));
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = replan_now(app).await {
                tracing::warn!(%error, "beyond-range replan failed");
            }
        });
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FollowView {
    pub active: bool,
    pub source: Option<String>,
    pub total_jumps: usize,
    pub jumps_left: usize,
    pub next: Option<Hop>,
    /// The next few hops from the cursor (for the HUD).
    pub ahead: Vec<Hop>,
    pub destination: Option<String>,
    pub next_index: usize,
}

pub fn load(conn: &Connection) -> Option<ActiveRoute> {
    let (json, next, source): (String, i64, Option<String>) = conn
        .query_row(
            "SELECT json, next, source FROM active_route WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .ok()??;
    let route: Route = serde_json::from_str(&json).ok()?;
    Some(ActiveRoute {
        route,
        next: next.max(0) as usize,
        source: source.unwrap_or_default(),
    })
}

pub fn save_pub(conn: &Connection, ar: &ActiveRoute) -> Result<(), String> {
    save(conn, ar)
}

fn save(conn: &Connection, ar: &ActiveRoute) -> Result<(), String> {
    let json = serde_json::to_string(&ar.route).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO active_route (id, json, next, source, updated) VALUES (1, ?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ','now'))
         ON CONFLICT(id) DO UPDATE SET json = excluded.json, next = excluded.next, source = excluded.source, updated = excluded.updated",
        params![json, ar.next as i64, ar.source],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn view(ar: Option<&ActiveRoute>) -> FollowView {
    match ar {
        None => FollowView {
            active: false,
            source: None,
            total_jumps: 0,
            jumps_left: 0,
            next: None,
            ahead: Vec::new(),
            destination: None,
            next_index: 0,
        },
        Some(a) => {
            let n = a.route.hops.len();
            let next = a.next.min(n);
            FollowView {
                active: true,
                source: Some(a.source.clone()),
                total_jumps: n.saturating_sub(1),
                jumps_left: n.saturating_sub(next),
                next: a.route.hops.get(next).cloned(),
                ahead: a.route.hops.iter().skip(next).take(6).cloned().collect(),
                destination: a.route.hops.last().map(|h| h.name.clone()),
                next_index: next,
            }
        }
    }
}

/// Spoken line for the state after an advance.
/// The briefing for a route followed from the app: the same shape as the
/// game-route one, from the plotted hops.
pub fn brief_text(ar: &ActiveRoute) -> String {
    let hops = &ar.route.hops;
    let jumps = hops.len().saturating_sub(1);
    let dest = hops
        .last()
        .map(|h| h.name.as_str())
        .unwrap_or("destination");
    let mut parts = vec![format!(
        "Route plotted: {jumps} jump{} to {dest}, {:.0} light years.",
        if jumps == 1 { "" } else { "s" },
        ar.route.total_ly
    )];
    let boosted = hops.iter().filter(|h| h.boosted).count();
    let scoops = hops.iter().filter(|h| h.refuel).count();
    let mut notes = Vec::new();
    if boosted > 0 {
        notes.push(format!("{boosted} supercharged"));
    }
    if scoops > 0 {
        notes.push(format!(
            "{scoops} scoop stop{}",
            if scoops == 1 { "" } else { "s" }
        ));
    }
    if !notes.is_empty() {
        let mut n = notes.join(", ");
        if let Some(c) = n.get_mut(0..1) {
            c.make_ascii_uppercase();
        }
        parts.push(format!("{n}."));
    }
    if hops.first().is_some_and(|h| h.refuel) && ar.next <= 1 {
        parts.push("Scoop here before you leave.".into());
    }
    if let Some(next) = hops.get(ar.next) {
        parts.push(format!(
            "First: {}{}.",
            next.name,
            if next.boosted { ", supercharged" } else { "" }
        ));
    }
    parts.join(" ")
}

/// Say and show the briefing for a route that just started being followed.
pub fn announce(app: &AppHandle, ar: &ActiveRoute) {
    announce_with(&app.state::<AppState>().announcer(), ar);
}

pub fn announce_with(announcer: &crate::watcher::Announcer, ar: &ActiveRoute) {
    let c = crate::callouts::Callout {
        kind: "route",
        text: brief_text(ar),
        priority: 1,
        speak: true,
        ts: chrono_now(),
    };
    announcer.deliver(vec![(c, None)]);
}

fn chrono_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ed_store::session::iso_from_epoch(secs as i64)
}

pub fn advance_text(ar: &ActiveRoute) -> String {
    let n = ar.route.hops.len();
    let left = n.saturating_sub(ar.next);
    if left == 0 {
        return format!(
            "Arrived at {}. Route complete.",
            ar.route
                .hops
                .last()
                .map(|h| h.name.as_str())
                .unwrap_or("destination")
        );
    }
    let next = &ar.route.hops[ar.next.min(n - 1)];
    let mut s = format!(
        "{left} jump{} left. Next: {}",
        if left == 1 { "" } else { "s" },
        next.name
    );
    let mut notes = Vec::new();
    if let Some(inj) = &next.injection {
        let recipe = ed_galaxy::router::INJECTION_RECIPES
            .iter()
            .find(|r| r.1 == inj.as_str())
            .map(|r| r.2.join(", "))
            .unwrap_or_default();
        notes.push(if recipe.is_empty() {
            format!("synthesise a {inj} FSD injection before this jump")
        } else {
            format!("synthesise a {inj} FSD injection before this jump: {recipe}")
        });
    }
    // Whether to fly the cone at the next system depends on the hop after it.
    let boost_there = ar
        .route
        .hops
        .get(ar.next + 1)
        .is_some_and(|after| after.boosted);
    match next.class {
        ed_galaxy::StarClass::Neutron => notes.push(if boost_there {
            "neutron star, supercharge there".into()
        } else {
            "neutron star".into()
        }),
        ed_galaxy::StarClass::WhiteDwarf => notes.push(if boost_there {
            "white dwarf, supercharge there".into()
        } else {
            "white dwarf".into()
        }),
        _ => {}
    }
    // A synthesized hop is a system the index does not hold yet (the
    // server bridged it from a position): its star is unknown, so say
    // so instead of briefing it like an indexed star.
    if next.synthesized {
        notes.push("not in the index yet, star unknown, no scoop there".into());
    }
    if next.refuel {
        notes.push(if next.class == ed_galaxy::StarClass::Neutron {
            "fuel stop, scoop at the companion star".into()
        } else {
            "fuel stop, scoop there".into()
        });
    }
    // Item 40a: the negative case says nothing — "fuel stop" callouts
    // mean something because silence is the default; the HUD carries
    // fuel-stop status visually instead.
    if !notes.is_empty() {
        s.push_str(", ");
        s.push_str(&notes.join(", "));
    }
    s.push('.');
    s
}

/// The rest of the followed route, flown from the real tank.
pub enum PlanCheck {
    Fine,
    /// The next hop is physically out of reach only because the tank is
    /// heavier than planned: burning `burn_t` tonnes fixes it.
    TooHeavy {
        hop: usize,
        name: String,
        over_by: f32,
        burn_t: f32,
    },
    /// A hop cannot be flown from this tank at all (fuel too low, or the
    /// schedule has drifted past a scoop).
    Broken {
        hop: usize,
        name: String,
    },
}

/// Simulate the remaining hops from `fuel_now` with the ship's fuel model
/// (physical reach, no planning margin). `from` is the index of the hop
/// the ship is at.
pub fn check_plan(conn: &Connection, ar: &ActiveRoute, from: usize, fuel_now: f32) -> PlanCheck {
    let Some((m, boost, _, _)) = crate::routing::ship_fuel(conn) else {
        return PlanCheck::Fine;
    };
    let hops = &ar.route.hops;
    let mut f = fuel_now;
    // At a planned fuel stop the tank is about to be filled: judge the
    // next hop from a full tank, not from what arrived here.
    if hops.get(from).is_some_and(|h| h.refuel || h.scoopable) {
        f = m.capacity;
    }
    for k in (from + 1)..hops.len() {
        let prev = &hops[k - 1];
        let h = &hops[k];
        let b = if h.boosted {
            boost.for_class(prev.class)
        } else {
            1.0
        };
        let fits = |fuel: f32| -> bool {
            let burn = m.fuel_for(h.distance_ly, fuel, b);
            h.distance_ly <= m.range_at(fuel) * b.max(1.0) - 0.2
                && burn <= m.max_fuel_per_jump + 1e-3
                && fuel - burn >= 0.0
        };
        if fits(f) {
            let left = f - m.fuel_for(h.distance_ly, f, b);
            f = if h.scoopable || h.refuel {
                m.capacity
            } else {
                left
            };
            continue;
        }
        // Would it fit with less in the tank? Then the fix is to burn some.
        let floor = (m.fuel_for(h.distance_ly, 0.0, b) + 0.5).max(0.0);
        if fits(floor) {
            let (mut lo, mut hi) = (floor, f);
            for _ in 0..40 {
                let mid = (lo + hi) / 2.0;
                if fits(mid) {
                    lo = mid
                } else {
                    hi = mid
                }
            }
            let over_by = (h.distance_ly - m.range_at(f) * b.max(1.0)).max(0.0);
            return PlanCheck::TooHeavy {
                hop: k,
                name: h.name.clone(),
                over_by,
                burn_t: (f - lo).max(0.1),
            };
        }
        return PlanCheck::Broken {
            hop: k,
            name: h.name.clone(),
        };
    }
    PlanCheck::Fine
}

/// With more fuel aboard than the plan assumed, re-fly the remaining hops
/// from the real tank and clear the fuel stops that are no longer needed --
/// only where the rest of the route to the following stop still flies
/// without them (a stop the route relies on is never dropped). Returns a
/// note when something changed.
pub fn relax_stops(
    conn: &Connection,
    ar: &mut ActiveRoute,
    from: usize,
    fuel_now: f32,
) -> Option<String> {
    let (m, boost, _, _) = crate::routing::ship_fuel(conn)?;
    let hops = &ar.route.hops;
    let planned_here = hops.get(from).and_then(|h| h.fuel_after)?;
    if fuel_now < planned_here + 5.0 {
        return None;
    }
    // Simulate forward; at each planned stop ask whether skipping it still
    // reaches the next stop (or the end) with the safety margin intact.
    let flies = |mut f: f32, from_k: usize, to_k: usize| -> bool {
        for k in (from_k + 1)..=to_k.min(hops.len() - 1) {
            let prev = &hops[k - 1];
            let h = &hops[k];
            let b = if h.boosted {
                boost.for_class(prev.class)
            } else {
                1.0
            };
            if h.distance_ly > m.reach(f, b) {
                return false;
            }
            let burn = m.fuel_for(h.distance_ly, f, b);
            if burn > m.max_fuel_per_jump || f - burn < m.max_fuel_per_jump * 0.75 {
                return false;
            }
            f -= burn;
            if k < to_k && (h.scoopable && !h.refuel) {
                // an unplanned scoopable star on the way does not count
            }
        }
        true
    };
    let mut f = if hops[from].refuel || hops[from].scoopable {
        m.capacity
    } else {
        fuel_now
    };
    let mut cleared: Vec<String> = Vec::new();
    let mut k = from + 1;
    while k < hops.len() {
        let prev = &hops[k - 1];
        let h = &hops[k];
        let b = if h.boosted {
            boost.for_class(prev.class)
        } else {
            1.0
        };
        let burn = m.fuel_for(h.distance_ly, f, b);
        f = (f - burn).max(0.0);
        if h.refuel {
            let next_stop = ((k + 1)..hops.len())
                .find(|&j| hops[j].refuel)
                .unwrap_or(hops.len() - 1);
            if flies(f, k, next_stop) {
                cleared.push(h.name.clone());
            } else {
                f = m.capacity;
            }
        } else if h.scoopable && h.fuel_after.is_some_and(|p| p >= m.capacity - 0.5) {
            f = m.capacity;
        }
        k += 1;
    }
    if cleared.is_empty() {
        return None;
    }
    for h in ar.route.hops.iter_mut() {
        if cleared.iter().any(|c| c == &h.name) {
            h.refuel = false;
        }
    }
    ar.route.refuel_stops = ar.route.hops.iter().filter(|h| h.refuel).count();
    save(conn, ar).ok()?;
    tracing::info!(
        cleared = cleared.len(),
        fuel = fuel_now,
        planned = planned_here,
        "fuller than planned: fuel stops no longer needed were cleared"
    );
    Some(format!(
        " Tank is fuller than planned; {} fuel stop{} no longer needed.",
        cleared.len(),
        if cleared.len() == 1 { " is" } else { "s are" }
    ))
}

/// Callouts that make no sense against the followed route: a low-fuel
/// warning right before a planned fuel stop.
/// Item 32: the SCO burn-down coach. Nobody scoops to the gram; when the
/// tank is a few tonnes too heavy for the next planned hop, the fix in
/// the cockpit is seconds of overcharge burn — so the coach says how
/// much to burn, and `check_plan` flipping back to Fine on a later
/// status tick IS the moment to say stop. One announcement per arming;
/// a hop advance, a re-plan, or a Broken verdict disarms silently.
/// (The low side — burning too far — stays the trap guard's job.)
struct BurnDown {
    hop: usize,
    announced: bool,
}
static BURNDOWN: std::sync::Mutex<Option<BurnDown>> = std::sync::Mutex::new(None);

/// Disarm the coach: the cursor moved or the route changed.
pub fn burndown_reset() {
    *BURNDOWN.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// One status tick of the coach: the plan verdict for the tank as it is
/// now, against the followed route's next hop. Returns the callout to
/// speak, if this tick crosses one of the two edges.
pub fn burndown_tick(next: usize, check: &PlanCheck) -> Option<crate::callouts::Callout> {
    let mut g = BURNDOWN.lock().unwrap_or_else(|e| e.into_inner());
    match check {
        PlanCheck::TooHeavy { hop, name, burn_t, .. } if *hop == next => {
            let announced = g.as_ref().is_some_and(|b| b.hop == next && b.announced);
            if announced {
                return None;
            }
            *g = Some(BurnDown { hop: next, announced: true });
            Some(crate::callouts::Callout::new(
                "burndown",
                &chrono_now(),
                2,
                true,
                format!(
                    "A lighter ship makes the jump to {name}: burn about {:.0} tonnes with the overcharge, then jump. Or say re-plan.",
                    burn_t.max(1.0)
                ),
            ))
        }
        PlanCheck::Fine => {
            let was_armed = g.take().is_some_and(|b| b.announced);
            was_armed.then(|| {
                crate::callouts::Callout::new(
                    "burndown",
                    &chrono_now(),
                    2,
                    true,
                    "That's the weight. Cut the overcharge and jump when ready.".into(),
                )
            })
        }
        _ => {
            *g = None;
            None
        }
    }
}

/// Item 46: what happens to a fuel-kind callout while a route is
/// followed. `covered` = the live re-sim of the remaining hops from the
/// CURRENT tank completes with margin (never the plot-time ledger —
/// item 45's divergence must not silence a real emergency); `off_plan`
/// = the re-sim says the route no longer flies. Threshold chatter
/// ("Fuel low.", the unscoopable-target caution) is suppressed while
/// covered; the trap and overcharge guards always keep their voice;
/// positives pass; and going off plan turns the low-fuel line into an
/// explicit off-plan warning instead of silence.
pub(crate) enum FuelChatter {
    Keep,
    Drop,
    Reword(String),
}

pub(crate) fn fuel_chatter(covered: bool, off_plan: bool, kind: &str, text: &str) -> FuelChatter {
    if kind != "fuel" {
        return FuelChatter::Keep;
    }
    // The chatter family, from the ship's own log: "Fuel low.", the
    // targeted-system caution, and "Warning: {class} class star ahead,
    // not scoopable. Fuel at N percent." The trap and overcharge
    // guards' warnings are reachability verdicts, not thresholds, and
    // always speak.
    let chatter = text.starts_with("Fuel low")
        || text.starts_with("Caution:")
        || (text.starts_with("Warning:")
            && text.contains("not scoopable")
            && !text.contains("fuel trap")
            && !text.contains("overcharge"));
    if chatter && covered {
        return FuelChatter::Drop;
    }
    if off_plan && text.starts_with("Fuel low") {
        return FuelChatter::Reword(
            "Fuel low — and the remaining route no longer flies from this tank. Off plan: scoop, or say re-plan.".into(),
        );
    }
    FuelChatter::Keep
}

pub fn filter_callouts(
    conn: &Connection,
    out: &mut Vec<(crate::callouts::Callout, Option<serde_json::Value>)>,
    fuel_now: Option<f32>,
) {
    let Some(ar) = load(conn) else { return };
    // Item 46: within route bounds — judged by the LIVE re-sim of the
    // remaining hops from the current tank — fuel threshold chatter is
    // noise; off plan, the low-fuel line speaks up instead.
    if let Some(f) = fuel_now {
        let check = check_plan(conn, &ar, ar.next.saturating_sub(1), f);
        let covered = matches!(check, PlanCheck::Fine | PlanCheck::TooHeavy { .. });
        let off_plan = matches!(check, PlanCheck::Broken { .. });
        out.retain_mut(|(c, _)| match fuel_chatter(covered, off_plan, c.kind, &c.text) {
            FuelChatter::Keep => true,
            FuelChatter::Drop => false,
            FuelChatter::Reword(t) => {
                c.text = t;
                true
            }
        });
    }
    let next_is_stop = ar
        .route
        .hops
        .get(ar.next)
        .is_some_and(|h| h.refuel || h.scoopable);
    if next_is_stop {
        // Threshold warnings are covered by the coming stop; the trap and
        // overcharge guards already reasoned about reachability, so their
        // verdicts stand even with a scoop stop ahead.
        out.retain(|(c, _)| {
            !(c.kind == "fuel"
                && c.text.starts_with("Warning:")
                && !c.text.contains("fuel trap")
                && !c.text.contains("overcharge"))
        });
    }
}

/// Called by the watcher on every FSDJump. Advances the cursor when the
/// system is a hop ahead of it; returns the spoken line when it moved.
/// With the tank level known, the rest of the plan is checked against it:
/// a hop that only needs a lighter ship gets an instruction, anything
/// worse gets the route re-planned from here.
fn persist_arrival(conn: &Connection, ar: &mut ActiveRoute, pos: usize) -> Result<bool, String> {
    ar.next = pos + 1;
    if ar.next >= ar.route.hops.len() {
        conn.execute("DELETE FROM active_route WHERE id = 1", [])
            .map_err(|e| e.to_string())?;
        Ok(true)
    } else {
        save(conn, ar)?;
        Ok(false)
    }
}

/// After startup catch-up: jumps made while the app was down never
/// reached the watcher (field case 2026-09-06 — a dev rebuild landed
/// mid-witchspace, and "target next route" kept offering the system
/// the commander was already in). Align the cursor with wherever the
/// journal says they actually are — SILENTLY: no voice, no replan,
/// just the cursor, the HUD event, and the coach reset. Off-route on
/// startup deliberately does nothing; a boot must never launch a plot
/// unasked — the next live jump gets the normal treatment.
pub fn reconcile(conn: &Connection, events: &dyn crate::events::Emitter) {
    use crate::events::EmitExt as _;
    let Some(mut ar) = load(conn) else { return };
    let Some(here) = ed_store::query::location(conn)
        .ok()
        .flatten()
        .and_then(|l| l.system_name)
    else {
        return;
    };
    let n = ar.route.hops.len();
    let Some(pos) = (0..n).find(|&i| ar.route.hops[i].name.eq_ignore_ascii_case(&here)) else {
        tracing::info!(%here, "startup: current system is off the followed route; cursor untouched until the next live jump");
        return;
    };
    if ar.next == pos + 1 {
        return; // Already aligned — the usual case.
    }
    burndown_reset();
    target_attempts_reset();
    match persist_arrival(conn, &mut ar, pos) {
        Ok(true) => {
            tracing::info!(%here, "startup: followed route completed while the app was away; cleared");
            events.emit(crate::events::ROUTE_FOLLOW, view(None));
        }
        Ok(false) => {
            tracing::info!(%here, next = %ar.route.hops[ar.next].name, "startup: follow cursor reconciled to the commander's actual system");
            events.emit(crate::events::ROUTE_FOLLOW, view(Some(&ar)));
        }
        Err(error) => tracing::warn!(%error, "startup follow reconcile failed"),
    }
}

/// Witchspace entry on a followed route: the leg IS flown, so the
/// cursor moves HERE rather than 18 seconds later at arrival (maintainer,
/// 2026-09-06: "mark that leg as complete when they enter witchspace,
/// not wait to arrival").
///
/// Safe to do early because the journal says where we are going:
/// measured on the maintainer's DB, all 1,633 Hyperspace `StartJump` rows
/// carry `StarSystem` + `SystemAddress`, so the advance lands on the
/// named hop instead of guessing, and the tunnel it front-runs is a
/// solid 18 s (p50 18, p90 19, over the same 1,633 jumps).
///
/// Deliberately NOT done here — each of these needs the ship to actually
/// BE there, and would lie mid-tunnel:
/// - the final hop's completion, and a trade leg's terminal guidance:
///   "Arrived at X" is false in witchspace, so the last hop still
///   completes at arrival;
/// - off-route replanning, which plans FROM the current system — in the
///   tunnel that is still the system being left;
/// - the fuel judgement and the burn-down coach. NOT because the tank is
///   stale: the maintainer corrected this (2026-09-06) — the jump's fuel burns
///   on ENTERING witchspace, so the tunnel tank is already the post-jump
///   one and `check_plan` would read the same number here that it reads
///   on arrival. It stays at arrival because its two outcomes both need
///   normal space: a re-plan must start from a system the ship is in,
///   and burn-down advice is something the commander acts on flying, not
///   in the tunnel. Moving it earlier is a live option, not a blocked
///   one.
///
/// Silent by design: the spoken line stays on arrival, so a jump is
/// still one utterance. This moves the cursor and the HUD only.
///
/// Takes an [`Emitter`](crate::events::Emitter) rather than an
/// `AppHandle` for the same reason [`reconcile`] does: it makes the
/// cursor rule testable without a running Tauri app.
pub fn on_witchspace(
    conn: &Connection,
    events: &dyn crate::events::Emitter,
    target: &str,
) -> bool {
    use crate::events::EmitExt as _;
    let Some(mut ar) = load(conn) else { return false };
    let n = ar.route.hops.len();
    let Some(pos) = (0..n).find(|&i| ar.route.hops[i].name.eq_ignore_ascii_case(target)) else {
        // Off-route: the warning already fired at targeting, and the
        // re-plan belongs to arrival, where "from here" means something.
        return false;
    };
    // The destination hop completes at arrival, with its ceremony intact.
    if pos + 1 >= n {
        return false;
    }
    // Already counted: a re-read of the journal file, or a second
    // StartJump for the same hop.
    if ar.next == pos + 1 {
        return false;
    }
    // The cursor is moving: advice armed for the hop being left is
    // stale, and the target-retry ledger belongs to the previous hop.
    burndown_reset();
    target_attempts_reset();
    if let Err(error) = persist_arrival(conn, &mut ar, pos) {
        tracing::warn!(%error, "witchspace cursor advance failed");
        return false;
    }
    tracing::info!(%target, next = %ar.route.hops[ar.next].name, "leg marked complete at witchspace entry");
    events.emit(crate::events::ROUTE_FOLLOW, view(Some(&ar)));
    true
}

pub fn on_jump(
    app: &AppHandle,
    conn: &Connection,
    system: &str,
    fuel_now: Option<f32>,
) -> Option<String> {
    let mut ar = load(conn)?;
    let n = ar.route.hops.len();
    let Some(pos) = (0..n).find(|&i| ar.route.hops[i].name.eq_ignore_ascii_case(system)) else {
        // Not on the route at all: re-plan from here rather than wait to be asked.
        if ar.route.hops.len() > 1 {
            tracing::info!(%system, "jumped off the followed route; re-planning from here");
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move { crate::routing::replan_followed(app2).await });
            return Some(format!(
                "{system} is not on the route. Re-planning from here."
            ));
        }
        return None;
    };
    // The cursor is about to move (or the plan re-judged from a new
    // system): whatever burn-down advice was armed is stale, and the
    // target-retry ledger belongs to the previous hop.
    burndown_reset();
    target_attempts_reset();
    if persist_arrival(conn, &mut ar, pos).ok()? {
        // Completion is a terminal state, not an active route with a cursor
        // one past the final hop. Clearing the persisted row also makes a
        // restart agree with the HUD and lets the frontend remove the map.
        tracing::info!(%system, "followed route completed and cleared");
        let _ = app.emit(crate::events::ROUTE_FOLLOW, view(None));
        // A TRADE leg's completion is not an ending — the commander is
        // in the stop's system with a pad to find. Fold the ceremony
        // into the one useful instruction (maintainer, 2026-09-05), and the
        // Docked hook picks up from there as usual.
        if ar.source == "trade" {
            if let Some(tr) = crate::trade_follow::load(conn) {
                let stop = tr.current();
                if stop.system.eq_ignore_ascii_case(system) {
                    return Some(format!(
                        "Target {} via the System Map for terminal guidance.",
                        stop.station
                    ));
                }
            }
        }
        return Some(advance_text(&ar));
    }
    let _ = app.emit(crate::events::ROUTE_FOLLOW, view(Some(&ar)));
    let mut text = advance_text(&ar);
    // Swapped ships mid-route: the plan is for the other one.
    if let (Some(planned), Some(now)) = (ar.route.ship_id, crate::routing::current_ship_id(conn)) {
        if planned != now {
            tracing::info!(
                planned,
                now,
                "followed route is for another ship; re-planning"
            );
            text.push_str(&format!(
                " This route was plotted for {}; re-planning for this ship.",
                ar.route.ship.as_deref().unwrap_or("another ship")
            ));
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move { crate::routing::replan_followed(app2).await });
            return Some(text);
        }
    }
    if let Some(fuel) = fuel_now {
        match check_plan(conn, &ar, pos, fuel) {
            PlanCheck::Fine => {
                // More fuel than the plan expected: stops it no longer needs
                // are cleared, so the callouts stop echoing an old assumption.
                if let Some(note) = relax_stops(conn, &mut ar, pos, fuel) {
                    text.push_str(&note);
                    let _ = app.emit(crate::events::ROUTE_FOLLOW, view(Some(&ar)));
                }
            }
            PlanCheck::TooHeavy { over_by, .. } if over_by < 0.1 => {}
            PlanCheck::TooHeavy {
                hop,
                name,
                over_by,
                burn_t,
            } if hop == ar.next && burn_t <= 20.0 => {
                text.push_str(&format!(" Warning: {name} is {over_by:.1} light years beyond reach at this tank. Burn about {burn_t:.0} tonnes first, or say re-plan."));
                // The arrival line just gave the burn advice: arm the
                // coach as already-announced so the status ticks only
                // watch for the stop cue.
                let _ = burndown_tick(hop, &PlanCheck::TooHeavy { hop, name, over_by, burn_t });
            }
            PlanCheck::TooHeavy { hop, name, .. } | PlanCheck::Broken { hop, name } => {
                tracing::info!(hop, %name, fuel, "followed route no longer flies from the real tank; re-planning");
                text.push_str(" The plan no longer fits the tank. Re-planning from here.");
                let app2 = app.clone();
                tauri::async_runtime::spawn(
                    async move { crate::routing::replan_followed(app2).await },
                );
            }
        }
    }
    Some(text)
}

/// One way the game's next-system action can be triggered, and whether we watch it.
#[derive(Debug, Clone, Serialize)]
pub struct TargetKeyBinding {
    pub source: &'static str, // "game" (from Custom.binds) | "app" (captured here)
    pub human: String,
    pub watched: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetKeyStatus {
    pub bindings: Vec<TargetKeyBinding>,
    pub trigger: Option<crate::listen::PttSource>,
}

const TARGET_ACTION: &str = "TargetNextRouteSystem";

static TARGET_TX: std::sync::OnceLock<std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>> =
    std::sync::OnceLock::new();
static KEY_WATCHED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The sender that turns a watched press into "next hop on the clipboard";
/// its thread is started once.
fn target_sender(app: &AppHandle) -> std::sync::mpsc::Sender<()> {
    let slot = TARGET_TX.get_or_init(|| std::sync::Mutex::new(None));
    let mut g = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tx) = g.as_ref() {
        return tx.clone();
    }
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let app2 = app.clone();
    std::thread::spawn(move || {
        let mut last = std::time::Instant::now() - std::time::Duration::from_secs(10);
        while rx.recv().is_ok() {
            // One physical press can arrive twice (a hat that is also mapped
            // to the key, a bounce): the second within half a second is the same press.
            if last.elapsed() < std::time::Duration::from_millis(500) {
                continue;
            }
            last = std::time::Instant::now();
            let state = app2.state::<AppState>();
            let focused = ed_input::send::game_is_focused();
            tracing::info!(focused, "next-system press seen");
            if !focused {
                continue;
            }
            // Usually only an app route needs interception. The exception is
            // a nearby destination waiting for this press to run Plot Route.
            if state.with_read(|s| load(s.conn())).is_none() && !in_game_plot_pending() {
                tracing::info!("next-system press: no app route is followed");
                continue;
            }
            // Exactly what the Target next button does: clipboard, or the
            // galaxy-map recipe when that is switched on.
            match target_next_state(&state) {
                Ok(msg) => {
                    tracing::info!(%msg, "next-system press handled");
                    state.voice.say(crate::commands::speakable(&msg));
                    if let Some(ar) = state.with_read(|s| load(s.conn())) {
                        let _ = app2.emit(crate::events::ROUTE_FOLLOW, view(Some(&ar)));
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "next-system press failed");
                    state.voice.say(crate::commands::speakable(&format!("Couldn't target: {e}")));
                }
            }
        }
    });
    *g = Some(tx.clone());
    tx
}

/// A plain key name from a captured hotkey ("F10" -> Key_F10); chords
/// with modifiers cannot be watched without consuming them.
fn plain_key_scan(hotkey: &str) -> Option<ed_input::keys::ScanCode> {
    if hotkey.contains('+') {
        return None;
    }
    // The capture names keys as browser codes (KeyS, Digit5, ArrowUp); the
    // binds names are what the scan-code table knows.
    let binds_name = match hotkey {
        k if k.len() == 4 && k.starts_with("Key") => format!("Key_{}", &k[3..]),
        k if k.len() == 6 && k.starts_with("Digit") => format!("Key_{}", &k[5..]),
        "ArrowUp" => "Key_UpArrow".into(),
        "ArrowDown" => "Key_DownArrow".into(),
        "ArrowLeft" => "Key_LeftArrow".into(),
        "ArrowRight" => "Key_RightArrow".into(),
        "Backquote" => "Key_Grave".into(),
        "Equal" => "Key_Equals".into(),
        "BracketLeft" => "Key_LeftBracket".into(),
        "BracketRight" => "Key_RightBracket".into(),
        "Semicolon" => "Key_SemiColon".into(),
        "Quote" => "Key_Apostrophe".into(),
        k if k.starts_with("Numpad") => format!("Key_Numpad_{}", &k[6..]),
        other => format!("Key_{other}"),
    };
    ed_input::keys::scan_code(&binds_name).or_else(|| ed_input::keys::scan_code(hotkey))
}

fn watch_key_once(
    tag: String,
    sc: ed_input::keys::ScanCode,
    tx: &std::sync::mpsc::Sender<()>,
    human: &str,
) {
    if KEY_WATCHED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(&tag)
    {
        return;
    }
    let tx2 = tx.clone();
    match ed_input::record::watch(
        sc,
        Box::new(move || {
            let _ = tx2.send(());
        }),
    ) {
        Ok(()) => {
            KEY_WATCHED
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(tag);
            tracing::trace!(key = %human, "watching a next-system key for app routes");
        }
        Err(e) => tracing::warn!(error = %e, key = %human, "could not watch a next-system key"),
    }
}

/// What triggers "next system" for app routes: the game's own keyboard
/// binding(s) for TargetNextRouteSystem (watched, never consumed) and the
/// key or stick button captured in the app. Safe to call again.
pub fn watch_game_target_key(app: AppHandle) {
    let state = app.state::<AppState>();
    let trigger = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .target_trigger
        .clone();
    let tx = target_sender(&app);
    if let Some(binds) = ed_input::binds::Binds::default_dir()
        .and_then(|d| ed_input::binds::Binds::find_latest(&d))
        .and_then(|p| ed_input::binds::Binds::load(&p).ok())
    {
        for c in binds
            .actions
            .get(TARGET_ACTION)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            if !c.modifiers.is_empty() {
                tracing::info!(chord = %c.human(), "the game's next-system key has modifiers; not watched");
                continue;
            }
            if let Some((_, sc)) = c.scan_codes() {
                watch_key_once(format!("kb:{}", c.key), sc, &tx, &c.human());
            }
        }
    }
    ed_input::joy::unwatch_all();
    match trigger {
        Some(crate::listen::PttSource::Keyboard { hotkey }) => match plain_key_scan(&hotkey) {
            Some(sc) => watch_key_once(format!("kb:{hotkey}"), sc, &tx, &hotkey),
            None => {
                tracing::info!(%hotkey, "captured next-system key is a chord or unknown; not watched")
            }
        },
        Some(crate::listen::PttSource::Joystick {
            device,
            button,
            name,
        }) => {
            // The saved id while its name still matches (generic names like
            // "Microsoft PC-joystick driver" can cover several sticks); else
            // the first device with that name, in case the ids moved.
            let devs = ed_input::joy::devices();
            let same =
                |d: &&ed_input::joy::JoyDevice| !d.name.is_empty() && name.starts_with(&d.name);
            let id = devs
                .iter()
                .find(|d| d.id == device && same(d))
                .or_else(|| devs.iter().find(same))
                .map(|d| d.id)
                .unwrap_or(device);
            let tx2 = tx.clone();
            ed_input::joy::watch(
                id,
                button,
                Box::new(move || {
                    let _ = tx2.send(());
                }),
            );
            tracing::trace!(joystick = id, button, %name, "watching a next-system button for app routes");
        }
        _ => {}
    }
}

/// The game's bindings for the action and the app's captured trigger.
#[tauri::command]
pub async fn target_key_status(state: State<'_, AppState>) -> Result<TargetKeyStatus, String> {
    let trigger = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .target_trigger
        .clone();
    let mut bindings = Vec::new();
    if let Some(binds) = ed_input::binds::Binds::default_dir()
        .and_then(|d| ed_input::binds::Binds::find_latest(&d))
        .and_then(|p| ed_input::binds::Binds::load(&p).ok())
    {
        for c in binds
            .actions
            .get(TARGET_ACTION)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let ok = c.modifiers.is_empty() && c.scan_codes().is_some();
            bindings.push(TargetKeyBinding {
                source: "game",
                human: c.human(),
                watched: ok,
                note: (!ok).then(|| "a chord with modifiers is not watched".into()),
            });
        }
        for d in binds.device_binds(TARGET_ACTION) {
            bindings.push(TargetKeyBinding {
                source: "game",
                human: format!("{} on {}", d.key, d.device),
                watched: false,
                note: Some(
                    "a stick or throttle: capture the same button below and it will be watched"
                        .into(),
                ),
            });
        }
    }
    if let Some(t) = &trigger {
        let (human, watched, note) = match t {
            crate::listen::PttSource::Keyboard { hotkey } => (hotkey.clone(), plain_key_scan(hotkey.as_str()).is_some(), plain_key_scan(hotkey.as_str()).is_none().then(|| "a chord with modifiers (or an unknown key) is not watched; capture a plain key".into())),
            crate::listen::PttSource::Joystick { name, .. } => (name.clone(), true, None),
            crate::listen::PttSource::None => ("none".into(), false, None),
        };
        bindings.push(TargetKeyBinding {
            source: "app",
            human,
            watched,
            note,
        });
    }
    Ok(TargetKeyStatus { bindings, trigger })
}

/// Capture a key or joystick button as the app's "next system" trigger
/// (10 s), save it, and start watching it.
#[tauri::command]
pub async fn target_trigger_capture(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TargetKeyStatus, String> {
    let src = crate::listen::ptt_capture(Some(10)).await?;
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.target_trigger = Some(src);
        cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    }
    watch_game_target_key(app);
    target_key_status(state).await
}

/// Forget the captured trigger (the game's keyboard binding stays watched).
#[tauri::command]
pub async fn target_trigger_clear(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TargetKeyStatus, String> {
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.target_trigger = None;
        cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    }
    watch_game_target_key(app);
    target_key_status(state).await
}

#[tauri::command]
pub async fn route_active_get(state: State<'_, AppState>) -> Result<Option<Route>, String> {
    Ok(state.with_read(|s| load(s.conn())).and_then(|ar| {
        // Restore only a journey still in progress (maintainer, 2026-09-05:
        // "sometimes when we start up I notice a stale route"): a
        // completed route's persisted row lingers, and unconditionally
        // resurrecting it put last week's journey on the tab at launch.
        if ar.next + 1 >= ar.route.hops.len() {
            None
        } else {
            Some(ar.route)
        }
    }))
}

/// Voice/tool entry: re-plan the followed route from the current system.
pub async fn replan_now(app: AppHandle) -> Result<String, String> {
    crate::routing::replan_followed(app).await
}

#[tauri::command]
pub async fn route_activate(
    app: AppHandle,
    state: State<'_, AppState>,
    route: Route,
    source: Option<String>,
) -> Result<FollowView, String> {
    // A plan is only good for the ship whose fuel model made it.
    let current = state.with_read(|s| crate::routing::current_ship_id(s.conn()));
    if let (Some(planned), Some(now)) = (route.ship_id, current) {
        if planned != now {
            let flying = state
                .with_read(|s| crate::routing::ship_fuel(s.conn()))
                .map(|(_, _, _, l)| l.split(" · ").next().unwrap_or("this ship").to_string())
                .unwrap_or_else(|| "this ship".into());
            return Err(format!("wrong ship: this route was plotted for {}, and you're flying {flying}. Re-plot it for this ship.", route.ship.as_deref().unwrap_or("another ship")));
        }
    }
    // A plan may assume scoop stops the ship cannot make: plotting it is
    // fine (it shows what the trip would take), following it is not.
    let has_scoop = state.with_read(|s| crate::routing::ship_has_fuel_scoop(s.conn(), None));
    if let Some(reason) = scoop_refusal(route.refuel_stops, has_scoop) {
        return Err(reason);
    }
    let here = state.with_read(|s| {
        ed_store::query::location(s.conn())
            .ok()
            .flatten()
            .and_then(|l| l.system_name)
    });
    // Start at the first hop after the current system when we are on it.
    let next = here
        .as_deref()
        .and_then(|h| {
            route
                .hops
                .iter()
                .position(|x| x.name.eq_ignore_ascii_case(h))
        })
        .map(|i| i + 1)
        .unwrap_or(1)
        .min(route.hops.len());
    let ar = ActiveRoute {
        route,
        next,
        source: source.unwrap_or_else(|| "plot".into()),
    };
    // Following something else while a trade route is active means the
    // commander changed plans: the trade layer steps aside, audibly —
    // never silently, and never fighting over the jump route.
    if ar.source != "trade" {
        let trading = state.with_read(|s| crate::trade_follow::load(s.conn()).is_some());
        if trading {
            state.with_store(|s| {
                crate::trade_follow::clear(s.conn());
                Ok::<(), String>(())
            })?;
            use crate::events::EmitExt as _;
            state.events.emit(crate::events::TRADE_FOLLOW, crate::trade_follow::view(None));
            state.voice.say("Trade route stopped — following your new route instead.".to_string());
        }
    }
    state.with_store(|s| save(s.conn(), &ar))?;
    let v = view(Some(&ar));
    let _ = app.emit(crate::events::ROUTE_FOLLOW, &v);
    announce(&app, &ar);
    Ok(v)
}

#[tauri::command]
pub async fn route_clear(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.with_store(|s| {
        s.conn()
            .execute("DELETE FROM active_route WHERE id = 1", [])
            .map(|_| ())
            .map_err(|e| e.to_string())
    })?;
    let _ = app.emit(crate::events::ROUTE_FOLLOW, view(None));
    Ok(())
}

#[tauri::command]
pub async fn route_follow_status(state: State<'_, AppState>) -> Result<FollowView, String> {
    Ok(state.with_read(|s| view(load(s.conn()).as_ref())))
}

/// Move the cursor by hand (+1 / -1), e.g. after jumping off-route.
#[tauri::command]
pub async fn route_advance(
    app: AppHandle,
    state: State<'_, AppState>,
    delta: i64,
) -> Result<FollowView, String> {
    let mut ar = state
        .with_read(|s| load(s.conn()))
        .ok_or("no active route")?;
    let n = ar.route.hops.len() as i64;
    ar.next = (ar.next as i64 + delta).clamp(1, n) as usize;
    state.with_store(|s| save(s.conn(), &ar))?;
    let v = view(Some(&ar));
    let _ = app.emit(crate::events::ROUTE_FOLLOW, &v);
    Ok(v)
}


/// Item 41: close whichever map the game reports open using the map's
/// OWN toggle key — never Escape. Esc is context-dependent (pause menu
/// in the cockpit, deselect in panels): with a stale Status.json it
/// lands somewhere the pilot can feel — the flight's map-loop burst
/// read "like it was hitting esc" because it was. A map key only ever
/// toggles its own screen; if it is unbound the macro aborts with a
/// named error instead of pressing anything else.
fn close_open_map_steps(focus: u8) -> Vec<MacroStep> {
    let action = if focus == 7 { "SystemMapOpen" } else { "GalaxyMapOpen" };
    vec![
        MacroStep::Bind { action: action.into() },
        MacroStep::WaitGui { focus: 0, timeout_ms: 3000 },
    ]
}


/// Item 41: the map-thrash guards. The flight fingerprint (62 map opens
/// over 49 jumps, 7 in the final two minutes) was a pilot re-pressing
/// the target key while each press re-ran the FULL recipe against a
/// target that had not stuck. Three gates now sit between the press and
/// the map, decided here and tested as pure logic.
enum TargetRetry {
    /// Run the recipe.
    Fresh,
    /// A re-press moments after a run: acknowledge, do not re-run.
    Cooldown,
    /// The recipe has failed twice for this hop: clipboard, not the map.
    Fallback,
}

const TARGET_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(10);

fn target_retry(same_hop: bool, since_last: std::time::Duration, runs: u32) -> TargetRetry {
    if !same_hop {
        TargetRetry::Fresh
    } else if since_last < TARGET_COOLDOWN {
        TargetRetry::Cooldown
    } else if runs >= 2 {
        TargetRetry::Fallback
    } else {
        TargetRetry::Fresh
    }
}

/// (hop name, last recipe run, completed runs for this hop).
static TARGET_ATTEMPTS: Mutex<Option<(String, std::time::Instant, u32)>> = Mutex::new(None);

/// A new hop (or a new route) wipes the retry ledger.
pub fn target_attempts_reset() {
    *TARGET_ATTEMPTS.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Serialises macro runs so a double-press does not interleave keystrokes.
static MACRO_LOCK: Mutex<()> = Mutex::new(());
static PENDING_GAME_PLOT: Mutex<Option<String>> = Mutex::new(None);

fn in_game_plot_pending() -> bool {
    PENDING_GAME_PLOT.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

/// Put the next system into the game's galaxy map (see module docs).
pub fn target_next(app: &AppHandle) -> Result<String, String> {
    let state = app.state::<AppState>();
    let msg = target_next_state(&state)?;
    if let Some(ar) = state.with_read(|s| load(s.conn())) {
        let _ = app.emit(crate::events::ROUTE_FOLLOW, view(Some(&ar)));
    }
    Ok(msg)
}

/// Same, without an app handle (the ship computer's tool path).
pub fn target_next_state(state: &AppState) -> Result<String, String> {
    let pending_plot = PENDING_GAME_PLOT.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(system) = pending_plot {
        let steps = state.config.lock().unwrap_or_else(|e| e.into_inner()).map_points.as_ref().and_then(point_plot_macro)
            .ok_or("teach all four Galaxy Map controls in Setup first")?;
        if let Err(e) = run_macro(state, &steps, &system, 100.0) {
            *PENDING_GAME_PLOT.lock().unwrap_or_else(|x| x.into_inner()) = Some(system);
            return Err(e);
        }
        return Ok(format!("Asked Elite to plot a route to {system}."));
    }
    let following = state.with_read(|s| load(s.conn()));
    let Some(ar) = following else {
        // No app route: the game's own plotted route has its own button.
        let game_route = state.with_read(|s| ed_store::route::current(s.conn()).ok().flatten());
        return match game_route {
            Some(b) if b.hops.len() > 1 => crate::control::press("target_next_route", 1)
                .map(|_| "Targeting the next system on the game's route.".into()),
            _ => Err("no route -- plot one here or in the galaxy map".into()),
        };
    };
    let next = ar.route.hops.get(ar.next).ok_or("route complete")?;
    let name = next.name.clone();
    let hop_ly = next.distance_ly;
    let (steps, use_macro) = {
        let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        (configured_macro(&cfg), cfg.target_macro_enabled)
    };
    if !use_macro {
        // The reliable way: the name is on the clipboard, paste it into the
        // galaxy map's search box.
        set_clipboard(&name)?;
        return Ok(format!(
            "Next: {name}. It's on the clipboard, paste it into the galaxy map."
        ));
    }
    // Item 41 gate 0: the game may already have it — a press after a
    // slow-feeling success re-ran the whole recipe for nothing.
    let already = state
        .with_read(|s| ed_store::query::nav_target(s.conn()).ok().flatten())
        .and_then(|t| t.target_system)
        .is_some_and(|t| t.eq_ignore_ascii_case(&name));
    if already {
        target_attempts_reset();
        return Ok(format!("{name} is already targeted."));
    }
    // Gates 1-3: cooldown and the two-strikes clipboard fallback.
    {
        let mut g = TARGET_ATTEMPTS.lock().unwrap_or_else(|e| e.into_inner());
        let (same, since, runs) = match g.as_ref() {
            Some((h, at, n)) if h.eq_ignore_ascii_case(&name) => (true, at.elapsed(), *n),
            _ => (false, std::time::Duration::ZERO, 0),
        };
        match target_retry(same, since, runs) {
            TargetRetry::Cooldown => {
                return Ok(format!("Still working on {name} — give it a few seconds before pressing again."));
            }
            TargetRetry::Fallback => {
                set_clipboard(&name)?;
                return Ok(format!("The galaxy-map recipe isn't sticking for {name}. It's on the clipboard — paste it into the map search."));
            }
            TargetRetry::Fresh => {
                *g = Some((name.clone(), std::time::Instant::now(), runs + 1));
            }
        }
    }
    // The map key is a toggle: if a map is already open, close it first and
    // wait for the game to say so, or the macro would close what it means to open.
    if let Some(f @ (6 | 7)) = gui_focus(state) {
        if ed_input::send::game_is_focused() {
            run_macro(
                state,
                &close_open_map_steps(f),
            &name,
            hop_ly,
        )?;
        }
    }
    run_macro(state, &steps, &name, hop_ly)?;
    // Item 41: verify the target actually stuck before claiming it did —
    // an unverified "targeted" is what taught the pilot to hammer the key.
    if wait_for_game_target_within(state, &name, std::time::Duration::from_secs(5)).is_ok() {
        target_attempts_reset();
        Ok(format!("targeted {name}"))
    } else {
        Ok(format!("Ran the recipe, but the game hasn't confirmed {name} yet. Press again in a few seconds to retry."))
    }
}

/// Run the galaxy-map recipe once with a sample system -- the next hop of
/// the route being followed, or the public setup system -- whatever the route state; the game
/// must be in the foreground.
#[tauri::command]
pub async fn target_macro_test(state: State<'_, AppState>) -> Result<String, String> {
    let name = state
        .with_read(|s| load(s.conn()))
        .and_then(|ar| ar.route.hops.get(ar.next).map(|h| h.name.clone()))
        .unwrap_or_else(|| map_setup_system(&state));
    let steps = configured_macro(&state.config.lock().unwrap_or_else(|e| e.into_inner()));
    if let Some(f @ (6 | 7)) = gui_focus(&state) {
        if ed_input::send::game_is_focused() {
            run_macro(
                &state,
                &close_open_map_steps(f),
            &name,
            100.0,
        )?;
        }
    }
    run_macro(&state, &steps, &name, 100.0)?;
    Ok(format!(
        "Ran the recipe for {name}. Is it targeted in the game?"
    ))
}

/// Run the taught Plot Route recipe immediately for onboarding verification.
/// Unlike `route_plot_in_game`, this does not arm the routing control; the
/// three-second frontend countdown gives the commander time to focus Elite.
#[tauri::command]
pub async fn route_plot_test(state: State<'_, AppState>, system: String) -> Result<String, String> {
    let steps = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map_points
        .as_ref()
        .and_then(point_plot_macro)
        .ok_or("teach all four Galaxy Map controls in Setup first")?;
    if let Some(f @ (6 | 7)) = gui_focus(&state) {
        if ed_input::send::game_is_focused() {
            run_macro(&state, &close_open_map_steps(f), &system, 100.0)?;
        }
    }
    run_macro(&state, &steps, &system, 100.0)?;
    // Item 35a: the test cleans up after itself — but only when the game
    // CONFIRMED the plot, because the taught plot button is a toggle and
    // a blind second press would plot instead of clear.
    let confirmed = wait_for_game_route(&state, Some(&system)).is_ok();
    let (clear, msg) = plot_test_outcome(confirmed, &system);
    if clear {
        run_macro(&state, &steps, &system, 100.0)?;
        wait_for_game_route(&state, None)?;
    }
    Ok(msg)
}

/// Item 35a's decision, kept pure for the test: clear only a confirmed
/// route; an unconfirmed one is left for the commander to judge.
fn plot_test_outcome(confirmed: bool, system: &str) -> (bool, String) {
    if confirmed {
        (true, format!("Ran the Plot Route recipe for {system}: the game reported the route, and the test route was cleared."))
    } else {
        (false, format!("Ran the Plot Route recipe for {system}. Is the route plotted in the game?"))
    }
}

/// Complete the four-point teaching flow while Elite still has the selected
/// system open: toggle off the plotted route and target, close the map, then
/// exercise both taught recipes from a clean state.
#[tauri::command]
pub async fn map_setup_test(app: AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    MAP_SETUP_TESTING.store(true, Ordering::SeqCst);
    let _guard = MapSetupGuard;
    let points = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map_points
        .clone()
        .ok_or("teach all four Galaxy Map controls first")?;
    points.target.ok_or("teach the Target button first")?;
    let plot = points.plot.ok_or("teach the Plot Route button first")?;
    // Picked per run: the nearest populated neighbor, so the test plot
    // succeeds wherever the commander lives — bubble or Colonia.
    let test_system = map_setup_system(&state);
    map_setup_prompt(&app, &state, "Great, Galaxy Map setup is complete. In three seconds, I will clear the route, close and open the Galaxy Map, and test targeting and routing.");
    std::thread::sleep(std::time::Duration::from_secs(3));
    map_setup_prompt(&app, &state, "I'm clearing the route.");
    run_macro(&state, &[MacroStep::Click { x: plot.0, y: plot.1 }, MacroStep::Wait { ms: 900 }], &test_system, 100.0)?;
    map_setup_prompt(&app, &state, "I'm closing the Galaxy Map.");
    run_macro(
        &state,
        &[
            MacroStep::Bind { action: "GalaxyMapOpen".into() },
            MacroStep::WaitGui { focus: 0, timeout_ms: 4000 },
        ],
        &test_system,
        100.0,
    )?;
    let target_steps = point_macro(&points).ok_or("teach all four Galaxy Map controls first")?;
    map_setup_prompt(&app, &state, "I'm opening the Galaxy Map and testing targeting.");
    run_macro(&state, &target_steps, &test_system, 100.0)?;
    // Elite's Target button is a TOGGLE, and the teaching flow guarantees
    // the test system is already targeted (the commander clicked Target to
    // teach the button). The first automated click therefore UNTARGETS —
    // silently, no journal event (field case 2026-09-04, Dyavata). One
    // more pass toggles it back on and emits a fresh FSDTarget; only a
    // recipe that misses twice is actually broken.
    if wait_for_game_target(&state, &test_system).is_err() {
        map_setup_prompt(&app, &state, "The Target button may have switched an existing target off. Testing once more.");
        run_macro(&state, &target_steps, &test_system, 100.0)?;
        if let Err(error) = wait_for_game_target(&state, &test_system) {
            // Diagnose before giving up: a DIFFERENT system in the raw
            // target means the fuzzy map search selected a sibling (the
            // procedural-name trap); otherwise it is likely Elite's
            // stuck-target state, which a relog clears. Persona line is
            // the maintainer's, verbatim (2026-09-05).
            let observed = state
                .with_read(|s| Ok::<_, String>(ed_store::query::latest_fsd_target_name(s.conn()).ok().flatten()))
                .ok()
                .flatten();
            let hint = match observed {
                Some(other) if !other.eq_ignore_ascii_case(&test_system) => format!(
                    " The game targeted {other} instead — the map search likely selected a similarly-named system."
                ),
                _ => " You may have a stuck target, Commander. Rest assured I know how to drive the ship.".to_string(),
            };
            return Err(format!("{error}{hint}"));
        }
    }
    let plot_steps = point_plot_macro(&points).ok_or("teach all four Galaxy Map controls first")?;
    map_setup_prompt(&app, &state, "Now I'm testing route plotting.");
    run_macro(&state, &plot_steps, &test_system, 100.0)?;
    wait_for_game_route(&state, Some(test_system.as_str()))?;
    map_setup_prompt(&app, &state, "Targeting and route plotting passed. I'm clearing the test route.");
    // Repeat the complete taught search-and-plot recipe. With the route to
    // this destination already active, the same button is Elite's Clear
    // Route control; reopening directly did not reliably retain the
    // destination panel containing that button.
    run_macro(&state, &plot_steps, &test_system, 100.0)?;
    wait_for_game_route(&state, None)?;
    map_setup_prompt(&app, &state, "Galaxy Map setup is complete.");
    Ok(format!("Galaxy Map setup complete. Targeting and route plotting were tested with {test_system}."))
}

fn wait_for_game_target(state: &AppState, expected: &str) -> Result<(), String> {
    wait_for_game_target_within(state, expected, std::time::Duration::from_secs(12))
}

fn wait_for_game_target_within(state: &AppState, expected: &str, patience: std::time::Duration) -> Result<(), String> {
    let started = std::time::Instant::now();
    while started.elapsed() < patience {
        // Two acceptable proofs (field case 2026-09-05): the HUD-grade
        // nav_target, OR the raw latest FSDTarget event. The raw check
        // covers the teach-then-test ordering where the system is
        // ALREADY targeted from the commander's own teaching click:
        // Elite emits no event for re-targeting the current target, and
        // the test's route-clear suppresses nav_target permanently —
        // without the raw fallback this test could never pass.
        let matched = state.with_read(|s| {
            let hud = ed_store::query::nav_target(s.conn())
                .map_err(|e| e.to_string())?
                .and_then(|t| t.target_system)
                .is_some_and(|t| t.eq_ignore_ascii_case(expected));
            if hud {
                return Ok::<bool, String>(true);
            }
            Ok(ed_store::query::latest_fsd_target_name(s.conn())
                .map_err(|e| e.to_string())?
                .is_some_and(|t| t.eq_ignore_ascii_case(expected)))
        })?;
        if matched {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(format!("Elite did not report {expected} as the selected target; targeting test failed"))
}

fn wait_for_game_route(state: &AppState, expected: Option<&str>) -> Result<(), String> {
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(12) {
        let destination = state.with_read(|s| {
            Ok::<Option<String>, String>(ed_store::route::current(s.conn())
                .map_err(|e| e.to_string())?
                .and_then(|r| r.hops.last().map(|h| h.system.clone())))
        })?;
        let matched = match (expected, destination.as_deref()) {
            (Some(want), Some(got)) => got.eq_ignore_ascii_case(want),
            (None, None) => true,
            _ => false,
        };
        if matched {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Err(match expected {
        Some(system) => format!("Elite did not report a plotted route to {system}; route plotting test failed"),
        None => "Elite still reports a plotted route; test-route cleanup failed".into(),
    })
}

#[tauri::command]
pub async fn route_plot_in_game(state: State<'_, AppState>, system: String) -> Result<String, String> {
    plot_in_game(state.inner(), system)
}

/// Arm the game's own plotter for `system`: clipboard loaded, the next
/// Target Next press pastes and plots. Errors when the Galaxy Map
/// controls are not taught, so callers can fall through to EDDA's planner.
pub fn plot_in_game(state: &AppState, system: String) -> Result<String, String> {
    if state.config.lock().unwrap_or_else(|e| e.into_inner()).map_points.as_ref().and_then(point_plot_macro).is_none() {
        return Err("teach all four Galaxy Map controls in Setup first".into());
    }
    set_clipboard(&system)?;
    *PENDING_GAME_PLOT.lock().unwrap_or_else(|e| e.into_inner()) = Some(system.clone());
    let message = format!("{system} is ready. Press Target Next System in Route to ask Elite to plot it.");
    // Spoken too (maintainer, 2026-09-05): the commander is in the cockpit
    // waiting on this cue, not reading the Route tab.
    state.voice.say(crate::commands::speakable(&message));
    Ok(message)
}

/// What a macro needs from the world besides a key sink: the commander's
/// binds, the game's GUI focus, and somewhere to put the system name.
/// The app implements it over `AppState`; tests implement it in memory.
pub trait MacroHost {
    fn binds(&self) -> Option<&ed_input::binds::Binds>;
    fn gui_focus(&mut self) -> Option<u8>;
    fn set_clipboard(&mut self, text: &str) -> Result<(), String>;
}

struct AppHost<'a> {
    state: &'a AppState,
    binds: Option<ed_input::binds::Binds>,
}

impl MacroHost for AppHost<'_> {
    fn binds(&self) -> Option<&ed_input::binds::Binds> {
        self.binds.as_ref()
    }
    fn gui_focus(&mut self) -> Option<u8> {
        gui_focus(self.state)
    }
    fn set_clipboard(&mut self, text: &str) -> Result<(), String> {
        set_clipboard(text)
    }
}

/// The command boundary: one macro at a time, only with the game focused,
/// through this host's native sink.
fn run_macro(
    state: &AppState,
    steps: &[MacroStep],
    system: &str,
    hop_ly: f32,
) -> Result<(), String> {
    let _guard = MACRO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let needs_game = steps.iter().any(|s| {
        !matches!(
            s,
            MacroStep::Clipboard
                | MacroStep::Wait { .. }
                | MacroStep::WaitPan { .. }
                | MacroStep::WaitGui { .. }
                | MacroStep::Mouse { .. }
        )
    });
    if needs_game && !ed_input::send::game_is_focused() {
        // Still leave the name on the clipboard: that part is always useful.
        if steps.iter().any(|s| matches!(s, MacroStep::Clipboard)) {
            let _ = set_clipboard(system);
        }
        return Err(format!(
            "the game window is not focused (foreground: {:?}); {system} is on the clipboard",
            ed_input::send::foreground_title()
        ));
    }
    let binds = ed_input::binds::Binds::default_dir()
        .and_then(|d| ed_input::binds::Binds::find_latest(&d))
        .and_then(|p| ed_input::binds::Binds::load(&p).ok());
    let mut host = AppHost { state, binds };
    let mut sink = ed_input::send::NativeSink::default();
    run_macro_with(&mut sink, &mut host, steps, system, hop_ly)
}

/// Run the steps against any sink and host. No focus check here: that is
/// policy, and it lives in [`run_macro`].
pub fn run_macro_with(
    sink: &mut dyn ed_input::send::KeySink,
    host: &mut dyn MacroHost,
    steps: &[MacroStep],
    system: &str,
    hop_ly: f32,
) -> Result<(), String> {
    // Item 35b: the app borrows the commander's cursor and always gives
    // it back — captured before the first mouse-driving step, restored
    // after the run whether it succeeded or failed partway. Macros that
    // never touch the mouse never touch the cursor either.
    let drives_mouse = steps.iter().any(|s| matches!(s, MacroStep::Mouse { .. } | MacroStep::Click { .. }));
    let saved_cursor = if drives_mouse { sink.cursor_pos() } else { None };
    let result = run_macro_steps(sink, host, steps, system, hop_ly);
    if let Some((x, y)) = saved_cursor {
        if !sink.restore_cursor(x, y) {
            tracing::warn!(x, y, "could not restore the commander's cursor");
        }
    }
    result
}

fn run_macro_steps(
    sink: &mut dyn ed_input::send::KeySink,
    host: &mut dyn MacroHost,
    steps: &[MacroStep],
    system: &str,
    hop_ly: f32,
) -> Result<(), String> {
    use ed_input::send::{press_chord, type_text, Timing};
    let t = Timing::default();
    let run_started = std::time::Instant::now();
    tracing::info!(steps = steps.len(), system, "macro run");
    for step in steps {
        tracing::debug!(
            at_ms = run_started.elapsed().as_millis() as u64,
            ?step,
            "macro step"
        );
        match step {
            MacroStep::Clipboard => {
                let _ = host.set_clipboard(system);
            }
            MacroStep::Paste => {
                let ctrl = ed_input::keys::scan_code("Key_LeftControl")
                    .ok_or("unknown key Key_LeftControl")?;
                let v = ed_input::keys::scan_code("Key_V").ok_or("unknown key Key_V")?;
                press_chord(sink, &[ctrl], v, t);
            }
            MacroStep::Mouse { x, y } => {
                if !sink.move_mouse(*x, *y) {
                    tracing::warn!("could not move the mouse in the game window");
                }
            }
            MacroStep::Click { x, y } => {
                if !sink.click(*x, *y) {
                    tracing::warn!("could not click in the game window");
                }
                sink.sleep(std::time::Duration::from_millis(150));
            }
            MacroStep::Wait { ms } => sink.sleep(std::time::Duration::from_millis(*ms)),
            MacroStep::WaitPan { base_ms, per_ly } => {
                let ms = *base_ms + (hop_ly.max(0.0) * per_ly) as u64;
                sink.sleep(std::time::Duration::from_millis(ms.min(8000)));
            }
            MacroStep::WaitGui { focus, timeout_ms } => {
                let started = std::time::Instant::now();
                loop {
                    let now = host.gui_focus();
                    if now == Some(*focus) {
                        break;
                    }
                    if started.elapsed().as_millis() as u64 > *timeout_ms {
                        // The numbers go to the trace; the commander gets what
                        // happened and what to do (maintainer, 2026-09-08: "the game's
                        // GUI focus" is nonsense to actual users).
                        tracing::warn!(
                            expected = focus,
                            actual = now.map(|v| v.to_string()).unwrap_or_else(|| "unknown".into()),
                            timeout_ms,
                            "galaxy-map macro: the expected screen did not open in time"
                        );
                        return Err(if *focus == GUI_GALAXY_MAP {
                            format!("The galaxy map didn't open, so nothing was typed. Check the Galaxy Map key in Settings → Route control. {system} is on your clipboard.")
                        } else {
                            format!("The expected screen didn't open in time, so nothing was typed. {system} is on your clipboard.")
                        });
                    }
                    sink.sleep(std::time::Duration::from_millis(150));
                }
            }
            MacroStep::Bind { action } => {
                let b = host.binds()
                    .ok_or("no Custom.binds found; set the macro to plain keys in Settings")?;
                let chord = b.chord(action).ok_or_else(|| {
                    format!("{action} has no keyboard binding in {}", b.path.display())
                })?;
                let (mods, key) = chord.scan_codes().ok_or_else(|| {
                    format!(
                        "{action} is bound to keys this app cannot press ({})",
                        chord.human()
                    )
                })?;
                press_chord(sink, &mods, key, t);
            }
            MacroStep::Key { key } => {
                let sc =
                    ed_input::keys::scan_code(key).ok_or_else(|| format!("unknown key {key}"))?;
                press_chord(sink, &[], sc, t);
            }
            MacroStep::Hold { action, ms } => {
                let b = host.binds()
                    .ok_or("no Custom.binds found; set the macro to plain keys in Settings")?;
                let chord = b.chord(action).ok_or_else(|| {
                    format!("{action} has no keyboard binding in {}", b.path.display())
                })?;
                let (mods, key) = chord.scan_codes().ok_or_else(|| {
                    format!(
                        "{action} is bound to keys this app cannot press ({})",
                        chord.human()
                    )
                })?;
                for m in &mods {
                    sink.key_down(*m);
                }
                sink.sleep(std::time::Duration::from_millis(t.modifier_lead));
                sink.key_down(key);
                sink.sleep(std::time::Duration::from_millis(*ms));
                sink.key_up(key);
                for m in mods.iter().rev() {
                    sink.key_up(*m);
                }
                sink.sleep(std::time::Duration::from_millis(t.gap));
            }
            MacroStep::Type { text } => {
                let text = text.replace("{system}", system);
                let skipped = type_text(sink, &text, t);
                if !skipped.is_empty() {
                    tracing::warn!(?skipped, "characters the macro could not type");
                }
            }
        }
    }
    tracing::info!(system, steps = steps.len(), "target-next macro sent");
    Ok(())
}

pub(crate) fn set_clipboard(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut c| c.set_text(text.to_string()))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn route_target_next(app: AppHandle) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || target_next(&app))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Debug, Serialize)]
pub struct MacroCheck {
    pub binds_file: Option<String>,
    pub game_focused: bool,
    /// One row per action/key the macro presses.
    pub steps: Vec<MacroStepCheck>,
    pub ok: bool,
    /// Related bindings worth knowing about (bound or not).
    pub related: Vec<MacroStepCheck>,
}

#[derive(Debug, Serialize)]
pub struct MacroStepCheck {
    pub action: String,
    pub bound: bool,
    pub chord: Option<String>,
    pub note: Option<String>,
}

/// Does the commander's Custom.binds have keyboard bindings for everything
/// the macro presses? Never assume; a missing one is flagged here and in
/// the error when the macro runs.
#[tauri::command]
pub async fn target_macro_check(state: State<'_, AppState>) -> Result<MacroCheck, String> {
    Ok({
        let steps = configured_macro(&state.config.lock().unwrap_or_else(|e| e.into_inner()));
        let path = ed_input::binds::Binds::default_dir()
            .and_then(|d| ed_input::binds::Binds::find_latest(&d));
        let binds = path
            .as_ref()
            .and_then(|p| ed_input::binds::Binds::load(p).ok());
        // Shift (or whatever UIFocus is) as a modifier is a trap: the game
        // treats the press as entering focus mode and never sees the chord.
        let ui_focus: Option<String> = binds
            .as_ref()
            .and_then(|b| b.chord("UIFocus"))
            .filter(|c| c.modifiers.is_empty())
            .map(|c| c.key.clone());
        let check_action = |action: &str| -> MacroStepCheck {
            match binds.as_ref().and_then(|b| b.chord(action)) {
                Some(c) => {
                    let focus_clash = ui_focus
                        .as_ref()
                        .is_some_and(|f| c.modifiers.iter().any(|m| m == f));
                    let pressable = c.scan_codes().is_some();
                    MacroStepCheck {
                        action: action.to_string(),
                        bound: pressable && !focus_clash,
                        chord: Some(c.human()),
                        note: if focus_clash {
                            Some(format!("its modifier {} is your UIFocus key: the game enters focus mode and ignores the chord. Rebind {action} to a plain key (F9 is free)", c.modifiers.join("+")))
                        } else if !pressable {
                            Some("bound to a key this app cannot press".into())
                        } else {
                            None
                        },
                    }
                }
                None => MacroStepCheck {
                    action: action.to_string(),
                    bound: false,
                    chord: None,
                    note: Some(
                        "no keyboard binding in Custom.binds -- bind one in the game's Controls"
                            .into(),
                    ),
                },
            }
        };
        let mut out = Vec::new();
        for s in &steps {
            match s {
                MacroStep::Bind { action } | MacroStep::Hold { action, .. } => {
                    out.push(check_action(action))
                }
                MacroStep::Key { key } => out.push(MacroStepCheck {
                    action: key.clone(),
                    bound: ed_input::keys::scan_code(key).is_some(),
                    chord: Some(key.clone()),
                    note: ed_input::keys::scan_code(key)
                        .is_none()
                        .then(|| "unknown key name".into()),
                }),
                _ => {}
            }
        }
        let related: Vec<MacroStepCheck> = [
            "GalaxyMapOpen",
            "GalaxyMapHome",
            "UI_Select",
            "UI_Back",
            "UIFocus",
            "TargetNextRouteSystem",
            "CycleNextPanel",
        ]
        .iter()
        .map(|a| check_action(a))
        .collect();
        let ok = binds.is_some() && out.iter().all(|s| s.bound);
        MacroCheck {
            binds_file: path.map(|p| p.display().to_string()),
            game_focused: ed_input::send::game_is_focused(),
            steps: out,
            ok,
            related,
        }
    })
}

/// Start capturing the commander's keystrokes (low-level hook) so the
/// macro can be taught by doing it once in the game.
#[tauri::command]
pub async fn macro_record_start() -> Result<(), String> {
    ed_input::record::start()
}

/// Stop capturing and turn the keystrokes into macro steps: chords become
/// binds when Custom.binds maps them to an action, the typed system name
/// becomes `{system}`, pauses over 150 ms become waits, and a wait for the
/// galaxy map is inserted after the key that opens it.
#[tauri::command]
pub async fn macro_record_stop(system: Option<String>) -> Result<Vec<MacroStep>, String> {
    let (events, clicks) = ed_input::record::stop_with_clicks();
    // Only what happened in the game window is the recipe: the Alt+Tab
    // there and back, and the click on this app's Stop button, are not.
    let events: Vec<ed_input::record::KeyEvent> = events.into_iter().filter(|e| e.game).collect();
    let clicks: Vec<ed_input::record::Click> = clicks.into_iter().filter(|c| c.game).collect();
    if events.is_empty() && clicks.is_empty() {
        return Err("nothing was recorded (was the game focused?)".into());
    }
    let binds = ed_input::binds::Binds::default_dir()
        .and_then(|d| ed_input::binds::Binds::find_latest(&d))
        .and_then(|p| ed_input::binds::Binds::load(&p).ok());
    // Reverse map: (modifier names, key name) -> action.
    let mut by_chord: std::collections::HashMap<(Vec<String>, String), String> =
        std::collections::HashMap::new();
    if let Some(b) = &binds {
        for (action, chords) in &b.actions {
            for c in chords {
                let mut mods: Vec<String> = c
                    .modifiers
                    .iter()
                    .map(|m| m.trim_start_matches("Key_").to_string())
                    .collect();
                mods.sort();
                by_chord
                    .entry((mods, c.key.trim_start_matches("Key_").to_string()))
                    .or_insert_with(|| action.clone());
            }
        }
    }
    let is_mod = |n: &str| {
        matches!(
            n,
            "LeftShift" | "RightShift" | "LeftControl" | "RightControl" | "LeftAlt" | "RightAlt"
        )
    };

    // Presses: a non-modifier key-down with whichever modifiers are held.
    let mut held: Vec<String> = Vec::new();
    let mut presses: Vec<(Vec<String>, String, u64)> = Vec::new(); // (mods, key, at_ms)
    for e in &events {
        let Some(name) = ed_input::record::key_name(e.sc) else {
            continue;
        };
        if is_mod(name) {
            if e.down {
                if !held.iter().any(|h| h == name) {
                    held.push(name.to_string());
                }
            } else {
                held.retain(|h| h != name);
            }
            continue;
        }
        if e.down {
            let mut mods = held.clone();
            mods.sort();
            // Window switching is never part of the recipe.
            let switching = (name == "Tab" && mods.iter().any(|m| m.ends_with("Alt")))
                || name.contains("Windows")
                || (name == "Escape" && mods.iter().any(|m| m.ends_with("Alt")));
            if switching {
                continue;
            }
            presses.push((mods, name.to_string(), e.at_ms));
        }
    }
    if presses.is_empty() && clicks.is_empty() {
        return Err("no key presses or clicks recorded".into());
    }

    // Typed text: runs of plain character keys are collapsed; if they spell
    // the system name (case-insensitive, spaces included) they become {system}.
    let char_of = |mods: &[String], key: &str| -> Option<char> {
        let shifted = mods.iter().any(|m| m.ends_with("Shift"));
        if mods.iter().any(|m| !m.ends_with("Shift")) {
            return None;
        }
        let c = match key {
            k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => {
                k.chars().next()?
            }
            "Space" => ' ',
            "Minus" => {
                if shifted {
                    '_'
                } else {
                    '-'
                }
            }
            "Period" => '.',
            "Apostrophe" => '\'',
            _ => return None,
        };
        Some(if shifted {
            c.to_ascii_uppercase()
        } else {
            c.to_ascii_lowercase()
        })
    };

    let mut steps: Vec<MacroStep> = vec![MacroStep::Clipboard];
    let mut last_ms = presses
        .first()
        .map(|p| p.2)
        .unwrap_or(0)
        .min(clicks.first().map(|c| c.at_ms).unwrap_or(u64::MAX));
    let mut i = 0;
    let mut ci = 0;
    let system_lower = system.as_deref().map(|s| s.to_lowercase());
    // Clicks are interleaved with the keys by time; each becomes a Click step.
    let emit_clicks_before =
        |until: u64, steps: &mut Vec<MacroStep>, last_ms: &mut u64, ci: &mut usize| {
            while *ci < clicks.len() && clicks[*ci].at_ms <= until {
                let c = clicks[*ci];
                if c.at_ms.saturating_sub(*last_ms) >= 150 {
                    steps.push(MacroStep::Wait {
                        ms: (c.at_ms - *last_ms).min(5000),
                    });
                }
                steps.push(MacroStep::Click {
                    x: (c.xf * 1000.0).round() / 1000.0,
                    y: (c.yf * 1000.0).round() / 1000.0,
                });
                *last_ms = c.at_ms;
                *ci += 1;
            }
        };
    while i < presses.len() {
        let (mods, key, at) = &presses[i];
        emit_clicks_before(*at, &mut steps, &mut last_ms, &mut ci);
        if at.saturating_sub(last_ms) >= 150 {
            steps.push(MacroStep::Wait {
                ms: (at - last_ms).min(5000),
            });
        }
        // Try to consume a typed run.
        if let Some(c0) = char_of(mods, key) {
            let mut text = String::new();
            let mut j = i;
            let mut end_ms = *at;
            while j < presses.len() {
                let (m, k, t) = &presses[j];
                match char_of(m, k) {
                    Some(c) if j == i || t.saturating_sub(end_ms) < 1500 => {
                        text.push(c);
                        end_ms = *t;
                        j += 1;
                    }
                    _ => break,
                }
            }
            let _ = c0;
            let is_name = system_lower
                .as_deref()
                .is_some_and(|s| !s.is_empty() && text.to_lowercase().trim() == s);
            // A lone character is more likely a keybind than typing.
            if text.len() > 1 || is_name {
                steps.push(MacroStep::Type {
                    text: if is_name { "{system}".into() } else { text },
                });
                last_ms = end_ms;
                i = j;
                continue;
            }
        }
        let full_key = format!("Key_{key}");
        if let Some(action) = by_chord.get(&(mods.clone(), key.clone())) {
            steps.push(MacroStep::Bind {
                action: action.clone(),
            });
            if action == "GalaxyMapOpen" {
                steps.push(MacroStep::WaitGui {
                    focus: GUI_GALAXY_MAP,
                    timeout_ms: 15000,
                });
            }
        } else if mods.is_empty() {
            steps.push(MacroStep::Key { key: full_key });
        } else {
            // A chord with no binding: press it raw as a type-less key sequence is not
            // expressible; record it as a bind step the check will flag.
            steps.push(MacroStep::Bind {
                action: format!("{}+{}", mods.join("+"), key),
            });
        }
        last_ms = *at;
        i += 1;
    }
    emit_clicks_before(u64::MAX, &mut steps, &mut last_ms, &mut ci);
    Ok(steps)
}

#[tauri::command]
pub async fn target_macro_enabled_set(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<bool, String> {
    let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    cfg.target_macro_enabled = enabled;
    cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    Ok(enabled)
}

#[tauri::command]
pub async fn target_macro_enabled_get(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .target_macro_enabled)
}

#[tauri::command]
pub async fn game_route_max_get(state: State<'_, AppState>) -> Result<u32, String> {
    Ok(state.config.lock().unwrap_or_else(|e| e.into_inner()).game_route_max_ly)
}

#[tauri::command]
pub async fn game_route_max_set(state: State<'_, AppState>, lightyears: u32) -> Result<u32, String> {
    let value = lightyears.min(20_000);
    let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    cfg.game_route_max_ly = value;
    cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    Ok(value)
}

#[tauri::command]
pub async fn target_macro_get(state: State<'_, AppState>) -> Result<Vec<MacroStep>, String> {
    Ok(configured_macro(&state.config.lock().unwrap_or_else(|e| e.into_inner())))
}

#[tauri::command]
pub async fn target_macro_set(
    state: State<'_, AppState>,
    steps: Option<Vec<MacroStep>>,
) -> Result<Vec<MacroStep>, String> {
    let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    cfg.target_macro = steps;
    let out = configured_macro(&cfg);
    cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Clear the game's plotted route: the same map walk to the destination
/// the game has (from NavRoute.json), with 7 downs -- the plot/clear
/// control. No-op when the game has no route.
pub fn clear_in_game(state: &AppState) -> Result<String, String> {
    let dest: Option<String> = state.with_read(|s| {
        ed_store::route::current(s.conn())
            .ok()
            .flatten()
            .and_then(|b| b.hops.last().map(|h| h.system.clone()))
    });
    let Some(dest) = dest else {
        return Ok("the game has no route plotted".into());
    };
    let steps = map_macro(7);
    if let Some(f @ (6 | 7)) = gui_focus(state) {
        if ed_input::send::game_is_focused() {
            run_macro(
                state,
                &close_open_map_steps(f),
            &dest,
            0.0,
        )?;
        }
    }
    run_macro(state, &steps, &dest, 0.0)?;
    Ok(format!("cleared the game's route to {dest}"))
}

#[tauri::command]
pub async fn route_clear_in_game(state: State<'_, AppState>) -> Result<String, String> {
    clear_in_game(&state)
}

#[cfg(test)]
mod macro_tests {
    use super::*;
    use ed_input::keys::scan_code;
    use ed_input::send::Recorder;

    /// A host whose galaxy map "opens" on the first GuiFocus poll and
    /// "closes" on the next, and whose clipboard is a string.
    struct FakeHost {
        binds: ed_input::binds::Binds,
        polls: u32,
        clipboard: Option<String>,
    }

    impl MacroHost for FakeHost {
        fn binds(&self) -> Option<&ed_input::binds::Binds> {
            Some(&self.binds)
        }
        fn gui_focus(&mut self) -> Option<u8> {
            self.polls += 1;
            Some(if self.polls == 1 { GUI_GALAXY_MAP } else { 0 })
        }
        fn set_clipboard(&mut self, text: &str) -> Result<(), String> {
            self.clipboard = Some(text.to_string());
            Ok(())
        }
    }

    fn binds() -> ed_input::binds::Binds {
        let mut xml = String::from(r#"<Root PresetName="Custom"><KeyboardLayout>en-US</KeyboardLayout>"#);
        for (action, key) in [
            ("GalaxyMapOpen", "Key_M"), ("CamZoomIn", "Key_Z"), ("UI_Up", "Key_W"), ("UI_Down", "Key_S"),
            ("UI_Select", "Key_Space"), ("UI_Right", "Key_D"), ("CamTranslateLeft", "Key_A"), ("CamTranslateRight", "Key_D"),
        ] {
            xml.push_str(&format!(r#"<{action}><Primary Device="Keyboard" Key="{key}" /></{action}>"#));
        }
        xml.push_str("</Root>");
        ed_input::binds::Binds::parse(&xml).unwrap()
    }

    fn down(name: &str) -> String {
        let sc = scan_code(name).unwrap();
        format!("down {:#04x}{}", sc.code, if sc.extended { "e" } else { "" })
    }

    #[test]
    fn default_map_macro_presses_expected_keys() {
        let mut host = FakeHost { binds: binds(), polls: 0, clipboard: None };
        let mut rec = Recorder::default();
        run_macro_with(&mut rec, &mut host, &default_macro(), "Sol", 100.0).unwrap();

        assert_eq!(host.clipboard.as_deref(), Some("Sol"));
        assert_eq!(host.polls, 2, "one poll per WaitGui");
        assert!(rec.events.contains(&"mouse 0.50,0.50".to_string()), "{:?}", rec.events);

        let downs: Vec<&String> = rec.events.iter().filter(|e| e.starts_with("down")).collect();
        let mut want = vec![
            down("Key_M"),            // GalaxyMapOpen
            down("Key_Z"),            // CamZoomIn held
            down("Key_W"),            // UI_Up
            down("Key_Space"),        // UI_Select: search box
            down("Key_LeftShift"), down("Key_S"), down("Key_O"), down("Key_L"), // "Sol"
            down("Key_DownArrow"),    // into the results
            down("Key_Space"),        // UI_Select: target
            down("Key_A"), down("Key_D"), // settle nudge
            down("Key_Space"),        // UI_Select: info panel
            down("Key_D"),            // UI_Right
        ];
        want.extend(std::iter::repeat_n(down("Key_S"), 8)); // 8 downs to the targeting control
        want.push(down("Key_Space"));
        want.push(down("Key_M"));     // close the map
        assert_eq!(downs, want.iter().collect::<Vec<_>>());
    }

    /// Item 46: fuel chatter is quiet exactly while the live re-sim
    /// covers the tank; the trap guard always speaks; going off plan
    /// turns silence into an explicit warning, never the reverse.
    #[test]
    fn fuel_chatter_is_quiet_within_bounds_and_loud_off_plan() {
        use super::{fuel_chatter, FuelChatter};
        assert!(matches!(fuel_chatter(true, false, "fuel", "Fuel low."), FuelChatter::Drop));
        assert!(matches!(fuel_chatter(true, false, "fuel", "Caution: Colonia is not scoopable and fuel is at 12 percent."), FuelChatter::Drop));
        assert!(matches!(fuel_chatter(true, false, "fuel", "Warning: fuel trap ahead."), FuelChatter::Keep), "the trap guard always speaks");
        assert!(matches!(fuel_chatter(true, false, "fuel", "Warning: N class star ahead, not scoopable. Fuel at 20 percent."), FuelChatter::Drop), "the maintainer's exact log line is covered chatter");
        assert!(matches!(fuel_chatter(false, false, "fuel", "Warning: N class star ahead, not scoopable. Fuel at 20 percent."), FuelChatter::Keep), "uncovered, it stands");
        assert!(matches!(fuel_chatter(true, false, "fuel", "Fuel tank full."), FuelChatter::Keep), "positives pass");
        assert!(matches!(fuel_chatter(false, false, "fuel", "Fuel low."), FuelChatter::Keep), "not covered, not off plan: normal warning stands");
        assert!(matches!(fuel_chatter(false, true, "fuel", "Fuel low."), FuelChatter::Reword(_)), "off plan speaks up");
        assert!(matches!(fuel_chatter(true, false, "route", "Fuel low."), FuelChatter::Keep), "only fuel-kind is touched");
    }

    /// Item 41: the press-to-target path stops map-thrash at three
    /// gates — a fresh hop always runs; a re-press inside the cooldown
    /// is acknowledged, not re-run; the third run for one hop falls
    /// back to the clipboard instead of fighting the map again.
    #[test]
    fn the_target_retry_gates_stop_the_map_thrash_loop() {
        use std::time::Duration as D;
        assert!(matches!(target_retry(false, D::from_secs(0), 5), TargetRetry::Fresh), "new hop always runs");
        assert!(matches!(target_retry(true, D::from_secs(3), 1), TargetRetry::Cooldown), "re-press at 3 s is the burst");
        assert!(matches!(target_retry(true, D::from_secs(30), 1), TargetRetry::Fresh), "a patient retry runs");
        assert!(matches!(target_retry(true, D::from_secs(30), 2), TargetRetry::Fallback), "third run: clipboard, not the map");
    }

    /// Item 35b: the app borrows the commander's cursor and always
    /// gives it back. A macro that drives the mouse captures the cursor
    /// before the first move and restores it after the run — including
    /// runs that FAIL partway. A macro with no mouse steps never
    /// touches it.
    #[test]
    fn a_mouse_driving_macro_restores_the_cursor_even_on_failure() {
        let mut host = FakeHost { binds: binds(), polls: 0, clipboard: None };
        let mut rec = Recorder { cursor: Some((123, 456)), ..Default::default() };
        run_macro_with(&mut rec, &mut host, &[MacroStep::Mouse { x: 0.5, y: 0.5 }], "Sol", 0.0).unwrap();
        assert_eq!(rec.events.last().unwrap(), "restore 123,456", "{:?}", rec.events);
        let mut rec = Recorder { cursor: Some((9, 9)), ..Default::default() };
        let steps = [MacroStep::Click { x: 0.1, y: 0.1 }, MacroStep::Key { key: "NoSuchKey".into() }];
        run_macro_with(&mut rec, &mut host, &steps, "Sol", 0.0).unwrap_err();
        assert_eq!(rec.events.last().unwrap(), "restore 9,9", "failure still restores: {:?}", rec.events);
        let mut rec = Recorder { cursor: Some((7, 7)), ..Default::default() };
        run_macro_with(&mut rec, &mut host, &[MacroStep::Wait { ms: 0 }], "Sol", 0.0).unwrap();
        assert!(rec.events.iter().all(|e| !e.starts_with("restore")), "no mouse, no touch: {:?}", rec.events);
    }

    /// Item 35a: the onboarding plot test cleans up after itself — but
    /// only when the game CONFIRMED the route, because the taught plot
    /// button is a toggle and a blind second press would plot instead
    /// of clear.
    #[test]
    fn the_plot_test_clears_only_a_confirmed_route() {
        let (clear, msg) = plot_test_outcome(true, "Diso");
        assert!(clear, "confirmed route gets cleared");
        assert!(msg.contains("cleared"), "{msg}");
        let (clear, msg) = plot_test_outcome(false, "Diso");
        assert!(!clear, "unconfirmed route is left for the commander to judge");
        assert!(msg.contains("Is the route plotted"), "{msg}");
    }

    #[test]
    fn a_macro_stops_when_the_map_does_not_open() {
        struct NeverOpens;
        impl MacroHost for NeverOpens {
            fn binds(&self) -> Option<&ed_input::binds::Binds> { None }
            fn gui_focus(&mut self) -> Option<u8> { Some(0) }
            fn set_clipboard(&mut self, _: &str) -> Result<(), String> { Ok(()) }
        }
        let mut rec = Recorder::default();
        let steps = [MacroStep::WaitGui { focus: GUI_GALAXY_MAP, timeout_ms: 1 }, MacroStep::Key { key: "Key_Enter".into() }];
        let err = run_macro_with(&mut rec, &mut NeverOpens, &steps, "Sol", 0.0).unwrap_err();
        assert!(err.contains("galaxy map didn't open"), "{err}");
        assert!(rec.events.is_empty(), "nothing typed after the guard fails: {:?}", rec.events);
    }
}

#[cfg(test)]
mod tests {
    /// The witchspace-restart hole (field, 2026-09-06): a jump flown
    /// while the app was down reaches the store but not the watcher, so
    /// the cursor must be reconciled at startup — silently, forward or
    /// to completion, and never touched when the commander is off-route.
    #[test]
    fn startup_reconcile_moves_the_cursor_to_the_real_system() {
        struct Null;
        impl crate::events::Emitter for Null {
            fn emit_value(&self, _: &'static str, _: serde_json::Value) {}
        }
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        let hop = |idx: u32, name: &str| {
            serde_json::json!({
                "idx": idx, "id64": idx + 1, "name": name, "pos": [0.0, 0.0, 0.0],
                "class": "unknown", "scoopable": false, "distance_ly": 10.0,
                "boosted": false, "total_ly": 10.0 * (idx + 1) as f32,
                "fuel_after": null, "refuel": false,
            })
        };
        let route: ed_galaxy::router::Route = serde_json::from_value(serde_json::json!({
            "range_ly": 30.0,
            "hops": [hop(0, "Alpha"), hop(1, "Beta"), hop(2, "Gamma"), hop(3, "Delta")],
            "jumps": 3, "total_ly": 90.0, "straight_ly": 80.0,
            "boosted_jumps": 0, "expansions": 0, "elapsed_ms": 0, "refuel_stops": 0,
        }))
        .unwrap();
        super::save_pub(&conn, &super::ActiveRoute { route, next: 1, source: "plot".into() }).unwrap();
        conn.execute(
            "INSERT INTO location (id, ts, system_name, docked) VALUES (1, '2026-09-06T14:00:00Z', 'Gamma', 0)",
            [],
        )
        .unwrap();
        super::reconcile(&conn, &Null);
        assert_eq!(super::load(&conn).unwrap().next, 3, "cursor lands past Gamma, targeting Delta");
        // Aligned already: a second pass changes nothing.
        super::reconcile(&conn, &Null);
        assert_eq!(super::load(&conn).unwrap().next, 3);
        // Off-route: cursor untouched, no replan launched from a boot.
        conn.execute("UPDATE location SET system_name = 'Nowhere' WHERE id = 1", []).unwrap();
        super::reconcile(&conn, &Null);
        assert_eq!(super::load(&conn).unwrap().next, 3, "off-route startup leaves the cursor alone");
        // At the final hop: the route completed while the app was away.
        conn.execute("UPDATE location SET system_name = 'Delta' WHERE id = 1", []).unwrap();
        super::reconcile(&conn, &Null);
        assert!(super::load(&conn).is_none(), "completed while away means cleared");
    }

    /// Maintainer, 2026-09-06: "mark that leg as complete when they enter
    /// witchspace when jumping to the target, not wait to arrival."
    /// The cursor moves at witchspace entry — 18 s earlier (p50, over
    /// 1,633 of his jumps) — but only for a hop the plan names, only
    /// once, and never for the destination, whose completion ceremony
    /// belongs to arrival where "Arrived at X" is actually true.
    #[test]
    fn witchspace_marks_the_leg_complete_before_arrival() {
        struct Null;
        impl crate::events::Emitter for Null {
            fn emit_value(&self, _: &'static str, _: serde_json::Value) {}
        }
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        let hop = |idx: u32, name: &str| {
            serde_json::json!({
                "idx": idx, "id64": idx + 1, "name": name, "pos": [0.0, 0.0, 0.0],
                "class": "unknown", "scoopable": false, "distance_ly": 10.0,
                "boosted": false, "total_ly": 10.0 * (idx + 1) as f32,
                "fuel_after": null, "refuel": false,
            })
        };
        let route: ed_galaxy::router::Route = serde_json::from_value(serde_json::json!({
            "range_ly": 30.0,
            "hops": [hop(0, "Alpha"), hop(1, "Beta"), hop(2, "Gamma"), hop(3, "Delta")],
            "jumps": 3, "total_ly": 90.0, "straight_ly": 80.0,
            "boosted_jumps": 0, "expansions": 0, "elapsed_ms": 0, "refuel_stops": 0,
        }))
        .unwrap();
        super::save_pub(&conn, &super::ActiveRoute { route, next: 1, source: "plot".into() }).unwrap();

        // Entering witchspace bound for Beta: the leg is done now.
        assert!(super::on_witchspace(&conn, &Null, "beta"), "case-insensitive");
        assert_eq!(super::load(&conn).unwrap().next, 2, "cursor past Beta");
        // The same tunnel seen twice (a journal re-read) advances once.
        assert!(!super::on_witchspace(&conn, &Null, "Beta"));
        assert_eq!(super::load(&conn).unwrap().next, 2);
        // Off the plan entirely: the cursor is not touched here -- the
        // warning fired at targeting and the re-plan belongs to arrival.
        assert!(!super::on_witchspace(&conn, &Null, "Nowhere"));
        assert_eq!(super::load(&conn).unwrap().next, 2);
        // Gamma advances; Delta is the destination and must NOT complete
        // in the tunnel -- the route survives for the arrival ceremony.
        assert!(super::on_witchspace(&conn, &Null, "Gamma"));
        assert_eq!(super::load(&conn).unwrap().next, 3);
        assert!(!super::on_witchspace(&conn, &Null, "Delta"), "the destination completes at arrival");
        assert!(super::load(&conn).is_some(), "route still live for the arrival line");
        assert_eq!(super::load(&conn).unwrap().next, 3);
    }

    /// Maintainer, 2026-09-06: "if on a route we could say they aren't on the
    /// route and we'll recalculate if they're following an edda route
    /// and not a game planned route." Said at TARGETING, once per
    /// system, and never when the game owns the plot.
    #[test]
    fn targeting_off_an_edda_route_warns_once_and_stays_quiet_on_game_routes() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        let target = |name: &str, addr: i64| {
            serde_json::json!({"event": "FSDTarget", "Name": name, "SystemAddress": addr})
        };
        // No EDDA route: the GAME owns the plot, so we say nothing.
        let mut warned = None;
        assert!(
            super::on_target_off_route(&conn, &target("Nowhere", 99), &mut warned).is_none(),
            "no followed route means no route to be off"
        );

        let hop = |idx: u32, name: &str| {
            serde_json::json!({
                "idx": idx, "id64": idx + 1, "name": name, "pos": [0.0, 0.0, 0.0],
                "class": "unknown", "scoopable": false, "distance_ly": 10.0,
                "boosted": false, "total_ly": 10.0, "fuel_after": null, "refuel": false,
            })
        };
        let route: ed_galaxy::router::Route = serde_json::from_value(serde_json::json!({
            "range_ly": 30.0,
            "hops": [hop(0, "Alpha"), hop(1, "Beta"), hop(2, "Gamma")],
            "jumps": 2, "total_ly": 20.0, "straight_ly": 20.0,
            "boosted_jumps": 0, "expansions": 0, "elapsed_ms": 0, "refuel_stops": 0,
        }))
        .unwrap();
        super::save_pub(&conn, &super::ActiveRoute { route, next: 1, source: "plot".into() }).unwrap();

        let text = super::on_target_off_route(&conn, &target("Nowhere", 99), &mut warned)
            .expect("off-plan target warns");
        assert!(text.contains("Nowhere is not on the route to Gamma"), "{text}");
        assert!(text.contains("re-plan"), "it promises the recalculation: {text}");
        // Same system re-targeted: one line, not one per event.
        assert!(super::on_target_off_route(&conn, &target("Nowhere", 99), &mut warned).is_none());
        // Any hop the plan names is on-route -- the next one, one
        // further along, and one already passed.
        for on_plan in ["Beta", "Gamma", "Alpha"] {
            let mut w = None;
            assert!(
                super::on_target_off_route(&conn, &target(on_plan, 1), &mut w).is_none(),
                "{on_plan} is on the plan"
            );
        }
    }

    /// Item 32: the SCO burn-down coach. A TooHeavy verdict on the next
    /// hop announces the burn once (not on every status tick); the
    /// armed -> Fine transition IS the stop cue ("that's the weight");
    /// a hop advance or a broken plan disarms silently; and Fine with
    /// nothing armed says nothing at all.
    #[test]
    fn the_burndown_coach_says_start_once_and_stop_on_fine() {
        super::burndown_reset();
        let heavy = super::PlanCheck::TooHeavy {
            hop: 5,
            name: "Wredguia AB-C d1".into(),
            over_by: 1.2,
            burn_t: 7.0,
        };
        // Fine with nothing armed: silence.
        assert!(super::burndown_tick(5, &super::PlanCheck::Fine).is_none());
        // TooHeavy on the next hop: one start callout, with the tonnage.
        let start = super::burndown_tick(5, &heavy).expect("start cue");
        assert_eq!(start.kind, "burndown");
        assert!(start.text.contains("7 tonnes"), "{}", start.text);
        assert!(super::burndown_tick(5, &heavy).is_none(), "no repeat while armed");
        // Burned enough: the plan reads Fine again -> the stop cue, once.
        let stop = super::burndown_tick(5, &super::PlanCheck::Fine).expect("stop cue");
        assert!(stop.text.contains("weight"), "{}", stop.text);
        assert!(super::burndown_tick(5, &super::PlanCheck::Fine).is_none(), "disarmed after stop");
        // A TooHeavy for a hop that is NOT next never arms.
        let far = super::PlanCheck::TooHeavy { hop: 9, name: "x".into(), over_by: 1.0, burn_t: 3.0 };
        assert!(super::burndown_tick(5, &far).is_none());
        // Broken disarms silently: no stop cue afterwards.
        super::burndown_tick(5, &heavy).expect("re-armed");
        assert!(super::burndown_tick(5, &super::PlanCheck::Broken { hop: 5, name: "x".into() }).is_none());
        assert!(super::burndown_tick(5, &super::PlanCheck::Fine).is_none(), "broken cleared the arm");
        // A hop advance disarms too.
        super::burndown_tick(5, &heavy).expect("armed again");
        super::burndown_reset();
        assert!(super::burndown_tick(6, &super::PlanCheck::Fine).is_none());
    }

    #[test]
    fn a_route_with_scoop_stops_is_refused_without_a_scoop_only() {
        assert!(scoop_refusal(0, Some(false)).is_none(), "no stops: nothing to scoop");
        assert!(scoop_refusal(6, Some(true)).is_none());
        assert!(scoop_refusal(6, None).is_none(), "no Loadout to judge by: do not refuse");
        let why = scoop_refusal(6, Some(false)).unwrap();
        assert!(why.starts_with("no fuel scoop fitted") && why.contains("6 scoop stops"), "{why}");
        assert!(scoop_refusal(1, Some(false)).unwrap().contains("1 scoop stop."));
    }

    use super::*;

    fn hop(name: &str, class: ed_galaxy::StarClass, boosted: bool, refuel: bool) -> Hop {
        Hop {
            idx: 0,
            id64: 0,
            name: name.into(),
            pos: [0.0; 3],
            class,
            scoopable: class.scoopable(),
            distance_ly: 50.0,
            boosted,
            injection: None,
            total_ly: 0.0,
            fuel_after: None,
            refuel,
            fuel_optional: false,
            synthesized: false,
            via_secondary: None,
        }
    }

    #[test]
    fn advance_text_counts_down_and_flags_scoops() {
        let route = Route {
        variants_run: 0,
        variants_finished: 0,
        ship_has_scoop: None,
        fsd_integrity: None,
        integrity_loss_per_boost: None,
        ship_has_afmu: None,
            range_ly: 50.0,
            hops: vec![
                hop("A", ed_galaxy::StarClass::G, false, false),
                hop("B", ed_galaxy::StarClass::Neutron, false, false),
                hop("C", ed_galaxy::StarClass::K, true, true),
                hop("D", ed_galaxy::StarClass::G, false, false),
            ],
            jumps: 3,
            total_ly: 0.0,
            straight_ly: 0.0,
            boosted_jumps: 1,
            expansions: 0,
            elapsed_ms: 0,
            refuel_stops: 1,
            injections: 0, secondary_boosts: 0,
            ship_id: None,
            ship: None,
        };
        let ar = ActiveRoute {
            route,
            next: 1,
            source: "test".into(),
        };
        assert_eq!(advance_text(&ar), "3 jumps left. Next: B, neutron star, supercharge there.", "item 40a: the negative fuel case is silent");
        let ar = ActiveRoute { next: 3, ..ar };
        assert_eq!(advance_text(&ar), "1 jump left. Next: D.");
        let ar = ActiveRoute { next: 4, ..ar };
        assert_eq!(advance_text(&ar), "Arrived at D. Route complete.");
    }

    /// The mass blind spot, pinned (maintainer field case 2026-09-05: 1,008 t
    /// of palladium shrank a 37.6 ly plan to a 24.6 ly ship while
    /// docked, and no jump event could ever fire). A plain hop beyond
    /// the laden range flags; a supercharged hop is allowed its
    /// departure star's multiplier; passed hops are nobody's problem.
    #[test]
    fn a_route_that_no_longer_fits_the_ship_names_its_worst_hop() {
        let boost = ed_galaxy::fuel::BoostProfile { neutron: 4.0, white_dwarf: 1.5 };
        let mut hops = vec![
            hop("Start", ed_galaxy::StarClass::G, false, false),
            hop("Mid", ed_galaxy::StarClass::Neutron, false, false),
            hop("Boosted", ed_galaxy::StarClass::K, true, false),
            hop("Far", ed_galaxy::StarClass::G, false, false),
        ];
        hops[1].distance_ly = 30.0; // plain hop, planned at 37.6
        hops[2].distance_ly = 90.0; // supercharged off the neutron: 4x allowance
        hops[3].distance_ly = 33.0;
        let route = Route {
            variants_run: 0, variants_finished: 0, ship_has_scoop: None,
            fsd_integrity: None, integrity_loss_per_boost: None, ship_has_afmu: None,
            range_ly: 37.6, hops, jumps: 3, total_ly: 0.0, straight_ly: 0.0,
            boosted_jumps: 1, expansions: 0, elapsed_ms: 0, refuel_stops: 0,
            injections: 0, secondary_boosts: 0, ship_id: None, ship: None,
        };
        let ar = ActiveRoute { route, next: 1, source: "plot".into() };
        // Empty hold: everything fits, boosted hop included (90 <= 37.6*4).
        assert!(infeasible_hop(&ar, 37.6, &boost).is_none());
        // Laden at 24.6: the 30 and 33 ly plain hops both fail; the worst
        // is named. The boosted 90 ly still fits (24.6*4 = 98.4).
        let (name, d) = infeasible_hop(&ar, 24.6, &boost).unwrap();
        assert_eq!((name.as_str(), d), ("Far", 33.0));
        // Truly overweight: even the boost allowance breaks.
        let (name, _) = infeasible_hop(&ar, 20.0, &boost).unwrap();
        assert_eq!(name, "Boosted", "90 > 20*4: the supercharged hop is now the worst");
        // Hops already flown never flag.
        let ar = ActiveRoute { next: 4, ..ar };
        assert!(infeasible_hop(&ar, 1.0, &boost).is_none());
    }

    #[test]
    fn the_default_macro_opens_the_map_and_types_the_name() {
        let m = default_macro();
        assert!(matches!(m[1], MacroStep::Bind { ref action } if action == "GalaxyMapOpen"));
        assert!(m
            .iter()
            .any(|s| matches!(s, MacroStep::Type { text } if text.contains("{system}"))));
    }

    #[test]
    fn final_arrival_removes_the_persisted_active_route() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE active_route (
                id INTEGER PRIMARY KEY, json TEXT NOT NULL, next INTEGER NOT NULL,
                source TEXT, updated TEXT
             );",
        )
        .unwrap();
        let route = Route {
        variants_run: 0,
        variants_finished: 0,
        ship_has_scoop: None,
        fsd_integrity: None,
        integrity_loss_per_boost: None,
        ship_has_afmu: None,
            range_ly: 50.0,
            hops: vec![
                hop("A", ed_galaxy::StarClass::G, false, false),
                hop("B", ed_galaxy::StarClass::K, false, false),
            ],
            jumps: 1,
            total_ly: 50.0,
            straight_ly: 50.0,
            boosted_jumps: 0,
            expansions: 0,
            elapsed_ms: 0,
            refuel_stops: 0,
            injections: 0, secondary_boosts: 0,
            ship_id: None,
            ship: None,
        };
        let mut ar = ActiveRoute {
            route,
            next: 1,
            source: "test".into(),
        };
        save(&conn, &ar).unwrap();
        assert!(persist_arrival(&conn, &mut ar, 1).unwrap());
        assert!(load(&conn).is_none());
    }
}
