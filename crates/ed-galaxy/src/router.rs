//! Jump-route planning over the star index.
//!
//! Point-to-point is A* on an implicit graph: a node is a system, its
//! neighbours are every system within the current jump range, cost is one
//! per jump, and the heuristic is straight-line distance divided by the
//! best range the ship can reach -- the standard "minimise jumps" plot the
//! galaxy map does, plus what it does not: **supercharging**. Leaving a
//! neutron star the next jump is 4× range, a white dwarf 1.5×, when the
//! request allows it. The heuristic uses the *plain* range so it stays
//! close to admissible over the long stretches where no boost is on offer;
//! with `weight > 1` it becomes weighted A*, which trades exactness for
//! speed on long routes -- the same trade Spansh makes.
//!
//! What this does **not** model yet: fuel. Every hop reports whether its
//! star is scoopable, and `max_dry_jumps` refuses paths that string more
//! than N unscoopable arrivals together, which is the practical guard. A
//! proper fuel-state search (mass changing range, refuel timing) is where
//! the ant-colony planner takes over.

use crate::format::{dist, Galaxy};
use crate::fuel::{BoostProfile, FuelModel};
use crate::star::{StarClass, StarClassCode as _};
use serde::Serialize;
use std::cmp::Ordering;
use rustc_hash::FxHashMap as HashMap;
use std::collections::BinaryHeap;

#[derive(Debug, Clone, Serialize)]
pub struct RouteRequest {
    pub from: u32,
    pub to: u32,
    /// Unboosted jump range in ly.
    pub range_ly: f32,
    /// Use neutron stars and white dwarfs to supercharge.
    pub supercharge: bool,
    /// Longest run of consecutive unscoopable arrivals allowed. 0 = no limit.
    pub max_dry_jumps: u32,
    /// Weighted-A* factor; 1.0 is exact, 1.5 is much faster on long routes.
    pub weight: f32,
    /// Give up after this many expansions (0 = no limit).
    pub max_expansions: u64,
    /// Exact search: weight 1.0 and a heuristic that assumes the best boost
    /// is always available (admissible), so no boost-heavy shortcut is
    /// missed. Much slower on long routes.
    pub thorough: bool,
    /// Supercharge multipliers for this drive.
    pub boost: BoostProfile,
    /// Fuel physics. `None` = ignore fuel (range fixed at `range_ly`).
    pub fuel: Option<FuelModel>,
    /// Tonnes aboard at departure.
    pub start_fuel: f32,
    /// FSD injection allowed as a last resort: (range multiplier, grade
    /// name, how many the commander can synthesise). An injected jump is
    /// charged three jumps' worth of cost, so it is only taken across a gap.
    pub injection: Option<(f32, &'static str, u32)>,
    /// Wall-clock budget for a long plot in ms (0 = none): every variant
    /// that has not finished by then is dropped and the best finished one
    /// is the answer.
    pub time_budget_ms: u64,
    /// Long-plot variants run in parallel; once one has a route, the rest
    /// get this long to finish (they may still win) and are then
    /// cancelled. 0 = wait for every variant (the same answer, later).
    pub grace_ms: u64,
    /// Only stop for fuel when the tank requires it (strictly opt-in,
    /// like white dwarfs): the plan keeps the same hops but drops every
    /// top-up the remaining legs do not need, keeping one max-effort
    /// escape jump (`max_fuel_per_jump`) in hand at every arrival where
    /// a scoop stop can provide it. Off, the plan tops up at every
    /// scoopable arrival below capacity.
    pub min_fuel: bool,
    /// Jumps-vs-refuels dial for route judging: multiplies the refuel
    /// term (stop overhead + fill time) of the pilot-seconds score.
    /// 1.0 = the journal-fit model as measured; 0.0 = judge by flying
    /// time alone (absolute fewest jumps, the "try hard" preset);
    /// higher trades jumps for fewer stops.
    pub stop_weight: f32,
    /// Per-plot override of the wave-continuation break-even bar
    /// (`None` = the ED_PRIZE_K env / 1.0 default). 0.0 keeps waves
    /// digging while the budget lasts, regardless of expected payoff.
    pub prize_k: Option<f32>,
    /// Item 28, per-commander time models: seconds one jump costs THIS
    /// pilot, fitted from their own journal cadence. `None` falls back
    /// to ED_TJUMP_S, then the built-in 70 s. The public website/server
    /// passes a conservative 60 s explicitly; the app passes the
    /// commander's per-ship fit once enough journal history exists.
    pub t_jump_s: Option<f32>,
    /// Seconds of approach overhead one fuel stop costs this pilot
    /// (the fill itself is priced separately from tonnage and scoop
    /// rate). `None` = ED_STOP_OVERHEAD_S, then 36 s; the public
    /// default passes 120 s of total conservatism explicitly.
    pub stop_overhead_s: Option<f32>,
    /// Experiment (2026-09-10): supercharge from a neutron star or white
    /// dwarf that is not the arrival star when it sits within this many
    /// light seconds of arrival (`boost.bin`, [`crate::boost_side`]). 0
    /// = off, the product's behaviour. The hop records the run so a
    /// judge can price it; the search itself still counts jumps.
    pub secondary_boost_ls: f32,
}

impl Default for RouteRequest {
    fn default() -> Self {
        RouteRequest { from: 0, to: 0, range_ly: 30.0, supercharge: true, max_dry_jumps: 0, weight: 1.3, max_expansions: 0, thorough: false, boost: BoostProfile::default(), fuel: None, start_fuel: 0.0, injection: None, time_budget_ms: 0, grace_ms: 0, min_fuel: false, stop_weight: 1.0, prize_k: None, t_jump_s: None, stop_overhead_s: None, secondary_boost_ls: 0.0 }
    }
}

/// Extra cost of an injected jump, in jumps: materials are finite and a
/// route should only spend them where nothing else crosses.
const INJECTION_PENALTY: u32 = 3;

/// FSD injection synthesis, best grade first: (range multiplier, grade,
/// one unit of each material). Per Inara; an injection does not stack
/// with a neutron or white dwarf supercharge, so it only ever helps a
/// jump that would otherwise be unboosted.
pub const INJECTION_RECIPES: [(f32, &str, &[&str]); 3] = [
    (2.0, "premium", &["carbon", "germanium", "arsenic", "niobium", "yttrium", "polonium"]),
    (1.5, "standard", &["carbon", "vanadium", "germanium", "cadmium", "niobium"]),
    (1.25, "basic", &["carbon", "vanadium", "germanium"]),
];

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Hop {
    pub idx: u32,
    pub id64: u64,
    pub name: String,
    pub pos: [f32; 3],
    pub class: StarClass,
    pub scoopable: bool,
    /// Distance jumped to get here.
    pub distance_ly: f32,
    /// This jump used a supercharge from the previous star.
    pub boosted: bool,
    /// Cumulative from the start.
    pub total_ly: f32,
    /// Tonnes in the tank on arrival (after scooping, if this star is
    /// scoopable). `None` when no fuel model was used.
    pub fuel_after: Option<f32>,
    /// The plan expects you to scoop here.
    pub refuel: bool,
    /// Not a node of the index: the server bridged a position it only
    /// knew from Postgres or EDSM (or the client's journal) to the
    /// nearest indexed system and synthesized this hop — star class
    /// unknown, scoopable false, fuel logic treats it as a dry jump.
    /// Wire-only from older servers/clients: absent means false.
    #[serde(default)]
    pub synthesized: bool,
    /// Item 39: this stop is an eager comfort top-up the tank does not
    /// need — fuel is AVAILABLE here, not required. Set on eager plans
    /// by the pruned-truth simulation; always false on min-fuel plans,
    /// whose surviving stops are all load-bearing by construction.
    #[serde(default)]
    pub fuel_optional: bool,
    /// Synthesise this FSD injection before the jump to this hop.
    #[serde(default)]
    pub injection: Option<String>,
    /// The boost into this hop came from a star that was not the
    /// previous system's arrival star: how far the commander must
    /// supercruise there first, light seconds. Experiment; `None` in
    /// product plans.
    #[serde(default)]
    pub via_secondary_ls: Option<f32>,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct Route {
    /// The unboosted range the plan was made with.
    pub range_ly: f32,
    pub hops: Vec<Hop>,
    pub jumps: usize,
    pub total_ly: f32,
    pub straight_ly: f32,
    pub boosted_jumps: usize,
    pub expansions: u64,
    pub elapsed_ms: u64,
    /// Scoop stops the plan relies on.
    pub refuel_stops: usize,
    /// Hops that need an FSD injection synthesised first.
    #[serde(default)]
    pub injections: usize,
    /// Hops boosted from a secondary star (experiment; 0 in product plans).
    #[serde(default)]
    pub secondary_boosts: usize,
    /// The ship (journal ShipID) and its label this plan was made for; a
    /// route is only valid for the ship whose fuel model produced it.
    #[serde(default)]
    pub ship_id: Option<i64>,
    #[serde(default)]
    pub ship: Option<String>,
    /// Long-plot variants launched and the number that finished with a
    /// route (the rest were cancelled by the grace rule or the budget).
    /// Stable across runs, unlike wall-clock; the pins assert on it.
    #[serde(default)]
    pub variants_run: u32,
    #[serde(default)]
    pub variants_finished: u32,
    /// Whether the ship this plan was made for carries a fuel scoop, when
    /// the app knows (`None` for a plan without a ship). The planner still
    /// assumes scooping; a route with refuel stops cannot be followed
    /// without one.
    #[serde(default)]
    pub ship_has_scoop: Option<bool>,
    /// The drive's integrity when the plan was made (0..1, from the ship's
    /// latest Loadout) and what each supercharge costs it, so a reader can
    /// project integrity along the route and say where a repair is due.
    /// Not a planning input: the loss is a static value per drive.
    #[serde(default)]
    pub fsd_integrity: Option<f32>,
    #[serde(default)]
    pub integrity_loss_per_boost: Option<f32>,
    /// Whether the ship carries an AFMU (`int_repairer_*`): a repair on the
    /// way can then happen in flight rather than at a station.
    #[serde(default)]
    pub ship_has_afmu: Option<bool>,
}

/// Rewrite a plan's scoop stops to the fewest the tank needs
/// (`RouteRequest::min_fuel`). The hops are fixed; only the stop
/// placement changes: simulate the route without scooping, and when a
/// jump would land under the floor -- one max-effort escape jump
/// (`max_fuel_per_jump`), or the model's own reserve if higher -- scoop
/// to full at the latest scoopable hop already passed and re-simulate.
/// Skipping a scoop only ever lightens the ship (longer reach, smaller
/// burn), so a plan that survives topping up everywhere always survives
/// this pass; in the worst case every scoopable is marked again and the
/// result IS the eager plan. A stretch with no scoopable to mark (a low
/// departure tank, a genuinely dry run the planner accepted) is allowed
/// down to what `FuelModel::jump` itself allows -- the floor holds
/// wherever a stop can provide it, and `false` comes back only when
/// even eager scooping cannot fund a jump (the flags are then left as
/// the planner wrote them).
/// Item 39: even an eager plan knows which of its stops are load-bearing.
/// Simulate the pruned truth on a clone and mark the stops it drops as
/// `fuel_optional` — "fuel available here, not needed" — so a HUD can
/// tell a real stop from a comfort top-up without changing the plan.
pub fn mark_optional_stops(m: &FuelModel, boost: &BoostProfile, injection_mult: Option<f32>, route: &mut Route, start_fuel: f32) {
    let mut pruned = route.clone();
    if !minimize_refuels(m, boost, injection_mult, &mut pruned, start_fuel) {
        return;
    }
    for (h, p) in route.hops.iter_mut().zip(pruned.hops.iter()) {
        h.fuel_optional = h.refuel && !p.refuel;
    }
}

pub fn minimize_refuels(m: &FuelModel, boost: &BoostProfile, injection_mult: Option<f32>, route: &mut Route, start_fuel: f32) -> bool {
    let n = route.hops.len();
    if n == 0 || m.capacity <= 0.0 {
        return true;
    }
    let floor = m.reserve.max(m.max_fuel_per_jump);
    let mut scoop_at = vec![false; n];
    let tank = loop {
        // One no-scoop simulation over the current stop set.
        let mut tank = vec![start_fuel.min(m.capacity)];
        // Scoopable hops passed without stopping, latest last.
        let mut skipped: Vec<usize> = Vec::new();
        let mut short_at: Option<usize> = None;
        for i in 1..n {
            let hop = &route.hops[i];
            let b = if hop.boosted {
                boost.for_class(route.hops[i - 1].class)
            } else if hop.injection.is_some() {
                injection_mult.unwrap_or(1.0)
            } else {
                1.0
            };
            let fuel = tank[i - 1];
            let left = match m.jump(hop.distance_ly, fuel, b) {
                Some(left) => left,
                None => {
                    short_at = Some(i);
                    break;
                }
            };
            if left < floor && !skipped.is_empty() {
                short_at = Some(i);
                break;
            }
            if scoop_at[i] {
                tank.push(m.capacity);
            } else {
                tank.push(left);
                if hop.scoopable {
                    skipped.push(i);
                }
            }
        }
        match short_at {
            None => break tank,
            Some(_) => match skipped.pop() {
                Some(j) => scoop_at[j] = true,
                None => return false,
            },
        }
    };
    let mut stops = 0usize;
    for (i, hop) in route.hops.iter_mut().enumerate() {
        hop.fuel_after = Some(tank[i]);
        hop.refuel = scoop_at[i];
        stops += scoop_at[i] as usize;
    }
    route.refuel_stops = stops;
    true
}

/// Cooperative hooks: cancel check and `(expansions, best_remaining_ly)`.
pub struct Control<'a> {
    pub cancelled: &'a (dyn Fn() -> bool + Sync),
    pub progress: &'a (dyn Fn(u64, f32) + Sync),
    /// ("coarse" | "refine", leg, legs): which phase a long plot is in.
    pub stage: &'a (dyn Fn(&'static str, u32, u32) + Sync),
    /// A complete candidate route exists (a long plot runs several
    /// variants; the best at the end wins, this is one of them).
    pub found: &'a (dyn Fn(&Route) + Sync),
    /// Search trace for visualisation: `(phase, position, value)`.
    /// `"exact"`/`"coarse"`: a star expanded, value = jumps so far;
    /// `"chain"`: a coarse waypoint, value = its index; `"leg"`: a hop of
    /// a refined leg, value = the leg index. Cheap when it is a no-op.
    pub trace: &'a (dyn Fn(&'static str, [f32; 3], f32) + Sync),
}

impl Control<'_> {
    pub fn none() -> Control<'static> {
        Control { cancelled: &|| false, progress: &|_, _| {}, stage: &|_, _, _| {}, found: &|_| {}, trace: &|_, _, _| {} }
    }
}

#[derive(Debug)]
pub enum RouteError {
    Cancelled,
    NoRoute,
    Budget,
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RouteError::Cancelled => write!(f, "route cancelled"),
            RouteError::NoRoute => write!(f, "Sorry, no route possible. A gap on the way is wider than the ship can jump."),
            RouteError::Budget => write!(f, "Sorry, no route found within the search limit."),
        }
    }
}
impl std::error::Error for RouteError {}

#[derive(Copy, Clone, PartialEq)]
struct Open {
    inj: u32,
    f: f32,
    g: u32,
    /// Light-years flown so far: the tie-break among equal-jump paths, so
    /// the plot is deterministic and prefers tighter legs (which is also
    /// what favours a boosted hop over a marginal unboosted one).
    ly: f32,
    /// Boosted jumps so far. At equal jump counts the plain path wins: a
    /// supercharge only earns its keep when it removes a jump, because every
    /// cone pass risks FSD damage. (The game does the opposite -- measured
    /// Wongi -> LHS 20 via two white dwarfs at 156.7 ly over an unboosted
    /// 146.8 ly path -- and the commander prefers ours.)
    boosts: u32,
    idx: u32,
    /// Dry-run length without a fuel model; quantised tank level with one.
    dry: u32,
    fuel: f32,
}
impl Eq for Open {}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on f, then fewer jumps, then fewer light-years.
        o.f.partial_cmp(&self.f)
            .unwrap_or(Ordering::Equal)
            .then(o.g.cmp(&self.g))
            .then(o.boosts.cmp(&self.boosts))
            .then(o.ly.partial_cmp(&self.ly).unwrap_or(Ordering::Equal))
    }
}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Stars relaxed per expansion of the exact planner (see the scan below).
const LEG_FANOUT: usize = 512;

pub fn plan(g: &Galaxy, req: &RouteRequest, ctl: &Control) -> Result<Route, RouteError> {
    let started = std::time::Instant::now();
    let goal = g.record(req.to).pos();
    let start_rec = g.record(req.from);
    let straight = dist(start_rec.pos(), goal);
    let range = req.range_ly.max(1.0);
    let fuel_model = req.fuel;
    // With a fuel model the state carries the tank level, quantised to
    // 1/255 of capacity, in the slot the dry-run counter otherwise uses.
    let quant = |fuel: f32| -> u32 {
        match fuel_model {
            Some(m) if m.capacity > 0.0 => ((fuel / m.capacity) * 255.0).round().clamp(0.0, 255.0) as u32,
            _ => 0,
        }
    };
    // Thorough: admissible heuristic over the best reachable boost, exact A*.
    let (h_range, weight) = if req.thorough {
        (range * if req.supercharge { req.boost.neutron } else { 1.0 }, 1.0)
    } else {
        (range, req.weight)
    };
    let h = |p: [f32; 3]| dist(p, goal) / h_range * weight;

    // State is (system, dry-run length) when dry runs are limited; the
    // dry component is what makes "refuel before it" enforceable. With a
    // fuel model the slot carries the quantised tank instead. With neither
    // (no limit, no fuel) the counter constrains nothing, and keying on it
    // minted a state per distinct dry length at every system: an
    // impossible plot over 145k systems expanded 7.25 M times before
    // saying no (2026-09-10, galos spike). Then the system is the state.
    let dry_matters = fuel_model.is_some() || req.max_dry_jumps > 0;
    let key = move |idx: u32, dry: u32| -> u64 { ((idx as u64) << 8) | if dry_matters { dry.min(255) as u64 } else { 0 } };
    // Best (jumps, ly) seen per state; a path with equal jumps but fewer ly
    // still improves on the recorded one.
    // (jumps, boosts, ly): lexicographically smaller is better.
    let mut best_g: HashMap<u64, (u32, i32, f32)> = HashMap::default();
    let mut parent: HashMap<u64, (u64, bool, f32, f32, bool, bool)> = HashMap::default();
    let mut open = BinaryHeap::new();
    // Per system, the non-dominated (jumps, fuel) pairs reached so far. A
    // state with no fewer jumps and no more fuel than one already known is
    // worthless: fuel levels quantised to 1/255 otherwise multiply every
    // system into dozens of near-identical states.
    let mut pareto: HashMap<u32, Vec<(u32, f32)>> = HashMap::default();
    let dominated = |pareto: &mut HashMap<u32, Vec<(u32, f32)>>, idx: u32, jumps: u32, fuel: f32| -> bool {
        let e = pareto.entry(idx).or_default();
        if e.iter().any(|&(j, f)| j <= jumps && f >= fuel - 0.05) {
            return true;
        }
        e.retain(|&(j, f)| !(jumps <= j && fuel >= f));
        e.push((jumps, fuel));
        false
    };

    let start_fuel = match fuel_model {
        Some(m) => req.start_fuel.clamp(0.0, m.capacity),
        None => 0.0,
    };
    let start_key = key(req.from, quant(start_fuel));
    best_g.insert(start_key, (0, 0, 0.0));
    // Scalar cost: jumps first, then a small penalty per boosted leg (use a
    // supercharge only when it removes a jump), then a smaller charge per
    // light-year for determinism. Both extras are far below one jump so
    // they never change the jump count.
    let cost = |g: u32, boosts: u32, ly: f32| g as f32 + 0.01 * boosts as f32 + 0.0001 * ly / range;
    open.push(Open { f: h(start_rec.pos()), g: 0, ly: 0.0, boosts: 0, idx: req.from, dry: quant(start_fuel), fuel: start_fuel, inj: 0 });

    let mut expansions: u64 = 0;
    let mut cands: Vec<(f32, u32, f32)> = Vec::with_capacity(4096);
    let fanout: usize = std::env::var("ED_LEG_FANOUT").ok().and_then(|v| v.parse().ok()).unwrap_or(LEG_FANOUT);
    let mut best_remaining = straight;

    while let Some(cur) = open.pop() {
        let cur_key = key(cur.idx, cur.dry);
        let cur_rank = (cur.g + cur.inj * INJECTION_PENALTY, cur.boosts as i32, cur.ly);
        if best_g.get(&cur_key).is_some_and(|&(bg, bb, bl)| (bg, bb, bl) < cur_rank) {
            continue; // stale entry
        }
        if cur.idx == req.to {
            return Ok(reconstruct(g, req, &parent, cur_key, straight, expansions, started));
        }
        expansions += 1;
        if expansions.is_multiple_of(200) {
            if (ctl.cancelled)() {
                return Err(RouteError::Cancelled);
            }
            (ctl.progress)(expansions, best_remaining);
            if req.max_expansions > 0 && expansions > req.max_expansions {
                return Err(RouteError::Budget);
            }
            // The time budget binds here too: a short plot never passes
            // through plan_long's deadline closure, and a no-route search
            // over fuel-quantised states is effectively unbounded (item
            // 15's 18-minute Beagle -> Oevasy Mandalay).
            if req.time_budget_ms > 0 && started.elapsed().as_millis() as u64 > req.time_budget_ms {
                return Err(RouteError::Budget);
            }
        }

        let rec = g.record(cur.idx);
        let here = rec.pos();
        (ctl.trace)("exact", here, cur.g as f32);
        let class = g.class(&rec);
        let goal_idx = req.to;
        let boost = if !req.supercharge {
            1.0
        } else {
            let own = req.boost.for_class(class);
            if own > 1.0 || req.secondary_boost_ls <= 0.0 {
                own
            } else {
                // A boost star off the arrival point, within the allowed
                // supercruise run: the experiment's second kind of boost.
                match g.boost_secondary(cur.idx) {
                    Some((secondary, ls)) if ls <= req.secondary_boost_ls => req.boost.for_class(secondary),
                    _ => 1.0,
                }
            }
        };
        let reach = match fuel_model {
            Some(m) => m.reach(cur.fuel, boost),
            None => range * boost * (1.0 - crate::fuel::range_margin()),
        };
        let remaining = dist(here, goal);
        if remaining < best_remaining {
            best_remaining = remaining;
        }

        // Cells that cannot bring the ship closer than (remaining + one
        // unboosted jump) to the goal hold nothing a weighted search will
        // take; from a neutron that halves a 400 ly sphere.
        let toward = if req.thorough { None } else { Some((goal, remaining + range)) };
        // One pass at the ship's reach; if injections are allowed and the
        // commander still has some, a second pass over the longer reach for
        // the candidates only an injection gets to.
        let injected_reach = match req.injection {
            Some((mult, _, max)) if cur.inj < max => Some(reach / boost.max(1.0) * mult),
            _ => None,
        };
        for pass in 0..2 {
            let (scan, injected) = match pass {
                0 => (reach, false),
                _ => match injected_reach {
                    Some(r) if r > reach => (r, true),
                    _ => break,
                },
            };
            let eff_boost = if injected { req.injection.map(|(m, _, _)| m).unwrap_or(1.0) } else { boost };
            // In the core a boosted hop's sphere holds thousands of stars;
            // relaxing them all costs milliseconds per expansion. Only the
            // ones making the most progress (scoopables ranked a jump
            // ahead, so a fuel stop is never thinned away) get an edge.
            cands.clear();
            g.for_each_within_toward(here, scan, toward, |n_idx, d| {
                if n_idx == cur.idx || (injected && d <= reach) {
                    return;
                }
                let to_goal = dist(g.pos_of(n_idx), goal);
                let scoop = g.class_code(n_idx) != crate::StarClass::Unknown.code() && crate::StarClass::from_code(g.class_code(n_idx)).scoopable() || g.flags(n_idx) & crate::format::FLAG_SCOOP_NEARBY != 0;
                cands.push((if scoop || n_idx == goal_idx { to_goal - range } else { to_goal }, n_idx, d));
            });
            if cands.len() > fanout {
                cands.select_nth_unstable_by(fanout, |a, b| a.0.total_cmp(&b.0));
                cands.truncate(fanout);
            }
            for &(_, n_idx, d) in cands.iter() {
                let n_code = g.class_code(n_idx);
                let n_class = if n_code == crate::StarClass::Unknown.code() { g.class(&g.record(n_idx)) } else { crate::StarClass::from_code(n_code) };
                let scoop = n_class.scoopable() || g.flags(n_idx) & crate::format::FLAG_SCOOP_NEARBY != 0;
                // Fuel: burn for the hop; refill on a scoopable arrival.
                let (n_fuel, refuel, dry) = match fuel_model {
                    Some(m) => {
                        let Some(left) = m.jump(d, cur.fuel, eff_boost) else { continue };
                        let refuel = scoop && left < m.capacity;
                        let after = if scoop { m.capacity } else { left };
                        (after, refuel, quant(after))
                    }
                    None => (0.0, false, if scoop { 0 } else { cur.dry + 1 }),
                };
                if fuel_model.is_none() && req.max_dry_jumps > 0 && dry > req.max_dry_jumps && n_idx != req.to {
                    continue;
                }
                let ng = cur.g + 1;
                let ninj = cur.inj + injected as u32;
                let nl = cur.ly + d;
                // Boosted when beyond plain reach, or when the plain jump is
                // not fundable from the tank (fuel was charged at the boosted rate).
                let boosted = !injected && boost > 1.0 && (d > reach / boost || fuel_model.is_some_and(|m| m.jump(d, cur.fuel, 1.0).is_none()));
                let nb = cur.boosts + boosted as u32;
                let nk = key(n_idx, dry);
                let rank = (ng + ninj * INJECTION_PENALTY, nb as i32, nl);
                if best_g.get(&nk).is_some_and(|&(bg, bb, bl)| (bg, bb, bl) <= rank) {
                    continue;
                }
                if fuel_model.is_some() && n_idx != goal_idx && dominated(&mut pareto, n_idx, ng + ninj * INJECTION_PENALTY, n_fuel) {
                    continue;
                }
                best_g.insert(nk, rank);
                parent.insert(nk, (cur_key, boosted, d, n_fuel, refuel, injected));
                open.push(Open { f: cost(ng + ninj * INJECTION_PENALTY, nb, nl) + h(g.pos_of(n_idx)), g: ng, ly: nl, boosts: nb, idx: n_idx, dry, fuel: n_fuel, inj: ninj });
            }
        }
    }
    Err(RouteError::NoRoute)
}

fn reconstruct(
    g: &Galaxy,
    req: &RouteRequest,
    parent: &HashMap<u64, (u64, bool, f32, f32, bool, bool)>,
    end: u64,
    straight: f32,
    expansions: u64,
    started: std::time::Instant,
) -> Route {
    let mut chain: Vec<(u64, bool, f32, f32, bool, bool)> = Vec::new();
    let mut k = end;
    loop {
        match parent.get(&k) {
            Some(&(p, boosted, d, fuel, refuel, injected)) => {
                chain.push((k, boosted, d, fuel, refuel, injected));
                k = p;
            }
            None => {
                chain.push((k, false, 0.0, req.start_fuel, false, false));
                break;
            }
        }
    }
    chain.reverse();
    let mut hops = Vec::with_capacity(chain.len());
    let mut total = 0.0f32;
    let mut boosted_jumps = 0;
    let mut refuel_stops = 0;
    let mut injections = 0;
    let mut secondary_boosts = 0;
    let mut prev_idx: Option<u32> = None;
    for (k, boosted, d, fuel, refuel, injected) in chain {
        if injected {
            injections += 1;
        }
        let idx = (k >> 8) as u32;
        let r = g.record(idx);
        let class = g.class(&r);
        total += d;
        if boosted {
            boosted_jumps += 1;
        }
        // A boosted hop out of a system whose own arrival star grants no
        // boost came from its secondary: record the supercruise run.
        let via_secondary_ls = match (boosted, prev_idx) {
            (true, Some(p)) if req.secondary_boost_ls > 0.0 && req.boost.for_class(g.class(&g.record(p))) <= 1.0 => {
                g.boost_secondary(p).map(|(_, ls)| ls)
            }
            _ => None,
        };
        if via_secondary_ls.is_some() {
            secondary_boosts += 1;
        }
        prev_idx = Some(idx);
        if refuel {
            refuel_stops += 1;
        }
        hops.push(Hop {
            idx,
            id64: r.id64,
            name: g.name(&r).to_string(),
            pos: r.pos(),
            class,
            scoopable: g.scoopable(idx),
            distance_ly: d,
            boosted,
            total_ly: total,
            fuel_after: req.fuel.map(|_| fuel),
            refuel,
            fuel_optional: false,
            injection: if injected { req.injection.map(|(_, name, _)| name.to_string()) } else { None },
            synthesized: false,
            via_secondary_ls,
        });
    }
    Route {
        variants_run: 0,
        variants_finished: 0,
        ship_has_scoop: None,
        fsd_integrity: None,
        integrity_loss_per_boost: None,
        ship_has_afmu: None,
        ship_id: None,
        ship: None,
        range_ly: req.range_ly,
        jumps: hops.len().saturating_sub(1),
        total_ly: total,
        straight_ly: straight,
        boosted_jumps,
        injections,
        secondary_boosts,
        expansions,
        elapsed_ms: started.elapsed().as_millis() as u64,
        refuel_stops,
        hops,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import_reader;

    /// A line of stars 20 ly apart along x, a neutron star at 40, and a
    /// far target that only a boosted jump can reach in one go.
    fn galaxy() -> (tempfile::TempDir, Galaxy) {
        let mut lines = vec!["[".to_string()];
        let mut id = 1;
        for i in 0..=10 {
            let sub = if i == 2 { "Neutron Star" } else if i == 5 { "L (Brown dwarf) Star" } else { "K (Yellow-Orange) Star" };
            lines.push(format!(
                r#"{{"id64":{id},"name":"S{i}","coords":{{"x":{},"y":0,"z":0}},"bodies":[{{"type":"Star","subType":"{sub}","mainStar":true}}]}},"#,
                i * 20
            ));
            id += 1;
        }
        // An island 100 ly off the line, reachable only by a 4x boost from S2 (x=40).
        lines.push(format!(r#"{{"id64":{id},"name":"Island","coords":{{"x":40,"y":100,"z":0}},"bodies":[{{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}}]}}"#));
        lines.push("]".into());
        let dir = tempfile::tempdir().unwrap();
        import_reader(Box::new(std::io::Cursor::new(lines.join("\n").into_bytes())), dir.path(), &mut |_| {}).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        (dir, g)
    }

    /// A route that cannot exist must be refused in about one pass over the
    /// reachable systems. Measured 2026-09-10 (galos spike): with no dry-jump
    /// limit the search still keyed its state on the dry-run counter, so
    /// every unscoopable arrival minted a fresh state per system and an
    /// impossible plot over 145k systems expanded 7.25 M times (161 s).
    #[test]
    fn an_impossible_route_is_refused_in_one_pass_over_the_reachable_systems() {
        // Forty brown dwarfs (unscoopable) in a line 10 ly apart, and a goal
        // 1,000 ly away that nothing reaches at a 15 ly range.
        let mut lines = vec!["[".to_string()];
        for i in 0..40 {
            lines.push(format!(
                r#"{{"id64":{},"name":"D{i}","coords":{{"x":{},"y":0,"z":0}},"bodies":[{{"type":"Star","subType":"L (Brown dwarf) Star","mainStar":true}}]}},"#,
                i + 1,
                i * 10
            ));
        }
        lines.push(r#"{"id64":999,"name":"Far","coords":{"x":1000,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]}"#.into());
        lines.push("]".into());
        let dir = tempfile::tempdir().unwrap();
        import_reader(Box::new(std::io::Cursor::new(lines.join("\n").into_bytes())), dir.path(), &mut |_| {}).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let req = RouteRequest { from: g.find("D0").unwrap(), to: g.find("Far").unwrap(), range_ly: 15.0, supercharge: true, max_dry_jumps: 0, ..Default::default() };
        let expanded = std::sync::atomic::AtomicU64::new(0);
        let progress = |n: u64, _: f32| expanded.store(n, std::sync::atomic::Ordering::Relaxed);
        let ctl = Control { progress: &progress, ..Control::none() };
        assert!(matches!(plan(&g, &req, &ctl), Err(RouteError::NoRoute)));
        let n = expanded.load(std::sync::atomic::Ordering::Relaxed);
        assert!(n <= 80, "refusing an impossible route took {n} expansions over 40 reachable systems");
    }

    /// The experiment's second kind of boost: a system whose arrival star
    /// grants nothing but whose neutron secondary sits within the allowed
    /// run supercharges the next jump, and the hop says how far the run
    /// was. Off, or with the secondary too far, the plan is the unboosted
    /// one.
    #[test]
    fn a_secondary_neutron_within_the_allowed_run_supercharges_the_next_jump() {
        // Sol -> Twin (30 ly) -> Far (30 + 100 ly). Range 30: only a x4
        // boost out of Twin reaches Far; Twin's arrival star is K, its
        // neutron sits at 4,000 ls.
        let json = r#"[
{"id64":1,"name":"Sol","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}]},
{"id64":2,"name":"Twin","coords":{"x":30,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true},{"type":"Star","subType":"Neutron Star","mainStar":false,"distanceToArrival":4000.0}]},
{"id64":3,"name":"Far","coords":{"x":130,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]}
]"#;
        let dir = tempfile::tempdir().unwrap();
        import_reader(Box::new(std::io::Cursor::new(json.as_bytes().to_vec())), dir.path(), &mut |_| {}).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let base = RouteRequest { from: g.find("Sol").unwrap(), to: g.find("Far").unwrap(), range_ly: 30.0, supercharge: true, ..Default::default() };
        let ctl = Control::none();
        assert!(matches!(plan(&g, &base, &ctl), Err(RouteError::NoRoute)), "off: the 100 ly gap is unbridgeable");
        let too_far = RouteRequest { secondary_boost_ls: 1_000.0, ..base.clone() };
        assert!(matches!(plan(&g, &too_far, &ctl), Err(RouteError::NoRoute)), "a 4,000 ls secondary is outside a 1,000 ls run");
        let allowed = RouteRequest { secondary_boost_ls: 5_000.0, ..base.clone() };
        let route = plan(&g, &allowed, &ctl).unwrap();
        assert_eq!(route.jumps, 2);
        assert_eq!(route.secondary_boosts, 1);
        assert_eq!(route.boosted_jumps, 1);
        let far = route.hops.last().unwrap();
        assert!(far.boosted);
        assert_eq!(far.via_secondary_ls, Some(4000.0));
        assert_eq!(route.hops[1].via_secondary_ls, None, "the hop into Twin was a plain jump");
    }

    /// Item 39: an eager plan's comfort top-ups get labelled — the tank
    /// never needed them, the HUD should say "fuel available", not
    /// "fuel here". A pruned route keeps its labels clean: every stop
    /// that survives the rewrite is load-bearing by construction.
    #[test]
    fn eager_comfort_stops_are_marked_optional_and_pruned_stops_are_not() {
        let (_d, g) = galaxy();
        let m = FuelModel::from_loadout(100.0, 100.0, 8.0, 5, false, false, 75.0, 0.0, 0.0);
        let req = RouteRequest {
            from: g.find("S0").unwrap(), to: g.find("S10").unwrap(),
            range_ly: m.range_at(m.capacity), supercharge: false,
            fuel: Some(m), start_fuel: 100.0, ..Default::default()
        };
        let mut r = plan(&g, &req, &Control::none()).unwrap();
        assert!(r.refuel_stops > 0, "the eager plan tops up at scoopable stars: {:?}", r.hops.iter().map(|h| (h.name.as_str(), h.refuel)).collect::<Vec<_>>());
        mark_optional_stops(&m, &req.boost, None, &mut r, req.start_fuel);
        let optional = r.hops.iter().filter(|h| h.refuel && h.fuel_optional).count();
        assert!(optional > 0, "a 100 t tank over 200 ly needs none of those stops");
        // The pruned route: surviving stops are needed, so nothing is optional.
        let mut lean = r.clone();
        assert!(minimize_refuels(&m, &req.boost, None, &mut lean, req.start_fuel));
        mark_optional_stops(&m, &req.boost, None, &mut lean, req.start_fuel);
        assert!(lean.hops.iter().all(|h| !h.fuel_optional), "pruned stops are all load-bearing");
    }

    #[test]
    fn a_star_walks_the_line_in_minimum_jumps() {
        let (_d, g) = galaxy();
        let req = RouteRequest { from: g.find("S0").unwrap(), to: g.find("S10").unwrap(), range_ly: 45.0, supercharge: false, ..Default::default() };
        let r = plan(&g, &req, &Control::none()).unwrap();
        // 200 ly at 45 ly/jump over 20 ly spacing: 40 ly hops -> 5 jumps.
        assert_eq!(r.jumps, 5, "{:?}", r.hops.iter().map(|h| h.name.as_str()).collect::<Vec<_>>());
        assert_eq!(r.hops.first().unwrap().name, "S0");
        assert_eq!(r.hops.last().unwrap().name, "S10");
        assert_eq!(r.boosted_jumps, 0);
    }

    #[test]
    fn supercharging_reaches_the_island_and_is_reported() {
        let (_d, g) = galaxy();
        let req = RouteRequest { from: g.find("S0").unwrap(), to: g.find("Island").unwrap(), range_ly: 30.0, supercharge: true, ..Default::default() };
        let r = plan(&g, &req, &Control::none()).unwrap();
        assert_eq!(r.hops.last().unwrap().name, "Island");
        assert!(r.hops.last().unwrap().boosted, "the last jump must be the 4x from the neutron star");
        assert_eq!(r.boosted_jumps, 1);
        // Without supercharging the island is unreachable at 30 ly.
        let plain = RouteRequest { supercharge: false, ..req.clone() };
        assert!(matches!(plan(&g, &plain, &Control::none()), Err(RouteError::NoRoute)));
    }

    #[test]
    fn dry_run_limit_is_enforced_and_scoopability_reported() {
        let (_d, g) = galaxy();
        // S5 is a brown dwarf; with max_dry_jumps=1 the route may pass it
        // but must not chain two dry arrivals.
        let req = RouteRequest { from: g.find("S0").unwrap(), to: g.find("S10").unwrap(), range_ly: 25.0, supercharge: false, max_dry_jumps: 1, ..Default::default() };
        let r = plan(&g, &req, &Control::none()).unwrap();
        let dry_runs = r.hops.iter().skip(1).fold((0, 0), |(cur, max), h| if h.scoopable { (0, max) } else { (cur + 1, (cur + 1).max(max)) }).1;
        assert!(dry_runs <= 1);
        assert!(r.hops.iter().any(|h| !h.scoopable), "S5 is on the only path at 25 ly");
    }

    /// The time budget must bind INSIDE the exact planner too. A short
    /// (<1,500 ly) plot never passes through plan_long's deadline
    /// closure, so a no-route search explored its whole fuel-quantised
    /// state space with no clock at all -- measured on Beagle Point ->
    /// Oevasy CA-A d0 (1,353 ly, Mandalay, --budget 30): 18 minutes of
    /// CPU before being killed (ROUTING-NEXT item 15). With the budget
    /// honoured the search returns Budget once the deadline passes.
    #[test]
    fn a_short_plot_with_no_route_stops_at_its_time_budget() {
        let mut lines = vec!["[".to_string()];
        let mut id = 1;
        for x in 0..60 {
            for z in 0..60 {
                lines.push(format!(
                    r#"{{"id64":{id},"name":"G{x}x{z}","coords":{{"x":{},"y":0,"z":{}}},"bodies":[{{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}}]}},"#,
                    x * 15,
                    z * 15
                ));
                id += 1;
            }
        }
        lines.push(format!(
            r#"{{"id64":{id},"name":"Faraway","coords":{{"x":5000,"y":0,"z":0}},"bodies":[{{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}}]}}"#
        ));
        lines.push("]".into());
        let dir = tempfile::tempdir().unwrap();
        import_reader(Box::new(std::io::Cursor::new(lines.join("\n").into_bytes())), dir.path(), &mut |_| {}).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let req = RouteRequest {
            from: g.find("G0x0").unwrap(),
            to: g.find("Faraway").unwrap(),
            range_ly: 45.0,
            supercharge: false,
            time_budget_ms: 1,
            ..Default::default()
        };
        let t = std::time::Instant::now();
        let r = plan(&g, &req, &Control::none());
        assert!(matches!(r, Err(RouteError::Budget)), "the deadline must bind before exhaustion: {r:?}");
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn cancellation_is_honoured() {
        let (_d, g) = galaxy();
        let req = RouteRequest { from: g.find("S0").unwrap(), to: g.find("S10").unwrap(), range_ly: 25.0, ..Default::default() };
        let ctl = Control { cancelled: &|| true, progress: &|_, _| {}, stage: &|_, _, _| {}, found: &|_| {}, trace: &|_, _, _| {} };
        // Tiny graph: may finish before the first cancel check; either way it must not panic.
        let _ = plan(&g, &req, &ctl);
    }
}
