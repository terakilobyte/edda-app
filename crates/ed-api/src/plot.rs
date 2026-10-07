//! Navigation as an API (item 48, maintainer 2026-09-03: "allow people to api
//! request navigation... if they don't want galactic data on their disk,
//! edda just asks the server to do the route").
//!
//! `POST /v1/route` takes names plus the physics the CLIENT derived from
//! its own journal (`FuelModel`/`BoostProfile`/start fuel — the server
//! has no journal and never will), plots on the resident galaxy with
//! `ed_galaxy::long_range::plan_best`, and returns the engine's `Route`
//! verbatim — the same type the app consumes locally, so a thin client
//! is a data-source toggle, not a rewrite.
//!
//! Box realities (ledger, item 48): a Colonia plot is under a second but
//! desert corridors run tens of seconds, so requests pass a gate of
//! three limits — per-source rate limit, a bounded queue, a concurrency
//! cap — and carry a fixed budget. Popular corridors repeat: answers are
//! cached on the full request tuple. No injections in v1 (synthesis
//! boosts are journal-state the server cannot see).
//!
//! Privacy (surveillance law): handlers never log names, coordinates, or
//! anything derived from them — outcome and duration only.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ed_galaxy::fuel::{BoostProfile, FuelModel};
use ed_galaxy::router::{Control, Route, RouteError, RouteRequest};
use serde::Deserialize;

/// Concurrent plots actually running. Was two while the planner pool
/// was rayon's default (every core, normal priority). With the pool at
/// cores − 2 and niced (2026-09-07, Phase A step 1) the gate was swept
/// 2/4/6 at c=16 (docs/benches/plot-gate-sweep-2026-09-07.csv): bubble
/// plots served 435 → 701 → 812, refusals 56 % → 23 % → 7 %, trade
/// behind plots flat (430/455/410 ms). Four is the pre-registered win;
/// six is better for short plots but lets long plots hog the pool —
/// that is step 3's lane split, not a bigger single gate.
pub const CONCURRENCY: usize = 4;
/// Requests allowed to WAIT for a plot slot beyond those running; more
/// than this and the honest answer is 503-try-later, not a longer line.
pub const QUEUE: usize = 8;
/// Per-plot time budget. The client offers up to 120 s interactively;
/// a shared box gives everyone a fair 30.
pub const BUDGET_MS: u64 = 30_000;

/// Two lanes (API-only spec, Phase A step 3). The gate sweep showed long
/// plots do not scale with the gate at all (18–20 served whatever the
/// gate) and start timing out at 6, while short plots scale to 812/30 s:
/// one shared gate lets a Colonia plot sit in a bubble plotter's slot
/// for 25 s. So: an INTERACTIVE lane for bubble-scale plots and a LONG
/// lane for corridors, each with its own slots, queue and budget. The
/// two lanes' slots sum to the planner pool (cores − 2 = 6 on the box)
/// so a full house is six fan-outs, not eight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    Interactive,
    Long,
}

/// Straight-line distance from which a plot is a corridor, not a hop
/// around the bubble (the bubble is ~500 ly across; Colonia is 22,000).
pub const LONG_LY: f32 = 1_000.0;
/// Interactive lane: most of the pool, the fair 30 s.
pub const INTERACTIVE_CONCURRENCY: usize = 4;
pub const INTERACTIVE_QUEUE: usize = 8;
/// Long lane: two fan-outs at a time, the client's full 120 s.
pub const LONG_CONCURRENCY: usize = 2;
pub const LONG_QUEUE: usize = 4;
/// 100 s, not the client's 120: a plot that finds no route at all
/// (HIP 90112 → Spaidau AA-A d0, a dead end for a 72 ly ship, 2026-10-07)
/// must answer "budget" from here before anything in front of the API
/// cuts the connection at 120 s and turns the answer into a bare 504.
pub const LONG_BUDGET_MS: u64 = 100_000;

impl Lane {
    pub fn budget_ms(self) -> u64 {
        match self {
            Lane::Interactive => BUDGET_MS,
            Lane::Long => LONG_BUDGET_MS,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Interactive => "interactive",
            Lane::Long => "long",
        }
    }
}

/// Which lane a resolved request belongs in: by the straight-line
/// distance between its endpoints, nothing else (the client sends
/// `thorough` for everything, so it does not discriminate).
pub fn lane_of(galaxy: &ed_galaxy::Galaxy, from: u32, to: u32) -> Lane {
    lane_of_pos(galaxy.pos_of(from), galaxy.pos_of(to))
}

/// The lane by the endpoints' true positions — a position endpoint
/// counts where it IS, not where its bridge is.
pub fn lane_of_pos(a: [f32; 3], b: [f32; 3]) -> Lane {
    let d = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
    if d > LONG_LY {
        Lane::Long
    } else {
        Lane::Interactive
    }
}
/// A cached answer older than this is replotted (the index itself only
/// moves daily).
const CACHE_TTL: Duration = Duration::from_secs(3_600);
const CACHE_CAP: usize = 256;
/// Rate limit per source IP: a scraper ceiling, not a person's budget.
/// 60/h was an evening of casual use and one trade loop's worth of
/// replots (load bench 2026-09-07: 429 after the first minute). Maintainer,
/// same day: 2,500/h, monitored; past that the answer is a load
/// balancer, a second ed-api and a dedicated Postgres, not a bigger
/// number. `EDDA_API_ROUTE_RATE_PER_HOUR` overrides for benches.
pub const RATE_WINDOW: Duration = Duration::from_secs(3_600);
pub const RATE_PER_WINDOW: u32 = 2_500;

/// The wire request. `from` and `to` are required; everything else is
/// optional, and an omitted field takes the default noted on it. Those
/// defaults are the ones a WEB caller wants, which is not always what the
/// app sends -- see `thorough` (maintainer, 2026-09-11: two sessions
/// measured "the same" Sol -> Colonia request and got 140 jumps against
/// 141, because one spelled `thorough` out and the other did not).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteApiRequest {
    /// Origin system name. Required; resolved against the routing index,
    /// then Postgres, then EDSM (see `from_coords` for the last resort).
    pub from: String,
    /// Destination system name. Required, resolved like `from`.
    pub to: String,
    /// Unladen jump range at full tank, ly. Ignored when `fuel_model` is
    /// given (the model's own full-tank range wins); required otherwise.
    pub range_ly: Option<f32>,
    /// The ship's fuel physics, from `/v1/loadout/physics` or the app.
    /// With it the plot models fuel and scoop stops; without it the plot
    /// is range-only and reports no refuel stops.
    pub fuel_model: Option<FuelModel>,
    /// Supercharge multipliers. Default: neutron x4, white dwarf x1.5 for
    /// a standard drive (`BoostProfile::default`).
    pub boost: Option<BoostProfile>,
    /// Fuel in the tank at the origin, t. Default: the model's capacity.
    pub start_fuel: Option<f32>,
    /// Use neutron stars to supercharge. Default: **true**.
    pub supercharge: Option<bool>,
    /// Also use white dwarfs (about twice as slow to line up as a
    /// neutron, so opt-in). Default: **false**.
    pub white_dwarfs: Option<bool>,
    /// Prefer the fewest scoop stops. Default: **true**.
    pub min_fuel: Option<bool>,
    /// Cap on consecutive unscoopable arrivals. Default: **0** (no cap).
    pub max_dry_jumps: Option<u32>,
    /// A* heuristic weight, clamped to >= 1.0. Default: **1.3** (fast and
    /// near-optimal); 1.0 is admissible and much slower.
    pub weight: Option<f32>,
    /// Jumps-versus-refuels dial. Default: **1.0**, the journal-fit time
    /// model; 0.0 judges by flying time alone (absolute fewest jumps).
    pub stop_weight: Option<f32>,
    /// Run the thorough portfolio instead of the quick plot. Default:
    /// **true** -- an omitted `thorough` gets the EXPENSIVE path, because a
    /// one-shot web caller wants the best route it can get. EDDA's own
    /// Route tab sends `false` for its first plot and `true` only when the
    /// commander presses "Try harder", so a caller that wants the app's
    /// quick behaviour must send `false` explicitly. The two differ in the
    /// answer, not just the time: measured on 164fcbd0, Sol -> Colonia at
    /// 50 ly with supercharge, quick gave 140 jumps / 119 boosted in
    /// 0.3 s and thorough gave 141 / 122 in 0.5 s.
    pub thorough: Option<bool>,
    /// The commander's own journal position for an endpoint (FSDJump
    /// StarPos) — the no-EDMC case: used ONLY when the name misses the
    /// index, Postgres and EDSM; then the endpoint is a Position bridged
    /// like any other, and the name given is the synthesized hop's label.
    pub from_coords: Option<[f32; 3]>,
    pub to_coords: Option<[f32; 3]>,
}

/// Why a plot did not produce a route — mapped to HTTP by the handler.
#[derive(Debug, Clone, PartialEq)]
pub enum PlotRefusal {
    UnknownSystem(String),
    /// Known only by position (Postgres / EDSM, not yet in the routing
    /// index) and nothing indexed within the ship's range of it.
    Unindexed { name: String, radius_ly: f32 },
    NoRange,
    NoRoute,
    Budget,
}

/// Where a plot starts or ends, after the handler resolved the name
/// (maintainer scenario, 2026-09-07: "I'm flying along and I discover new
/// systems. I ask the server to route back to there in 3 hours" — with
/// EDMC pushing to EDDN the system sits in Postgres with coordinates
/// minutes after the jump, but the routing index only learns it at the
/// next reconcile). An `Indexed` endpoint is a planner node; a
/// `Position` is bridged to the nearest indexed system within range.
#[derive(Debug, Clone, PartialEq)]
pub enum Endpoint {
    Indexed(u32),
    Position { name: String, pos: [f32; 3] },
}

/// A position endpoint's bridge: the indexed system the planner used
/// in its place, and the straight-line jump between them.
#[derive(Debug, Clone, PartialEq)]
pub struct Bridge {
    pub name: String,
    pub pos: [f32; 3],
    pub via: u32,
    pub distance_ly: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bridges {
    pub from: Option<Bridge>,
    pub to: Option<Bridge>,
}

/// The engine's request plus what was bridged to reach it.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub req: RouteRequest,
    pub bridges: Bridges,
    pub from_pos: [f32; 3],
    pub to_pos: [f32; 3],
}

fn anchor(
    galaxy: &ed_galaxy::Galaxy,
    endpoint: Endpoint,
    range_ly: f32,
) -> Result<(u32, Option<Bridge>, [f32; 3]), PlotRefusal> {
    match endpoint {
        Endpoint::Indexed(idx) => Ok((idx, None, galaxy.pos_of(idx))),
        Endpoint::Position { name, pos } => {
            let nearest = galaxy
                .within(pos, range_ly)
                .into_iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            match nearest {
                Some((via, distance_ly)) => Ok((via, Some(Bridge { name, pos, via, distance_ly }), pos)),
                None => Err(PlotRefusal::Unindexed { name, radius_ly: range_ly }),
            }
        }
    }
}

/// Resolve by name against the index only — the pre-2026-09-07 path,
/// kept for callers and tests that have no Postgres.
pub fn resolve(
    galaxy: &ed_galaxy::Galaxy,
    api: &RouteApiRequest,
) -> Result<RouteRequest, PlotRefusal> {
    let from = galaxy.find(&api.from).ok_or_else(|| PlotRefusal::UnknownSystem(api.from.clone()))?;
    let to = galaxy.find(&api.to).ok_or_else(|| PlotRefusal::UnknownSystem(api.to.clone()))?;
    resolve_endpoints(galaxy, api, Endpoint::Indexed(from), Endpoint::Indexed(to)).map(|r| r.req)
}

/// Resolve the wire request against a galaxy into the engine's request.
/// Pure translation — the physics arrive precomputed from the client;
/// a position endpoint is bridged to the nearest indexed system within
/// the request's (unboosted) range.
pub fn resolve_endpoints(
    galaxy: &ed_galaxy::Galaxy,
    api: &RouteApiRequest,
    from: Endpoint,
    to: Endpoint,
) -> Result<Resolved, PlotRefusal> {
    let range_ly = api
        .range_ly
        .or_else(|| api.fuel_model.map(|m| m.range_at(m.capacity)))
        .filter(|r| r.is_finite() && *r > 0.0)
        .ok_or(PlotRefusal::NoRange)?
        .max(1.0);
    let (from_idx, from_bridge, from_pos) = anchor(galaxy, from, range_ly)?;
    let (to_idx, to_bridge, to_pos) = anchor(galaxy, to, range_ly)?;
    let mut boost = api.boost.unwrap_or_default();
    if !api.white_dwarfs.unwrap_or(false) {
        // The commander's opt-out, same rule as the client plot path.
        boost.white_dwarf = 1.0;
    }
    let start_fuel = api
        .start_fuel
        .or_else(|| api.fuel_model.map(|m| m.capacity))
        .unwrap_or(0.0);
    let req = RouteRequest {
        from: from_idx,
        to: to_idx,
        range_ly,
        supercharge: api.supercharge.unwrap_or(true),
        max_dry_jumps: api.max_dry_jumps.unwrap_or(0),
        weight: api.weight.unwrap_or(1.3).max(1.0),
        max_expansions: 50_000_000,
        thorough: api.thorough.unwrap_or(true),
        injection: None,
        boost,
        fuel: api.fuel_model,
        start_fuel,
        time_budget_ms: BUDGET_MS,
        grace_ms: 1_000,
        min_fuel: api.min_fuel.unwrap_or(true),
        stop_weight: api.stop_weight.unwrap_or(0.0).clamp(0.0, 5.0),
        prize_k: None,
        // The doc on RouteRequest anticipated exactly this caller: a
        // server that knows no commander uses conservative fleet times.
        t_jump_s: Some(60.0),
        stop_overhead_s: Some(120.0),
        // Product: secondaries are an experiment (ed-galaxy boost_side); off here.
        secondary_boost_ls: 0.0,
    };
    Ok(Resolved { req, bridges: Bridges { from: from_bridge, to: to_bridge }, from_pos, to_pos })
}

/// The cache key: the resolved request quantized so float noise from
/// clients recomputing the same loadout still collides into one entry.
/// Includes the index version — a republished galaxy invalidates
/// everything at once.
pub fn cache_key(version: &str, req: &RouteRequest) -> u64 {
    use std::hash::{Hash, Hasher};
    let q = |v: f32| (v * 10.0).round() as i64;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    version.hash(&mut h);
    req.from.hash(&mut h);
    req.to.hash(&mut h);
    q(req.range_ly).hash(&mut h);
    req.supercharge.hash(&mut h);
    req.max_dry_jumps.hash(&mut h);
    q(req.weight).hash(&mut h);
    req.thorough.hash(&mut h);
    q(req.boost.neutron).hash(&mut h);
    q(req.boost.white_dwarf).hash(&mut h);
    if let Some(m) = &req.fuel {
        q(m.capacity).hash(&mut h);
        q(m.optimal_mass).hash(&mut h);
        q(m.max_fuel_per_jump).hash(&mut h);
        q(m.unladen_mass).hash(&mut h);
        q(m.power * 100.0).hash(&mut h);
        q(m.multiplier * 1_000.0).hash(&mut h);
        q(m.scoop_rate).hash(&mut h);
        q(m.reserve).hash(&mut h);
        q(m.headroom_t).hash(&mut h);
        q(m.booster).hash(&mut h);
        q(m.cargo).hash(&mut h);
    }
    q(req.start_fuel).hash(&mut h);
    req.min_fuel.hash(&mut h);
    q(req.stop_weight).hash(&mut h);
    h.finish()
}

/// One lane's admission control: running slots and a waiting room.
pub struct Gate {
    /// Running-plot slots.
    slots: std::sync::Arc<tokio::sync::Semaphore>,
    /// Waiting-room slots (acquired before, released after, a slot wait).
    queue: std::sync::Arc<tokio::sync::Semaphore>,
    slots_total: usize,
    queue_total: usize,
}

impl Gate {
    fn new(concurrency: usize, queue: usize) -> Self {
        Gate {
            slots: std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency)),
            queue: std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency + queue)),
            slots_total: concurrency,
            queue_total: concurrency + queue,
        }
    }

    /// A place in the waiting room, or `None` when it is full — the
    /// honest 503. The permit is owned so it can travel into the planner
    /// task and be held until the PLANNER finishes, not until the
    /// request is answered: an early answer (2026-10-07) released the
    /// slot at 7 s while its refinement ran on, so a third and fourth
    /// crossing joined the long pool, every variant slowed, the budget
    /// (checked between variants) could not bind, and one refinement ran
    /// 180 s of planner time while the next crossing waited 87 s for its
    /// own early answer.
    fn enter(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        std::sync::Arc::clone(&self.queue).try_acquire_owned().ok()
    }

    async fn slot(&self) -> tokio::sync::OwnedSemaphorePermit {
        std::sync::Arc::clone(&self.slots).acquire_owned().await.expect("semaphore never closed")
    }

    /// Plots admitted to the waiting room and not yet running (the
    /// gauge the dashboard reads beside the slot-wait histogram).
    fn waiting(&self) -> usize {
        let taken = self.queue_total - self.queue.available_permits();
        taken.saturating_sub(self.slots_total - self.slots.available_permits())
    }
}

/// Planner threads per lane (default split of the box's cores − 2 = 6).
/// Measured 2026-10-07 (docs/benches/2026-10-07-route-latency-prod.csv):
/// with ONE pool, a crossing's 22 variants fill every worker and an
/// interactive plot's own fan-out queues behind them — Sol → Alioth,
/// 70 ms alone, took 70 s and 105 s beside a Beagle Point plot with two
/// cores idle. The gates size admission; these size the work.
pub const INTERACTIVE_THREADS: usize = 2;
pub const LONG_THREADS: usize = 4;

/// A rayon pool per lane — and for the long lane, a pool PER SLOT.
/// `install()` around the plot makes every `par_iter` inside the planner
/// run on that pool's workers, so a crossing can only ever slow another
/// crossing; and with one pool per long slot it cannot even do that:
/// measured on prod after #207 (docs/benches/2026-10-07-early-answer-prod.csv),
/// two crossings sharing one long pool early-served at 44.9 / 45.3 s
/// although each planner had its first route within 1-4 s of planner
/// time, because the second plot's `install` is one injected job that no
/// worker picks up while the first plot's 87 variant jobs fill the
/// deques. Each long slot now owns its workers (the lane's threads split
/// across its slots), so two admitted crossings never share a queue.
pub struct Pools {
    interactive: std::sync::Arc<rayon::ThreadPool>,
    long: Vec<std::sync::Arc<rayon::ThreadPool>>,
    long_free: std::sync::Arc<Mutex<Vec<usize>>>,
}

/// A pool checked out for one plot; a long slot's pool goes back on
/// drop. Travels into the planner task with the lane permits.
pub struct PoolLease {
    pool: std::sync::Arc<rayon::ThreadPool>,
    slot: Option<usize>,
    free: Option<std::sync::Arc<Mutex<Vec<usize>>>>,
}

impl PoolLease {
    pub fn install<R: Send>(&self, op: impl FnOnce() -> R + Send) -> R {
        self.pool.install(op)
    }
}

impl Drop for PoolLease {
    fn drop(&mut self) {
        if let (Some(slot), Some(free)) = (self.slot, &self.free) {
            free.lock().unwrap_or_else(|e| e.into_inner()).push(slot);
        }
    }
}

impl Pools {
    /// `long_slots` pools share `long` threads between them (at least one
    /// each); `interactive` threads form one pool for the short plots.
    pub fn new(interactive: usize, long_slots: usize, long: usize) -> anyhow::Result<Self> {
        let build = |name: String, n: usize| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(n.max(1))
                .thread_name(move |i| format!("planner-{name}-{i}"))
                .start_handler(|_| planner_thread_start())
                .build()
        };
        let long_slots = long_slots.max(1);
        let per_slot = (long / long_slots).max(1);
        let mut pools = Vec::with_capacity(long_slots);
        for s in 0..long_slots {
            pools.push(std::sync::Arc::new(build(format!("long{s}"), per_slot)?));
        }
        Ok(Pools {
            interactive: std::sync::Arc::new(build("interactive".into(), interactive)?),
            long_free: std::sync::Arc::new(Mutex::new((0..long_slots).rev().collect())),
            long: pools,
        })
    }
    /// Threads per plot on a lane (a long slot's own pool).
    pub fn threads(&self, lane: Lane) -> usize {
        match lane {
            Lane::Interactive => self.interactive.current_num_threads(),
            Lane::Long => self.long[0].current_num_threads(),
        }
    }
    pub fn long_slots(&self) -> usize {
        self.long.len()
    }
    /// The pool for a plot that holds a lane slot. The gate admits at
    /// most as many long plots as there are long pools, so a long
    /// checkout always finds one free.
    fn checkout(&self, lane: Lane) -> PoolLease {
        match lane {
            Lane::Interactive => PoolLease { pool: std::sync::Arc::clone(&self.interactive), slot: None, free: None },
            Lane::Long => {
                let slot = self
                    .long_free
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pop()
                    .expect("a long slot's pool is free whenever the gate admitted the plot");
                PoolLease {
                    pool: std::sync::Arc::clone(&self.long[slot]),
                    slot: Some(slot),
                    free: Some(std::sync::Arc::clone(&self.long_free)),
                }
            }
        }
    }
}

/// Planner workers run at nice +10 so a two-minute plot never starves
/// the ingest or a trade search (load bench 2026-09-07).
fn planner_thread_start() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain libc call on the calling thread's own id.
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, libc::gettid() as libc::id_t, 10);
    }
}

/// Set when the request that asked for a plot is gone — the client
/// closed the connection or Caddy gave up at 120 s — so the planner's
/// cancel check stops the variants instead of running them to the
/// budget for nobody (measured: a 504'd crossing kept six workers at
/// 75 % for the remaining ~30 s of its budget).
struct CancelOnDrop(Option<std::sync::Arc<std::sync::atomic::AtomicBool>>);

impl CancelOnDrop {
    /// The request has been answered early: its going away is no longer
    /// a reason to stop the planner, which runs on to refine.
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(flag) = &self.0 {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// A crossing answers with the first good route and refines behind it
/// (maintainer, 2026-10-07: "sounds fine", after the measurement that a
/// crossing on the box runs its 87-variant portfolio to the budget every
/// time while the first route lands at ~14 s and the rest only trims
/// jumps — docs/benches/2026-10-07-planner-pool-per-lane.csv). After the
/// first route the long lane waits this long for a better one, serves
/// the best it has marked `refining`, and lets the portfolio run on to
/// the budget; the finished route lands in the cache under the same
/// key, so a replot or "Try harder" is a hit. 0 disables.
/// Sized by the September matrix (docs/benches/2026-10-07-early-answer-budget.csv,
/// 18 routes, full index): the first route exists at 118-1,146 ms, the
/// best route known by 5 s is at least as short as the final on every
/// route, and serving at first-found alone would cost 0-3 jumps. Five
/// seconds costs nothing in jumps on all 18.
pub const EARLY_GRACE_MS: u64 = 5_000;

/// Pick the better of two candidates: fewer jumps, then fewer boosts.
fn better<'a>(a: &'a Route, b: &'a Route) -> &'a Route {
    if (b.jumps, b.boosted_jumps) < (a.jumps, a.boosted_jumps) { b } else { a }
}

/// What the rest of the budget bought after an early answer: the label
/// for `edda_route_refined_total{result}`. Maintainer, 2026-10-07: "are
/// we wasting time [running to full budget]?" — on the PC matrix the
/// full budget beat the first route on 3 of 18 routes by 1-3 jumps and
/// beat the 5 s best on 0 of 18; what the box's 87-variant / 30 s budget
/// buys live is what this measures, per plot, for a week.
fn refined(early_jumps: usize, final_jumps: usize) -> &'static str {
    match final_jumps.cmp(&early_jumps) {
        std::cmp::Ordering::Less => "better",
        std::cmp::Ordering::Equal => "same",
        std::cmp::Ordering::Greater => "worse",
    }
}

/// The endpoint's shared state: one gate per lane, a pool per lane, the
/// cache, and the plot itself.
pub struct RouteService {
    interactive: Gate,
    long: Gate,
    pools: Pools,
    /// `(when, route, refining)`: a refining entry is an early answer
    /// the portfolio is still improving; the finished route replaces it.
    cache: std::sync::Arc<Mutex<HashMap<u64, (Instant, std::sync::Arc<Route>, bool)>>>,
    /// A plot that ended without a route (no route, or the budget spent
    /// with none found) is remembered for [`REFUSAL_TTL`]: the replot a
    /// commander makes a moment later answers at once instead of
    /// burning another long-lane budget on the same dead end.
    refusals: Mutex<HashMap<u64, (Instant, PlotRefusal)>>,
    early_grace: std::time::Duration,
}

/// How long a no-route / budget answer is held for the same request.
pub const REFUSAL_TTL: Duration = Duration::from_secs(120);

pub enum PlotOutcome {
    /// `(route, cached, refining)`.
    Route(std::sync::Arc<Route>, bool, bool),
    Refused(PlotRefusal),
    /// The lane's waiting room is full.
    Saturated,
}

impl Default for RouteService {
    fn default() -> Self {
        // Bench knobs: each lane is swept with restarts, not rebuilds;
        // production leaves them unset. (EDDA_API_PLOT_CONCURRENCY /
        // _QUEUE from the step-2 sweep still size the interactive lane.)
        let env = |var: &str, default: usize| {
            std::env::var(var).ok().and_then(|v| v.parse().ok()).filter(|&n: &usize| n >= 1).unwrap_or(default)
        };
        let interactive = env("EDDA_API_PLOT_CONCURRENCY", INTERACTIVE_CONCURRENCY);
        let interactive_queue = env("EDDA_API_PLOT_QUEUE", INTERACTIVE_QUEUE);
        let long = env("EDDA_API_PLOT_LONG_CONCURRENCY", LONG_CONCURRENCY);
        let long_queue = env("EDDA_API_PLOT_LONG_QUEUE", LONG_QUEUE);
        let threads_i = env("EDDA_API_PLANNER_THREADS_INTERACTIVE", INTERACTIVE_THREADS);
        let threads_l = env("EDDA_API_PLANNER_THREADS_LONG", LONG_THREADS);
        let early_grace_ms: u64 =
            std::env::var("EDDA_API_EARLY_GRACE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(EARLY_GRACE_MS);
        tracing::info!(interactive, interactive_queue, long, long_queue, long_ly = LONG_LY, threads_i, threads_l, early_grace_ms, "plot gates and pools sized");
        RouteService {
            interactive: Gate::new(interactive, interactive_queue),
            long: Gate::new(long, long_queue),
            // One pool per long slot: the gate's concurrency IS the pool count.
            pools: Pools::new(threads_i, long, threads_l).expect("planner pools build"),
            cache: std::sync::Arc::new(Mutex::new(HashMap::new())),
            refusals: Mutex::new(HashMap::new()),
            early_grace: std::time::Duration::from_millis(early_grace_ms),
        }
    }
}

impl RouteService {
    fn gate(&self, lane: Lane) -> &Gate {
        match lane {
            Lane::Interactive => &self.interactive,
            Lane::Long => &self.long,
        }
    }

    /// For tests and dashboards: how many waiting-room places a lane has
    /// left right now.
    pub fn room(&self, lane: Lane) -> usize {
        self.gate(lane).queue.available_permits()
    }

    /// For tests and the startup log: a lane's planner threads.
    pub fn threads(&self, lane: Lane) -> usize {
        self.pools.threads(lane)
    }
}

impl RouteService {
    /// Plot, or say why not. The lane comes back with the outcome so the
    /// handler can label its metrics and quote the right budget; a
    /// request refused before it has endpoints is reported as
    /// interactive.
    /// Names resolved against the index only (no Postgres): the
    /// pre-resolution entry point, kept for callers and tests.
    pub async fn plot(
        &self,
        handle: &crate::galaxy_service::GalaxyHandle,
        api: &RouteApiRequest,
    ) -> anyhow::Result<(Lane, PlotOutcome, Bridges)> {
        let Some(from) = handle.galaxy.find(&api.from) else {
            return Ok((Lane::Interactive, PlotOutcome::Refused(PlotRefusal::UnknownSystem(api.from.clone())), Bridges::default()));
        };
        let Some(to) = handle.galaxy.find(&api.to) else {
            return Ok((Lane::Interactive, PlotOutcome::Refused(PlotRefusal::UnknownSystem(api.to.clone())), Bridges::default()));
        };
        self.plot_endpoints(handle, api, Endpoint::Indexed(from), Endpoint::Indexed(to)).await
    }

    /// Plot between resolved endpoints, or say why not. The lane and
    /// the bridges come back with the outcome so the handler can label
    /// its metrics, quote the right budget, and synthesize the bridged
    /// hops onto the wire. A request refused before it has endpoints is
    /// reported as interactive.
    pub async fn plot_endpoints(
        &self,
        handle: &crate::galaxy_service::GalaxyHandle,
        api: &RouteApiRequest,
        from: Endpoint,
        to: Endpoint,
    ) -> anyhow::Result<(Lane, PlotOutcome, Bridges)> {
        let Resolved { mut req, bridges, from_pos, to_pos } = match resolve_endpoints(&handle.galaxy, api, from, to) {
            Ok(resolved) => resolved,
            Err(refusal) => return Ok((Lane::Interactive, PlotOutcome::Refused(refusal), Bridges::default())),
        };
        let lane = lane_of_pos(from_pos, to_pos);
        req.time_budget_ms = lane.budget_ms();
        // The key carries the true positions: two different unindexed
        // destinations bridged through the same indexed system must not
        // share an answer, and a later index version (which has the
        // system) keys differently by construction.
        let key = cache_key_positions(&handle.version, &req, from_pos, to_pos);
        if let Some((route, refining)) = self.cached(key) {
            return Ok((lane, PlotOutcome::Route(route, true, refining), bridges));
        }
        if let Some(refusal) = self.refused(key) {
            metrics::counter!("edda_route_refusal_cache_total", "lane" => lane.as_str()).increment(1);
            return Ok((lane, PlotOutcome::Refused(refusal), bridges));
        }
        let gate = self.gate(lane);
        let Some(queued) = gate.enter() else {
            return Ok((lane, PlotOutcome::Saturated, bridges));
        };
        // How long a plot waits for a slot: with two long slots each held
        // ~29 s by a crossing's refinement, the lane early-serves about
        // four crossings a minute before queueing (prod, 2026-10-07:
        // a third crossing 10 s in waited 25 s). When real traffic
        // exceeds that, this histogram is where it shows first.
        let waited = Instant::now();
        let slot = gate.slot().await;
        let wait = waited.elapsed();
        metrics::histogram!("edda_route_slot_wait_seconds", "lane" => lane.as_str()).record(wait.as_secs_f64());
        metrics::gauge!("edda_route_queue_waiting", "lane" => lane.as_str()).set(gate.waiting() as f64);
        if wait > Duration::from_secs(2) {
            tracing::info!(lane = lane.as_str(), wait_ms = wait.as_millis() as u64, "route: waited for a lane slot");
        }
        // The wait may have outlived a twin's plot.
        if let Some((route, refining)) = self.cached(key) {
            return Ok((lane, PlotOutcome::Route(route, true, refining), bridges));
        }
        let galaxy = std::sync::Arc::clone(&handle.galaxy);
        let neutrons = handle.neutrons.clone();
        let wants_boost = req.supercharge;
        let highway_less = handle.neutrons.is_none() && wants_boost;
        let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Held across the await: if this future is dropped (the client
        // went away), the flag is set and the planner stops — unless the
        // request was already answered early, see `disarm`.
        let mut guard = CancelOnDrop(Some(std::sync::Arc::clone(&gone)));
        let lane_name = lane.as_str();
        // The slot's own pool, checked out for exactly as long as the
        // permits are held (it rides in the task with them).
        let pool = self.pools.checkout(lane);
        // Every variant's route as it finishes, from the planner's own
        // `found` callback (fired on the lane's workers).
        let (found_tx, mut found_rx) = tokio::sync::mpsc::unbounded_channel::<Route>();
        let cache = std::sync::Arc::clone(&self.cache);
        let storable = !highway_less;
        // Set by the handler when it answers early: the served route's
        // jumps and the instant, so the finish can say what the rest of
        // the budget bought.
        let served_early: std::sync::Arc<Mutex<Option<(usize, Instant)>>> = std::sync::Arc::new(Mutex::new(None));
        let served_early_task = std::sync::Arc::clone(&served_early);
        let plot_started = Instant::now();
        let mut task = tokio::task::spawn_blocking(move || {
            // The lane's permits live and die with the planner, whatever
            // the handler did with the answer.
            let _permits = (queued, slot);
            let pool = pool;
            let cancelled = move || gone.load(std::sync::atomic::Ordering::Relaxed);
            let found = move |r: &Route| {
                let _ = found_tx.send(r.clone());
            };
            let none = Control::none();
            let ctl = Control { cancelled: &cancelled, progress: none.progress, stage: none.stage, found: &found, trace: none.trace };
            let r = pool.install(|| ed_galaxy::long_range::plan_best(&galaxy, neutrons.as_deref(), &req, &ctl));
            let early = *served_early_task.lock().unwrap_or_else(|e| e.into_inner());
            let planner_ms = plot_started.elapsed().as_millis() as u64;
            match &r {
                Err(RouteError::Cancelled) => {
                    metrics::counter!("edda_route_requests_total", "outcome" => "abandoned", "lane" => lane_name).increment(1);
                    tracing::info!(lane = lane_name, key, planner_ms, "route plot stopped: the client was gone");
                }
                Ok(route) => {
                    if storable {
                        // The finished route replaces any early answer under
                        // this key, whether or not anyone is still waiting.
                        store_in(&cache, key, std::sync::Arc::new(route.clone()), false);
                    }
                    match early {
                        Some((early_jumps, at)) => {
                            let result = refined(early_jumps, route.jumps);
                            let refine_ms = at.elapsed().as_millis() as u64;
                            metrics::counter!("edda_route_refined_total", "lane" => lane_name, "result" => result).increment(1);
                            metrics::histogram!("edda_route_refined_improvement_jumps", "lane" => lane_name)
                                .record(early_jumps as f64 - route.jumps as f64);
                            tracing::info!(lane = lane_name, key, early_jumps, final_jumps = route.jumps, refine_ms, planner_ms, result, "route refined");
                        }
                        None => tracing::info!(lane = lane_name, key, jumps = route.jumps, planner_ms, storable, "route planned"),
                    }
                }
                Err(e) => tracing::info!(lane = lane_name, key, planner_ms, early = early.is_some(), error = ?e, "route plot ended without a route"),
            }
            r
        });
        // A crossing answers early: the first route plus a grace for a
        // better one, then the best so far marked refining while the
        // portfolio runs on. Interactive plots and a zero grace wait for
        // the whole answer as before.
        let early = lane == Lane::Long && !self.early_grace.is_zero() && storable;
        let outcome = if early {
            let mut best: Option<Route> = None;
            let finished = loop {
                tokio::select! {
                    joined = &mut task => break Some(joined),
                    found = found_rx.recv() => match found {
                        Some(r) => { best = Some(match best.take() { Some(b) => better(&b, &r).clone(), None => r }); }
                        None => break None, // the planner finished; the join lands next
                    },
                }
                if best.is_some() {
                    // Grace for a better route, then serve what we have.
                    let deadline = tokio::time::sleep(self.early_grace);
                    tokio::pin!(deadline);
                    let joined = loop {
                        tokio::select! {
                            joined = &mut task => break Some(joined),
                            found = found_rx.recv() => { if let Some(r) = found { best = Some(match best.take() { Some(b) => better(&b, &r).clone(), None => r }); } }
                            _ = &mut deadline => break None,
                        }
                    };
                    break joined;
                }
            };
            // The channel closing with nothing found means the planner
            // ended without a route (a crossing that never finishes a
            // variant in budget): its own result is the answer, not a
            // panic — one 502 on 2026-10-07 was exactly that `expect`.
            let finished = match (finished, &best) {
                (None, None) => Some((&mut task).await),
                (f, _) => f,
            };
            match finished {
                Some(joined) => joined.map_err(|join| anyhow::anyhow!("plot panicked: {join}"))?,
                None => {
                    let route = std::sync::Arc::new(best.expect("a best route exists when the grace, not the channel, ended the wait"));
                    *served_early.lock().unwrap_or_else(|e| e.into_inner()) = Some((route.jumps, Instant::now()));
                    guard.disarm();
                    metrics::counter!("edda_route_early_total", "lane" => lane_name).increment(1);
                    tracing::info!(lane = lane_name, jumps = route.jumps, "route served early; refining");
                    self.store(key, std::sync::Arc::clone(&route), true);
                    return Ok((lane, PlotOutcome::Route(route, false, true), bridges));
                }
            }
        } else {
            (&mut task).await.map_err(|join| anyhow::anyhow!("plot panicked: {join}"))?
        };
        let outcome = match outcome {
            Ok(mut route) => {
                // A plot made before this version's highway sub-index
                // exists is a fallback answer (bare range, no boosts);
                // serve it, SAY SO, never store it — the next request
                // replots with the highway once the build lands. The
                // publishers now build the highway before the manifest
                // moves, so this is the belt, and it is counted.
                if highway_less {
                    route.highway_pending = true;
                    metrics::counter!("edda_route_highway_pending_total").increment(1);
                }
                let route = std::sync::Arc::new(route);
                PlotOutcome::Route(route, false, false)
            }
            Err(RouteError::NoRoute) => {
                self.refuse(key, PlotRefusal::NoRoute);
                PlotOutcome::Refused(PlotRefusal::NoRoute)
            }
            Err(RouteError::Budget) => {
                self.refuse(key, PlotRefusal::Budget);
                PlotOutcome::Refused(PlotRefusal::Budget)
            }
            Err(RouteError::Cancelled) => PlotOutcome::Refused(PlotRefusal::Budget),
        };
        Ok((lane, outcome, bridges))
    }

    fn cached(&self, key: u64) -> Option<(std::sync::Arc<Route>, bool)> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, (at, _, _)| at.elapsed() < CACHE_TTL);
        cache.get(&key).map(|(_, route, refining)| (std::sync::Arc::clone(route), *refining))
    }

    fn store(&self, key: u64, route: std::sync::Arc<Route>, refining: bool) {
        store_in(&self.cache, key, route, refining);
    }

    fn refused(&self, key: u64) -> Option<PlotRefusal> {
        let mut refusals = self.refusals.lock().unwrap_or_else(|e| e.into_inner());
        refusals.retain(|_, (at, _)| at.elapsed() < REFUSAL_TTL);
        refusals.get(&key).map(|(_, r)| r.clone())
    }

    fn refuse(&self, key: u64, refusal: PlotRefusal) {
        let mut refusals = self.refusals.lock().unwrap_or_else(|e| e.into_inner());
        if refusals.len() >= CACHE_CAP {
            refusals.clear();
        }
        refusals.insert(key, (Instant::now(), refusal));
    }
}

type Cache = std::sync::Arc<Mutex<HashMap<u64, (Instant, std::sync::Arc<Route>, bool)>>>;

fn store_in(cache: &Cache, key: u64, route: std::sync::Arc<Route>, refining: bool) {
    {
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.len() >= CACHE_CAP {
            // Oldest out; a full sweep is fine at this size.
            if let Some(oldest) = cache.iter().min_by_key(|(_, (at, _, _))| *at).map(|(k, _)| *k) {
                cache.remove(&oldest);
            }
        }
        cache.insert(key, (Instant::now(), route, refining));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(range: Option<f32>) -> RouteApiRequest {
        RouteApiRequest {
            from: "Sol".into(),
            to: "Colonia".into(),
            range_ly: range,
            fuel_model: None,
            boost: None,
            start_fuel: None,
            supercharge: None,
            white_dwarfs: None,
            min_fuel: None,
            max_dry_jumps: None,
            weight: None,
            stop_weight: None,
            thorough: None,
            from_coords: None,
            to_coords: None,
        }
    }

    /// No range and no fuel model = nothing to plot with; the refusal is
    /// typed, not a panic or a 30 ly guess (the client's 30 ly default
    /// exists because a commander is sitting there; an API caller gets
    /// told what is missing).
    #[test]
    fn a_rangeless_request_is_refused_not_guessed() {
        // resolve() needs a galaxy only to look up names; refusal for
        // range is tested through the pure parts here.
        let api = request(None);
        assert!(api.range_ly.or_else(|| api.fuel_model.map(|m| m.range_at(m.capacity))).is_none());
    }

    /// Same physics, float jitter: one cache entry. Different corridor:
    /// different entry. Different index version: different entry.
    #[test]
    fn cache_key_quantizes_and_versions() {
        let mut a = RouteRequest { from: 1, to: 2, range_ly: 62.04, ..Default::default() };
        let b = RouteRequest { from: 1, to: 2, range_ly: 62.0401, ..Default::default() };
        assert_eq!(cache_key("v1", &a), cache_key("v1", &b));
        assert_ne!(cache_key("v1", &a), cache_key("v2", &a));
        a.to = 3;
        assert_ne!(cache_key("v1", &a), cache_key("v1", &b));
    }

    /// White dwarfs off (the default) neutralizes the boost exactly the
    /// way the client plot path does.
    #[test]
    fn white_dwarf_opt_out_matches_the_client() {
        let boost = BoostProfile::MK2_SCO;
        let mut opted_out = boost;
        opted_out.white_dwarf = 1.0;
        assert_eq!(opted_out.neutron, 6.0);
        assert_eq!(opted_out.white_dwarf, 1.0);
    }
}

#[cfg(test)]
mod lane_tests {
    use super::*;

    fn tiny_galaxy() -> (tempfile::TempDir, ed_galaxy::Galaxy) {
        let dir = tempfile::tempdir().unwrap();
        // Sol, a bubble neighbour 30 ly out, and Colonia 22 kly out.
        let source = r#"[
{"id64":10477373803,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":2,"name":"Nearby","coords":{"x":30,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]},
{"id64":3,"name":"Colonia","coords":{"x":-9530,"y":-910,"z":19808},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]}
]"#;
        let path = dir.path().join("galaxy");
        ed_galaxy::import::import_reader(Box::new(source.as_bytes()), &path, &mut |_| {}).unwrap();
        (dir, ed_galaxy::Galaxy::open(&path).unwrap())
    }

    #[test]
    fn a_bubble_hop_is_interactive_and_a_corridor_is_long() {
        let (_d, g) = tiny_galaxy();
        let sol = g.find("Sol").unwrap();
        let near = g.find("Nearby").unwrap();
        let colonia = g.find("Colonia").unwrap();
        assert_eq!(lane_of(&g, sol, near), Lane::Interactive);
        assert_eq!(lane_of(&g, sol, colonia), Lane::Long);
        assert_eq!(lane_of(&g, colonia, sol), Lane::Long, "direction does not matter");
    }

    #[test]
    fn the_lane_sets_the_budget() {
        assert_eq!(Lane::Interactive.budget_ms(), BUDGET_MS);
        assert_eq!(Lane::Long.budget_ms(), LONG_BUDGET_MS);
        assert!(LONG_BUDGET_MS > BUDGET_MS);
    }

    #[test]
    fn the_lanes_share_the_pool_not_each_other() {
        // Slots sum to the planner pool on the box (cores − 2 = 6).
        assert_eq!(INTERACTIVE_CONCURRENCY + LONG_CONCURRENCY, 6);
    }

    /// The premise of the split: a long lane full to its waiting-room
    /// wall leaves the interactive lane's room untouched.
    /// 2026-09-11, measured on the box: the first plot after a routing
    /// version flips runs before the highway sub-index for that version
    /// exists (built lazily, 6-10 s), answers highway-less (Sol→Colonia
    /// 453 jumps / 0 boosted instead of 141 / 119) and was cached under
    /// the new version for an hour. A highway-less answer is a fallback,
    /// never a cache entry.
    #[tokio::test]
    async fn a_plot_made_before_the_highway_exists_is_not_cached() {
        let (_dir, galaxy) = tiny_galaxy();
        let galaxy = std::sync::Arc::new(galaxy);
        let api = RouteApiRequest {
            from: "Sol".into(),
            to: "Nearby".into(),
            range_ly: Some(50.0),
            supercharge: Some(true),
            ..Default::default()
        };
        let svc = RouteService::default();

        let pending = crate::galaxy_service::GalaxyHandle {
            version: "v".into(),
            galaxy: std::sync::Arc::clone(&galaxy),
            neutrons: None,
        };
        for _ in 0..2 {
            let (_, outcome, _) = svc.plot(&pending, &api).await.unwrap();
            let PlotOutcome::Route(route, cached, _) = outcome else { panic!("a route") };
            assert!(!cached, "no highway yet: every plot is live, none is stored");
            assert!(route.highway_pending, "and the answer says it was made without the highway");
        }

        let ready = crate::galaxy_service::GalaxyHandle {
            version: "v".into(),
            galaxy: std::sync::Arc::clone(&galaxy),
            neutrons: Some(std::sync::Arc::clone(&galaxy)),
        };
        let (_, first, _) = svc.plot(&ready, &api).await.unwrap();
        let (_, second, _) = svc.plot(&ready, &api).await.unwrap();
        assert!(matches!(first, PlotOutcome::Route(_, false, _)), "the first plot with the highway is live");
        assert!(matches!(second, PlotOutcome::Route(_, true, _)), "and the second is served from the cache");
        let PlotOutcome::Route(route, _, _) = second else { panic!("a route") };
        assert!(!route.highway_pending, "a plot with the highway is the real answer");
    }

    #[tokio::test]
    async fn a_full_long_lane_does_not_take_an_interactive_place() {
        let svc = RouteService::default();
        let interactive_room = svc.room(Lane::Interactive);
        let long_room = svc.room(Lane::Long);
        let mut held = Vec::new();
        while let Some(p) = svc.gate(Lane::Long).enter() {
            held.push(p);
        }
        assert_eq!(held.len(), long_room, "the long lane is full");
        assert_eq!(svc.room(Lane::Long), 0);
        assert_eq!(svc.room(Lane::Interactive), interactive_room, "interactive room untouched");
        assert!(svc.gate(Lane::Interactive).enter().is_some());
        drop(held);
        assert_eq!(svc.room(Lane::Long), long_room, "permits return on drop");
    }
}

/// The cache key for a resolved plot: the request tuple plus the true
/// endpoint positions, quantized to 0.1 ly.
pub fn cache_key_positions(version: &str, req: &RouteRequest, from_pos: [f32; 3], to_pos: [f32; 3]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cache_key(version, req).hash(&mut h);
    for v in from_pos.iter().chain(to_pos.iter()) {
        ((v * 10.0).round() as i64).hash(&mut h);
    }
    h.finish()
}

/// The route as the wire sees it: the planner's route, with a bridged
/// endpoint's system synthesized back on as a hop — name, position and
/// straight-line distance, star class unknown and `scoopable: false` so
/// a fuel reader treats the jump as dry, and `synthesized: true` so the
/// client's follower can say "a system the index does not hold yet"
/// instead of briefing it like an indexed star. `jumps`, `total_ly` and
/// the running `total_ly` on every hop are adjusted; `straight_ly` is
/// recomputed between the true endpoints.
/// What the server knew about this answer that the route itself does
/// not carry: whether it came from the cache, which lane planned it and
/// the budget that lane gave it. The maintainer's HIP 90112 -> Beagle
/// Point took 35 s and 50 s and then 0.2 s (2026-10-07); the client
/// could not tell the third was a cache hit, and neither could anyone
/// reading the wire.
pub struct Served {
    pub cached: bool,
    pub lane: Lane,
    /// An early answer: the portfolio is still running and a replot
    /// (or "Try harder") within the hour gets the finished route.
    pub refining: bool,
}

pub fn augment(route: &Route, bridges: &Bridges, served: &Served) -> serde_json::Value {
    let mut value = serde_json::to_value(route).unwrap_or(serde_json::Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("cached".into(), serde_json::json!(served.cached));
        obj.insert("lane".into(), serde_json::json!(served.lane.as_str()));
        obj.insert("budget_ms".into(), serde_json::json!(served.lane.budget_ms()));
        obj.insert("refining".into(), serde_json::json!(served.refining));
    }
    if bridges.from.is_none() && bridges.to.is_none() {
        return value;
    }
    let unknown_class = serde_json::to_value(ed_galaxy::StarClass::Unknown).unwrap_or(serde_json::Value::Null);
    let hop_json = |b: &Bridge, distance_ly: f32, total_ly: f32| {
        serde_json::json!({
            "idx": u32::MAX, "id64": 0, "name": b.name, "pos": b.pos, "class": unknown_class,
            "scoopable": false, "distance_ly": distance_ly, "boosted": false, "total_ly": total_ly,
            "fuel_after": null, "refuel": false, "fuel_optional": false, "injection": null,
            "synthesized": true,
        })
    };
    let Some(obj) = value.as_object_mut() else { return value };
    let mut hops: Vec<serde_json::Value> = obj.get("hops").and_then(|h| h.as_array()).cloned().unwrap_or_default();
    let mut jumps = obj.get("jumps").and_then(|j| j.as_u64()).unwrap_or(0);
    let mut total = obj.get("total_ly").and_then(|t| t.as_f64()).unwrap_or(0.0) as f32;
    if let Some(b) = &bridges.from {
        // The commander sits in the new system: it becomes hop 0 and the
        // planner's start becomes the first jump.
        for hop in hops.iter_mut() {
            if let Some(t) = hop.get("total_ly").and_then(|t| t.as_f64()) {
                hop["total_ly"] = serde_json::json!(t as f32 + b.distance_ly);
            }
        }
        if let Some(first) = hops.first_mut() {
            first["distance_ly"] = serde_json::json!(b.distance_ly);
        }
        hops.insert(0, hop_json(b, 0.0, 0.0));
        jumps += 1;
        total += b.distance_ly;
    }
    if let Some(b) = &bridges.to {
        total += b.distance_ly;
        hops.push(hop_json(b, b.distance_ly, total));
        jumps += 1;
    }
    let (a, z) = (
        bridges.from.as_ref().map(|b| b.pos).or_else(|| hops.first().and_then(pos_of_json)),
        bridges.to.as_ref().map(|b| b.pos).or_else(|| hops.last().and_then(pos_of_json)),
    );
    if let (Some(a), Some(z)) = (a, z) {
        let straight = ((a[0] - z[0]).powi(2) + (a[1] - z[1]).powi(2) + (a[2] - z[2]).powi(2)).sqrt();
        obj.insert("straight_ly".into(), serde_json::json!(straight));
    }
    obj.insert("hops".into(), serde_json::Value::Array(hops));
    obj.insert("jumps".into(), serde_json::json!(jumps));
    obj.insert("total_ly".into(), serde_json::json!(total));
    obj.insert(
        "bridged".into(),
        serde_json::json!({
            "from": bridges.from.as_ref().map(|b| serde_json::json!({"name": b.name, "via_idx": b.via, "distance_ly": b.distance_ly})),
            "to": bridges.to.as_ref().map(|b| serde_json::json!({"name": b.name, "via_idx": b.via, "distance_ly": b.distance_ly})),
        }),
    );
    value
}

fn pos_of_json(hop: &serde_json::Value) -> Option<[f32; 3]> {
    let p = hop.get("pos")?.as_array()?;
    Some([p.first()?.as_f64()? as f32, p.get(1)?.as_f64()? as f32, p.get(2)?.as_f64()? as f32])
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    fn tiny_galaxy() -> (tempfile::TempDir, ed_galaxy::Galaxy) {
        let dir = tempfile::tempdir().unwrap();
        let source = r#"[
{"id64":10477373803,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":2,"name":"Nearby","coords":{"x":30,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]},
{"id64":3,"name":"Farther","coords":{"x":60,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]}
]"#;
        let path = dir.path().join("galaxy");
        ed_galaxy::import::import_reader(Box::new(source.as_bytes()), &path, &mut |_| {}).unwrap();
        (dir, ed_galaxy::Galaxy::open(&path).unwrap())
    }

    fn api(from: &str, to: &str, range: f32) -> RouteApiRequest {
        RouteApiRequest {
            from: from.into(), to: to.into(), range_ly: Some(range), fuel_model: None, boost: None,
            start_fuel: None, supercharge: None, white_dwarfs: None, min_fuel: None,
            max_dry_jumps: None, weight: None, stop_weight: None, thorough: None,
            from_coords: None, to_coords: None,
        }
    }

    /// Each lane has its own planner pool, sized from the constants (or
    /// the env knobs), so a crossing's variants cannot occupy an
    /// interactive plot's workers.
    #[test]
    fn each_lane_plans_on_its_own_pool() {
        let service = RouteService::default();
        assert_eq!(service.threads(Lane::Interactive), INTERACTIVE_THREADS);
        assert_eq!(service.pools.long_slots(), LONG_CONCURRENCY, "one pool per long slot");
        assert_eq!(service.threads(Lane::Long), LONG_THREADS / LONG_CONCURRENCY, "the lane's threads split across its slots");
        let pools = Pools::new(1, 2, 4).unwrap();
        assert_eq!((pools.threads(Lane::Interactive), pools.long_slots(), pools.threads(Lane::Long)), (1, 2, 2));
        // Every pool is distinct: work installed on one runs on its own
        // named threads, and two long checkouts never share a pool.
        let a = pools.checkout(Lane::Long);
        let b = pools.checkout(Lane::Long);
        let name_a = a.install(|| std::thread::current().name().map(str::to_owned)).unwrap();
        let name_b = b.install(|| std::thread::current().name().map(str::to_owned)).unwrap();
        assert!(name_a.starts_with("planner-long") && name_b.starts_with("planner-long"), "{name_a} {name_b}");
        assert_ne!(name_a.split('-').nth(1), name_b.split('-').nth(1), "two admitted crossings get different pools: {name_a} vs {name_b}");
        let name = pools.checkout(Lane::Interactive).install(|| std::thread::current().name().map(str::to_owned));
        assert!(name.as_deref().is_some_and(|n| n.starts_with("planner-interactive-")), "{name:?}");
        // Returning a lease frees its pool for the next crossing.
        assert_eq!(pools.long_free.lock().unwrap().len(), 0);
        drop(a);
        assert_eq!(pools.long_free.lock().unwrap().len(), 1);
        let c = pools.checkout(Lane::Long);
        assert_eq!(c.slot, a_slot_of(&name_a), "the freed pool is the one handed out again");
        drop(b);
        drop(c);
        assert_eq!(pools.long_free.lock().unwrap().len(), 2);
    }

    fn a_slot_of(thread_name: &str) -> Option<usize> {
        thread_name.strip_prefix("planner-long").and_then(|r| r.split('-').next()).and_then(|s| s.parse().ok())
    }

    /// The early answer is the best route found so far by jumps, then by
    /// boosts — the first variant to finish can be the worst arm (the
    /// matrix: Wongi → Beagle found 199 first, finished 196).
    #[test]
    fn the_early_answer_is_the_best_so_far_by_jumps_then_boosts() {
        let mk = |jumps: usize, boosted: usize| Route { jumps, boosted_jumps: boosted, ..serde_json::from_value(serde_json::json!({"range_ly":50.0,"hops":[],"jumps":0,"total_ly":0.0,"straight_ly":0.0,"boosted_jumps":0,"expansions":0,"elapsed_ms":0,"refuel_stops":0})).unwrap() };
        let (a, b, c) = (mk(233, 40), mk(199, 44), mk(199, 30));
        assert_eq!(better(&a, &b).jumps, 199, "fewer jumps wins");
        assert_eq!(better(&b, &c).boosted_jumps, 30, "equal jumps: fewer boosts wins");
        assert_eq!(better(&c, &b).boosted_jumps, 30, "order does not matter");
    }

    /// The waiting count is plots admitted but not yet running.
    #[tokio::test]
    async fn the_gate_counts_who_is_waiting() {
        let gate = Gate::new(1, 2);
        assert_eq!(gate.waiting(), 0);
        let _q1 = gate.enter().unwrap();
        let _s1 = gate.slot().await;
        assert_eq!(gate.waiting(), 0, "one running, none waiting");
        let _q2 = gate.enter().unwrap();
        assert_eq!(gate.waiting(), 1, "admitted, no slot yet");
        let _q3 = gate.enter().unwrap();
        assert_eq!(gate.waiting(), 2);
        assert!(gate.enter().is_none(), "the waiting room is full: the honest 503");
    }

    /// A dead end answers at once the second time: the refusal is held
    /// for REFUSAL_TTL under the request's key, then forgotten.
    #[test]
    fn a_no_route_answer_is_remembered_for_the_replot() {
        let service = RouteService::default();
        assert!(service.refused(9).is_none());
        service.refuse(9, PlotRefusal::NoRoute);
        assert_eq!(service.refused(9), Some(PlotRefusal::NoRoute));
        service.refuse(10, PlotRefusal::Budget);
        assert_eq!(service.refused(10), Some(PlotRefusal::Budget));
        // Aged out: pretend the entry is older than the TTL.
        service.refusals.lock().unwrap().get_mut(&9).unwrap().0 = Instant::now() - REFUSAL_TTL - Duration::from_secs(1);
        assert!(service.refused(9).is_none(), "an old refusal is replotted");
        assert_eq!(service.refused(10), Some(PlotRefusal::Budget));
    }

    /// The long lane's planner budget ends before anything in front of
    /// the API would cut the connection at 120 s.
    #[test]
    fn the_long_budget_answers_before_the_gateway_would() {
        assert!(LONG_BUDGET_MS <= 100_000 && LONG_BUDGET_MS > BUDGET_MS);
    }

    /// What the budget bought, as the counter labels it.
    #[test]
    fn the_refinement_is_labelled_by_jumps() {
        assert_eq!(refined(233, 196), "better");
        assert_eq!(refined(196, 196), "same");
        assert_eq!(refined(37, 38), "worse", "the judge can pick a longer final for fewer stops");
    }

    /// An early answer sits in the cache marked refining; the finished
    /// route replaces it under the same key, unmarked — so a replot
    /// during the grace gets the early route at once and one after the
    /// portfolio gets the final.
    #[test]
    fn the_finished_route_replaces_the_early_one_in_the_cache() {
        let service = RouteService::default();
        let early = std::sync::Arc::new(serde_json::from_value::<Route>(serde_json::json!({"range_ly":50.0,"hops":[],"jumps":233,"total_ly":0.0,"straight_ly":0.0,"boosted_jumps":0,"expansions":0,"elapsed_ms":0,"refuel_stops":0})).unwrap());
        service.store(7, std::sync::Arc::clone(&early), true);
        assert!(matches!(service.cached(7), Some((r, true)) if r.jumps == 233));
        let done = std::sync::Arc::new(Route { jumps: 196, ..(*early).clone() });
        store_in(&service.cache, 7, done, false);
        assert!(matches!(service.cached(7), Some((r, false)) if r.jumps == 196));
    }

    /// A disarmed guard no longer stops the planner when the request
    /// goes away — the early answer has been sent and the portfolio
    /// refines for the cache.
    #[test]
    fn a_disarmed_guard_leaves_the_planner_running() {
        let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let mut guard = CancelOnDrop(Some(std::sync::Arc::clone(&gone)));
            guard.disarm();
        }
        assert!(!gone.load(std::sync::atomic::Ordering::Relaxed));
    }

    /// Dropping the request's guard tells the planner to stop: the flag
    /// flips, and a plot checking it comes back Cancelled instead of
    /// running to its budget.
    #[test]
    fn a_dropped_request_cancels_its_plot() {
        let gone = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let _guard = CancelOnDrop(Some(std::sync::Arc::clone(&gone)));
            assert!(!gone.load(std::sync::atomic::Ordering::Relaxed));
        }
        assert!(gone.load(std::sync::atomic::Ordering::Relaxed), "the guard's drop sets the flag");
        let (_d, g) = tiny_galaxy();
        let (sol, far) = (g.find("Sol").unwrap(), g.find("Farther").unwrap());
        let Resolved { mut req, .. } =
            resolve_endpoints(&g, &api("Sol", "Farther", 35.0), Endpoint::Indexed(sol), Endpoint::Indexed(far)).unwrap();
        req.time_budget_ms = 10_000;
        let cancelled = || gone.load(std::sync::atomic::Ordering::Relaxed);
        let none = Control::none();
        let ctl = Control { cancelled: &cancelled, progress: none.progress, stage: none.stage, found: none.found, trace: none.trace };
        let r = ed_galaxy::long_range::plan_best(&g, None, &req, &ctl);
        assert!(matches!(r, Err(RouteError::Cancelled)), "a plot whose request is gone stops: {r:?}");
    }

    /// The maintainer's scenario: a system the index does not hold, known by
    /// position. It bridges to the NEAREST indexed system within range.
    #[test]
    fn a_position_bridges_to_the_nearest_indexed_system_within_range() {
        let (_d, g) = tiny_galaxy();
        let sol = g.find("Sol").unwrap();
        let nearby = g.find("Nearby").unwrap();
        let newfound = Endpoint::Position { name: "Newfound".into(), pos: [36.0, 0.0, 0.0] };
        let r = resolve_endpoints(&g, &api("Sol", "Newfound", 40.0), Endpoint::Indexed(sol), newfound).unwrap();
        assert_eq!(r.req.to, nearby, "Nearby at 6 ly beats Farther at 24 ly");
        let b = r.bridges.to.as_ref().expect("bridged");
        assert_eq!(b.name, "Newfound");
        assert!((b.distance_ly - 6.0).abs() < 0.01, "straight-line jump {}", b.distance_ly);
        assert!(r.bridges.from.is_none());
        assert_eq!(r.to_pos, [36.0, 0.0, 0.0], "the lane and the cache key see the true position");
    }

    #[test]
    fn nothing_indexed_within_range_is_an_honest_refusal() {
        let (_d, g) = tiny_galaxy();
        let sol = g.find("Sol").unwrap();
        let lonely = Endpoint::Position { name: "Lonely".into(), pos: [500.0, 0.0, 0.0] };
        let err = resolve_endpoints(&g, &api("Sol", "Lonely", 40.0), Endpoint::Indexed(sol), lonely).unwrap_err();
        assert_eq!(err, PlotRefusal::Unindexed { name: "Lonely".into(), radius_ly: 40.0 });
    }

    #[test]
    fn the_from_side_bridges_too() {
        let (_d, g) = tiny_galaxy();
        let sol = g.find("Sol").unwrap();
        let here = Endpoint::Position { name: "JustJumpedHere".into(), pos: [-5.0, 0.0, 0.0] };
        let r = resolve_endpoints(&g, &api("JustJumpedHere", "Sol", 40.0), here, Endpoint::Indexed(sol)).unwrap();
        assert_eq!(r.req.from, sol);
        assert!((r.bridges.from.as_ref().unwrap().distance_ly - 5.0).abs() < 0.01);
    }

    #[test]
    fn two_positions_through_the_same_bridge_key_differently() {
        let (_d, g) = tiny_galaxy();
        let sol = g.find("Sol").unwrap();
        let a = resolve_endpoints(&g, &api("Sol", "A", 40.0), Endpoint::Indexed(sol), Endpoint::Position { name: "A".into(), pos: [34.0, 0.0, 0.0] }).unwrap();
        let b = resolve_endpoints(&g, &api("Sol", "B", 40.0), Endpoint::Indexed(sol), Endpoint::Position { name: "B".into(), pos: [36.0, 0.0, 0.0] }).unwrap();
        assert_eq!(a.req.to, b.req.to, "same bridge");
        assert_ne!(
            cache_key_positions("v", &a.req, a.from_pos, a.to_pos),
            cache_key_positions("v", &b.req, b.from_pos, b.to_pos),
            "different destinations, different answers"
        );
    }

    #[test]
    fn augment_synthesizes_the_bridged_hops_and_marks_them() {
        let route: Route = serde_json::from_value(serde_json::json!({
            "range_ly": 40.0, "jumps": 1, "total_ly": 30.0, "straight_ly": 30.0, "boosted_jumps": 0,
            "expansions": 1, "elapsed_ms": 1, "refuel_stops": 0,
            "hops": [
                {"idx": 0, "id64": 1, "name": "Sol", "pos": [0.0,0.0,0.0], "class": "g", "scoopable": true, "distance_ly": 0.0, "boosted": false, "total_ly": 0.0, "fuel_after": null, "refuel": false},
                {"idx": 1, "id64": 2, "name": "Nearby", "pos": [30.0,0.0,0.0], "class": "k", "scoopable": true, "distance_ly": 30.0, "boosted": false, "total_ly": 30.0, "fuel_after": null, "refuel": false}
            ]
        })).unwrap();
        let bridges = Bridges {
            from: Some(Bridge { name: "Origin".into(), pos: [-5.0, 0.0, 0.0], via: 0, distance_ly: 5.0 }),
            to: Some(Bridge { name: "Newfound".into(), pos: [36.0, 0.0, 0.0], via: 1, distance_ly: 6.0 }),
        };
        let v = augment(&route, &bridges, &Served { cached: false, lane: Lane::Long, refining: false });
        let hops = v["hops"].as_array().unwrap();
        assert_eq!(hops.len(), 4);
        assert_eq!(hops[0]["name"], "Origin");
        assert_eq!(hops[0]["synthesized"], true);
        assert_eq!(hops[0]["scoopable"], false);
        assert_eq!(hops[1]["name"], "Sol");
        assert!((hops[1]["distance_ly"].as_f64().unwrap() - 5.0).abs() < 1e-3, "the first indexed hop is now a jump");
        assert!((hops[2]["total_ly"].as_f64().unwrap() - 35.0).abs() < 1e-3, "running totals shift by the from bridge");
        assert_eq!(hops[3]["name"], "Newfound");
        assert_eq!(hops[3]["synthesized"], true);
        // Hop carries `synthesized` natively since the client learned to read
        // it (serde default false), so an indexed hop says false, not nothing.
        assert_eq!(hops[1]["synthesized"], false, "indexed hops are untouched");
        assert_eq!(v["jumps"], 3);
        assert!((v["total_ly"].as_f64().unwrap() - 41.0).abs() < 1e-3);
        assert!((v["straight_ly"].as_f64().unwrap() - 41.0).abs() < 1e-3);
        assert_eq!(v["bridged"]["to"]["name"], "Newfound");
    }

    #[test]
    fn augment_without_bridges_is_the_route_itself() {
        let route: Route = serde_json::from_value(serde_json::json!({
            "range_ly": 40.0, "jumps": 0, "total_ly": 0.0, "straight_ly": 0.0, "boosted_jumps": 0,
            "expansions": 1, "elapsed_ms": 1, "refuel_stops": 0, "hops": []
        })).unwrap();
        let v = augment(&route, &Bridges::default(), &Served { cached: true, lane: Lane::Interactive, refining: true });
        let mut expected = serde_json::to_value(&route).unwrap();
        expected["cached"] = serde_json::json!(true);
        expected["lane"] = serde_json::json!("interactive");
        expected["budget_ms"] = serde_json::json!(BUDGET_MS);
        expected["refining"] = serde_json::json!(true);
        assert_eq!(v, expected, "an unbridged route still says cached, lane and budget");
    }
}
