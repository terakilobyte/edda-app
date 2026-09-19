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
                                        // A completion is announced once per PASS, not per
                                        // event: a concurrent stack redirects several at the
                                        // same second, and a kill and its redirect usually
                                        // land together (31 of 39 within 2 s).
                                        let mut pass_events: Vec<Value> = Vec::new();
                                        for (mark, raw) in rows {
                                            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                                                pass_events.push(v.clone());
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
                                                    // The carrier changed in a way Frontier will report:
                                                    // refresh the live figures if the commander is linked.
                                                    Some(ev @ ("CarrierStats" | "CarrierBuy" | "CarrierTradeOrder" | "CarrierDepositFuel"))
                                                        if !stale_for_speech(v.get("timestamp").and_then(Value::as_str).unwrap_or("")) =>
                                                    {
                                                        crate::capi::on_carrier_event(&app, ev);
                                                    }
                                                    Some("Bounty") | Some("FactionKillBond") => {
                                                        out.extend(
                                                            mission_progress(conn, &crate::commands::now_iso(), &v)
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
                                            watermark = Some(mark);
                                        }
                                        // One completion line for the pass, from the game's
                                        // own signal rather than from our kill count — and it
                                        // REPLACES the pass's progress line. The store is
                                        // synced before this loop, so by the time the crossing
                                        // kill is handled its mission is already ready and the
                                        // progress line names the NEXT nearest one: without
                                        // this the commander hears "30 of 48" immediately
                                        // before "complete: 30 kills", two numbers about two
                                        // different missions. Same shape as the fuel caution
                                        // dropped by a trap warning above.
                                        if let Some(c) = mission_redirected(conn, &crate::commands::now_iso(), &pass_events) {
                                            supersede_progress(&mut out, c);
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
/// What a kill says. Progress only, and only for a mission still ACTIVE.
///
/// Completion is NOT spoken here. It comes from `mission_redirected`,
/// because the game states it outright and our own count does not agree
/// with the game: replaying the maintainer's journal from 2026-09-09,
/// of 42 missions only ONE matched `MissionRedirected` to the second —
/// 18 were inferred early (every one a same-giver duplicate, which the
/// game queues rather than running concurrently) and 19 redirected
/// without ever being inferred.
///
/// Speaking completion from kills also made the app lie. `active()`
/// includes ReadyToTurnIn, and this function used to report the FIRST
/// matching mission, so once anything was finished every later kill
/// re-announced it: 88 repeats of a completion that had happened, and
/// 361 utterances across two overnight sessions announcing a stack that
/// had been finished for hours. Restricting progress to Active missions
/// is what silences those.
fn mission_progress(conn: &rusqlite::Connection, now: &str, kill: &Value) -> Vec<Callout> {
    use ed_store::missions::MissionStatus;
    let ts = kill.get("timestamp").and_then(Value::as_str).unwrap_or("");
    let victim = kill.get("VictimFaction").and_then(Value::as_str);
    let pilot = kill
        .get("PilotName_Localised")
        .or_else(|| kill.get("PilotName"))
        .and_then(Value::as_str);
    let Ok(active) = ed_store::missions::active(conn, now) else { return Vec::new() };

    let mut out = Vec::new();
    // An assassination target dying is its own line and is not a count.
    for m in active.iter().filter(|m| m.status == MissionStatus::Active) {
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
        }
    }

    // One progress line per kill, for the mission NEAREST to completing.
    // On a concurrent stack every credited mission has the same count, so
    // that is simply the smallest target still active — the number a
    // commander is actually watching.
    let nearest = active
        .iter()
        .filter(|m| m.status == MissionStatus::Active)
        .filter(|m| m.kill_count.is_some() && victim.is_some() && m.target_faction.as_deref() == victim)
        .min_by_key(|m| ed_store::missions::kills_remaining(m).unwrap_or(i64::MAX));
    if let Some(m) = nearest {
        let total = m.kill_count.unwrap_or(0);
        let done = m.kills_done.min(total);
        let faction = m.target_faction.as_deref().unwrap_or("target");
        out.push(Callout {
            kind: "mission",
            text: format!("Mission progress: {done} of {total} {faction} kills."),
            priority: 0,
            speak: true,
            ts: ts.to_string(),
        });
    }
    out
}

/// What the game's own `MissionRedirected` says: this mission's objective
/// is met, go and hand it in. One callout for the whole pass, because a
/// concurrent stack finishes several at once — measured on the
/// maintainer's journal, 39 redirects arrived as 36 singles and one
/// triple, so the plural is real but rare.
///
/// Carries its own callout kind so it can be silenced separately from
/// the progress line, which fires every 80 seconds through an evening
/// against 23 completions in a week.
fn mission_redirected(conn: &rusqlite::Connection, now: &str, events: &[Value]) -> Option<Callout> {
    let redirects: Vec<&Value> = events
        .iter()
        .filter(|v| v.get("event").and_then(Value::as_str) == Some("MissionRedirected"))
        .collect();
    let first = redirects.first()?;
    let ts = first.get("timestamp").and_then(Value::as_str).unwrap_or("");
    let known = ed_store::missions::active(conn, now).unwrap_or_default();

    // The store knows the giver and the kill count; the event knows the
    // localised name and where to take it. Prefer ours, fall back to the
    // game's wording for missions that are not massacres.
    let described: Vec<(String, String)> = redirects
        .iter()
        .map(|v| {
            let id = v.get("MissionID").and_then(Value::as_i64);
            let stored = id.and_then(|id| known.iter().find(|m| m.id == id));
            let what = match stored.and_then(|m| m.kill_count.map(|k| (m, k))) {
                // Faction names carry their own full stop ("HIP 90112
                // Jet Central Corp."); do not add a second one.
                Some((m, kills)) => format!(
                    "{kills} {} kills for {}",
                    m.target_faction.as_deref().unwrap_or("target"),
                    m.faction.trim_end_matches('.')
                ),
                None => v
                    .get("LocalisedName")
                    .or_else(|| v.get("Name"))
                    .and_then(Value::as_str)
                    .unwrap_or("mission")
                    .to_string(),
            };
            (what, destination_of(v))
        })
        .collect();

    // Several missions finishing together do NOT necessarily share a
    // hand-in: measured on the maintainer's journal, one of his two
    // plural passes held two redirects with different destinations, and
    // taking the first mission's destination for the whole line sent him
    // to the wrong station (ids 1066168667 and 1066169080, 17:33Z).
    let mut destinations: Vec<&str> = described.iter().map(|(_, d)| d.as_str()).collect();
    destinations.sort_unstable();
    destinations.dedup();
    let one_destination = destinations.len() == 1;

    let text = match (described.len(), one_destination) {
        (1, _) => {
            let (what, dest) = &described[0];
            format!("Mission complete: {what}.{}", trailer(dest))
        }
        (n, true) => {
            let list: Vec<&str> = described.iter().map(|(w, _)| w.as_str()).collect();
            format!("{n} missions complete: {}.{}", list.join("; "), trailer(&described[0].1))
        }
        (n, false) => {
            let each: Vec<String> = described
                .iter()
                .map(|(what, dest)| match dest.is_empty() {
                    true => what.clone(),
                    false => format!("{what}, return to {dest}"),
                })
                .collect();
            format!("{n} missions complete: {}.", each.join("; "))
        }
    };
    Some(Callout { kind: "mission_complete", text, priority: 1, speak: true, ts: ts.to_string() })
}

/// A completion supersedes the pass's progress line.
///
/// The store is synced before the event loop, so by the time the
/// crossing kill is handled its mission is already ready to turn in and
/// the progress line names the NEXT nearest mission. Kept, the pass
/// would say "Mission progress: 30 of 48" and then "Mission complete: 30
/// kills for Labour Union of Ahayan" — two numbers, two different
/// missions, the wrong one first. The next kill reports progress again
/// anyway. Same shape as a trap warning dropping the fuel caution.
fn supersede_progress(out: &mut Vec<Sourced>, completion: Callout) {
    out.retain(|(c, _)| !(c.kind == "mission" && c.text.starts_with("Mission progress")));
    out.push((completion, None));
}

/// "Goeppert-Mayer Vision, Ahayan", or just the station, or nothing.
fn destination_of(redirect: &Value) -> String {
    let station = redirect
        .get("NewDestinationStation")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    let system = redirect
        .get("NewDestinationSystem")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty());
    match (station, system) {
        (Some(st), Some(sy)) => format!("{st}, {sy}"),
        (Some(st), None) => st.to_string(),
        _ => String::new(),
    }
}

fn trailer(destination: &str) -> String {
    if destination.is_empty() {
        String::new()
    } else {
        format!(" Return to {destination}.")
    }
}

#[cfg(test)]
mod tests {
    use super::{mission_progress, mission_redirected, stale_for_speech, Callout};
    use serde_json::{json, Value};

    /// A stack part-way through: two massacres against the same faction
    /// from different givers (the concurrent case), one already finished.
    fn db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        ed_store::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',1,'2026-09-16T10:00:00Z','MissionAccepted','{"timestamp":"2026-09-16T10:00:00Z","event":"MissionAccepted","Faction":"Ahayan Defence Party","Name":"Mission_Massacre","TargetFaction":"Anana Brotherhood","KillCount":9,"DestinationStation":"Goeppert-Mayer Vision","DestinationSystem":"Ahayan","Expiry":"2026-09-20T10:00:00Z","MissionID":11}'),
            ('J',2,'2026-09-16T10:00:01Z','MissionAccepted','{"timestamp":"2026-09-16T10:00:01Z","event":"MissionAccepted","Faction":"Labour Union of Ahayan","Name":"Mission_Massacre","TargetFaction":"Anana Brotherhood","KillCount":3,"DestinationStation":"Goeppert-Mayer Vision","DestinationSystem":"Ahayan","Expiry":"2026-09-21T10:00:00Z","MissionID":12}'),
            ('J',3,'2026-09-16T11:00:00Z','Bounty','{"timestamp":"2026-09-16T11:00:00Z","event":"Bounty","VictimFaction":"Anana Brotherhood","TotalReward":1}'),
            ('J',4,'2026-09-16T11:00:01Z','Bounty','{"timestamp":"2026-09-16T11:00:01Z","event":"Bounty","VictimFaction":"Anana Brotherhood","TotalReward":1}');
        "#).unwrap();
        conn
    }

    fn kill(ts: &str) -> Value {
        json!({"timestamp": ts, "event": "Bounty", "VictimFaction": "Anana Brotherhood"})
    }

    /// The progress line names the mission NEAREST to completing, not
    /// whichever happens to sit first in the list.
    #[test]
    fn progress_names_the_mission_closest_to_done() {
        let out = mission_progress(&db(), "2026-09-16T14:00:00Z", &kill("2026-09-16T11:00:01Z"));
        let progress: Vec<&str> = out.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(progress.len(), 1, "one line per kill: {progress:?}");
        // HUD order puts mission 11 first (it expires sooner) but it needs
        // 9 kills; mission 12 needs 3. The line must name 12. Without
        // this asymmetry the test passes against the old first-match
        // code and proves nothing.
        assert!(progress[0].contains("2 of 3"), "the nearest mission is named, not the first: {progress:?}");
        assert!(out.iter().all(|c| c.kind == "mission"));
    }

    /// The defect that made the app lie: 361 utterances across two
    /// overnight sessions announcing a stack finished hours earlier,
    /// because a ready-to-turn-in mission stayed first in the list and
    /// every later kill re-reported it. A kill can no longer say
    /// "complete" at all, and a finished mission produces no line.
    #[test]
    fn a_kill_never_announces_a_completion() {
        let conn = db();
        // Finish both missions outright: 12 more kills, then the game's
        // redirects. Any kill after this must be silent.
        let mut sql = String::new();
        for i in 0..12 {
            sql.push_str(&format!(
                "INSERT INTO events (file,offset,ts,event,raw) VALUES ('J',{},'2026-09-16T12:00:{:02}Z','Bounty','{{\"timestamp\":\"2026-09-16T12:00:{:02}Z\",\"event\":\"Bounty\",\"VictimFaction\":\"Anana Brotherhood\",\"TotalReward\":1}}');",
                100 + i, i, i
            ));
        }
        conn.execute_batch(&sql).unwrap();
        let out = mission_progress(&conn, "2026-09-16T14:00:00Z", &kill("2026-09-16T13:00:00Z"));
        assert!(
            !out.iter().any(|c| c.text.contains("complete")),
            "a kill must never claim a completion: {:?}",
            out.iter().map(|c| &c.text).collect::<Vec<_>>()
        );
        assert!(out.is_empty(), "nothing active remains, so the kill is silent: {:?}",
            out.iter().map(|c| &c.text).collect::<Vec<_>>());
    }

    fn redirect(id: i64, station: &str, system: &str) -> Value {
        json!({
            "timestamp": "2026-09-16T12:30:00Z", "event": "MissionRedirected", "MissionID": id,
            "Name": "Mission_MassacreWing", "LocalisedName": "Kill Anana Brotherhood faction Pirates",
            "NewDestinationStation": station, "NewDestinationSystem": system,
        })
    }

    /// Completion comes from the game's own signal, names the giver and
    /// the count from the store, and says where to take it.
    #[test]
    fn one_redirect_speaks_one_completion() {
        let c = mission_redirected(&db(), "2026-09-16T14:00:00Z", &[redirect(11, "Goeppert-Mayer Vision", "Ahayan")])
            .expect("a completion");
        assert_eq!(c.kind, "mission_complete", "separately mutable from progress");
        assert!(c.text.starts_with("Mission complete: 9 Anana Brotherhood kills for Ahayan Defence Party"), "{}", c.text);
        assert!(c.text.contains("Return to Goeppert-Mayer Vision, Ahayan."), "{}", c.text);
    }

    /// A concurrent stack finishes several at once: one line with a
    /// count, not one line each.
    #[test]
    fn several_redirects_in_a_pass_speak_once_with_a_count() {
        let events =
            [redirect(11, "Goeppert-Mayer Vision", "Ahayan"), redirect(12, "Goeppert-Mayer Vision", "Ahayan")];
        let c = mission_redirected(&db(), "2026-09-16T14:00:00Z", &events).expect("a completion");
        assert!(c.text.starts_with("2 missions complete:"), "{}", c.text);
        assert!(c.text.contains("Ahayan Defence Party"), "{}", c.text);
        assert!(c.text.contains("Labour Union of Ahayan"), "{}", c.text);
    }

    /// A completion supersedes the pass's progress line. The crossing
    /// kill's mission is already ready by the time the kill is handled,
    /// so the progress line in that pass is about a DIFFERENT mission
    /// and arrives before the completion it appears to belong to.
    #[test]
    fn a_completion_drops_the_progress_line_from_its_pass() {
        let progress = Callout {
            kind: "mission",
            text: "Mission progress: 30 of 48 Anana Brotherhood kills.".into(),
            priority: 0,
            speak: true,
            ts: "2026-09-16T12:30:00Z".into(),
        };
        let target = Callout {
            kind: "mission",
            text: "Target down: Saintmaur.".into(),
            priority: 1,
            speak: true,
            ts: "2026-09-16T12:30:00Z".into(),
        };
        let completion = Callout {
            kind: "mission_complete",
            text: "Mission complete: 30 kills.".into(),
            priority: 1,
            speak: true,
            ts: "2026-09-16T12:30:00Z".into(),
        };
        let mut out: Vec<super::Sourced> = vec![(progress, None), (target.clone(), None)];
        super::supersede_progress(&mut out, completion);
        let texts: Vec<&str> = out.iter().map(|(c, _)| c.text.as_str()).collect();
        assert!(!texts.iter().any(|t| t.starts_with("Mission progress")), "{texts:?}");
        assert!(texts.contains(&"Target down: Saintmaur."), "an unrelated mission line survives: {texts:?}");
        assert_eq!(texts.last(), Some(&"Mission complete: 30 kills."), "{texts:?}");
    }

    /// Missions that finish together need not share a hand-in. Measured
    /// on the maintainer's journal: the pass at 17:33Z on 2026-09-16
    /// held two redirects going to DIFFERENT stations (ids 1066168667
    /// and 1066169080), and taking the first one's destination for the
    /// whole line sent him to the wrong station. The other plural pass
    /// happened to share a destination, which is why it read fine.
    #[test]
    fn a_mixed_destination_pass_names_each_hand_in() {
        let events = [
            redirect(11, "Wheelock Port", "Puneith"),
            redirect(12, "Goeppert-Mayer Vision", "Ahayan"),
        ];
        let c = mission_redirected(&db(), "2026-09-16T14:00:00Z", &events).expect("a completion");
        assert!(c.text.contains("return to Wheelock Port, Puneith"), "{}", c.text);
        assert!(c.text.contains("return to Goeppert-Mayer Vision, Ahayan"), "{}", c.text);
        assert!(
            !c.text.contains(". Return to"),
            "no single trailing destination when they differ: {}",
            c.text
        );
    }

    /// A giver whose name ends in a full stop ("HIP 90112 Jet Central
    /// Corp.") must not produce "Corp.. Return to". It is the most
    /// common giver in the maintainer's stack, so it would be heard a
    /// lot.
    #[test]
    fn a_faction_ending_in_a_full_stop_does_not_double_it() {
        let conn = db();
        conn.execute_batch(
            r#"INSERT INTO events (file,offset,ts,event,raw) VALUES
               ('J',9,'2026-09-16T10:00:02Z','MissionAccepted','{"timestamp":"2026-09-16T10:00:02Z","event":"MissionAccepted","Faction":"HIP 90112 Jet Central Corp.","Name":"Mission_Massacre","TargetFaction":"Anana Brotherhood","KillCount":40,"DestinationStation":"Piaget Orbital","DestinationSystem":"HIP 90112","Expiry":"2026-09-22T10:00:00Z","MissionID":13}');"#,
        )
        .unwrap();
        let c = mission_redirected(&conn, "2026-09-16T14:00:00Z", &[redirect(13, "Piaget Orbital", "HIP 90112")])
            .expect("a completion");
        assert!(!c.text.contains(".."), "{}", c.text);
        assert!(c.text.contains("for HIP 90112 Jet Central Corp. Return to Piaget Orbital, HIP 90112."), "{}", c.text);
    }

    /// Redirects are not massacre-only: a mission the store cannot match
    /// still speaks, in the game's own words.
    #[test]
    fn an_unknown_mission_falls_back_to_the_games_wording() {
        let c = mission_redirected(&db(), "2026-09-16T14:00:00Z", &[redirect(999, "Jameson Memorial", "Shinrarta Dezhra")])
            .expect("a completion");
        assert!(c.text.contains("Kill Anana Brotherhood faction Pirates"), "{}", c.text);
    }

    /// A pass with no redirect says nothing.
    #[test]
    fn a_pass_without_a_redirect_is_silent() {
        assert!(mission_redirected(&db(), "2026-09-16T14:00:00Z", &[kill("2026-09-16T11:00:00Z")]).is_none());
    }


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
