//! Follow a TRADE route: the layer above the jump-route follower.
//!
//! Maintainer spec (2026-09-05, flying Talaria Towers <-> Metz Enterprise):
//! select a route from the Trade results and EDDA runs the loop with
//! you — a HUD block naming each end, a spoken briefing on every
//! arrival (sell first, then buy, then flip the navigation target),
//! and system-name-only retargeting because galaxy-map station
//! targeting is flaky while system names are solid.
//!
//! ONE code path (the maintainer's design constraint, verbatim "ideally it's
//! a single code path"): everything is a cyclic list of [`TradeStop`]s.
//! A plain leg is two stops with goods one way — its empty return hop
//! falls out of the cycle naturally. A round trip is the same two
//! stops with goods both ways. A ring is N stops. Nothing downstream
//! knows which it was.
//!
//! Per stop, the follower plots an ordinary jump route to the stop's
//! SYSTEM and hands it to the existing follow stack (source "trade"):
//! F10 targeting, per-jump callouts, fuel coaching and off-route
//! replanning all run unchanged underneath.

use serde::{Deserialize, Serialize};

/// One line of goods at a stop: what to buy or sell, and the price the
/// search saw (for staleness warnings, never for silent rescaling).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CargoItem {
    pub symbol: String,
    pub commodity: String,
    pub tons: i64,
    #[serde(default)]
    pub price: i64,
}

/// One station on the cycle and what happens at its pad.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeStop {
    pub system: String,
    pub system_id64: i64,
    pub station: String,
    /// The galaxy store's station id, which IS the game's MarketID for
    /// dockables — the primary arrival match; names are the fallback.
    pub station_id: i64,
    pub arrival_ls: Option<f64>,
    /// Spoken FIRST on arrival (the maintainer's order: sell, then buy).
    pub sell: Vec<CargoItem>,
    /// Spoken second; loaded before departing for the next stop.
    pub buy: Vec<CargoItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// En route to `stops[at]` (the jump follower owns the journey).
    Travelling,
    /// Docked at `stops[at]`; briefing delivered, departure pending.
    AtStop,
}

/// Observability that ships with the feature (doctrine rule 2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Counters {
    pub arrivals: u32,
    pub matched_by_market_id: u32,
    pub matched_by_name: u32,
    pub wrong_station_notes: u32,
    pub retargets: u32,
    pub laps: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TradeRoute {
    /// The cycle. Never empty; `at` always indexes into it.
    pub stops: Vec<TradeStop>,
    /// The stop the commander is heading to (Travelling) or at (AtStop).
    pub at: usize,
    pub phase: Phase,
    pub lap: u32,
    /// Journal timestamp when the current lap started, for measured
    /// lap-profit from the sales table.
    pub lap_started: String,
    /// Hold size when the route started; drift is announced, never
    /// silently rescaled.
    pub cargo_capacity: i64,
    pub expected_profit_per_lap: i64,
    /// "leg" | "round_trip" | "ring" — a display label only; the code
    /// path is identical for all three.
    pub kind: String,
    #[serde(default)]
    pub counters: Counters,
    /// The stop index a wrong-station redirect was already spoken for,
    /// so docking around the right system stays a single note.
    #[serde(default)]
    pub noted_wrong_at: Option<usize>,
    /// The stop index terminal guidance was already spoken for, so
    /// bouncing in and out of the system stays a single advisory.
    #[serde(default)]
    pub guided_at: Option<usize>,
}

// ── Wire inputs ─────────────────────────────────────────────────────
// Slim deserialize-only twins of ed_route::profit types: the frontend
// sends exactly the legs of whatever row was clicked ([leg],
// [out, back], or ring.legs) and serde ignores the report fields these
// don't name.

#[derive(Debug, Clone, Deserialize)]
pub struct StationRefInput {
    pub station_id: i64,
    pub station: String,
    pub system: String,
    pub system_id64: i64,
    #[serde(default)]
    pub arrival_ls: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CargoLineInput {
    pub symbol: String,
    pub commodity: String,
    pub tons: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegInput {
    pub from: StationRefInput,
    pub to: StationRefInput,
    pub symbol: String,
    pub commodity: String,
    pub tons: i64,
    #[serde(default)]
    pub buy_price: i64,
    #[serde(default)]
    pub sell_price: i64,
    #[serde(default)]
    pub profit: i64,
    #[serde(default)]
    pub extra: Vec<CargoLineInput>,
}

/// Build the cyclic stop list — THE single code path. Every leg
/// contributes its goods as a buy at `from` and a sell at `to`;
/// consecutive legs sharing a station merge into one stop; the list
/// closes into a cycle. A lone leg therefore becomes two stops with
/// goods one way, and the empty hop back to the buy stop is simply the
/// cycle wrapping — no special case anywhere.
pub fn stops_from_legs(legs: &[LegInput]) -> Vec<TradeStop> {
    let mut stops: Vec<TradeStop> = Vec::new();
    let stop_index = |stops: &mut Vec<TradeStop>, r: &StationRefInput| -> usize {
        if let Some(i) = stops.iter().position(|s| s.station_id == r.station_id) {
            return i;
        }
        stops.push(TradeStop {
            system: r.system.clone(),
            system_id64: r.system_id64,
            station: r.station.clone(),
            station_id: r.station_id,
            arrival_ls: r.arrival_ls,
            sell: Vec::new(),
            buy: Vec::new(),
        });
        stops.len() - 1
    };
    for leg in legs {
        let lines: Vec<(String, String, i64)> = std::iter::once((leg.symbol.clone(), leg.commodity.clone(), leg.tons))
            .chain(leg.extra.iter().map(|x| (x.symbol.clone(), x.commodity.clone(), x.tons)))
            .collect();
        let from = stop_index(&mut stops, &leg.from);
        for (symbol, commodity, tons) in &lines {
            stops[from].buy.push(CargoItem {
                symbol: symbol.clone(),
                commodity: commodity.clone(),
                tons: *tons,
                price: leg.buy_price,
            });
        }
        let to = stop_index(&mut stops, &leg.to);
        for (symbol, commodity, tons) in &lines {
            stops[to].sell.push(CargoItem {
                symbol: symbol.clone(),
                commodity: commodity.clone(),
                tons: *tons,
                price: leg.sell_price,
            });
        }
    }
    stops
}

impl TradeRoute {
    pub fn new(legs: &[LegInput], kind: &str, cargo_capacity: i64, started_ts: &str) -> Option<Self> {
        let stops = stops_from_legs(legs);
        if stops.is_empty() {
            return None;
        }
        Some(TradeRoute {
            stops,
            at: 0,
            phase: Phase::Travelling,
            lap: 1,
            lap_started: started_ts.to_string(),
            cargo_capacity,
            expected_profit_per_lap: legs.iter().map(|l| l.profit).sum(),
            kind: kind.to_string(),
            counters: Counters::default(),
            noted_wrong_at: None,
            guided_at: None,
        })
    }

    /// Start-while-docked, the maintainer's "if not already there" case, as
    /// pure cycle rotation: if the commander's current pad IS one of
    /// the stops, rotate the cycle so it is stop zero — the ordinary
    /// arrival handler then does the briefing. One code path.
    pub fn rotate_to_station(&mut self, market_id: Option<i64>, station: &str, system: &str) -> bool {
        let here = self.stops.iter().position(|s| {
            market_id.is_some_and(|id| id == s.station_id)
                || (s.station.eq_ignore_ascii_case(station) && s.system.eq_ignore_ascii_case(system))
        });
        match here {
            Some(i) => {
                self.stops.rotate_left(i);
                self.at = 0;
                true
            }
            None => false,
        }
    }

    /// Does this Docked event land on the stop we are heading for?
    /// MarketID is authoritative; names are the fallback (instrumented).
    pub fn matches_current(&mut self, market_id: Option<i64>, station: &str, system: &str) -> bool {
        let stop = &self.stops[self.at];
        if market_id.is_some_and(|id| id == stop.station_id) {
            self.counters.matched_by_market_id += 1;
            return true;
        }
        if stop.station.eq_ignore_ascii_case(station) && stop.system.eq_ignore_ascii_case(system) {
            self.counters.matched_by_name += 1;
            return true;
        }
        false
    }

    /// A lap completes when the commander ARRIVES back at stop zero —
    /// not when the cursor wraps past the last stop (field case
    /// 2026-09-05, first live run: "Lap 1 complete" spoke at the far
    /// end of the loop, half a lap early). Call BEFORE `advance`, with
    /// the arrival the caller just matched.
    pub fn arrival_completes_lap(&self) -> bool {
        self.at == 0 && self.counters.arrivals > 0
    }

    /// Advance past the current stop (the commander departs for the
    /// next one).
    pub fn advance(&mut self, now_ts: &str) {
        self.at = (self.at + 1) % self.stops.len();
        self.phase = Phase::Travelling;
        if self.at == 1 || self.stops.len() == 1 {
            // Departing stop zero starts the next lap's clock.
            self.lap_started = now_ts.to_string();
        }
    }

    /// Record a completed lap (arrival at stop zero).
    pub fn complete_lap(&mut self) -> u32 {
        let finished = self.lap;
        self.lap += 1;
        self.counters.laps += 1;
        finished
    }

    pub fn current(&self) -> &TradeStop {
        &self.stops[self.at]
    }

    /// Tons aboard when DEPARTING `stop`: its shopping list, capped at
    /// the hold — what the outbound leg should be planned to carry.
    pub fn departing_cargo_t(stop: &TradeStop, capacity: i64) -> i64 {
        stop.buy.iter().map(|i| i.tons).sum::<i64>().min(capacity).max(0)
    }

    pub fn next_stop(&self) -> &TradeStop {
        &self.stops[(self.at + 1) % self.stops.len()]
    }
}

// ── Speech (pure, tested; the player is "Commander", never sir/ma'am) ──

fn goods_list(items: &[CargoItem], capacity: i64) -> String {
    let mut parts: Vec<String> = Vec::new();
    for item in items {
        let tons = item.tons.min(capacity);
        if parts.is_empty() {
            parts.push(format!("{} tons of {}", tons, item.commodity));
        } else {
            parts.push(format!("{} of {}", tons, item.commodity));
        }
    }
    parts.join(", and ")
}

/// The start briefing.
pub fn start_line(route: &TradeRoute, capacity: i64) -> String {
    let first = route.current();
    let opening = if route.stops.len() > 2 {
        format!("Trade route, Commander: {} stops per lap.", route.stops.len())
    } else {
        "Trade route, Commander.".to_string()
    };
    let action = if !first.buy.is_empty() {
        format!(" First: {} in {} — buy {}.", first.station, first.system, goods_list(&first.buy, capacity))
    } else if !first.sell.is_empty() {
        format!(" First: {} in {} — sell {}.", first.station, first.system, goods_list(&first.sell, capacity))
    } else {
        format!(" First: {} in {}.", first.station, first.system)
    };
    format!("{opening}{action}")
}

/// The arrival briefing: sell first, then buy, then where the flip
/// points next (the maintainer's order, verbatim spec).
pub fn arrival_line(route: &TradeRoute, capacity: i64) -> String {
    let stop = route.current();
    let next = route.next_stop();
    let mut parts: Vec<String> = vec![format!("Docked at {}.", stop.station)];
    if !stop.sell.is_empty() {
        parts.push(format!("Sell {} here.", goods_list(&stop.sell, capacity)));
    }
    if !stop.buy.is_empty() {
        // "Then" only makes sense after a sell instruction.
        let verb = if stop.sell.is_empty() { "Buy" } else { "Then buy" };
        parts.push(format!("{verb} {}.", goods_list(&stop.buy, capacity)));
    }
    if next.station_id != stop.station_id {
        parts.push(format!("Next: {} in {} — targeting it now.", next.station, next.system));
    }
    parts.join(" ")
}

/// Docked in the right system, wrong pad.
pub fn wrong_station_line(route: &TradeRoute, docked_at: &str) -> String {
    let stop = route.current();
    match stop.arrival_ls {
        Some(ls) => format!("This is {docked_at}; the trade stop is {} — {:.0} light seconds.", stop.station, ls),
        None => format!("This is {docked_at}; the trade stop is {}.", stop.station),
    }
}

/// Lap completion: the feature's own measurement against its promise.
pub fn lap_line(lap_finished: u32, planned: i64, measured: Option<i64>) -> String {
    match measured {
        Some(m) => format!(
            "Lap {lap_finished} complete: {} measured against {} planned.",
            fmt_credits(m),
            fmt_credits(planned)
        ),
        None => format!("Lap {lap_finished} complete."),
    }
}

fn fmt_credits(cr: i64) -> String {
    if cr.abs() >= 1_000_000 {
        format!("{:.1} million", cr as f64 / 1e6)
    } else {
        format!("{cr} credits")
    }
}


// ── Persistence ─────────────────────────────────────────────────────

pub fn load(conn: &rusqlite::Connection) -> Option<TradeRoute> {
    conn.query_row("SELECT json FROM trade_route WHERE id = 1", [], |r| r.get::<_, String>(0))
        .ok()
        .and_then(|j| serde_json::from_str(&j).ok())
}

pub fn save(conn: &rusqlite::Connection, route: &TradeRoute) -> Result<(), String> {
    let json = serde_json::to_string(route).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO trade_route (id, json, updated) VALUES (1, ?1, strftime('%Y-%m-%dT%H:%M:%SZ','now'))
         ON CONFLICT(id) DO UPDATE SET json = excluded.json, updated = excluded.updated",
        [&json],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn clear(conn: &rusqlite::Connection) {
    let _ = conn.execute("DELETE FROM trade_route WHERE id = 1", []);
    crate::trade_timing::window_close(conn);
}

/// What the HUD and Trade page render.
pub fn view(route: Option<&TradeRoute>) -> serde_json::Value {
    match route {
        None => serde_json::json!({ "active": false }),
        Some(tr) => serde_json::json!({
            "active": true,
            "kind": tr.kind,
            "lap": tr.lap,
            "at": tr.at,
            "phase": tr.phase,
            "expected_profit_per_lap": tr.expected_profit_per_lap,
            "stops": tr.stops.iter().map(|s| serde_json::json!({
                "system": s.system, "station": s.station, "arrival_ls": s.arrival_ls,
                "buy": s.buy, "sell": s.sell,
            })).collect::<Vec<_>>(),
        }),
    }
}

/// Measured profit since `lap_started`, from the commander's own sales
/// rows — the feature's A/B against its promise. `None` when nothing
/// sold yet (a lap can legitimately end goods-in-hold on odd rings).
fn measured_lap_profit(conn: &rusqlite::Connection, since_ts: &str) -> Option<i64> {
    conn.query_row(
        "SELECT SUM(COALESCE(total_sale,0) - COALESCE(avg_price_paid,0) * count), COUNT(*)
         FROM sales WHERE ts >= ?1",
        [since_ts],
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, i64>(1)?)),
    )
    .ok()
    .and_then(|(sum, n)| if n > 0 { sum } else { None })
}

/// Live hold size; falls back to the plan's capacity when the loadout
/// row is missing (a fresh database).
fn live_capacity(conn: &rusqlite::Connection, fallback: i64) -> i64 {
    conn.query_row("SELECT cargo_capacity FROM loadout WHERE id = 1", [], |r| r.get::<_, Option<i64>>(0))
        .ok()
        .flatten()
        .filter(|c| *c > 0)
        .unwrap_or(fallback)
}

/// Arrived in the current stop's SYSTEM (any router — this is the
/// game-plotter path's half; the EDDA-followed path speaks the same
/// line from its route-completion seam): the one useful instruction,
/// once per stop, then silence until the Docked hook picks up.
pub fn arrival_guidance(conn: &rusqlite::Connection, system: &str) -> Option<String> {
    let mut tr = load(conn)?;
    if tr.phase != Phase::Travelling || tr.guided_at == Some(tr.at) {
        return None;
    }
    if !tr.current().system.eq_ignore_ascii_case(system) {
        return None;
    }
    tr.guided_at = Some(tr.at);
    let station = tr.current().station.clone();
    let _ = save(conn, &tr);
    Some(format!("Target {station} via the System Map for terminal guidance."))
}

// ── The arrival handler (called from the watcher's Docked arm) ──────

/// Handle a Docked event against the active trade route. Pushes any
/// spoken callouts (journal-ts-stamped, so the replay freshness gate
/// applies) and re-targets the next stop's SYSTEM when the cycle
/// advances.
pub fn on_docked(
    app: &tauri::AppHandle,
    conn: &rusqlite::Connection,
    v: &serde_json::Value,
    out: &mut Vec<crate::watcher::Sourced>,
) {
    use tauri::Manager as _;
    let Some(mut tr) = load(conn) else { return };
    let ts = v.get("timestamp").and_then(serde_json::Value::as_str).unwrap_or("");
    let station = v.get("StationName").and_then(serde_json::Value::as_str).unwrap_or("");
    let system = v.get("StarSystem").and_then(serde_json::Value::as_str).unwrap_or("");
    let market_id = v.get("MarketID").and_then(serde_json::Value::as_i64);

    if tr.matches_current(market_id, station, system) {
        let completes_lap = tr.arrival_completes_lap();
        tr.counters.arrivals += 1;
        tr.phase = Phase::AtStop;
        let capacity = live_capacity(conn, tr.cargo_capacity);
        let mut text = arrival_line(&tr, capacity);
        if capacity != tr.cargo_capacity {
            text.push_str(&format!(" Hold is {capacity} tons now."));
        }
        if completes_lap {
            // Measured over depart-zero -> arrive-zero: the closing sale
            // made at this pad lands in the NEXT lap's figure, so the
            // totals stay right across laps even though each line runs
            // one sale behind.
            let planned = tr.expected_profit_per_lap;
            let measured = measured_lap_profit(conn, &tr.lap_started);
            let finished = tr.complete_lap();
            text.push(' ');
            text.push_str(&lap_line(finished, planned, measured));
            tracing::info!(lap = finished, planned, measured, "trade_follow lap");
        }
        // Plan the leg OUT of this pad at the mass it will actually
        // carry: this stop's shopping list (maintainer, 2026-09-05 — "factor
        // in cargo for a planned trade route").
        let departing_cargo = TradeRoute::departing_cargo_t(tr.current(), capacity);
        tr.advance(ts);
        let next_system = tr.current().system.clone();
        tr.counters.retargets += 1;
        tracing::info!(
            stop = %station,
            matched_by_market = market_id.is_some(),
            departing_cargo,
            lap = tr.lap,
            "trade_follow arrival"
        );
        let _ = save(conn, &tr);
        use crate::events::EmitExt as _;
        app.state::<crate::state::AppState>().events.emit(crate::events::TRADE_FOLLOW, view(Some(&tr)));
        out.push((crate::callouts::Callout::new("trade", ts, 1, true, text), Some(v.clone())));
        if !next_system.eq_ignore_ascii_case(system) {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                plot_next(app, next_system, Some(departing_cargo)).await;
            });
        }
    } else if tr.current().system.eq_ignore_ascii_case(system) && tr.noted_wrong_at != Some(tr.at) {
        tr.counters.wrong_station_notes += 1;
        tr.noted_wrong_at = Some(tr.at);
        let text = wrong_station_line(&tr, station);
        let _ = save(conn, &tr);
        out.push((crate::callouts::Callout::new("trade", ts, 1, true, text), Some(v.clone())));
    }
}

/// Hand the next stop's SYSTEM to whichever router the commander's
/// distance setting picks (field catch 2026-09-05: trade-follow
/// ignored the slider) — the same gate the Route tab applies: a hop
/// within `game_route_max_ly`, with the galaxy-map recipe taught, goes
/// to the GAME's own plotter (clipboard armed, "press Target Next");
/// anything longer is planned by EDDA and followed under source
/// "trade".
async fn plot_next(app: tauri::AppHandle, system: String, cargo_t: Option<i64>) {
    use tauri::Manager as _;
    let state = app.state::<crate::state::AppState>();
    let max = state.config.lock().unwrap_or_else(|e| e.into_inner()).game_route_max_ly;
    if max > 0 {
        let distance = state.routing.galaxy(&state.data_dir).and_then(|g| {
            let here = state.with_read(|s| {
                ed_store::query::location(s.conn()).ok().flatten().and_then(|l| l.system_name)
            })?;
            let a = g.record(g.find(&here)?).pos();
            let b = g.record(g.find(&system)?).pos();
            Some(
                (((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)) as f64)
                    .sqrt(),
            )
        });
        if distance.is_some_and(|d| d <= max as f64) {
            // Errors (recipe not taught) fall through to EDDA's planner.
            if crate::follow::route_plot_in_game(app.state(), system.clone()).await.is_ok() {
                tracing::info!(%system, "trade_follow retarget via the game's plotter");
                return;
            }
        }
    }
    match crate::routing::plot_for_trade(app.clone(), system.clone(), cargo_t).await {
        Ok(route) => {
            let new = crate::follow::ActiveRoute { route, next: 1, source: "trade".into() };
            if state.with_store(|s| crate::follow::save_pub(s.conn(), &new)).is_ok() {
                use tauri::Emitter as _;
                let _ = app.emit(crate::events::ROUTE_FOLLOW, crate::follow::view(Some(&new)));
            }
        }
        Err(error) => {
            tracing::warn!(%error, "trade_follow: plot to next stop failed");
            state.voice.say(format!("Couldn't plot to {system}: {error}"));
        }
    }
}

/// Undocked while a trade route is at a stop: the hold now carries what
/// was bought, so the dock-time route (planned on the pre-purchase
/// mass — field catch 2026-09-05: "it calculates the jump distance off
/// an empty hold") is silently replanned at the true laden range. The
/// game-plotter path needs no replan: Elite plans with actual mass.
pub fn on_undocked(app: &tauri::AppHandle, conn: &rusqlite::Connection) {
    let Some(mut tr) = load(conn) else { return };
    if tr.phase != Phase::AtStop {
        return;
    }
    tr.phase = Phase::Travelling;
    let _ = save(conn, &tr);
    let followed_by_edda = crate::follow::load(conn).is_some_and(|ar| ar.source == "trade");
    if followed_by_edda {
        let target = tr.current().system.clone();
        tracing::info!(%target, "trade_follow: replanning at laden mass after undock");
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(route) = crate::routing::plot_for_trade(app.clone(), target, None).await {
                use tauri::Manager as _;
                let state = app.state::<crate::state::AppState>();
                let new = crate::follow::ActiveRoute { route, next: 1, source: "trade".into() };
                if state.with_store(|s| crate::follow::save_pub(s.conn(), &new)).is_ok() {
                    use tauri::Emitter as _;
                    let _ = app.emit(crate::events::ROUTE_FOLLOW, crate::follow::view(Some(&new)));
                }
            }
        });
    }
}

// ── Commands ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn trade_follow_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    legs: Vec<LegInput>,
    kind: String,
) -> Result<serde_json::Value, String> {
    let now = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let (capacity, location) = state.with_read(|s| {
        (live_capacity(s.conn(), 0), ed_store::query::location(s.conn()).ok().flatten())
    });
    let capacity = if capacity > 0 { capacity } else { 1 };
    let mut route = TradeRoute::new(&legs, &kind, capacity, &now).ok_or("no stops in the selection")?;
    tracing::info!(
        stops = route.stops.len(),
        kind = %route.kind,
        expected = route.expected_profit_per_lap,
        "trade_follow start"
    );
    // Already docked at one of the stops: rotate the cycle there and
    // brief as an arrival (the same code path as any other arrival).
    let docked_here = location
        .as_ref()
        .filter(|l| l.docked)
        .and_then(|l| l.station_name.clone().zip(l.system_name.clone()));
    let text = if let Some((station, system)) =
        docked_here.filter(|(st, sy)| route.rotate_to_station(None, st, sy))
    {
        let _ = (station, system);
        route.phase = Phase::AtStop;
        let capacity = route.cargo_capacity;
        let mut text = arrival_line(&route, capacity);
        // Departing THIS pad with its shopping list aboard: plan laden.
        let departing_cargo = TradeRoute::departing_cargo_t(route.current(), capacity);
        let next_system = route.next_stop().system.clone();
        route.advance(&now);
        let here_system = route.stops[if route.at == 0 { route.stops.len() - 1 } else { route.at - 1 }].system.clone();
        if !next_system.eq_ignore_ascii_case(&here_system) {
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move { plot_next(app2, next_system, Some(departing_cargo)).await });
        }
        text = text.replace("Docked at", "Trade route, Commander. You're docked at");
        text
    } else {
        // Flying to the first stop with whatever is aboard now.
        let first_system = route.current().system.clone();
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move { plot_next(app2, first_system, None).await });
        start_line(&route, capacity)
    };
    state.with_store(|s| {
        save(s.conn(), &route)?;
        // The follow's timing window opens here (trade_timing).
        crate::trade_timing::window_close(s.conn());
        crate::trade_timing::window_open(s.conn());
        Ok::<(), String>(())
    })?;
    use crate::events::EmitExt as _;
    state.events.emit(crate::events::TRADE_FOLLOW, view(Some(&route)));
    crate::watcher::deliver(
        &app,
        vec![(crate::callouts::Callout::new("trade", "", 1, true, text), None)],
    );
    Ok(view(Some(&route)))
}

#[tauri::command]
pub async fn trade_follow_stop(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<serde_json::Value, String> {
    let had_trade_route = state.with_read(|s| {
        crate::follow::load(s.conn()).is_some_and(|ar| ar.source == "trade")
    });
    state.with_store(|s| {
        clear(s.conn());
        if had_trade_route {
            let _ = s.conn().execute("DELETE FROM active_route WHERE id = 1", []);
        }
        Ok::<(), String>(())
    })?;
    tracing::info!("trade_follow stop");
    use crate::events::EmitExt as _;
    state.events.emit(crate::events::TRADE_FOLLOW, view(None));
    if had_trade_route {
        use tauri::Emitter as _;
        let _ = app.emit(crate::events::ROUTE_FOLLOW, crate::follow::view(None));
    }
    Ok(view(None))
}

#[tauri::command]
pub async fn trade_follow_status(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<serde_json::Value, String> {
    Ok(state.with_read(|s| view(load(s.conn()).as_ref())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(id: i64, name: &str, system: &str, ls: f64) -> StationRefInput {
        StationRefInput {
            station_id: id,
            station: name.into(),
            system: system.into(),
            system_id64: id * 1000,
            arrival_ls: Some(ls),
        }
    }

    fn leg(from: &StationRefInput, to: &StationRefInput, commodity: &str, tons: i64) -> LegInput {
        LegInput {
            from: from.clone(),
            to: to.clone(),
            symbol: commodity.to_lowercase(),
            commodity: commodity.into(),
            tons,
            buy_price: 48_310,
            sell_price: 302_136,
            profit: (302_136 - 48_310) * tons,
            extra: vec![],
        }
    }

    /// The single code path, all three shapes: a leg is two stops with
    /// goods one way; a round trip carries goods both ways; a ring is
    /// N stops. Same constructor, no branches.
    #[test]
    fn one_constructor_covers_leg_trip_and_ring() {
        let talaria = station(1, "Talaria Towers", "Scorpii Sector ZE-A d160", 270.0);
        let metz = station(2, "Metz Enterprise", "Ega", 5394.0);
        let third = station(3, "Silves Dock", "Komovoy", 100.0);

        // Leg: buy at Talaria, sell at Metz, fly back empty.
        let stops = stops_from_legs(&[leg(&talaria, &metz, "Palladium", 1008)]);
        assert_eq!(stops.len(), 2);
        assert_eq!(stops[0].buy[0].commodity, "Palladium");
        assert!(stops[0].sell.is_empty());
        assert_eq!(stops[1].sell[0].commodity, "Palladium");
        assert!(stops[1].buy.is_empty(), "the empty return hop is just the cycle wrapping");

        // Round trip: the same two stops, goods both ways.
        let stops = stops_from_legs(&[
            leg(&talaria, &metz, "Palladium", 1008),
            leg(&metz, &talaria, "Gold", 900),
        ]);
        assert_eq!(stops.len(), 2, "shared stations merge into one stop each");
        assert_eq!(stops[1].sell[0].commodity, "Palladium");
        assert_eq!(stops[1].buy[0].commodity, "Gold");
        assert_eq!(stops[0].sell[0].commodity, "Gold");

        // Ring: three stops, each with a sell and a buy.
        let stops = stops_from_legs(&[
            leg(&talaria, &metz, "Palladium", 1008),
            leg(&metz, &third, "Gold", 900),
            leg(&third, &talaria, "Silver", 800),
        ]);
        assert_eq!(stops.len(), 3);
        for s in &stops {
            assert_eq!(s.buy.len(), 1);
            assert_eq!(s.sell.len(), 1);
        }
    }

    /// Start-while-docked is a rotation, not a branch: the cycle turns
    /// so the commander's pad is stop zero.
    #[test]
    fn starting_at_a_mid_cycle_stop_rotates_the_cycle() {
        let talaria = station(1, "Talaria Towers", "Scorpii Sector ZE-A d160", 270.0);
        let metz = station(2, "Metz Enterprise", "Ega", 5394.0);
        let mut route = TradeRoute::new(
            &[leg(&talaria, &metz, "Palladium", 1008), leg(&metz, &talaria, "Gold", 900)],
            "round_trip",
            1008,
            "2026-09-05T15:00:00Z",
        )
        .unwrap();
        assert!(route.rotate_to_station(Some(2), "irrelevant", "irrelevant"));
        assert_eq!(route.current().station, "Metz Enterprise");
        assert!(!route.rotate_to_station(Some(99), "Nowhere", "Nowhere"));
    }

    /// MarketID wins; names are the fallback; both are counted.
    #[test]
    fn arrival_matching_prefers_market_id_and_counts_the_fallback() {
        let talaria = station(1, "Talaria Towers", "Scorpii Sector ZE-A d160", 270.0);
        let metz = station(2, "Metz Enterprise", "Ega", 5394.0);
        let mut route =
            TradeRoute::new(&[leg(&talaria, &metz, "Palladium", 1008)], "leg", 1008, "t").unwrap();
        assert!(route.matches_current(Some(1), "?", "?"));
        assert!(route.matches_current(None, "talaria towers", "scorpii sector ze-a d160"));
        assert!(!route.matches_current(Some(2), "Metz Enterprise", "Ega"), "wrong stop is no match");
        assert_eq!(route.counters.matched_by_market_id, 1);
        assert_eq!(route.counters.matched_by_name, 1);
    }

    /// A lap completes on ARRIVAL back at stop zero — not when the
    /// cursor wraps past the last stop (the first live run spoke "Lap 1
    /// complete" at the far end of the loop, half a lap early).
    #[test]
    fn a_lap_completes_on_arrival_at_stop_zero() {
        let talaria = station(1, "Talaria Towers", "S", 270.0);
        let metz = station(2, "Metz Enterprise", "Ega", 5394.0);
        let mut route =
            TradeRoute::new(&[leg(&talaria, &metz, "Palladium", 1008)], "leg", 1008, "t0").unwrap();
        // Arrive at Talaria (stop 0, the very first arrival: no lap yet).
        assert!(!route.arrival_completes_lap(), "the starting arrival is not a lap");
        route.counters.arrivals += 1;
        route.advance("t1");
        assert_eq!(route.lap_started, "t1", "departing stop zero starts the lap clock");
        // Arrive at Metz (the far end): NOT a lap.
        assert!(!route.arrival_completes_lap(), "the far end is half a lap");
        route.counters.arrivals += 1;
        route.advance("t2");
        // Arrive back at Talaria: THAT completes lap 1.
        assert!(route.arrival_completes_lap());
        assert_eq!(route.complete_lap(), 1);
        assert_eq!(route.lap, 2);
    }

    /// The maintainer's exact briefing order: sell, then buy, then the flip.
    #[test]
    fn arrival_briefing_sells_then_buys_then_flips() {
        let talaria = station(1, "Talaria Towers", "Scorpii Sector ZE-A d160", 270.0);
        let metz = station(2, "Metz Enterprise", "Ega", 5394.0);
        let mut route = TradeRoute::new(
            &[leg(&talaria, &metz, "Palladium", 1008), leg(&metz, &talaria, "Gold", 900)],
            "round_trip",
            1008,
            "t",
        )
        .unwrap();
        route.advance("t1"); // heading to Metz
        let line = arrival_line(&route, 1008);
        assert_eq!(
            line,
            "Docked at Metz Enterprise. Sell 1008 tons of Palladium here. Then buy 900 tons of Gold. Next: Talaria Towers in Scorpii Sector ZE-A d160 — targeting it now."
        );
        let sell_at = line.find("Sell").unwrap();
        let buy_at = line.find("Then buy").unwrap();
        let flip_at = line.find("Next:").unwrap();
        assert!(sell_at < buy_at && buy_at < flip_at);
    }

    /// The departing load is the stop's whole shopping list, capped at
    /// the hold — the number the outbound plot is now planned at.
    #[test]
    fn departing_cargo_is_the_shopping_list_capped_at_the_hold() {
        let talaria = station(1, "Talaria Towers", "S", 270.0);
        let metz = station(2, "Metz Enterprise", "Ega", 5394.0);
        let mut l = leg(&talaria, &metz, "Palladium", 900);
        l.extra.push(CargoLineInput { symbol: "gold".into(), commodity: "Gold".into(), tons: 300 });
        let route = TradeRoute::new(&[l], "leg", 1008, "t").unwrap();
        assert_eq!(
            TradeRoute::departing_cargo_t(route.current(), 1008),
            1008,
            "900 + 300 caps at the hold"
        );
        assert_eq!(TradeRoute::departing_cargo_t(route.current(), 2000), 1200, "uncapped sum otherwise");
        assert_eq!(
            TradeRoute::departing_cargo_t(route.next_stop(), 1008),
            0,
            "the sell-only end departs empty — the cycle's return leg plans light"
        );
    }

    /// Terminal guidance speaks once on arriving in the stop's system —
    /// whichever router flew the leg — and never repeats for bounces.
    #[test]
    fn terminal_guidance_speaks_once_per_stop() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        let talaria = station(1, "Talaria Towers", "Scorpii Sector ZE-A d160", 270.0);
        let metz = station(2, "Metz Enterprise", "Ega", 5394.0);
        let route =
            TradeRoute::new(&[leg(&talaria, &metz, "Palladium", 1008)], "leg", 1008, "t").unwrap();
        save(&conn, &route).unwrap();
        assert_eq!(
            arrival_guidance(&conn, "scorpii sector ze-a d160").as_deref(),
            Some("Target Talaria Towers via the System Map for terminal guidance."),
            "case-insensitive, names the pad"
        );
        assert!(arrival_guidance(&conn, "Scorpii Sector ZE-A d160").is_none(), "spoken once");
        assert!(arrival_guidance(&conn, "Ega").is_none(), "not this stop's system");
        // Advance to Metz: its arrival guides afresh.
        let mut tr = load(&conn).unwrap();
        tr.advance("t1");
        save(&conn, &tr).unwrap();
        assert_eq!(
            arrival_guidance(&conn, "Ega").as_deref(),
            Some("Target Metz Enterprise via the System Map for terminal guidance.")
        );
    }

    /// Capacity drift caps the SPOKEN tons; the plan is never silently
    /// rescaled.
    #[test]
    fn spoken_tons_respect_a_smaller_hold() {
        let talaria = station(1, "Talaria Towers", "S", 270.0);
        let metz = station(2, "Metz Enterprise", "Ega", 5394.0);
        let route =
            TradeRoute::new(&[leg(&talaria, &metz, "Palladium", 1008)], "leg", 1008, "t").unwrap();
        assert!(start_line(&route, 512).contains("buy 512 tons of Palladium"));
        assert_eq!(route.current().buy[0].tons, 1008, "the plan itself is untouched");
    }

}
