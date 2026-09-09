//! Watches the journal folder, syncs the store, and tells the UI -- and the
//! voice.
//!
//! `notify` fires several events per journal write, so they are coalesced
//! with a short debounce. Each burst triggers one incremental sync -- which
//! reads only the bytes appended since the last checkpoint, not six whole
//! files -- then one `journal-changed` event for the frontend, then the
//! callout pass over exactly the events that sync added.
//!
//! The callout watermark starts at the newest event already in the store,
//! so history is never announced as though it were happening now.

use crate::callouts::{self, Callout, CalloutState};
use crate::state::{remember, AppState};
use ed_store::Store;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::Value;
use std::path::Path;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

/// Cap per sync so a burst (a whole session file appearing at once) cannot
/// turn into minutes of speech.
const MAX_EVENTS_PER_PASS: usize = 500;

/// A callout plus the journal line that produced it, so a persona can
/// rephrase it from the same facts. `None` for status-flag and synthetic
/// callouts.
pub type Sourced = (Callout, Option<Value>);

/// A quiet period also syncs: a sync can fail (the database busy under a
/// bulk import) with nothing to retry it. Cheap when there is nothing new.
const QUIET_SYNC: Duration = Duration::from_secs(5);
/// How often the loop looks at its cancellation token while idle.
const POLL: Duration = Duration::from_millis(500);

/// The journal-watcher job: blocking, on the supervisor's blocking pool.
/// Still holds an `AppHandle` for the callout pass, which reaches into
/// `follow` and `knowledge`; the sync-and-announce path itself goes
/// through the [`Announcer`].
pub fn run(token: CancellationToken, app: AppHandle, store: Arc<Mutex<Store>>, journal_dir: &Path) {
    let journal_dir = journal_dir.to_path_buf();
    {
        let (tx, rx) = channel();
        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "failed to create file watcher");
                return;
            }
        };

        if let Err(e) = watcher.watch(&journal_dir, RecursiveMode::NonRecursive) {
            tracing::warn!(error = %e, dir = ?journal_dir, "failed to watch journal directory");
            return;
        }

        // Callout context seeded from what the store already knows.
        let (mut watermark, mut cstate) = {
            let guard = store.lock().unwrap_or_else(|e| e.into_inner());
            (
                ed_store::session::last_event_key(guard.conn()).ok().flatten(),
                seed_state(guard.conn()),
            )
        };

        let mut idle = Duration::ZERO;
        // Noise rows older than the grace window are pruned once an hour
        // (see ed_store::maintenance); the first pass does it at startup.
        let mut last_prune: Option<std::time::Instant> = None;
        while !token.is_cancelled() {
            // File notifications drive syncs; a quiet period also syncs.
            let woke = match rx.recv_timeout(POLL) {
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break, // watcher dropped
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    idle += POLL;
                    idle >= QUIET_SYNC
                }
                Ok(_) => true,
            };
            if !woke {
                continue;
            }
            idle = Duration::ZERO;
            {
                {
                    // Drain the burst before doing any work.
                    while rx.recv_timeout(Duration::from_millis(400)).is_ok() {}

                    let synced = {
                        let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                        guard.sync()
                    };
                    match synced {
                        Ok(stats) => {
                            // Emit even for a companion-only change (Status.json,
                            // Cargo.json) so panels refresh fuel and cargo.
                            let _ = app.emit(crate::events::JOURNAL_CHANGED, stats.ingest.events_inserted);

                            if last_prune.is_none_or(|t| t.elapsed() >= Duration::from_secs(3600)) {
                                last_prune = Some(std::time::Instant::now());
                                let cutoff = (chrono::Utc::now() - chrono::Duration::hours(ed_store::maintenance::NOISE_GRACE_HOURS)).format("%Y-%m-%dT%H:%M:%SZ").to_string();
                                let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                                match ed_store::maintenance::prune_noise(guard.conn(), &cutoff) {
                                    Ok(0) => {}
                                    Ok(n) => tracing::info!(rows = n, %cutoff, "pruned noise events older than the grace window"),
                                    Err(e) => tracing::warn!(error = %e, "noise prune failed"),
                                }
                            }

                            let mut out: Vec<Sourced> = Vec::new();
                            {
                                let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                                let conn = guard.conn();
                                if stats.ingest.events_inserted > 0 {
                                    // The watermark is the last event we *announced*, which
                                    // can lag the store when a pass hits the cap; the next
                                    // pass simply continues from there.
                                    if let Ok(rows) = ed_store::session::events_after(
                                        conn,
                                        watermark.as_ref(),
                                        MAX_EVENTS_PER_PASS,
                                    ) {
                                        for (file, offset, raw) in rows {
                                            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                                                // Item 43: a real supercharge lights the HUD's
                                                // next-target box; the journal is the truth.
                                                if v.get("event").and_then(Value::as_str) == Some("JetConeBoost") {
                                                    let _ = app.emit(crate::events::SUPERCHARGE, ());
                                                }
                                                for mut c in callouts::from_event(&v, &mut cstate) {
                                                    // Catch-up is history, not news: an app booted
                                                    // mid-session replays the whole day's journal, and
                                                    // speaking it flooded the voice queue (dev flight
                                                    // 2026-09-05: 49 "Power conflict zone" callouts in
                                                    // 20 ms). Stale events reach the panels, never the
                                                    // speakers.
                                                    if c.speak && stale_for_speech(&c.ts) {
                                                        c.speak = false;
                                                    }
                                                    // A pickup is only useful with the running total: "3 -- you have 47 of 100".
                                                    if c.kind == "material" {
                                                        if let Some(symbol) = v.get("Name").and_then(Value::as_str) {
                                                            let have: Option<i64> = conn
                                                                .query_row("SELECT count FROM materials WHERE symbol = ?1 COLLATE NOCASE", [symbol], |r| r.get(0))
                                                                .ok();
                                                            if let Some(have) = have {
                                                                let cap = callouts::material_grade(symbol).map(|g| match g { 1 => 300, 2 => 250, 3 => 200, 4 => 150, _ => 100 }).unwrap_or(0);
                                                                let text = c.text.trim_end_matches('.').to_string();
                                                                c.text = if cap > 0 { format!("{text}. You have {have} of {cap}.") } else { format!("{text}. You have {have}.") };
                                                            }
                                                        }
                                                    }
                                                    out.push((c, Some(v.clone())));
                                                }
                                                // These need the store, so they live here
                                                // rather than in the pure rules.
                                                match v.get("event").and_then(Value::as_str) {
                                                    // The trade layer: arrival at a stop's pad speaks
                                                    // the briefing and re-targets the next system;
                                                    // departing replans at the true laden mass.
                                                    Some("Docked") => {
                                                        crate::trade_follow::on_docked(&app, conn, &v, &mut out);
                                                    }
                                                    // The carrier layer (Item 52 C): a followed
                                                    // carrier route advances on the journal.
                                                    Some("CarrierJumpRequest") | Some("CarrierJump") | Some("CarrierLocation") => {
                                                        crate::carrier_follow::on_event(&app, conn, &v, &mut out);
                                                    }
                                                    Some("Undocked") => {
                                                        crate::trade_follow::on_undocked(&app, conn);
                                                        // The general mass check (maintainer, 2026-09-05:
                                                        // "auto replan works for all scenarios"): a
                                                        // route made infeasible while DOCKED — cargo
                                                        // bought, modules swapped — produces no jump
                                                        // event, so the departure is the moment to
                                                        // notice. Trade routes replan themselves above.
                                                        if let Some(ar) = crate::follow::load(conn).filter(|ar| ar.source != "trade") {
                                                            if let Some((m, b, _, _)) = crate::routing::ship_fuel(conn) {
                                                                let range = f64::from(m.range_at(m.capacity));
                                                                if let Some((hop, d)) = crate::follow::infeasible_hop(&ar, range, &b) {
                                                                    tracing::info!(%hop, needed = d, range, "followed route no longer fits the ship; replanning");
                                                                    let state = app.state::<AppState>();
                                                                    state.voice.say(format!(
                                                                        "This plan doesn't fit the ship any more — {hop} needs {d:.0} light years and you can make {range:.0}. Replotting."
                                                                    ));
                                                                    let app2 = app.clone();
                                                                    tauri::async_runtime::spawn(async move {
                                                                        if let Err(error) = crate::follow::replan_now(app2).await {
                                                                            tracing::warn!(%error, "mass replan failed");
                                                                        }
                                                                    });
                                                                }
                                                            }
                                                        }
                                                    }
                                                    Some("Shutdown") => {
                                                        if let Some(c) = session_summary(conn) {
                                                            out.push((c, None));
                                                        }
                                                    }
                                                    // A targeted jump into a system the ship could
                                                    // neither refuel in nor escape from deserves a
                                                    // warning before the commander commits; the
                                                    // threshold-based caution is redundant under it.
                                                    Some("FSDTarget") => {
                                                        if let Some(c) = crate::trap::on_target(&app, conn, &v, &mut cstate.trap_warned_target) {
                                                            out.retain(|(c0, _)| !(c0.kind == "fuel" && c0.text.starts_with("Caution:")));
                                                            out.push((c, Some(v.clone())));
                                                        }
                                                        // The other half of the mass check: the game
                                                        // refuses an over-range jump only on the HUD —
                                                        // the journal never records the refusal — but
                                                        // FSDTarget + our fuel model make it computable
                                                        // the moment of targeting. Conservative: only
                                                        // a target beyond even a max-boosted laden
                                                        // full-tank jump triggers, so supercharge plans
                                                        // never false-alarm.
                                                        crate::follow::on_target_beyond_range(&app, conn, &v, &mut cstate.replanned_target);
                                                        // Off the EDDA plan: say it at TARGETING,
                                                        // while changing your mind is still free.
                                                        // Silent when the game owns the plot.
                                                        if let Some(text) = crate::follow::on_target_off_route(conn, &v, &mut cstate.off_route_target) {
                                                            out.push((Callout::new("route", v.get("timestamp").and_then(Value::as_str).unwrap_or(""), 1, true, text), Some(v.clone())));
                                                        }
                                                    }
                                                    // Witchspace: the leg is flown, so the cursor
                                                    // moves now rather than 18 s later at arrival.
                                                    // Cursor and HUD only -- the spoken line stays
                                                    // on arrival, so a jump is one utterance.
                                                    Some("StartJump")
                                                        if v.get("JumpType").and_then(Value::as_str) == Some("Hyperspace") =>
                                                    {
                                                        if let Some(target) = v.get("StarSystem").and_then(Value::as_str) {
                                                            crate::follow::on_witchspace(conn, app.state::<AppState>().events.as_ref(), target);
                                                        }
                                                    }
                                                    // The tank refilled on a route: say where it goes, not "let's go somewhere".
                                                    Some("FuelScoop") => {
                                                        if let Some(ar) = crate::follow::load(conn) {
                                                            for (c, _) in out.iter_mut() {
                                                                if c.kind == "fuel" && c.text.starts_with("Fuel tank full") {
                                                                    let left = ar.route.hops.len().saturating_sub(ar.next);
                                                                    let dest = ar.route.hops.last().map(|h| h.name.as_str()).unwrap_or("destination");
                                                                    c.text = format!("Tank full. {} {left} jump{} to {dest}.", crate::follow::advance_text(&ar).trim_end_matches('.'), if left == 1 { "" } else { "s" });
                                                                    c.text = c.text.replace(&format!("{left} jumps left. "), "");
                                                                }
                                                            }
                                                        }
                                                    }
                                                    // First-hand star classes: a scanned primary star fills a gap in the index.
                                                    Some("Scan") => {
                                                        if let (Some(t), Some(name), Some(addr)) = (v.get("StarType").and_then(Value::as_str), v.get("StarSystem").and_then(Value::as_str), v.get("SystemAddress").and_then(Value::as_u64)) {
                                                            if v.get("DistanceFromArrivalLS").and_then(Value::as_f64).unwrap_or(1e9) < 1.0 {
                                                                crate::spansh::learn_star_conn(&app.state::<AppState>(), conn, addr, name, t, "journal");
                                                            }
                                                        }
                                                    }
                                                    Some("Bounty") | Some("FactionKillBond") => {
                                                        out.extend(
                                                            mission_progress(conn, &v)
                                                                .into_iter()
                                                                .map(|c| (c, None)),
                                                        );
                                                    }
                                                    // A plotted route gets a briefing; each arrival
                                                    // gets the next star and the one after it.
                                                    Some("NavRoute") if !crate::follow::map_setup_testing() => {
                                                        let following = crate::follow::load(conn);
                                                        let targeting_hop = |b: &ed_store::route::RouteBrief| following.as_ref().is_some_and(|ar| b.hops.last().is_some_and(|l| ar.route.hops.iter().any(|h| h.name.eq_ignore_ascii_case(&l.system))));
                                                        if let Ok(Some(b)) = ed_store::route::current(conn).map(|o| o.filter(|b| !targeting_hop(b))) {
                                                            out.push((
                                                                Callout {
                                                                    kind: "route",
                                                                    text: ed_store::route::brief_text(&b, narration(conn)),
                                                                    priority: 1,
                                                                    speak: true,
                                                                    ts: v.get("timestamp").and_then(Value::as_str).unwrap_or("").to_string(),
                                                                },
                                                                None,
                                                            ));
                                                        }
                                                    }
                                                    Some("FSDJump") => {
                                                        let here = v.get("StarSystem").and_then(Value::as_str).unwrap_or("");
                                                        let fuel_now = v.get("FuelLevel").and_then(Value::as_f64).map(|f| f as f32);
                                                        // Arriving on a followed route: say whether this is a fuel stop.
                                                        if let Some(ar) = crate::follow::load(conn) {
                                                            if let Some(i) = ar.route.hops.iter().position(|h| h.name.eq_ignore_ascii_case(here)) {
                                                                let h = &ar.route.hops[i];
                                                                let boost_here = ar.route.hops.get(i + 1).is_some_and(|n| n.boosted);
                                                                if h.fuel_after.is_some() {
                                                                    for (c, _) in out.iter_mut() {
                                                                        if c.kind == "arrival" {
                                                                            if boost_here {
                                                                                c.text.push_str(" Supercharge here.");
                                                                            }
                                                                            // Only fuel stops get announced; "not a fuel
                                                                            // stop" on every arrival was steady-state noise
                                                                            // now that the trap guard watches the tank.
                                                                            if h.refuel {
                                                                                c.text.push_str(" Fuel stop, scoop here.");
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                                // A neutron fuel stop: say which companion, and how far.
                                                                if h.refuel && !h.scoopable {
                                                                    crate::knowledge::announce_companion(app.clone(), here.to_string());
                                                                }
                                                            }
                                                        }
                                                        // A route we are following: move the cursor and say what's
                                                        // next -- inside the arrival callout, so a jump is one
                                                        // utterance, not two.
                                                        let follow_line = crate::follow::on_jump(&app, conn, here, fuel_now)
                                                            // The game-plotter trade path has no EDDA route to
                                                            // complete: arriving at the stop's system gets the
                                                            // same terminal-guidance line either way — one
                                                            // behaviour for the commander, whichever router
                                                            // flew the leg (maintainer, 2026-09-05).
                                                            .or_else(|| crate::trade_follow::arrival_guidance(conn, here));
                                                        if let Some(text) = follow_line {
                                                            let arrival = out.iter_mut().rev().map(|(c, _)| c).find(|c| c.kind == "arrival");
                                                            if let Some(text) = callouts::merge_follow_into_arrival(arrival, text, here) {
                                                                out.push((
                                                                    Callout {
                                                                        kind: "follow",
                                                                        text,
                                                                        priority: 1,
                                                                        speak: true,
                                                                        ts: v.get("timestamp").and_then(Value::as_str).unwrap_or("").to_string(),
                                                                    },
                                                                    None,
                                                                ));
                                                            }
                                                        }
                                                        // While following an app route the follow callout already said what's next.
                                                        let following = crate::follow::load(conn).is_some();
                                                        if let Ok(Some(b)) = ed_store::route::current(conn).map(|o| o.filter(|_| !following)) {
                                                            if let Some(t) = ed_store::route::next_hop_text(&b, here, narration(conn)) {
                                                                out.push((
                                                                    Callout {
                                                                        kind: "route",
                                                                        text: t,
                                                                        priority: 0,
                                                                        speak: true,
                                                                        ts: v.get("timestamp").and_then(Value::as_str).unwrap_or("").to_string(),
                                                                    },
                                                                    None,
                                                                ));
                                                            }
                                                        }
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            watermark = Some((file, offset));
                                        }
                                    }
                                    // The pass's watched signals, spoken as one counted
                                    // line per kind ("5 power conflict zones on
                                    // sensors."); the freshness gate applies the same
                                    // as everywhere — a replayed backlog is shown
                                    // silently, whatever its count.
                                    for mut c in callouts::flush_signals(&mut cstate) {
                                        if c.speak && stale_for_speech(&c.ts) {
                                            c.speak = false;
                                        }
                                        out.push((c, None));
                                    }
                                }
                                if stats.ingest.snapshots_updated > 0 {
                                    if let Ok(Some(raw)) =
                                        ed_store::session::snapshot_raw(conn, "Status.json")
                                    {
                                        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                                            let mut fuel_calls = callouts::from_status(&v, &mut cstate);
                                            // "Fuel low" on a followed route only when the plan does not cover it.
                                            if let Some(ar) = crate::follow::load(conn) {
                                                let fuel_now = v.pointer("/Fuel/FuelMain").and_then(Value::as_f64).map(|f| f as f32);
                                                if let Some(f) = fuel_now {
                                                    let at = ar.next.saturating_sub(1);
                                                    let check = crate::follow::check_plan(conn, &ar, at, f);
                                                    // Item 46 owns fuel-chatter suppression in
                                                    // follow::filter_callouts now; this block only
                                                    // drives the burn-down coach.
                                                    // Item 32: the SCO burn-down coach — "burn N
                                                    // tonnes, then jump", and the stop cue when
                                                    // the plan reads Fine again.
                                                    if let Some(c) = crate::follow::burndown_tick(ar.next, &check) {
                                                        fuel_calls.push(c);
                                                    }
                                                }
                                            }
                                            out.extend(fuel_calls.into_iter().map(|c| (c, None)));
                                            // Overcharge burning toward a strand: warn while
                                            // aborting still leaves the fuel to escape.
                                            if let Some(c) = crate::trap::on_status(&app, conn, &v, &mut cstate) {
                                                out.push((c, None));
                                            }
                                        }
                                    }
                                }
                                crate::follow::filter_callouts(conn, &mut out, cstate.fuel_main.map(|f| f as f32));
                            }
                            deliver(&app, out);
                        }
                        Err(e) => tracing::warn!(error = %e, "journal sync failed"),
                    }
                }
            }
        }
    }
    tracing::info!("journal watcher stopped");
}

/// How old a journal event may be and still be worth SAYING. Journal
/// timestamps are the game's own clock (UTC); three minutes covers an
/// app restart without announcing an afternoon of history.
const SPEECH_FRESHNESS_S: i64 = 180;

/// True when the event's own timestamp is too old to speak. An
/// unparseable timestamp counts as fresh — synthetic callouts pass
/// `ts: ""` and must keep speaking.
/// How much the SPOKEN game-route layer should say right now.
///
/// A running trade route means another owner already holds the two facts
/// this layer would otherwise guess at: the router (the game's plotter,
/// or EDDA's under source "trade") owns fuel for the leg, and the trade
/// follower names the actual pad for terminal guidance. Maintainer, 2026-09-06,
/// mid-run: "'One jump is not scoopable' and 'docking at' is
/// confusing/needless in a trade route follow -- the game or we will
/// calculate for fuel." The Route tab and the model's `current_route`
/// tool still read the full briefing; this is the voice only.
fn narration(conn: &rusqlite::Connection) -> ed_store::route::Narration {
    if crate::trade_follow::load(conn).is_some() {
        ed_store::route::Narration::Leg
    } else {
        ed_store::route::Narration::Full
    }
}

fn stale_for_speech(ts: &str) -> bool {
    let Ok(event_at) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return false;
    };
    (chrono::Utc::now() - event_at.with_timezone(&chrono::Utc)).num_seconds() > SPEECH_FRESHNESS_S
}

/// Push callouts to every window, remember them, and speak the ones that
/// asked to be spoken -- in the selected persona's words.
pub fn deliver(app: &AppHandle, callouts: Vec<Sourced>) {
    app.state::<AppState>().announcer().deliver(callouts);
}

/// Everything callout delivery needs, without an `AppHandle`: cloneable
/// into a background job, and under test its events land in a
/// [`Recording`](crate::events::Recording).
#[derive(Clone)]
pub struct Announcer {
    pub config: Arc<Mutex<crate::state::AppConfig>>,
    pub callouts: Arc<Mutex<std::collections::VecDeque<Callout>>>,
    pub voice: Arc<crate::voice::VoiceHandle>,
    pub events: Arc<crate::events::EventBus>,
}

impl Announcer {
    pub fn deliver(&self, callouts: Vec<Sourced>) {
        use crate::events::EmitExt as _;
        if callouts.is_empty() {
            return;
        }
        let (persona, muted) = {
            let cfg = self.config.lock().unwrap_or_else(|e| e.into_inner());
            (crate::persona::by_id(cfg.persona.as_deref().unwrap_or("standard")), cfg.callouts_off.clone())
        };
        for (mut c, event) in callouts {
            if muted.iter().any(|k| k == c.kind) {
                tracing::info!(kind = c.kind, text = %c.text, "callout (muted)");
                continue;
            }
            if let Some(t) = crate::persona::restyle(persona, c.kind, event.as_ref(), &c.text) {
                c.text = t;
            }
            tracing::info!(kind = c.kind, priority = c.priority, speak = c.speak, text = %c.text, "callout");
            remember(&self.callouts, &c);
            self.events.emit(crate::events::CALLOUT, &c);
            if c.speak {
                self.voice.say(crate::commands::speakable(&c.text));
            }
        }
    }
}

/// "Session over: 14 kills, 3.2 million credits, 410 merits." Spoken on
/// `Shutdown`, summed from the store since the last `LoadGame`.
fn session_summary(conn: &rusqlite::Connection) -> Option<Callout> {
    let since = ed_store::session::session_start(conn).ok().flatten()?;
    let combat = ed_store::query::combat_summary(conn, Some(&since)).ok()?;
    let (merits, _) = ed_store::session::merits_since(conn, &since).unwrap_or((0, 0));
    let credits = combat.bounty_credits + combat.bond_credits;
    let mut parts = vec![format!(
        "Session over: {} kill{}",
        combat.kills,
        if combat.kills == 1 { "" } else { "s" }
    )];
    if credits > 0 {
        parts.push(callouts::spoken_credits(credits));
    }
    if merits > 0 {
        parts.push(format!("{merits} merits"));
    }
    if combat.deaths > 0 {
        parts.push(format!("{} death{}", combat.deaths, if combat.deaths == 1 { "" } else { "s" }));
    }
    Some(Callout {
        kind: "session",
        text: format!("{}.", parts.join(", ")),
        priority: 1,
        speak: true,
        ts: since,
    })
}

/// "Mission progress: 3 of 5 Kulkan Lung Blue Ring kills." after a kill
/// that counts toward an active massacre, or "Target down" for an
/// assassination. Reads the derived mission state, so it lives here.
fn mission_progress(conn: &rusqlite::Connection, kill: &Value) -> Vec<Callout> {
    let ts = kill.get("timestamp").and_then(Value::as_str).unwrap_or("");
    let victim = kill.get("VictimFaction").and_then(Value::as_str);
    let pilot = kill
        .get("PilotName_Localised")
        .or_else(|| kill.get("PilotName"))
        .and_then(Value::as_str);
    let now = crate::commands::now_iso();
    let Ok(active) = ed_store::missions::active(conn, &now) else { return Vec::new() };

    let mut out = Vec::new();
    let mut seen_faction = false;
    for m in active {
        let counts =
            m.kill_count.is_some() && victim.is_some() && m.target_faction.as_deref() == victim;
        let is_target = m.kind == "assassinate"
            && m.target.as_deref().zip(pilot).is_some_and(|(t, p)| t.eq_ignore_ascii_case(p));
        if is_target {
            out.push(Callout {
                kind: "mission",
                text: format!(
                    "Target down: {}. {}",
                    m.target.as_deref().unwrap_or("target"),
                    m.destination_station
                        .as_deref()
                        .map(|s| format!("Return to {s}."))
                        .unwrap_or_default()
                ),
                priority: 1,
                speak: true,
                ts: ts.to_string(),
            });
        } else if counts && !seen_faction {
            // Several massacres against the same faction stack; say it once.
            seen_faction = true;
            let total = m.kill_count.unwrap_or(0);
            let done = m.kills_done.min(total);
            let faction = m.target_faction.as_deref().unwrap_or("target");
            let text = if done >= total {
                format!(
                    "Mission complete: {total} {faction} kills. Return to {}.",
                    m.destination_station.as_deref().unwrap_or("the hand-in")
                )
            } else {
                format!("Mission progress: {done} of {total} {faction} kills.")
            };
            out.push(Callout {
                kind: "mission",
                text,
                priority: if done >= total { 1 } else { 0 },
                speak: true,
                ts: ts.to_string(),
            });
        }
    }
    out
}

/// Fuel capacity, commander and ship from the store so the first callouts
/// of a session have context.
fn seed_state(conn: &rusqlite::Connection) -> CalloutState {
    let mut st = CalloutState::default();
    st.commander = ed_store::session::commander_name(conn).ok().flatten();
    st.pledged = ed_store::session::latest_event_raw(conn, "Powerplay")
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("Power").and_then(Value::as_str).map(str::to_string));
    if let Ok(Some(raw)) = ed_store::session::latest_event_raw(conn, "Loadout") {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            st.fuel_capacity = v
                .get("FuelCapacity")
                .and_then(|c| c.get("Main"))
                .and_then(Value::as_f64);
            st.ship = v
                .get("Ship_Localised")
                .or_else(|| v.get("Ship"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
    }
    if let Ok(Some(raw)) = ed_store::session::snapshot_raw(conn, "Status.json") {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            // Prime the edge detectors without announcing anything.
            let _ = callouts::from_status(&v, &mut st);
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::stale_for_speech;

    /// The replay-storm gate (dev flight 2026-09-05): history is shown,
    /// never spoken; live and synthetic callouts keep their voice.
    #[test]
    fn only_fresh_events_are_spoken() {
        assert!(stale_for_speech("2020-01-01T00:00:00Z"), "hours-old history is silent");
        assert!(!stale_for_speech("2999-01-01T00:00:00Z"), "a clock skewed forward still speaks");
        assert!(!stale_for_speech(""), "synthetic callouts carry no ts and must speak");
        assert!(!stale_for_speech("not a timestamp"), "unparseable means fresh, never mute");
    }
}
