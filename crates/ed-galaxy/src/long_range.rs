//! Long-range planning: neutron-first, the way the neutron highway is
//! actually flown.
//!
//! The exact A* in `router.rs` is optimal but its frontier is the whole
//! sphere of systems within range -- fine to a few hundred ly, hopeless at
//! 22,000. Here the search runs over a much smaller graph, read from the
//! **neutron sub-index** (1.8 % of systems): an edge is one supercharged
//! jump, or a supercharge plus a few ordinary bridging jumps where no
//! neutron is in reach. With a fuel model the coarse state carries the tank
//! level too: a hop that would run the tank dry is replaced by a **scoop
//! edge** -- supercharge to a scoopable star short of the target, refill,
//! one ordinary jump on -- costing an extra jump, which is exactly how the
//! 22 refuel stops in a real Colonia run arise. Each coarse leg is then
//! refined with the exact planner, which finds the actual scoop star.

use crate::format::{dist, Galaxy};
use crate::router::{plan, CoarseStats, Control, LegKey, LegMemo, Route, RouteError, RouteRequest, VariantOutcome, BOOST_UNITS, INJECTION_SECONDS, INJECTION_UNITS, SCOOP_RATE_DEFAULT, STOP_UNITS, T_BOOST_S, T_PASS_SCOOP_S, T_PLAIN_S, T_STOP_S};
use crate::star::StarClassCode as _;
use std::cmp::Ordering;
use rustc_hash::FxHashMap as HashMap;
use std::collections::BinaryHeap;

/// Above this the exact planner is not attempted first.
pub const LONG_ROUTE_LY: f32 = 1_500.0;

thread_local! {
    /// Widen rescans the last coarse search on this thread fired (an
    /// instrument for the widen rule's unit test, which calls the search
    /// directly on one thread; the portfolio reads [`CoarseStats`]).
    static LAST_WIDENS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Widen rescans of the last coarse search on this thread (an instrument;
/// the R1 widen rule's test and the harness read it).
pub fn last_coarse_widens() -> u64 {
    LAST_WIDENS.with(|c| c.get())
}

/// Ordinary jumps a coarse edge may spend bridging to the next neutron.
const MAX_BRIDGE_JUMPS: u32 = 4;

/// Ordinary jumps the first coarse edge may take: the highway rarely
/// starts at the door.
const MAX_END_BRIDGE_JUMPS: u32 = 60;

/// Ordinary jumps the last coarse edge may take. Enough to close the final
/// approach into the bubble from the last neutron; anything longer is left
/// to the stall detector and the exact planner (an unbounded final edge
/// made the weighted search exhaustive on Beagle Point).
const MAX_GOAL_BRIDGE_JUMPS: u32 = 12;

/// Weighted A* for refinement legs (a few hundred ly), the exact-plotter trade.
const LEG_WEIGHT: f32 = 1.3;

/// A refinement leg's expansion backstop: 1,000 an ly with a 200,000
/// floor (a legitimate 742 ly rim leg took 218,774; at 400 an ly a 514 ly
/// one failed and the rim reverse lost 13 jumps). The cap that bites is
/// `leg_time_budget_ms`.
fn leg_expansion_cap(ly: f32) -> u64 {
    ((ly * 1_000.0) as u64).max(200_000)
}

/// A refinement leg's time: a quarter of the plot's budget with a 20 s
/// floor (never more than the plot has), so one leg the coarse chain
/// assumed bridgeable but that cannot be flown fails in 25 s of a 100 s
/// plot instead of 98 (the reverse Jongou route, 2026-10-17) while a
/// legitimate 742 ly rim leg (7.8 s alone) survives the interactive lane's
/// 30 s. 0 = no budget.
fn leg_time_budget_ms(plot_budget_ms: u64) -> u64 {
    if plot_budget_ms == 0 { 0 } else { (plot_budget_ms / 4).max(20_000.min(plot_budget_ms)) }
}

/// A coarse search that settles for its closest neutron hands the rest to
/// the exact planner only within this many boosted reaches of the goal;
/// further out the variant fails fast so the next wave runs. Measured on
/// 2026-10-17 (rim-to-rim stress route): at 20 reaches the waves it freed
/// did not cross either, and it cost Jongou AA-A d0 -> Byoi Fraae AT-U d2-1
/// its route (a 44,000 ly hand-off to a NEUTRON is a leg the exact planner
/// finishes in 60-85 s). Default off; `ED_STALL_HANDOFF_REACHES` for the
/// harness.
const STALL_HANDOFF_REACHES: f32 = f32::INFINITY;

/// Fuel quantisation for coarse states: about 4 t per level, so a 128 t
/// tank gets 32 levels and a 32 t tank 8 -- one level per level of tank
/// would multiply a small ship's states by four for nothing.
fn fuel_steps(_capacity: f32) -> f32 {
    32.0
}

/// Fraction of a max-fuel jump kept in hand when arriving at a neutron.
const RESERVE_FRACTION: f32 = 0.75;

/// Weighted A* on the coarse graph: hundreds of neutrons are in reach of
/// every hop, and an exact search wanders the whole corridor. 1.3 keeps the
/// jump count within a hop or two of optimal; measured on Wongi -> Colonia,
/// 1.3 took 3,120 expansions (6 s) for 72 jumps and 1.5 took 73 (71 ms)
/// for 66, so the "optimality" of the lower weight was noise.
const COARSE_WEIGHT: f32 = 1.5;

/// Neutron sub-index grid size. Its queries are ~500 ly; at the full
/// index's 50 ly a query is thousands of cell lookups.
pub const NEUTRON_CELL_LY: f32 = 250.0;

/// Neutrons relaxed per coarse expansion. In the core a ×4 ship has
/// thousands of neutrons in reach and relaxing every one made a Mandalay
/// plot 30 s; only the candidates that make the most progress toward the
/// goal (and any that refuel) are worth an edge.
const COARSE_FANOUT: usize = usize::MAX;

/// The thin family (2026-10-17, the rim pair Ood Fleau ZJ-I d9-0 <->
/// Sphiesi HX-L d7-0, 84,599 ly): a single and a bidi that relax at most
/// `THIN_FANOUT` candidates an expansion at weight `THIN_WEIGHT`. In the
/// bulge the unbounded scan refused 201 M already-beaten candidates in
/// 30,000 expansions (6,700 a step) and moved 2 kly; with fanout 32 and
/// weight 3 one variant on one thread plotted the whole route in 23.5 s
/// (424 jumps, 105,220 ly), the portfolio on the PC in 13.5 s (421 j),
/// and the external reference's figure is 453 j / 114,668 ly in 61 s.
/// Fanout 32 at weight 1.5 still stalls at 43.7 kly (the weight is
/// required); fanout 64 + sector 2 cost 36 jumps. The reverse direction
/// is the bidi's: a single-direction coarse hugs the straight line while
/// the only neutron band into an F-star goal runs 10 kly south of it, and
/// a thin single that stalls that far out fails fast (`THIN_HANDOFF_REACHES`)
/// so its worker moves on instead of exact-planning 8.5 kly for the rest
/// of the budget. The two run at indices 0 and 1 so rayon's range split
/// starts them first on the box's three threads. The thin bidi arms the
/// grace clock under the jump ceiling like the regular bidi (without an
/// early arming the singles stall and the x4 wave fires: Wongi -> Blaa
/// Hypai 1 -> 91 s); the thin single never does (its weight-3 answer
/// guillotined the dig behind it: Colonia -> Sol 55 -> 56 j). Each yields
/// its worker at a third of the budget (`thin_budget_ms`). ED_NO_THIN=1
/// removes them.
const THIN_FANOUT: usize = 32;
const THIN_WEIGHT: f32 = 3.0;
const THIN_HANDOFF_REACHES: f32 = 10.0;
/// The thin single's reserve fraction and thinning bucket: with the
/// portfolio under the thin knobs the quick single-direction finishers on
/// the rim were all reserve 0.4 / bucket 0.5 (a scan twin at 2.9 s, a
/// single at 9.4 s, an off-ramp at 9.2 s) while the 0.75 / 0.75 shape did
/// not finish in 100 s beside them.
const THIN_RESERVE: f32 = 0.4;
const THIN_BUCKET: f32 = 0.5;

/// How long a thin variant may hold a worker: a third of the plot's budget,
/// 30 s without one. On the box's three threads a thin variant that was not
/// quick kept its worker for the whole plot and the deep digs behind it
/// starved (arm thin2-3, 2026-10-17: Colonia -> Sol 55 -> 56 j, Colonia ->
/// Wongi 54 -> 55 j, Spase -> Colonia 87 -> 88 j, Jongou AA-A 396 -> 406 j,
/// Wongi -> Blaa Hypai 1 -> 98 s). The rim answers arrive at 6-27 s.
fn thin_budget_ms(time_budget_ms: u64) -> u64 {
    if time_budget_ms > 0 { time_budget_ms / 3 } else { 30_000 }
}

/// The heavy variants' first-wave slice (the maintainer's "hop between
/// techniques on a stall", 2026-10-18): an eighth of the budget (12.5 s on
/// the long lane), after which a heavy variant whose coarse chain has not
/// reached the goal yields its worker to the variants still queued, as a
/// thin variant does at a third. Measured on the three-thread debug traces
/// (Jongou XM-W -> CQ-G, Wongi -> Blaa Hypai): every first-wave variant
/// that found a route did so inside 5 s (one thin single at 10 s, under its
/// own cap); every variant past 12 s returned nothing -- six singles stalled
/// at the 30,000-expansion cap and spent 25 s each in the exact planner on
/// a 10,000 ly leg, holding the three workers while the scan twins that
/// carry the 402-jump route waited until 75-97 s. Escalation waves have no
/// slice (the x4 Blaa Hypai singles routed at 17-19 s). ED_SLICE_MS
/// overrides, 0 turns it off.
fn heavy_slice_ms(time_budget_ms: u64, first_wave: bool, knob: Option<u64>) -> Option<u64> {
    if !first_wave {
        return None;
    }
    match knob {
        Some(0) => None,
        Some(v) => Some(v),
        None if time_budget_ms > 0 => Some(time_budget_ms / 8),
        None => Some(12_500),
    }
}

/// Does a heavy variant yield at its slice. With coarse stats (a single):
/// unless the coarse chain reached the goal -- a chain that STALLED and
/// went to the exact leg planner is in the refine stage too, and that
/// 25 s refinement of a 10,000 ly stall is exactly what the slice cuts
/// (the Jongou XM-W -> Byoi CQ-G trace, three threads, 2026-10-18: five
/// singles at 26-27 s while the scan twins carrying the 402-jump route
/// waited until 67 s). Without them (a bidi): unless it is refining, as
/// the thin bidi does.
fn heavy_yields(past_slice: bool, has_coarse_stats: bool, coarse_done: bool, refining: bool) -> bool {
    past_slice && !(if has_coarse_stats { coarse_done } else { refining })
}

/// Does a thin variant yield its worker: past its deadline with the coarse
/// chain not yet at the goal. A chain that has reached the goal is refined
/// to the end (the rim reverse with the field, 2026-10-17: the chain was
/// done in 1,307 expansions and the 33 s cap cut the refinement, 462 j
/// instead of 447 for the wave).
fn thin_yields(past_deadline: bool, coarse_done: bool) -> bool {
    past_deadline && !coarse_done
}

/// The goal field is built when the straight-line precheck finds the
/// corridor non-uniform, or when forced (ED_FIELD=1, study knob,
/// 2026-10-17): the reverse Jongou approach leaves the line 6 kly south,
/// and a precheck that looks along the line never fires there.
fn field_wanted(forced: bool, nonuniform: bool) -> bool {
    forced || nonuniform
}

/// How far a plot end off the highway may be from the cell it snaps onto
/// for the goal field: the plain run the coarse search allows that end.
/// Until 2026-10-17 this was 8 x the range (578 ly for the Caspian), and
/// an island goal 873 ly from its nearest neutron got no field at all.
fn end_snap_ly(field_range: f32) -> f32 {
    field_range * MAX_END_BRIDGE_JUMPS as f32
}

/// Sector k-best thinning per coarse expansion; 0 = the bucket thinning.
/// Shipped at 0 until the 28-route matrix blesses a value (R1, 2026-10-17).
const COARSE_SECTOR_KEEP: usize = 0;

/// Item 13: the greedy cone engages only after a full scan saw at least
/// this many candidates in one expansion -- the "full shelf" that makes
/// choice worthless. Calibrated against density_probe: dense-corridor
/// expansions scan thousands (Colonia->Sag A* 5,260/exp), sparse ones
/// tens; the floor sits well above every desert and rim profile.
const GREEDY_SCAN_FLOOR: u64 = 512;

/// Coarse price of an injected bridging jump, on top of the jump itself.
/// The wave's twin of the exact router's injection price (INJECTION_UNITS).
const INJECTION_COARSE_PENALTY: f32 = INJECTION_UNITS;

/// Where the highway sub-index lives under the galaxy index directory:
/// `boost<cell>/`, neutrons and white dwarfs together. The grid size is in
/// the name so a new grid never has to overwrite an index a running
/// process still has mapped; the pre-white-dwarf `neutrons<cell>/` is a
/// different name for the same reason.
pub fn neutron_dir(galaxy_dir: &std::path::Path) -> std::path::PathBuf {
    galaxy_dir.join(format!("boost{}", NEUTRON_CELL_LY as u32))
}

/// Whether a star of `class` belongs in the highway sub-index. White
/// dwarfs are in: each coarse node supercharges by its own class (x1.5 /
/// x3 for a white dwarf), so a white-dwarf chain can bridge a neutron gap
/// in the coarse plan instead of costing plain jumps. They are opt-in at
/// every caller, though: a white-dwarf boost takes about twice as long
/// to line up as a neutron's, so unless a plot asks for them it carries
/// `BoostProfile.white_dwarf = 1.0` and the coarse scan skips them; see
/// `plan_long_with`.
///
/// Measured on the full index (2026-08-30, `examples/bench.rs`), neutron
/// only -> with white dwarfs: Explorer Mk II Wongi -> Colonia 58 -> 57
/// jumps (6 -> 5 refuels, 1,320 -> 1,208 ms), Sol -> Sagittarius A* 69 ->
/// 66 jumps (7 -> 3 refuels, 596 -> 310 ms), Colonia -> Beagle Point 182
/// both ways; Mandalay (x4 / x1.5) 93 -> 94, 114 -> 112, 257 -> 257.
/// Can the ship refuel within one plain jump of highway star `idx`? From
/// `scoop250.bin` when the sub-index carries it -- the nearest scoopable
/// star within the plain reach at the CURRENT tank, so a low tank sees
/// fewer options than a full one -- else the companion flag, as before
/// R2 (2026-10-17; the flag covers 9.4 % of highway stars, the byte 92 %
/// within 80 ly).
#[allow(dead_code)] // Task 4's dry-hop reset reads it; until then only its test does.
fn refuels_here(neutrons: &Galaxy, idx: u32, reach_now_ly: f32) -> bool {
    match neutrons.nearest_scoop_ly(idx) {
        Some(d) => d != crate::scoop_side::NONE && (d as f32) <= reach_now_ly,
        None => neutrons.flags(idx) & crate::format::FLAG_SCOOP_NEARBY != 0,
    }
}

/// Does the ship refill for free on arriving at highway star `idx`: a
/// scoopable companion within the run (the companion flag). The coarse
/// search that carries fuel in its state asks this; `refuels_here` (a
/// scoopable star within a jump) is a different question, Task 4's. Read
/// as a free refill it cost the matrix 2-8 jumps a route (2026-10-17, arm
/// r2-judgeonly: 14 of 28 worse), because the refiner pays the detour.
fn refills_on_arrival(neutrons: &Galaxy, idx: u32) -> bool {
    neutrons.flags(idx) & crate::format::FLAG_SCOOP_NEARBY != 0
}

pub fn highway_star(class: crate::StarClass) -> bool {
    class == crate::StarClass::Neutron || class == crate::StarClass::WhiteDwarf
}

#[derive(Copy, Clone, PartialEq)]
struct Open {
    f: f32,
    g: f32,
    idx: u32,
    fuel: f32,
    /// Injections spent getting here (the allowance is per route).
    inj: u32,
    /// Jumps the chain has committed to so far (a boosted hop, every
    /// bridging jump, an injected jump, a scoop stop's extra jump): the
    /// integer the shared incumbent prunes against (R3 Task 4).
    hops: u32,
}
impl Eq for Open {}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal).then(self.g.partial_cmp(&o.g).unwrap_or(Ordering::Equal))
    }
}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

const GOAL: u32 = u32::MAX;
const START: u32 = u32::MAX - 1;

/// Estimated cost of one coarse edge of length `d` leaving a star with
/// boost factor `boost` and unboosted range `range`, in plain-jump units
/// (R2): the first hop at `BOOST_UNITS` when it is supercharged (73 s
/// against a plain jump's 53) or 1.0 when it is not, plus 1.25 per
/// bridging jump, and the bridging jumps it assumed. `None` when it needs
/// more bridging than allowed.
fn edge_cost_max(d: f32, range: f32, boost: f32, max_bridge: u32) -> Option<(f32, u32)> {
    let first = range * boost;
    let hop = hop_units(boost);
    if d <= first {
        return Some((hop, 0));
    }
    let bridge = ((d - first) / range).ceil() as u32;
    (bridge <= max_bridge).then_some((hop + bridge as f32 * 1.25, bridge))
}

/// One hop leaving a star with `boost`, in plain-jump units.
fn hop_units(boost: f32) -> f32 {
    if boost > 1.0 && units_knob(&COARSE_UNITS, "ED_UNITS_COARSE") { BOOST_UNITS } else { 1.0 }
}

/// The one currency inside the SEARCHES is OFF by default (2026-10-17,
/// docs/benches/2026-10-17-planner-matrix.csv): the judge prices in
/// seconds everywhere, but pricing the coarse edges and heuristic in
/// units (ED_UNITS_COARSE=1) measured Jongou AA-A 396 -> 510 jumps,
/// Beagle Point +2 jumps and 18 -> 71 s, Blaa Hypai +1 stop (arm
/// r2b-seconds), and the exact surcharge with the scoop-stop price
/// (ED_UNITS_EXACT=1, ED_UNITS_VIA=1; arm r2b-nocoarse) lost Jongou XM-W
/// at the cut and ran Beagle Point 18 -> 75-82 s for +45 s of route. The
/// judge alone (arm r2b-judgeonly) is 9 better / 14 same / 5 wall-only
/// against r1. OPEN: exact alone and via alone are unmeasured. Read once
/// per process; `1` turns each on.
static COARSE_UNITS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
static VIA_UNITS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
pub(crate) static EXACT_UNITS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
pub(crate) fn units_knob(cell: &std::sync::OnceLock<bool>, var: &str) -> bool {
    *cell.get_or_init(|| std::env::var(var).is_ok_and(|v| v == "1"))
}

/// Whether a hop of `d` ly leaving a star with `boost` is flown
/// supercharged: when it is beyond the plain range, or when the plain jump
/// cannot be paid for from `fuel` (a near-empty tank under a neutron: the
/// fuel figure was charged at the boosted rate, so the hop must be boosted
/// or the re-simulation finds it impossible).
fn is_boosted(m: Option<crate::fuel::FuelModel>, d: f32, fuel: f32, boost: f32, range: f32) -> bool {
    boost > 1.0 && (d > range || m.is_some_and(|m| m.jump(d, fuel, 1.0).is_none()))
}

/// A leg that is one jump (the common supercharged hop neutron to neutron)
/// needs no search: build it directly when the jump is feasible from `fuel`.
fn direct_leg(g: &Galaxy, req: &RouteRequest, from: u32, to: u32, fuel: f32) -> Option<Route> {
    let a = g.record(from);
    let b = g.record(to);
    let d = dist(a.pos(), b.pos());
    let class_a = g.class(&a);
    let class_b = g.class(&b);
    let (boost, via) = crate::router::boost_source(g, req, from, class_a);
    let (range, fuel_after) = match req.fuel {
        Some(m) => (m.range_at(fuel), m.jump(d, fuel, boost)?),
        None => (req.range_ly.max(1.0), 0.0),
    };
    if d > range * boost * (1.0 - crate::fuel::range_margin()) + 1e-3 {
        return None;
    }
    let hop = |rec: &crate::format::StarRecord, idx: u32, class: crate::StarClass, dist_ly: f32, boosted: bool, total: f32, f: Option<f32>, refuel: bool| crate::router::Hop {
        idx,
        id64: rec.id64,
        name: g.name(rec).to_string(),
        pos: rec.pos(),
        class,
        scoopable: g.scoopable(idx),
        distance_ly: dist_ly,
        boosted,
        total_ly: total,
        fuel_after: f,
        refuel,
        fuel_optional: false,
        injection: None,
        synthesized: false,
        via_secondary: if boosted { via } else { None },
    };
    let boosted = is_boosted(req.fuel, d, fuel, boost, range);
    let b_scoop = g.scoopable(to);
    let arrive = req.fuel.map(|m| if b_scoop { m.capacity } else { fuel_after });
    let refuel = req.fuel.is_some() && b_scoop && fuel_after < req.fuel.map(|m| m.capacity).unwrap_or(0.0);
    Some(Route {
        highway_pending: false,
        variants_run: 0,
        variants_finished: 0,
        ship_has_scoop: None,
        fsd_integrity: None,
        integrity_loss_per_boost: None,
        ship_has_afmu: None,
        ship_id: None,
        ship: None,
        range_ly: req.range_ly,
        hops: vec![hop(&a, from, class_a, 0.0, false, 0.0, req.fuel.map(|_| fuel), false), hop(&b, to, class_b, d, boosted, d, arrive, refuel)],
        jumps: 1,
        total_ly: d,
        straight_ly: d,
        boosted_jumps: boosted as usize,
        expansions: 0,
        elapsed_ms: 0,
        refuel_stops: refuel as usize,
        injections: 0, secondary_boosts: 0,
    })
}

/// A leg that needs one scoop stop: supercharge from `from` to a scoopable
/// star within one ordinary jump of `to`, fill up, jump on. The candidate
/// stars are found in the small sphere around `to`, not the 400 ly one
/// around the neutron. Picks the stop that leaves the shortest final jump.
fn scoop_leg(g: &Galaxy, req: &RouteRequest, from: u32, to: u32, fuel: f32) -> Option<Route> {
    let m = req.fuel?;
    let a = g.record(from);
    let b = g.record(to);
    let a_pos = a.pos();
    let b_pos = b.pos();
    let (boost, via) = crate::router::boost_source(g, req, from, g.class(&a));
    let reach = m.reach(fuel, boost);
    let full = m.reach(m.capacity, 1.0);
    let mut best: Option<(u32, f32, f32, f32)> = None; // (idx, d1, d2, fuel after first jump)
    g.for_each_within(b_pos, full, |s_idx, d2| {
        if s_idx == from || s_idx == to {
            return;
        }
        if !g.scoopable(s_idx) {
            return;
        }
        let s_pos = g.pos_of(s_idx);
        let d1 = dist(a_pos, s_pos);
        if d1 > reach {
            return;
        }
        let Some(left) = m.jump(d1, fuel, boost) else { return };
        if m.jump(d2, m.capacity, 1.0).is_none() {
            return;
        }
        if best.is_none_or(|(_, _, bd2, _)| d2 < bd2) {
            best = Some((s_idx, d1, d2, left));
        }
    });
    let (s_idx, d1, d2, left) = best?;
    let s = g.record(s_idx);
    let class_a = g.class(&a);
    let class_s = g.class(&s);
    let class_b = g.class(&b);
    let hop = |rec: &crate::format::StarRecord, idx: u32, class: crate::StarClass, dist_ly: f32, boosted: bool, total: f32, f: f32, refuel: bool| crate::router::Hop {
        idx,
        id64: rec.id64,
        name: g.name(rec).to_string(),
        pos: rec.pos(),
        class,
        scoopable: g.scoopable(idx),
        distance_ly: dist_ly,
        boosted,
        total_ly: total,
        fuel_after: Some(f),
        refuel,
        fuel_optional: false,
        injection: None,
        synthesized: false,
        via_secondary: if boosted { via } else { None },
    };
    let after_b = m.jump(d2, m.capacity, 1.0)?;
    let b_scoop = g.scoopable(to);
    let arrive_b = if b_scoop { m.capacity } else { after_b };
    Some(Route {
        highway_pending: false,
        variants_run: 0,
        variants_finished: 0,
        ship_has_scoop: None,
        fsd_integrity: None,
        integrity_loss_per_boost: None,
        ship_has_afmu: None,
        ship_id: None,
        ship: None,
        range_ly: req.range_ly,
        hops: vec![
            hop(&a, from, class_a, 0.0, false, 0.0, fuel, false),
            hop(&s, s_idx, class_s, d1, is_boosted(Some(m), d1, fuel, boost, m.range_at(fuel)), d1, m.capacity, left < m.capacity),
            hop(&b, to, class_b, d2, false, d1 + d2, arrive_b, b_scoop && after_b < m.capacity),
        ],
        jumps: 2,
        total_ly: d1 + d2,
        straight_ly: dist(a_pos, b_pos),
        boosted_jumps: is_boosted(Some(m), d1, fuel, boost, m.range_at(fuel)) as usize,
        expansions: 0,
        elapsed_ms: 0,
        refuel_stops: 1 + (b_scoop && after_b < m.capacity) as usize,
        injections: 0, secondary_boosts: 0,
    })
}

/// A bridging leg: one supercharged hop to the star that lands closest to
/// `to` (found in the sphere of `k` ordinary jumps around `to`, not the
/// 400 ly one around the neutron), then a short exact search on to `to`.
/// A scoopable landing is preferred when the tank would otherwise be low.
fn bridge_leg(g: &Galaxy, req: &RouteRequest, ctl: &Control, from: u32, to: u32, fuel: f32, k: u32) -> Option<Route> {
    let a = g.record(from);
    let a_pos = a.pos();
    let b_pos = g.pos_of(to);
    let class_a = g.class(&a);
    let boost = crate::router::boost_at(g, req, from, class_a);
    let (range, full) = match req.fuel {
        Some(m) => (m.range_at(fuel), m.reach(m.capacity, 1.0)),
        None => (req.range_ly.max(1.0), req.range_ly.max(1.0) * (1.0 - crate::fuel::range_margin())),
    };
    let reach = range * boost * (1.0 - crate::fuel::range_margin());
    let low = req.fuel.is_some_and(|m| fuel < m.capacity * 0.5);
    // (idx, score): score = distance left to `to`, with a penalty when a
    // non-scoopable landing would leave the tank low.
    let mut best: Option<(u32, f32, f32)> = None;
    g.for_each_within(b_pos, full * k as f32, |s_idx, d2| {
        if s_idx == from || s_idx == to {
            return;
        }
        let s_pos = g.pos_of(s_idx);
        let d1 = dist(a_pos, s_pos);
        if d1 > reach || d1 < range * 0.5 {
            return;
        }
        let code = g.class_code(s_idx);
        let class = if code == crate::StarClass::Unknown.code() { g.class(&g.record(s_idx)) } else { crate::StarClass::from_code(code) };
        let left = match req.fuel {
            Some(m) => match m.jump(d1, fuel, boost) {
                Some(l) => l,
                None => return,
            },
            None => 0.0,
        };
        let score = d2 + if low && !g.scoopable(s_idx) { full } else { 0.0 } + if class.hazardous() { full } else { 0.0 };
        if best.is_none_or(|(_, bs, _)| score < bs) {
            best = Some((s_idx, score, left));
        }
    });
    let (s_idx, _, left) = best?;
    let s = g.record(s_idx);
    let class_s = g.class(&s);
    let d1 = dist(a_pos, s.pos());
    let s_scoop = g.scoopable(s_idx);
    let arrive = req.fuel.map(|m| if s_scoop { m.capacity } else { left });
    let first = crate::router::Hop {
        idx: from,
        id64: a.id64,
        name: g.name(&a).to_string(),
        pos: a_pos,
        class: class_a,
        scoopable: g.scoopable(from),
        distance_ly: 0.0,
        boosted: false,
        total_ly: 0.0,
        fuel_after: req.fuel.map(|_| fuel),
        refuel: false,
        fuel_optional: false,
        injection: None,
        synthesized: false,
        via_secondary: None,
    };
    let landing = crate::router::Hop {
        idx: s_idx,
        id64: s.id64,
        name: g.name(&s).to_string(),
        pos: s.pos(),
        class: class_s,
        scoopable: s_scoop,
        distance_ly: d1,
        boosted: is_boosted(req.fuel, d1, fuel, boost, range),
        total_ly: d1,
        fuel_after: arrive,
        refuel: req.fuel.is_some_and(|m| s_scoop && left < m.capacity),
        fuel_optional: false,
        injection: None,
        synthesized: false,
        via_secondary: None,
    };
    // The rest is a short ordinary run.
    let tail_req = RouteRequest { from: s_idx, to, weight: LEG_WEIGHT, thorough: false, max_expansions: 200_000, start_fuel: arrive.unwrap_or(0.0), ..req.clone() };
    let tail = plan(g, &tail_req, ctl).ok()?;
    let mut hops = vec![first, landing];
    for (j, mut h) in tail.hops.into_iter().enumerate() {
        if j == 0 {
            continue;
        }
        h.total_ly += d1;
        hops.push(h);
    }
    let total = hops.last().map(|h| h.total_ly).unwrap_or(d1);
    Some(Route {
        highway_pending: false,
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
        straight_ly: dist(a_pos, b_pos),
        boosted_jumps: is_boosted(req.fuel, d1, fuel, boost, range) as usize + tail.boosted_jumps,
        expansions: tail.expansions,
        elapsed_ms: 0,
        refuel_stops: hops.iter().filter(|h| h.refuel).count(),
        injections: 0, secondary_boosts: 0,
        hops,
    })
}

/// Cheapest way to refine one leg: direct hop, scoop hop, bridged hop with
/// a short tail search, and only then the full exact search.
/// `use_memo`: the first parallel pass over a chain takes a remembered leg;
/// a re-plan from the real tank (settle rounds, final pass) never does --
/// it exists because the remembered leg did not fly from that tank, and a
/// fresh plan from the exact figure is what converges (2026-10-17: with
/// the memo answering re-plans, eight settle rounds re-planned one leg
/// and Beagle Point -> Sol became no route).
fn refine_leg(g: &Galaxy, req: &RouteRequest, ctl: &Control, leg_of: &dyn Fn(u32, u32, f32) -> RouteRequest, from: u32, to: u32, fuel: f32, use_memo: bool) -> Result<Route, RouteError> {
    // Once per plot: the same leg from a tank the plan was made for, with
    // the same injections left and reserve, is the same answer for every
    // variant (R1 leg memo).
    let key = LegKey::new(from, to, req.injection.map(|(_, _, n)| n).unwrap_or(0), req.fuel.map(|m| m.reserve).unwrap_or(0.0));
    // ED_LEG_MEMO=0: bisect knob (R1 A/B).
    let memo_on = use_memo && std::env::var("ED_LEG_MEMO").ok().as_deref() != Some("0");
    let budget_ms = leg_of(from, to, fuel).time_budget_ms;
    if memo_on {
        if let Some(known) = ctl.legs.get(&key, fuel, budget_ms) {
            return known;
        }
    }
    let planned = refine_leg_uncached(g, req, ctl, leg_of, from, to, fuel);
    if memo_on && matches!(planned, Err(RouteError::Budget)) {
        ctl.legs.put_too_costly(key.clone(), fuel, budget_ms);
    }
    // Remembered for the first pass only; a re-plan from the real tank is
    // a repair of a leg the memo already holds (the memo keeps routes and
    // NoRoute, never Budget or Cancelled).
    if memo_on {
        ctl.legs.put(key, fuel, match &planned { Ok(r) => Ok(r.clone()), Err(e) => Err(e.clone_err()) });
    }
    planned
}

fn refine_leg_uncached(g: &Galaxy, req: &RouteRequest, ctl: &Control, leg_of: &dyn Fn(u32, u32, f32) -> RouteRequest, from: u32, to: u32, fuel: f32) -> Result<Route, RouteError> {
    if let Some(l) = direct_leg(g, req, from, to, fuel) {
        return Ok(l);
    }
    if std::env::var_os("ED_PLOT_DEBUG").is_some() {
        let a = g.record(from); let boost = crate::router::boost_at(g, req, from, g.class(&a));
        if let Some(m) = req.fuel { eprintln!("      direct failed {} -> {}: d {:.1} reach {:.1} (range {:.1} x {boost})", g.name(&a), g.name(&g.record(to)), dist(a.pos(), g.pos_of(to)), m.reach(fuel, boost), m.range_at(fuel)); }
    }
    // A scoop stop and a one-jump bridge both cost one extra jump; take the
    // scoop only when the tank is getting low, otherwise the plain bridge
    // (a coarse edge that missed by a hair should not become a fuel stop).
    let dbg = std::env::var_os("ED_PLOT_DEBUG").is_some();
    let note = |kind: &str| { if dbg { eprintln!("      leg {} -> {} ({:.0} ly, fuel {fuel:.0}): {kind}", g.name(&g.record(from)), g.name(&g.record(to)), dist(g.pos_of(from), g.pos_of(to))); } };
    let low = req.fuel.is_some_and(|m| fuel < m.capacity * 0.45);
    if low {
        if let Some(l) = scoop_leg(g, req, from, to, fuel) {
            note("scoop (low)");
            return Ok(l);
        }
    }
    if let Some(l) = bridge_leg(g, req, ctl, from, to, fuel, 1) {
        note("bridge1");
        return Ok(l);
    }
    if !low {
        if let Some(l) = scoop_leg(g, req, from, to, fuel) {
            note("scoop");
            return Ok(l);
        }
    }
    for k in 2..=3 {
        if let Some(l) = bridge_leg(g, req, ctl, from, to, fuel, k) {
            note("bridge k");
            return Ok(l);
        }
    }
    note("exact");
    plan(g, &leg_of(from, to, fuel), ctl)
}

/// Re-simulate a leg's fuel from `fuel` at its first hop, rewriting
/// `fuel_after`/`refuel` on the way. `false` when a jump is no longer
/// possible with the real tank level.
fn refuel_hops(g: &Galaxy, m: &crate::fuel::FuelModel, req: &RouteRequest, leg: &mut Route, fuel: f32) -> bool {
    let mut f = fuel;
    let mut refuels = 0usize;
    for i in 0..leg.hops.len() {
        if i == 0 {
            // The first hop is where the ship is: the tank is what it is, no
            // scooping is assumed there (the plan was made from this figure).
            leg.hops[0].fuel_after = Some(f);
            continue;
        }
        let prev_class = g.class(&g.record(leg.hops[i - 1].idx));
        let h = &mut leg.hops[i];
        let boost = if h.boosted {
            h.via_secondary.map_or_else(|| req.boost.for_class(prev_class), |(c, _)| req.boost.for_class(c))
        } else if h.injection.is_some() {
            req.injection.map(|(mult, _, _)| mult).unwrap_or(1.0)
        } else {
            1.0
        };
        let Some(left) = m.jump(h.distance_ly, f, boost) else { return false };
        let refuel = h.scoopable && left < m.capacity;
        f = if h.scoopable { m.capacity } else { left };
        h.fuel_after = Some(f);
        h.refuel = refuel;
        refuels += refuel as usize;
    }
    leg.refuel_stops = refuels;
    true
}

/// The least a straggler variant is ever given once a route exists.
/// Below this, equal-jump tie-break winners get cancelled mid-flight
/// (measured: a flat 350 ms grace cost Sol -> Sagittarius A* a scoop
/// stop); above the time-to-first, fast plots wait on variants that
/// lose anyway (a flat 1,000 ms held the 632 ms Colonia winner until
/// 1.4 s).
const GRACE_FLOOR_MS: u64 = 500;

/// The grace stragglers actually get: proportional to how long the
/// first route took, in BOTH directions. Fast first finishers shorten
/// the window (a 337 ms winner does not hold the plot for a second);
/// slow first finishers extend it past the configured value (a quick
/// mediocre variant must not cancel a better route that needs its
/// proportional share -- measured: the 114-jump Sol->Sagittarius A*
/// Mandalay route missed a capped window by 15 ms and 136 jumps
/// shipped instead), capped at 4x configured so true stragglers still
/// die. A configured grace below the floor stays authoritative.
fn effective_grace(configured: std::time::Duration, to_first: std::time::Duration) -> std::time::Duration {
    let floor = std::time::Duration::from_millis(GRACE_FLOOR_MS).min(configured);
    to_first.clamp(floor, configured * 4)
}

/// Route quality in pilot time, in DECISECONDS: the one currency (R2,
/// 2026-10-17, spec D1). Plain jumps x 53 s, boosted hops x 73 s (the
/// neutron-cone approach included), passing scoops x 6 s, full stops x
/// 120 s, injections x 1,800 s (the maintainer's price, spec D1), and the
/// scooped tonnes over the scoop rate (1.245 t/s, the
/// Caspian's 7A, when the model does not know). The 130-journal cut of
/// that day fitted the figures. A FITTED request (`t_jump_s`,
/// `stop_overhead_s`, item 28) keeps its per-commander plain jump and
/// stop overhead, with the boost surcharge (73 - 53 s) riding on top
/// until the journal fit learns it; ED_TJUMP_S / ED_STOP_OVERHEAD_S stay
/// the harness overrides. `stop_weight` scales everything refuelling
/// (stops, passing scoops, tonnes; item 20). Item 42's tonnage-blind
/// public stop (a flat 120 s) is superseded: the stop's tonnes are part
/// of the one currency everywhere, as the bench's own judge counts them.
#[allow(clippy::too_many_arguments)]
fn time_units(plain: usize, boosted: usize, passing_scoops: usize, injections: usize, refuel_stops: usize, scooped_t: f32, scoop_rate: f32, stop_weight: f32, t_jump_s: Option<f32>, stop_overhead_s: Option<f32>) -> u64 {
    let knob = |var: &str, default: f32| -> f32 {
        std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
    };
    let t_plain = t_jump_s.unwrap_or_else(|| knob("ED_TJUMP_S", T_PLAIN_S));
    let t_boost = t_plain + (T_BOOST_S - T_PLAIN_S);
    let rate = if scoop_rate > 0.0 { scoop_rate } else { SCOOP_RATE_DEFAULT };
    let overhead = stop_overhead_s.unwrap_or_else(|| knob("ED_STOP_OVERHEAD_S", T_STOP_S));
    let refuel_secs = refuel_stops as f32 * overhead + passing_scoops as f32 * T_PASS_SCOOP_S + scooped_t / rate;
    let secs = plain as f32 * t_plain + boosted as f32 * t_boost + injections as f32 * INJECTION_SECONDS + stop_weight * refuel_secs;
    (secs * 10.0) as u64
}

/// Arrivals where the tank rose without a marked stop: a top-up in
/// passing, priced at `T_PASS_SCOOP_S` plus its tonnes.
fn passing_scoops(r: &Route) -> usize {
    let (mut n, mut prev) = (0usize, None::<f32>);
    for h in &r.hops {
        if let (Some(f), Some(p)) = (h.fuel_after, prev) {
            if f > p + 0.05 && !h.refuel {
                n += 1;
            }
        }
        if h.fuel_after.is_some() {
            prev = h.fuel_after;
        }
    }
    n
}

/// Tonnes scooped along a route, from its own fuel ledger.
fn scooped_tonnes(r: &Route) -> f32 {
    let (mut t, mut prev) = (0.0f32, None::<f32>);
    for h in &r.hops {
        if let (Some(f), Some(p)) = (h.fuel_after, prev) {
            if f > p + 0.05 {
                t += f - p;
            }
        }
        if h.fuel_after.is_some() {
            prev = h.fuel_after;
        }
    }
    t
}

/// Item 29 study judge: the fuel gauge alone, in milli-tonnes. Exists
/// for the 29b swap gate — does judging on replenished tonnes rank
/// candidates like the fitted seconds model does? — and deliberately
/// prices nothing else: no jump time, no stop overhead. Those
/// blindnesses (five sips vs one fill; short-throw jump-heavy routes)
/// are what the study measures, so they stay unpatched here.
fn fuel_units(scooped_t: f32) -> u64 {
    (scooped_t.max(0.0) * 1000.0) as u64
}

/// The route judging used by best-selection, the grace clock, and the
/// wave improvement test. ED_JUDGE=fuel swaps in the item-29 study
/// gauge (study knob only; absent = the fitted seconds model).
/// The judge for callers outside this module (the server's early-answer
/// and best-so-far pickers): lower is better, the units the portfolio
/// ranks by. One judge everywhere (planner unification, R0, 2026-10-17).
pub fn route_score_public(r: &Route, req: &RouteRequest) -> u64 {
    route_score(r, req.fuel.map(|m| m.scoop_rate).unwrap_or(0.0), req)
}

fn route_score(r: &Route, scoop_rate: f32, req: &RouteRequest) -> u64 {
    if std::env::var("ED_JUDGE").is_ok_and(|v| v == "fuel") {
        return fuel_units(scooped_tonnes(r));
    }
    let boosted = r.boosted_jumps.min(r.jumps);
    time_units(r.jumps - boosted, boosted, passing_scoops(r), r.injections, r.refuel_stops, scooped_tonnes(r), scoop_rate, req.stop_weight, req.t_jump_s, req.stop_overhead_s)
}

/// The greedy cone's tunables, env-overridable for the knob-search
/// harness (defaults are the shipped values; the uber-experiment sets
/// these per run and the pin gate decides what ever becomes a new
/// default): ED_CONE_ANGLES="10,15,20" (widening half-angles, deg),
/// ED_CONE_CAP=8 (candidates relaxed per engaged expansion; below 4
/// measured +3 refuel stops), ED_CONE_BAND=0.8 (inner edge of the
/// reach band as a fraction of boosted reach).
fn cone_knobs() -> (Vec<f32>, usize, f32) {
    let angles = std::env::var("ED_CONE_ANGLES")
        .ok()
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect::<Vec<f32>>())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![10.0, 15.0, 20.0]);
    let cap = std::env::var("ED_CONE_CAP").ok().and_then(|v| v.parse().ok()).unwrap_or(8);
    let band_lo = std::env::var("ED_CONE_BAND").ok().and_then(|v| v.parse().ok()).unwrap_or(0.8f32).clamp(0.1, 0.99);
    (angles, cap, band_lo)
}

/// Item 24c rung 1: the on-ramp for a plot whose straight line opens
/// through thin highway (the Sol slab tax: 4x the off-slab twins'
/// coarse expansions, all spent discovering the sidestep euclid will
/// not suggest). Once per plot, find the cheapest credible entry onto
/// the goal field near the start: the occupied sub-index cell that
/// minimises start-distance (in reference jumps) plus the field's own
/// jumps-to-goal bound, gated by a sane angle off the goal bearing and
/// a small detour fraction so short and healthy plots never engage.
/// Everything derives from the measured field (chain-viable cells) --
/// no slab bounds, no named geography, per the item 24 covenant.
/// Returns (ramp centre, its field bound in jumps, its goal distance).
/// `mirror`: the off-ramp variant (ends swapped by the caller). The
/// contrast gate is skipped -- at a thin goal (the rim) every count is
/// 1-3 and flank-vs-inline contrast is meaningless; the inverted
/// gradient plus chainability carry the decision.
fn on_ramp(
    neutrons: &Galaxy,
    field: Option<&crate::cgraph::GoalField>,
    start_pos: [f32; 3],
    goal_pos: [f32; 3],
    straight: f32,
    ref_jump_ly: f32,
    mirror: bool,
) -> Option<([f32; 3], f32, f32)> {
    let knob = |var: &str, default: f32| -> f32 {
        std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
    };
    if knob("ED_RAMP", 1.0) == 0.0 {
        return None;
    }
    // The seed serves the corridor the field precheck calls healthy and
    // leaves blind (Sol pays its slab tax in exactly that state). Where
    // a goal field EXISTS, its honest crossing prices already own the
    // detour knowledge -- adding the ramp raise double-counts and
    // reproducibly degrades field cells (sweep16: Wongi->Spase E +6
    // stops, Pheia->Spase E +4, CS e86 +12 j via an early wave exit)
    // while the no-field cells took the wins. ED_RAMP=2 forces the
    // seed on field cells too, for the harness.
    // (The mirror WANTS the field: it prices toward the real goal, so
    // field.jumps_to_goal of a cell near that goal is exactly the
    // chain-quality the off-ramp selects on.)
    if field.is_some() && !mirror && knob("ED_RAMP", 1.0) < 2.0 {
        return None;
    }
    let cos_max = knob("ED_RAMP_ANGLE_DEG", 40.0).to_radians().cos();
    let detour_max = knob("ED_RAMP_DETOUR", 0.05);
    let radius = knob("ED_RAMP_RADIUS_LY", 6_000.0);
    let chain_floor = knob("ED_RAMP_CHAIN", 2.0) as u32;
    let cell_ly = neutrons.cell_ly;
    let to_goal = [goal_pos[0] - start_pos[0], goal_pos[1] - start_pos[1], goal_pos[2] - start_pos[2]];
    let lo = crate::format::cell_of_with(
        [start_pos[0] - radius, start_pos[1] - radius, start_pos[2] - radius],
        cell_ly,
    );
    let hi = crate::format::cell_of_with(
        [start_pos[0] + radius, start_pos[1] + radius, start_pos[2] + radius],
        cell_ly,
    );
    // Engagement is decided by measured density CONTRAST, not geometry
    // alone: the seed exists for starts whose straight line opens thin
    // while a concentration sits on the flank (Sol's slab). A healthy
    // start -- dense cells along its own bearing -- must disengage
    // entirely, leaving the heuristic byte-identical (rung-1 v1 had no
    // such gate and perturbed the off-slab twins +1 jump each).
    let inline_cos = knob("ED_RAMP_INLINE_DEG", 10.0).to_radians().cos();
    let contrast = knob("ED_RAMP_CONTRAST", 3.0);
    let degree_floor = knob("ED_RAMP_DEGREE", 8.0) as usize;
    // (score, centre, field jumps, goal dist, inline?, cell count)
    let mut cands: Vec<(f32, [f32; 3], f32, f32, bool, u32)> = Vec::new();
    neutrons.for_each_cell_in_box(lo, hi, |cx, cy, cz, _start, count| {
        if count < chain_floor {
            return std::ops::ControlFlow::Continue(());
        }
        let c = [(cx as f32 + 0.5) * cell_ly, (cy as f32 + 0.5) * cell_ly, (cz as f32 + 0.5) * cell_ly];
        let ds = dist(start_pos, c);
        if ds > radius || ds < cell_ly {
            return std::ops::ControlFlow::Continue(());
        }
        let v = [c[0] - start_pos[0], c[1] - start_pos[1], c[2] - start_pos[2]];
        let dot = v[0] * to_goal[0] + v[1] * to_goal[1] + v[2] * to_goal[2];
        if dot < ds * straight * cos_max {
            return std::ops::ControlFlow::Continue(());
        }
        let dg = dist(c, goal_pos);
        if (ds + dg - straight) / straight > detour_max {
            return std::ops::ControlFlow::Continue(());
        }
        let Some(i) = neutrons.cell_index(cx, cy, cz) else {
            return std::ops::ControlFlow::Continue(());
        };
        // Chainability: a ramp must be a highway CONCENTRATION, not a
        // lone star (the scoop-blind autopsy). With a goal field, the
        // field's own keep filter is the oracle -- no bound means the
        // cell chains to nothing. Without one (a corridor the
        // non-uniformity precheck calls healthy -- Sol pays its 4x slab
        // tax in exactly that state), the cell graph's highway degree
        // is the measured stand-in.
        let fj = match field {
            Some(f) => match f.jumps_to_goal(i) {
                Some(fj) => fj,
                None => return std::ops::ControlFlow::Continue(()),
            },
            None => {
                let connected = neutrons
                    .cell_graph()
                    .is_some_and(|cg| cg.neighbours(i).len() >= degree_floor);
                if !connected {
                    return std::ops::ControlFlow::Continue(());
                }
                dg / ref_jump_ly
            }
        };
        let inline = dot >= ds * straight * inline_cos;
        // Mirror scoring: the through-cost of entering the thin goal
        // region at this cell is the START-side euclid (dg, toward the
        // swapped "goal" = the real start) plus the cell's field bound
        // to the real goal -- ds here is the tiny goal-side hop and
        // carries no signal about which entry CHAINS.
        let score = if mirror { dg / ref_jump_ly + fj } else { ds / ref_jump_ly + fj };
        cands.push((score, c, fj, dg, inline, count));
        std::ops::ControlFlow::Continue(())
    });
    // Engagement and selection are separate questions. ENGAGE only when
    // the start measures slab-like: some flank concentration out-densities
    // the best inline cell by the contrast factor (healthy starts fail
    // this and stay byte-identical). Once engaged, SELECT the best-scoring
    // ramp among all candidates, inline included -- on Sol the inline
    // forward beacon wins the route (67 j / 7 stops) while the flank pick
    // paid 2 extra stops for its expansion cut; the search follows the
    // score, not the reason we woke it.
    let d_in = cands.iter().filter(|c| c.4).map(|c| c.5).max().unwrap_or(0).max(1);
    // Gradient gate: the seed only helps routes INTO denser highway
    // (sweep16 decomposition: every reproducible loss -- Wongi->Spase
    // +6 stops, Pheia->Spase +4 -- flies toward the desert, where the
    // opening beacon anchors a scoopy lane the thinning field never
    // corrects; every kept win flies toward concentration). Measured
    // the same way at both ends: densest cell in the goal's radius box
    // must match the start's inline density.
    // Default 2.0: the seed's wins measured goal/start gradients of 10x+
    // (Sol -> Sag A* 191/18) while its reproducible losses sat at 1.4x
    // (Wongi -> Spase 27/19) -- routes INTO concentration benefit, routes
    // into thinning space get anchored onto scoopy lanes.
    let gradient = knob("ED_RAMP_GRADIENT", 2.0);
    // The goal's own inline lane, measured exactly like d_in but looking
    // back along the corridor -- a raw box-max leaks (Spase's box holds
    // desert-shore pockets that pass any threshold while the approach
    // lane itself is empty).
    let goal_density = {
        let glo = crate::format::cell_of_with(
            [goal_pos[0] - radius, goal_pos[1] - radius, goal_pos[2] - radius],
            cell_ly,
        );
        let ghi = crate::format::cell_of_with(
            [goal_pos[0] + radius, goal_pos[1] + radius, goal_pos[2] + radius],
            cell_ly,
        );
        let back = [start_pos[0] - goal_pos[0], start_pos[1] - goal_pos[1], start_pos[2] - goal_pos[2]];
        let mut max = 0u32;
        neutrons.for_each_cell_in_box(glo, ghi, |cx, cy, cz, _start, count| {
            let c = [(cx as f32 + 0.5) * cell_ly, (cy as f32 + 0.5) * cell_ly, (cz as f32 + 0.5) * cell_ly];
            let dg = dist(goal_pos, c);
            if dg <= radius && dg >= cell_ly {
                let v = [c[0] - goal_pos[0], c[1] - goal_pos[1], c[2] - goal_pos[2]];
                let dot = v[0] * back[0] + v[1] * back[1] + v[2] * back[2];
                if dot >= dg * straight * inline_cos {
                    max = max.max(count);
                }
            }
            std::ops::ControlFlow::Continue(())
        });
        max
    };
    let engaged = goal_density as f32 >= gradient * d_in as f32
        && (mirror || cands.iter().any(|c| !c.4 && c.5 as f32 >= contrast * d_in as f32));
    let best: Option<(f32, [f32; 3], f32, f32)> = engaged
        .then(|| {
            cands
                .iter()
                .map(|&(s, c, fj, dg, _, _)| (s, c, fj, dg))
                .min_by(|a, b| a.0.total_cmp(&b.0))
        })
        .flatten();
    if std::env::var("ED_RAMP_DEBUG").is_ok() {
        let flank_max = cands.iter().filter(|c| !c.4).map(|c| c.5).max().unwrap_or(0);
        eprintln!("    ramp gate: d_in {d_in} goal_density {goal_density} flank {flank_max} engaged {engaged}");
        match best {
            Some((s, c, fj, dg)) => eprintln!(
                "    ramp: [{:.0} {:.0} {:.0}] score {s:.1} (field {fj:.1} j, {dg:.0} ly to goal, {:.0} ly from start; d_in {d_in} flank {flank_max}, {} cands)",
                c[0], c[1], c[2], dist(start_pos, c), cands.len()
            ),
            None => eprintln!(
                "    ramp: none (d_in {d_in} flank {flank_max}, {} cands, radius {radius:.0}, detour {detour_max:.3})",
                cands.len()
            ),
        }
    }
    best.map(|(_, c, fj, dg)| (c, fj, dg))
}

/// Is a scoop-flagged candidate a THROUGH-stop -- does the highway
/// continue goalward within the departing boosted reach? A dead-end
/// scoopable pulls the route off the chain and taxes it coming back
/// (item 23, measured: the ascending Beagle Mandalay spent 4,138 ly of
/// plain flying around its refuels against the descent's 1,572 -- the
/// bulk of the 50-jump direction asymmetry). One cell probe at 0.75 x
/// reach along the goalward line: coarse, cheap, and only consulted
/// for flagged candidates.
fn through_stop(neutrons: &Galaxy, cand: [f32; 3], target: [f32; 3], reach: f32) -> bool {
    let d = dist(cand, target);
    if d <= reach {
        return true; // the goal itself continues the run
    }
    let u = [(target[0] - cand[0]) / d, (target[1] - cand[1]) / d, (target[2] - cand[2]) / d];
    let p = [cand[0] + u[0] * reach * 0.75, cand[1] + u[1] * reach * 0.75, cand[2] + u[2] * reach * 0.75];
    neutrons.cell_index_of_pos(p).is_some()
}

/// Item 23 knobs: bonus magnitude in boosted-reach units and the
/// fraction a dead-end stop keeps. Search-harness parameters.
fn refuel_knobs() -> (f32, f32) {
    let k = std::env::var("ED_REFUEL_BONUS").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0f32);
    // Default 1.0 = shipped behaviour exactly: the coarse-level
    // through-stop discount measured INERT on the Beagle Mandalay
    // (the dead-end choice is made in the exact LEG planner, which
    // picks the scoop star inside each neutron-to-neutron leg -- see
    // item 23's window analysis). The knob stays for the search
    // harness; the mechanism waits for the leg-level fix.
    let dead = std::env::var("ED_DEADEND_FACTOR").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0f32);
    (k, dead)
}

/// The grace clock (item 14a). Armed by the first credible finish; RESET
/// by any later finish that strictly improves the best (jumps, stops)
/// tuple, so the portfolio ends when improvement stops rather than when
/// the first finisher's window expires. Resets are bounded by construction
/// (a variant finishes once) and the wave budget backstops everything.
/// `seed` is the standing best from earlier waves: an escalation wave
/// exists only to improve, so a finish that merely re-confirms the seed
/// must not arm the guillotine over the deep variants still digging.
struct GraceClock {
    seed: Option<u64>,
    /// (armed-or-reset at, time-to-first-finish, best score seen)
    state: Option<(std::time::Instant, std::time::Duration, u64)>,
}

impl GraceClock {
    fn new(seed: Option<u64>) -> Self {
        Self { seed, state: None }
    }
    /// A variant finished with `score` = [`route_score`] units; `arms` is
    /// the caller's credibility verdict for this variant kind.
    fn finish(&mut self, now: std::time::Instant, since_start: std::time::Duration, score: u64, arms: bool) {
        match &mut self.state {
            // Armed: any strict improvement renews the window -- even from
            // a non-arming kind, since a renewal only delays cancellation.
            Some((at, _, best)) => {
                if score < *best {
                    *at = now;
                    *best = score;
                }
            }
            // Unarmed: a credible finish arms, but past the first wave only
            // when it actually beats the standing best -- a re-confirmation
            // must not guillotine the deep variants still digging.
            None => {
                if arms && self.seed.is_none_or(|s| score < s) {
                    self.state = Some((now, since_start, score));
                }
            }
        }
    }
    /// Expired: armed, past the effective window, and past the quality
    /// floor when the wave has one (`floor_until`).
    fn expired(&self, now: std::time::Instant, grace: std::time::Duration, floor_until: Option<std::time::Instant>) -> bool {
        if floor_until.is_some_and(|f| now < f) {
            return false;
        }
        self.state
            .is_some_and(|(at, to_first, _)| now.saturating_duration_since(at) > effective_grace(grace, to_first))
    }
}

/// "Spend a little" (the maintainer, 2026-10-18): the first wave is not
/// guillotined before this long after it started, so the slow diggers get
/// a worker on the plots the quick variants answer in a second. Ten
/// seconds, at most a tenth of the budget (the interactive lane's 30 s
/// gets 3 s), the first wave only (escalation waves are seeded and keep
/// their own clocks). The pilot still gets the first route at first-found
/// (the long lane answers at first + 5 s early grace and keeps refining):
/// the floor delays the final answer, never the first. `knob` is
/// ED_QUALITY_FLOOR_MS for the harness: its value stands, 0 turns it off.
const QUALITY_FLOOR_MS: u64 = 10_000;
fn quality_floor(time_budget_ms: u64, first_wave: bool, knob: Option<u64>) -> std::time::Duration {
    if !first_wave {
        return std::time::Duration::ZERO;
    }
    let ms = match knob {
        Some(v) => v,
        None if time_budget_ms > 0 => QUALITY_FLOOR_MS.min(time_budget_ms / 10),
        None => QUALITY_FLOOR_MS,
    };
    std::time::Duration::from_millis(ms)
}

/// Run `n` variants over the rayon pool. Once one has a route, the others
/// get [`effective_grace`] of `grace` to finish (they may still win) and
/// are then told to stop through the cancel check handed to them; `None`
/// waits for all. Returns every result in variant order and how many
/// finished with a route.
/// The coarse widen rule. A scan that saw nothing is a void: widen. A scan
/// whose candidates were all already known widens too, every time (the
/// shipped rule): the R0 timers showed that rescan is where the coarse
/// millisecond goes (22,700 of 44,000 expansions on the rim pair, 6.3 of
/// 9 s per variant), but the two cheaper rules measured on 2026-10-17 both
/// paid in route quality -- empty-only: +9 jumps on Sol -> Beagle Point,
/// +30 s end to end on Colonia -> Spase; once per 250 ly cell: still +9
/// on Sol -> Beagle Point -- because those rescans supply the long
/// bridge edges the coarse graph has no other way to see. The maintainer's
/// order is route first, time second, so the edges stay; R3 moves the long
/// bridges into the ordinary scan and retires the rescan properly.
/// `ED_COARSE_WIDEN_ONCE_PER_CELL=1` selects the per-cell rule for A/B.
fn widen_due(pushed: u64, scan_cands: usize, cell_widened_before: bool, once_per_cell: bool) -> bool {
    pushed == 0 && (scan_cands == 0 || !once_per_cell || !cell_widened_before)
}

/// Direction sector of `there` seen from `here`: each axis -1 / 0 / +1,
/// a component under 30 % of the distance counting as 0 (26 sectors plus
/// the impossible all-zero one).
fn sector_of(here: [f32; 3], there: [f32; 3]) -> u8 {
    let d = [there[0] - here[0], there[1] - here[1], there[2] - here[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
    let mut s = 0u8;
    for c in d {
        let v = if c.abs() < 0.3 * len { 1 } else if c > 0.0 { 2 } else { 0 };
        s = s * 3 + v;
    }
    s
}

/// Sector k-best thinning of a coarse scan (R1, 2026-10-17): per
/// direction sector from `here`, keep the `keep` candidates with the most
/// progress (lowest score), the nearest one (least fuel to get there) and
/// the best-scoring refuel option. In the core a scan holds 500-1,700
/// neutrons and the bucket thinning still relaxed 500-1,250 of them; the
/// coarse plan cannot tell neighbours in one direction apart, so at most
/// 26 x (keep + 2) survive. Order is not preserved. Candidates are
/// (score, index, position, distance).
pub(crate) fn thin_by_sector(cands: &mut Vec<(f32, u32, [f32; 3], f32)>, here: [f32; 3], refuels: &dyn Fn(u32) -> bool, keep: usize) {
    let mut sectors: Vec<Vec<usize>> = vec![Vec::new(); 27];
    for (i, c) in cands.iter().enumerate() {
        sectors[sector_of(here, c.2) as usize].push(i);
    }
    let mut chosen: Vec<usize> = Vec::with_capacity(27 * (keep + 2));
    for members in sectors.iter_mut() {
        if members.is_empty() {
            continue;
        }
        members.sort_by(|&a, &b| cands[a].0.total_cmp(&cands[b].0));
        chosen.extend(members.iter().take(keep.max(1)));
        if let Some(&near) = members.iter().min_by(|&&a, &&b| cands[a].3.total_cmp(&cands[b].3)) {
            chosen.push(near);
        }
        if let Some(&fuel) = members.iter().find(|&&i| refuels(cands[i].1)) {
            chosen.push(fuel);
        }
    }
    chosen.sort_unstable();
    chosen.dedup();
    let kept: Vec<(f32, u32, [f32; 3], f32)> = chosen.into_iter().map(|i| cands[i]).collect();
    *cands = kept;
}

/// The family a portfolio slot belongs to, by the same index arithmetic
/// the dispatch and the debug label use.
#[allow(clippy::too_many_arguments)]
fn variant_kind(i: usize, witness: usize, thin: usize, nb: usize, bidi: bool, retro: bool, greedy_slots: usize, offramp_slots: usize, scan_twins: usize, corridor_slot: bool, total: usize) -> &'static str {
    let lead = witness + thin;
    if i < lead {
        return lead_kind(i, witness);
    }
    let (i, total) = (i - lead, total - lead);
    let b = usize::from(bidi);
    if i < nb {
        "single"
    } else if bidi && i == nb {
        "bidi"
    } else if retro && i == total - 1 {
        "retro"
    } else if greedy_slots >= 1 && i == nb + b {
        "greedy single"
    } else if greedy_slots > 1 && i == nb + b + 1 {
        "greedy bidi"
    } else if scan_twins > 0 && i >= nb + b + greedy_slots + offramp_slots && i < nb + b + greedy_slots + offramp_slots + scan_twins {
        "scan-twin"
    } else if corridor_slot && i == nb + b + greedy_slots + offramp_slots + scan_twins {
        "corridor"
    } else {
        "offramp"
    }
}

/// Run the wave's variants, starting them in `order` (a permutation of
/// `0..n`) as workers come free, or, with no order, in rayon's range
/// split as every wave did before R1 -- the default, see
/// `declared_order_on`. On the box a long plot has three threads, so the
/// first three variants started decide the first route. No stage
/// boundary: a head stage whose `collect` had to finish before the tail
/// could start measured as a loss on the matrix (arm r1-head3: Sol ->
/// Beagle Point 18 -> 101 s, Wongi -> Blaa Hypai 1 -> 60 s, same routes),
/// because one slow head variant held back every quick one behind it.
/// `par_bridge` pulls the sequential order into whichever worker is idle.
fn run_variants<T: Send>(
    n: usize,
    grace: Option<std::time::Duration>,
    floor: std::time::Duration,
    seed: Option<u64>,
    cancelled: &(dyn Fn() -> bool + Sync),
    score: impl Fn(&T) -> u64 + Sync,
    run: impl Fn(usize, &(dyn Fn() -> bool + Sync)) -> (Result<T, RouteError>, bool) + Sync,
    order: &[usize],
) -> (Vec<Result<T, RouteError>>, u32, u32) {
    use rayon::prelude::*;
    let wave_started = std::time::Instant::now();
    let floor_until = (!floor.is_zero()).then(|| wave_started + floor);
    let clock: std::sync::Mutex<GraceClock> = std::sync::Mutex::new(GraceClock::new(seed));
    let stop = |grace: std::time::Duration| {
        clock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .expired(std::time::Instant::now(), grace, floor_until)
    };
    let one = |i: usize| -> Result<T, RouteError> {
        let check = || cancelled() || grace.is_some_and(stop);
        // `arms`: whether this finish may start the grace countdown.
        // A variant with a credible route arms it; one whose result
        // is suspect (the bidi variant far off the theoretical jump
        // floor) finishes without hurrying the others. The clock also
        // renews on any finish that improves the best score (14a).
        let (res, arms) = run(i, &check);
        if let Ok(t) = &res {
            clock.lock().unwrap_or_else(|e| e.into_inner()).finish(
                std::time::Instant::now(),
                wave_started.elapsed(),
                score(t),
                arms,
            );
        }
        res
    };
    let mut slots: Vec<Option<Result<T, RouteError>>> = (0..n).map(|_| None).collect();
    let done: Vec<(usize, Result<T, RouteError>)> = if order.len() == n {
        order.iter().copied().par_bridge().map(|i| (i, one(i))).collect()
    } else {
        (0..n).into_par_iter().map(|i| (i, one(i))).collect()
    };
    for (i, r) in done {
        slots[i] = Some(r);
    }
    let started = slots.iter().filter(|r| r.is_some()).count() as u32;
    let results: Vec<Result<T, RouteError>> = slots.into_iter().map(|r| r.unwrap_or(Err(RouteError::Cancelled))).collect();
    let finished = results.iter().filter(|r| r.is_ok()).count() as u32;
    (results, finished, started)
}

/// Whether a wave starts its variants in the declared `variant_order`
/// (ED_VARIANT_ORDER=1) or in rayon's range split, the default. The
/// declared order (R1) measured NEGATIVE where it is visible at all --
/// the box's three planner threads (arm r1-3 against before3, 2026-10-17,
/// docs/benches/2026-10-17-planner-matrix.csv): 8 of 28 routes worse,
/// Colonia <-> Spase first route 0.8 -> 25.6 s and the plot ran to the
/// budget, Sol -> Beagle Point first 0.13 -> 1.3 s and 50 -> 100 s wall,
/// Jongou AA-A 396 -> 455 jumps at the cut. The variant that finds those
/// routes in under a second sits late in the family order, and rayon's
/// split happens to start it among the first three. On the PC (24
/// threads, 17 variants) the order is moot. Making that start intentional
/// is R3's tabulation of which family wins which route class; until it
/// is measured, the split order stands.
fn declared_order_on(knob: Option<&str>) -> bool {
    knob == Some("1")
}

/// No banned edges: for callers outside a plot's portfolio (tests, the
/// retrograde and crossing planners).
pub(crate) static NO_BANS: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());

/// No incumbent: the coarse search prunes nothing.
pub(crate) static NO_INCUMBENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);

/// Run a coarse variant, re-running it while a refinement failure banned a
/// new coarse edge (LazySP, R3 Task 3): at most `REPAIRS` repairs. A re-run
/// sets `repaired`: its route is a rescue where the coarse graph lied, not
/// a credible optimum, and the caller never lets it arm the grace clock
/// (arm w5p-3, 2026-10-18: the repaired base single's 425 j rim route armed
/// at 6 s and guillotined the 412 j dig).
const REPAIRS: usize = 2;
fn with_repair<T>(banned: &std::sync::Mutex<Vec<(u32, u32)>>, repaired: &std::sync::atomic::AtomicBool, mut run: impl FnMut() -> Result<T, RouteError>) -> Result<T, RouteError> {
    let count = || banned.lock().unwrap_or_else(|e| e.into_inner()).len();
    let mut known = count();
    let mut res = run();
    for _ in 0..REPAIRS {
        let now = count();
        // Only a failure that banned a new edge (this variant's or another's)
        // is worth a re-run.
        if res.is_ok() || matches!(res, Err(RouteError::Cancelled)) || now == known {
            break;
        }
        known = now;
        repaired.store(true, std::sync::atomic::Ordering::Relaxed);
        res = run();
    }
    res
}

/// R3 Task 6, the bound-gap stop (default on; ED_NO_BOUND_STOP=1 ablates): no escalation wave
/// while the standing best is under the credibility ceiling -- the gap to
/// the bound is inside the slack the arming already accepts, so no dig can
/// pay for itself (Wongi -> Blaa Hypai, arm w6-3: a 38 j best under a 38.7
/// ceiling escalated to the x4 wave because the first wave ended 50 ms
/// before the grace window did, and the plot ran 100 s instead of 1.6 s).
fn wave_stops_at_bound(best_jumps: Option<usize>, ceiling: f32, enabled: bool) -> bool {
    enabled && best_jumps.is_some_and(|j| j as f32 <= ceiling)
}

/// Whether the base single's route arms the grace clock. Always, unless it
/// came from a LazySP repair ABOVE the credibility ceiling: a rescue where
/// the coarse graph lied is not a credible optimum (arm w5p-3, 2026-10-18:
/// the repaired 425 j rim route armed at 6 s and guillotined the 412 j
/// dig), while a repair under the ceiling is as good as the unrepaired
/// route (a refused 38 j Wongi -> Blaa Hypai ran 100 s for nothing). With
/// `ceiling_gated` (ED_SINGLE_CEILING=1) the single arms only under the
/// ceiling, like the bidi.
fn single_arms(repaired: bool, jumps: f32, ceiling: f32, ceiling_gated: bool) -> bool {
    let under = jumps <= ceiling;
    if ceiling_gated { under } else { !repaired || under }
}

/// The witness variant (R3 Task 2): the feasibility search's flyable star
/// sequence, every star a waypoint, refined by `refine_waypoints` (one-hop
/// legs are direct; an injected or unfundable hop goes to the exact leg
/// planner; the settle rounds fly it with the real tank). Long where the
/// greedy sides were weak, but a route -- on the reverse Jongou pair the
/// only one (2026-10-18, `docs/benches/2026-10-18-witness.csv`).
fn plan_witness(g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, ctl: &Control, started: std::time::Instant) -> Result<Route, RouteError> {
    // The witness's waypoints are not coarse waypoints: its refinement does
    // not draw on the search trace (a plain hop through a white dwarf the
    // profile cannot boost is a witness hop, not a highway choice).
    let quiet = |_: &'static str, _: [f32; 3], _: f32| {};
    let ctl = &Control { cancelled: ctl.cancelled, progress: ctl.progress, stage: ctl.stage, found: ctl.found, legs: ctl.legs, variant: ctl.variant, trace: &quiet };
    let w = crate::feasible::find_witness(g, neutrons, req, ctl.cancelled)?;
    let fuel_model = req.fuel.map(|m| crate::fuel::FuelModel { reserve: m.reserve.max(m.max_fuel_per_jump * RESERVE_FRACTION), ..m });
    let capacity = fuel_model.map(|m| m.capacity).unwrap_or(0.0);
    let start_fuel = fuel_model.map(|m| req.start_fuel.clamp(0.0, m.capacity)).unwrap_or(0.0);
    let straight = dist(g.pos_of(req.from), g.pos_of(req.to));
    if std::env::var_os("ED_PLOT_DEBUG").is_some() {
        eprintln!("    witness: {} hops, {} injections, {} expansions at {} ms", w.stars.len().saturating_sub(1), w.injections.len(), w.expansions, started.elapsed().as_millis());
    }
    // Every witness star a waypoint: one-hop legs are direct, an injected
    // or unfundable hop goes to the exact leg planner, the settle rounds fly
    // it with the real tank. ED_WITNESS_WINDOW_LY=<ly> refines in windows
    // instead (measured 2026-10-18: a failing 1,000 ly window cost its time
    // cap and Colonia -> Spase's first route moved 6 -> 26 s; the
    // optimisers improve on the witness, the witness is the floor).
    // Eager refuelling: the witness assumed a refill at every scoopable
    // star; the min-fuel refinement stripped the stops it relied on and its
    // final pass met a neutron hop with 7 t in the tank (reverse Jongou,
    // 2026-10-18). The judge prunes the eager route afterwards.
    let eager = witness_refinement_request(req);
    let req = &eager;
    let window_ly: f32 = std::env::var("ED_WITNESS_WINDOW_LY").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let windowed = (window_ly > 0.0).then(|| witness_windows(g, &w.stars, window_ly, capacity));
    let raw = || w.stars.iter().map(|&s| (s, capacity)).collect::<Vec<(u32, f32)>>();
    let refined = match windowed {
        Some(windows) => match refine_waypoints(g, req, ctl, windows, start_fuel, fuel_model, w.expansions, straight, started) {
            Ok(r) => Ok(r),
            Err(RouteError::Cancelled) => Err(RouteError::Cancelled),
            Err(e) => {
                if std::env::var_os("ED_PLOT_DEBUG").is_some() {
                    eprintln!("    witness: windowed refinement failed ({e:?}); the raw witness stands");
                }
                refine_waypoints(g, req, ctl, raw(), start_fuel, fuel_model, w.expansions, straight, started)
            }
        },
        None => refine_waypoints(g, req, ctl, raw(), start_fuel, fuel_model, w.expansions, straight, started),
    };
    let mut route = refined?;
    // The eager refinement marks a stop at every scoopable star (683 on the
    // rim reverse): prune to the stops the tank needs before anyone sees it.
    if let Some(m) = fuel_model {
        let mut lean = route.clone();
        if crate::router::minimize_refuels(&m, &req.boost, req.injection.map(|(mult, _, _)| mult), &mut lean, start_fuel) {
            route = lean;
        }
    }
    // Offered at once: the server's best-so-far picker ranks it against
    // every later answer. A 3 s hold measured negative on three threads
    // (2026-10-18, arm w4-3): the holding worker slept a third of the lane,
    // short routes' walls doubled, the delayed arming let the x4 waves fire.
    Ok(route)
}

/// The request the witness is refined under: the caller's, with eager
/// refuelling whatever its fuel mode.
fn witness_refinement_request(req: &RouteRequest) -> RouteRequest {
    RouteRequest { min_fuel: false, ..req.clone() }
}

/// Waypoints along the witness every ~`window_ly` of path, always at a
/// witness star, the ends included; the tank estimate at each is full.
fn witness_windows(g: &Galaxy, stars: &[u32], window_ly: f32, capacity: f32) -> Vec<(u32, f32)> {
    let mut out = vec![(stars[0], capacity)];
    let mut run = 0.0f32;
    for w in stars.windows(2) {
        run += dist(g.pos_of(w[0]), g.pos_of(w[1]));
        if run >= window_ly {
            out.push((w[1], capacity));
            run = 0.0;
        }
    }
    if out.last().map(|l| l.0) != stars.last().copied() {
        out.push((*stars.last().unwrap(), capacity));
    }
    out
}

/// The order a wave starts its variants in: by family, the measured
/// first-to-answer families first, the deep diggers last; within a
/// family, index order. From the r1 matrix arm's per-variant outcomes
/// (2026-10-17, 28 routes, this PC; `knobs/variant_first_found.py`):
/// first to answer on a route -- greedy single 10, scan-twin 5, greedy
/// bidi 4, single 2, offramp 2, bidi 2; median ms to a route -- offramp
/// 31, greedy single 31, single 32, greedy bidi 123, bidi 125, scan-twin
/// 241 (scan twins are slow on easy routes and first on the hard ones).
/// The leading slots: the witness (R3, when on) then the thin pair.
fn lead_kind(i: usize, witness: usize) -> &'static str {
    match i + 1 - witness {
        0 => "witness",
        1 => "thin single",
        _ => "thin bidi",
    }
}

const KIND_ORDER: &[&str] = &["witness", "thin single", "thin bidi", "greedy single", "scan-twin", "greedy bidi", "single", "offramp", "bidi", "corridor", "retro"];

#[allow(clippy::too_many_arguments)]
fn variant_order(total: usize, witness: usize, thin: usize, nb: usize, bidi: bool, retro: bool, greedy_slots: usize, offramp_slots: usize, scan_twins: usize, corridor_slot: bool) -> Vec<usize> {
    let rank = |i: usize| KIND_ORDER.iter().position(|k| *k == variant_kind(i, witness, thin, nb, bidi, retro, greedy_slots, offramp_slots, scan_twins, corridor_slot, total)).unwrap_or(KIND_ORDER.len());
    let mut order: Vec<usize> = (0..total).collect();
    order.sort_by_key(|&i| (rank(i), i));
    order
}

/// `neutrons` is the sub-index written by `import::subset` (neutron stars
/// only). Coarse nodes are neutron-index positions; they are mapped back
/// to the full index by name when the plan is refined.
pub fn plan_long(g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, ctl: &Control) -> Result<Route, RouteError> {
    // The coarse cost model is a proxy, so "more optimal" under it is not
    // fewer jumps, and the greedy pass flips between scoop and straight
    // edges on tiny parameter changes. What works is planning a few
    // variants at once (they run in parallel) and keeping the shortest:
    // three for a normal plot, six for thorough.
    let started = std::time::Instant::now();
    let base: Vec<(f32, f32)> = if req.thorough {
        [1.3f32, 1.5, 1.8].iter().flat_map(|&w| [0.4f32, 0.75].iter().map(move |&r| (w, r))).collect()
    } else {
        vec![(COARSE_WEIGHT, RESERVE_FRACTION), (COARSE_WEIGHT, 0.4), (1.3, RESERVE_FRACTION)]
    };
    // Candidate thinning is a second axis: a coarser cell is faster and
    // sometimes better, a finer one keeps more lateral options, and no
    // thinning at all (thorough only) is the slow variant that found the
    // best Explorer routes -- it runs within the time budget or not at all.
    let buckets: &[f32] = if req.thorough { &[0.5, 0.75, 0.0] } else { &[0.5, 0.75] };
    let variants: Vec<(f32, f32, f32)> = base.iter().flat_map(|&(w, r)| buckets.iter().map(move |&b| (w, r, b))).collect();
    // The budget: past the deadline every unfinished variant stops (its
    // work is not the answer), and Stop from the user means the same --
    // whatever has finished is what we have.
    let deadline = (req.time_budget_ms > 0).then(|| started + std::time::Duration::from_millis(req.time_budget_ms));
    let user_cancelled = ctl.cancelled;
    let cancelled = move || user_cancelled() || deadline.is_some_and(|d| std::time::Instant::now() > d);
    let ctl = &Control { cancelled: &cancelled, progress: ctl.progress, stage: ctl.stage, found: ctl.found, legs: ctl.legs, variant: ctl.variant, trace: ctl.trace };
    let debug = std::env::var_os("ED_PLOT_DEBUG").is_some();
    // Item 16: per-plot goal fields -- jumps-to-goal over the cell graph,
    // desert crossings priced at plain jumps (the number euclid and even
    // ALT's boosted conversion can never know). One Dijkstra per end,
    // built once here and shared read-only by every variant. Gated on
    // the same non-uniformity precheck that arms ALT: a healthy corridor
    // pays nothing, and the pathological ones (Colonia -> Spase stalled
    // its coarse at 19,700 ly remaining against the 30 k cap while a
    // 4 kly highway arc sat unexplored) are exactly where the field's
    // guidance sends the frontier up the arc first. fields[s] prices
    // travel TOWARD ends[s]. ED_NO_FIELD=1 removes them for A/B.
    let field_range = match req.fuel {
        Some(m) => m.reach(m.capacity, 1.0),
        None => req.range_ly.max(1.0),
    };
    let ends = [g.pos_of(req.from), g.pos_of(req.to)];
    let end_cells = [neutrons.cell_index_of_pos(ends[0]), neutrons.cell_index_of_pos(ends[1])];
    let fields: [Option<crate::cgraph::GoalField>; 2] = {
        let alt = if std::env::var_os("ED_NO_ALT").is_some() { None } else { neutrons.alt() };
        let nonuniform = alt.is_some_and(|a| {
            (0..2).any(|side| {
                end_cells[side].is_some_and(|gc| {
                    alt_line_is_nonuniform(neutrons, a, ends[0], ends[1], ends[side], gc, field_range)
                })
            })
        });
        match neutrons.cell_graph() {
            Some(cg) if field_wanted(std::env::var_os("ED_FIELD").is_some(), nonuniform) && std::env::var_os("ED_NO_FIELD").is_none() => {
                let t = std::time::Instant::now();
                // Chain-viability floor: the field only routes through
                // cells holding enough highway stars to actually chain.
                // Lone-star cells LOOK traversable at cell level (the
                // axis-desert trap) while no ship can ride them. The
                // floor shipped at 4; the knob search (runs/2026-09-02,
                // gated on the full matrix) settled it at 2 -- with the
                // 18d honest edge pricing the extra exclusions were
                // creating the very coverage-edge wander they once
                // prevented (Spase -> Colonia E 108 -> 94 j, e86 return
                // 105 -> 87 j), and floor 2 still bars the lone-star
                // trap cells.
                let min_stars: u32 = std::env::var("ED_FIELD_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(2);
                let keep = |c: usize| {
                    end_cells.contains(&Some(c))
                        || neutrons.cell_records(c).is_some_and(|(_, n)| n >= min_stars)
                };
                let f = std::array::from_fn(|s| {
                    end_cells[s]
                        .or_else(|| crate::cgraph::nearest_cell(neutrons, ends[s], end_snap_ly(field_range)))
                        .map(|c| cg.goal_field_where(c, keep))
                });
                if debug {
                    eprintln!("    goal fields built in {} ms (chain floor {min_stars} stars/cell)", t.elapsed().as_millis());
                }
                f
            }
            _ => [None, None],
        }
    };
    let fields = [fields[0].as_ref(), fields[1].as_ref()];
    // 19b: the ship-scaled, trust-gated field bound -- the floor of what
    // any dig on this corridor could achieve. Shared by the prize-aware
    // wave continuation below; None (off-reference ship or no field)
    // means the old always-dig behaviour.
    let ship_bound: Option<f32> = {
        let boost = if req.supercharge { req.boost.neutron } else { 1.0 };
        let reach = req.fuel.map(|m| m.reach(m.capacity, 1.0)).unwrap_or(req.range_ly.max(1.0)) * boost;
        let ratio = crate::cgraph::REF_BOOSTED_REACH_LY / reach.max(1.0);
        let trust: f32 = std::env::var("ED_FLOOR_TRUST_RATIO").ok().and_then(|v| v.parse().ok()).unwrap_or(1.3);
        (ratio <= trust)
            .then(|| fields[1].zip(end_cells[0]).and_then(|(f, c)| f.jumps_to_goal(c)))
            .flatten()
            .map(|b| b * ratio.max(1.0))
    };
    let mut best: Option<Route> = None;
    let mut any_cancelled = false;
    // A variant that hit `max_expansions` (or a leg search's own cap) is
    // over budget, not proof of a gap: without this the caller would be
    // told "no route possible".
    let mut any_budget = false;
    // Waves: the first with the normal coarse allowance, then -- while the
    // budget has room and the last wave improved something -- the same
    // variants with the coarse search allowed four times as many
    // expansions before it settles for the closest neutron reached. On a
    // route that runs out of highway (Beagle Point) the first wave stops
    // at the allowance thousands of ly short; the later waves are what
    // the time budget is for.
    let mut cap_mult: u64 = 1;
    // Legs refined once per plot, shared by every variant and every wave.
    let legs = LegMemo::for_plot();
    // Coarse edges a leg could not fly, banned for the plot (R3 Task 3).
    let banned: std::sync::Mutex<Vec<(u32, u32)>> = std::sync::Mutex::new(Vec::new());
    // The fewest jumps any variant has finished with, this plot (R3 Task 4):
    // every coarse search prunes chains that cannot beat it.
    let incumbent = std::sync::atomic::AtomicU32::new(u32::MAX);
    let grace = (req.grace_ms > 0).then(|| std::time::Duration::from_millis(req.grace_ms));
    let slice_knob: Option<u64> = std::env::var("ED_SLICE_MS").ok().and_then(|v| v.parse().ok());
    let (mut variants_run, mut variants_finished) = (0u32, 0u32);
    loop {
        let wave_started = std::time::Instant::now();
        // Set by a variant whose coarse search settled on the allowance
        // rather than reaching the goal: only then can more allowance help.
        let stalled = std::sync::atomic::AtomicBool::new(false);
        // Within a wave the first route starts the grace clock: the slow
        // variants (no thinning, low weight) are cancelled once the fast
        // ones have answered, and their Cancelled is not a reason to stop
        // the waves. A wave that follows starts its own clock.
        // One extra slot beyond the (weight, reserve, bucket) grid: the
        // bidirectional variant (item 12). ED_NO_BIDI=1 removes it for
        // A/B benching.
        let bidi = std::env::var_os("ED_NO_BIDI").is_none();
        // The credibility ceiling for bidi arming the grace clock (19a).
        // The euclid form (straight/(reach*boost) * 1.4 + 2) is blind to
        // deserts, so which ship "arms" on a desert corridor was a
        // coin-flip of where its floor happened to land: stock E's 94 j
        // passed ITS ceiling and guillotined at 1.4 s while E-86's 99 j
        // missed ITS ceiling, dug, and found 87 j -- the correct outcome
        // by pilot time. Where the goal field covers the start cell and
        // the ship is close enough to the reference for the bound to be
        // trustworthy (reach ratio under ED_FLOOR_TRUST_RATIO), the
        // ceiling comes from the FIELD's bound with a TIGHT slack:
        // arming then means "actually near this corridor's optimum".
        // All four constants are search-harness parameters.
        let boost = if req.supercharge { req.boost.neutron } else { 1.0 };
        let reach = req.fuel.map(|m| m.reach(m.capacity, 1.0)).unwrap_or(req.range_ly.max(1.0)) * boost;
        let jump_floor = dist(g.pos_of(req.from), g.pos_of(req.to)) / reach;
        let arm_ceiling = {
            let knob = |var: &str, default: f32| std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
            // Slack shipped at 1.4; the v3 study (all ten top trials
            // <= 1.28, matrix-gated) settled 1.25: the looser ceiling
            // let bidi's quick 203 j Beagle answer arm the guillotine
            // over a reproducible 193 j / 42 stop dig.
            let euclid = jump_floor * knob("ED_FLOOR_SLACK", 1.25) + knob("ED_FLOOR_PAD", 2.0);
            let ratio = crate::cgraph::REF_BOOSTED_REACH_LY / reach.max(1.0);
            let field_bound = fields[1]
                .zip(end_cells[0])
                .filter(|_| ratio <= knob("ED_FLOOR_TRUST_RATIO", 1.3))
                .and_then(|(f, c)| f.jumps_to_goal(c));
            match field_bound {
                // Field slack shipped at 1.10; the v4 study (seconds
                // objective, matrix-gated) settled 1.04 -- the looser
                // ceiling let a 93 j Spase-return answer arm the
                // guillotine over a reproducible 90 j dig.
                Some(b) => (b * ratio.max(1.0) * knob("ED_FIELD_FLOOR_SLACK", 1.04) + knob("ED_FLOOR_PAD", 2.0)).max(jump_floor),
                None => euclid,
            }
        };
        // ED_SINGLE_CEILING=1 (arm, 2026-10-18): the base single arms only
        // under the ceiling like the bidi.
        let single_ceiling = std::env::var_os("ED_SINGLE_CEILING").is_some();
        // Two more slots (item 13): greedy-cone twins of the base
        // single and the bidi. In dense space they finish in a fraction
        // of the baseline's time and sometimes find BETTER routes (the
        // cone found Wongi->Beagle at 215 jumps against a 226 pin); in
        // sparse space the cone never engages and they duplicate the
        // baseline at its price. They NEVER arm the grace clock: a
        // greedy answer must never cancel the full search whose quality
        // the tie-break protects (forcing the cone on every variant
        // measured exactly that failure: Sol->Sag A* at 70 jumps /
        // 14 stops because greedy bidi's quick answer cancelled the
        // 69-jump singles). ED_NO_GREEDY=1 removes both for A/B.
        let greedy_slots = if std::env::var_os("ED_NO_GREEDY").is_none() { 1 + usize::from(bidi) } else { 0 };
        // The retrograde variant (descent skeleton, reversed, refined
        // forward) is OPT-IN (ED_RETRO=1) after measuring NEGATIVE on
        // both its named criteria: full waypoint pinning re-flew the
        // Mandalay's 313 j descent at 365 j / 127 stops (downhill scoop
        // positions poison the forward fuel rounds), and a loose
        // stride-3 skeleton could not be fueled at all for the 32 t
        // tank (NoRoute). The descent advantage lives in
        // fuel-direction-specific micro-structure, not portable
        // geometry -- the original instinct to park this was right.
        let retro = std::env::var_os("ED_RETRO").is_some();
        // The thin family leads (see THIN_FANOUT); ED_NO_THIN=1 removes it.
        let thin: usize = if std::env::var_os("ED_NO_THIN").is_some() { 0 } else { 2 };
        // The witness (R3 Task 2, 2026-10-18): the feasibility search's
        // flyable route, refined, runs as the first member of the first
        // wave, so a plot has an answer in seconds and never "no route" on
        // a connected pair; it never arms the grace clock (its route is the
        // floor, not the answer). ED_NO_WITNESS=1 removes it.
        let witness: usize = if std::env::var_os("ED_NO_WITNESS").is_some() { 0 } else { 1 };
        let lead = witness + thin;
        // Item 23 residual: two off-ramp twins -- the base single and a
        // light-thinning single with the goal-side entry seed enabled.
        // No engagement gate beyond the seed's own gradient check: the
        // PORTFOLIO is the gate, route_score picks the winner (the
        // global-bias form reproducibly hurt cells the score would have
        // simply out-voted: E Beagle +6 j, Pheia +3 stops). They arm
        // the grace clock only under the credibility ceiling, like
        // bidi. ED_OFFRAMP=0 removes both for A/B.
        let offramp_slots = if std::env::var("ED_OFFRAMP").map(|v| v == "0").unwrap_or(false) { 0 } else { 2 };
        // Rung 3, PARKED OPT-IN like retro (ED_CORRIDOR=1) after a
        // negative verdict (2026-09-02 eve): the segmented corridor
        // variant is fast (22 ms-3 s) and quality-competitive on the
        // Mandalay rim (335/127 vs the off-ramp twin's eventual 329)
        // but won nothing outright, and its arming anchor (same-ship
        // descent jumps x slack) is UNSOUND -- valid only when descent
        // is the corridor's truth, and the E ascent BEATS its descent
        // (193 < 196), so the anchor armed a 196/40 answer at 1 s and
        // guillotined the dig that finds 193/42: -66 s pilot for
        // -16 s wall, backwards under the seconds objective. The
        // deeper reframe: post-item-27, the remaining rim-ascent walls
        // are rationally priced digging (wave 2 buys 5-16 jumps for
        // 10-14 s), not waste to engineer away.
        let corridor_slot = std::env::var_os("ED_CORRIDOR").is_some();
        let corridor_anchor = std::sync::atomic::AtomicU32::new(0);
        // Item 25c: the min-fuel scan gate, decomposed and then raced.
        // The two scan modes win DIFFERENT cells (lean-scan finds the
        // Wongi -> Colonia 58/2 shape; eager-scan finds CS-back M's
        // 166 j and the 10-stop Beagle ascent), so in min-fuel mode the
        // portfolio runs BOTH: the full grid in the base mode plus a
        // compact twin set in the other, and the pruned-truth judge --
        // which scores every candidate on what the pilot will fly --
        // picks the winner. Route_score is the gate; same lesson as the
        // off-ramp twins. ED_MINFUEL_SCAN overrides the base mode for
        // the harness; ED_NO_SCANTWIN=1 removes the twins for A/B.
        let lean_base = std::env::var("ED_MINFUEL_SCAN")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .map(|v| v != 0)
            .unwrap_or(req.min_fuel);
        let scan_twins: usize = if req.min_fuel && std::env::var_os("ED_NO_SCANTWIN").is_none() { 6 } else { 0 };
        // The eager-scan wins concentrate at the HIGHEST weight
        // (eager scan + w1.8 builds the straightest chains with casual
        // top-ups, which the pruned-truth judge then strips), but the
        // winning shape moves between THINNING BUCKETS -- the Beagle
        // ascent's 10-prunable route lives at b0.75 while its b0.5
        // sibling only prunes to 15. So the twins are the full w1.8
        // bucket row plus two mid-weight guards (CS-back M's 166 j
        // came from the mids).
        let twin_cfg: [(f32, f32, f32); 5] = [
            (1.8, 0.75, 0.5),
            (1.8, 0.75, 0.75),
            (1.8, 0.75, 0.0),
            (COARSE_WEIGHT, RESERVE_FRACTION, 0.5),
            (1.3, 0.4, 0.5),
        ];
        let total = lead + variants.len() + usize::from(bidi) + greedy_slots + offramp_slots + scan_twins + usize::from(corridor_slot) + usize::from(retro);
        // Past the first wave the clock is seeded with the standing best:
        // escalation waves exist only to improve, so re-confirmations do
        // not arm the guillotine over the deep variants (14a).
        let scoop_rate = req.fuel.map(|m| m.scoop_rate).unwrap_or(0.0);
        // Item 25: the min-fuel judging seam. In min-fuel mode the judge
        // scores each candidate on its PRUNED clone -- the stops
        // minimize_refuels proves the remaining legs never need --
        // instead of the eager stop count the rewrite will erase.
        // Measured: the eager judge outvoted a 58 j / 2-stop lean truth
        // on Wongi -> Colonia because its eager form wears 7 stops
        // (the winner it kept pruned only to 58/4). Judge what the
        // user will FLY: default mode keeps the eager judge (the
        // fitted seconds model was journal-fit on pilots who take
        // their marked stops), min-fuel judges lean.
        // ED_PRUNE_JUDGE=1/0 overrides for the harness.
        let prune_judge = std::env::var("ED_PRUNE_JUDGE")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .map(|v| v != 0)
            .unwrap_or(req.min_fuel);
        let judge = |r: &Route| -> u64 {
            if prune_judge {
                if let Some(m) = &req.fuel {
                    let mut sim = r.clone();
                    if crate::router::minimize_refuels(m, &req.boost, req.injection.map(|(mult, _, _)| mult), &mut sim, req.start_fuel) {
                        return route_score(&sim, scoop_rate, req);
                    }
                }
            }
            route_score(r, scoop_rate, req)
        };
        let seed = (cap_mult > 1).then(|| best.as_ref().map(&judge)).flatten();
        let order = if declared_order_on(std::env::var("ED_VARIANT_ORDER").ok().as_deref()) {
            variant_order(total, witness, thin, variants.len(), bidi, retro, greedy_slots, offramp_slots, scan_twins, corridor_slot)
        } else {
            Vec::new()
        };
        let floor = quality_floor(req.time_budget_ms, cap_mult == 1, std::env::var("ED_QUALITY_FLOOR_MS").ok().and_then(|v| v.parse().ok()));
        let (results, finished, launched) = run_variants(total, grace, floor, seed, &cancelled, |r: &Route| judge(r), |i, check| {
            let variant_started = std::time::Instant::now();
            let coarse = CoarseStats::default();
            let ctl = Control { cancelled: check, progress: ctl.progress, stage: ctl.stage, found: ctl.found, legs: &legs, variant: ctl.variant, trace: ctl.trace };
            let nb = variants.len();
            let greedy_single = nb + usize::from(bidi);
            // (kind: 0 = baseline single, 1 = bidi, 2 = greedy twin)
            let thin_deadline = variant_started + std::time::Duration::from_millis(thin_budget_ms(req.time_budget_ms));
            // The refine stage lifts the cap for both thin variants (the bidi
            // has no CoarseStats to set coarse_done; review of PR #237), and
            // the thin single's fail-fast stall is its own, not the wave's
            // escalation flag.
            let thin_refining = std::sync::atomic::AtomicBool::new(false);
            // Set when this variant's route came from a LazySP re-run.
            let repaired = std::sync::atomic::AtomicBool::new(false);
            let thin_stage = |name: &'static str, a: u32, b: u32| {
                if name == "refine" {
                    thin_refining.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                (ctl.stage)(name, a, b)
            };
            let thin_stalled = std::sync::atomic::AtomicBool::new(false);
            let thin_check = || check() || thin_yields(std::time::Instant::now() > thin_deadline, coarse.coarse_done.load(std::sync::atomic::Ordering::Relaxed) || thin_refining.load(std::sync::atomic::Ordering::Relaxed));
            let (res, kind) = if witness == 1 && i == 0 {
                // The witness (kind 9): wave 1 only; later waves exist to
                // improve on what wave 1 found.
                if cap_mult > 1 {
                    (Err(RouteError::Cancelled), 9)
                } else {
                    (plan_witness(g, neutrons, req, &ctl, started), 9)
                }
            } else if thin > 0 && i < lead {
                // The thin family (kind 7): the quick diggers, first on
                // every thread count, yielding their worker at a third of
                // the budget. See THIN_FANOUT and thin_budget_ms.
                let ctl = Control { cancelled: &thin_check, progress: ctl.progress, stage: &thin_stage, found: ctl.found, legs: ctl.legs, variant: ctl.variant, trace: ctl.trace };
                if i == witness {
                    (with_repair(&banned, &repaired, || plan_long_with(g, neutrons, req, &ctl, THIN_WEIGHT, THIN_RESERVE, THIN_BUCKET, cap_mult, false, false, lean_base, None, &thin_stalled, &coarse, fields[1], true, &banned, &incumbent)), 7)
                } else {
                    (plan_long_bidi(g, neutrons, req, &ctl, THIN_WEIGHT, cap_mult, false, lean_base, fields), 8)
                }
            } else {
            let i = i - lead;
            // The heavy variants' slice: past it with the chain not at the
            // goal (and not yet refining), yield the worker. See heavy_slice_ms.
            let slice_deadline = heavy_slice_ms(req.time_budget_ms, cap_mult == 1, slice_knob).map(|ms| variant_started + std::time::Duration::from_millis(ms));
            // A bidi slot has no CoarseStats: the bidi (nb), the greedy bidi, the sixth scan twin.
            let bidi_slot = (bidi && i == nb) || (greedy_slots > 1 && i == greedy_single + 1) || (scan_twins > 5 && i == greedy_single + greedy_slots + offramp_slots + 5);
            let heavy_check = || check() || slice_deadline.is_some_and(|d| heavy_yields(std::time::Instant::now() > d, !bidi_slot, coarse.coarse_done.load(std::sync::atomic::Ordering::Relaxed), thin_refining.load(std::sync::atomic::Ordering::Relaxed)));
            let ctl = Control { cancelled: &heavy_check, progress: ctl.progress, stage: &thin_stage, found: ctl.found, legs: ctl.legs, variant: ctl.variant, trace: ctl.trace };
            if i < nb {
                // ED_ONLY_BIDI: debug rig -- silence the singles so the
                // bidi variant's behaviour is readable in the trace.
                if std::env::var_os("ED_ONLY_BIDI").is_some() {
                    return (Err(RouteError::Cancelled), false);
                }
                let (w, r, b) = variants[i];
                (with_repair(&banned, &repaired, || plan_long_with(g, neutrons, req, &ctl, w, r, b, cap_mult, false, false, lean_base, None, &stalled, &coarse, fields[1], false, &banned, &incumbent)), 0)
            } else if bidi && i == nb {
                (plan_long_bidi(g, neutrons, req, &ctl, COARSE_WEIGHT, cap_mult, false, lean_base, fields), 1)
            } else if greedy_slots > 0 && i == greedy_single {
                (with_repair(&banned, &repaired, || plan_long_with(g, neutrons, req, &ctl, COARSE_WEIGHT, RESERVE_FRACTION, 0.75, cap_mult, true, false, lean_base, None, &stalled, &coarse, fields[1], false, &banned, &incumbent)), 2)
            } else if greedy_slots > 1 && i == greedy_single + 1 {
                (plan_long_bidi(g, neutrons, req, &ctl, COARSE_WEIGHT, cap_mult, true, lean_base, fields), 2)
            } else if offramp_slots > 0 && i == greedy_single + greedy_slots {
                (with_repair(&banned, &repaired, || plan_long_with(g, neutrons, req, &ctl, COARSE_WEIGHT, RESERVE_FRACTION, 0.75, cap_mult, false, true, lean_base, None, &stalled, &coarse, fields[1], false, &banned, &incumbent)), 4)
            } else if offramp_slots > 1 && i == greedy_single + greedy_slots + 1 {
                (with_repair(&banned, &repaired, || plan_long_with(g, neutrons, req, &ctl, 1.3, 0.4, 0.5, cap_mult, false, true, lean_base, None, &stalled, &coarse, fields[1], false, &banned, &incumbent)), 4)
            } else if scan_twins > 0 && i >= greedy_single + greedy_slots + offramp_slots && i < greedy_single + greedy_slots + offramp_slots + scan_twins {
                let s = i - greedy_single - greedy_slots - offramp_slots;
                if s < 5 {
                    let (w, r, b) = twin_cfg[s];
                    (with_repair(&banned, &repaired, || plan_long_with(g, neutrons, req, &ctl, w, r, b, cap_mult, false, false, !lean_base, None, &stalled, &coarse, fields[1], false, &banned, &incumbent)), 6)
                } else {
                    (plan_long_bidi(g, neutrons, req, &ctl, COARSE_WEIGHT, cap_mult, false, !lean_base, fields), 6)
                }
            } else if corridor_slot && i == greedy_single + greedy_slots + offramp_slots + scan_twins {
                (plan_long_corridor(g, neutrons, req, &ctl, cap_mult, fields, &corridor_anchor), 5)
            } else {
                // The last slot: retrograde (kind 3). Its internal
                // descending plan aims at the original START, so it
                // consults fields[0].
                (plan_long_retro(g, neutrons, req, &ctl, cap_mult, fields[0]), 3)
            }
            };
            let arms = match (&res, kind) {
                // Scan twins (kind 6) arm ceiling-gated like bidi. Both
                // arming policies were benched on the knife-edge cells:
                // disarmed twins let the lean base arm instead and
                // guillotine the eager twin mid-flight (CS-back M loses
                // its 166 j, -254 s), while armed twins occasionally
                // guillotine the flagship's slow 58/2-finder (-39 s,
                // ~half the rolls). Seconds-weighted, armed wins; the
                // flagship flicker is the documented cost.
                // The thin bidi (kind 8) arms like the regular bidi: on
                // three threads the regular bidi starts late and the singles
                // stall before anything arms, so the x4 wave fired on routes
                // that took a second before (Wongi -> Blaa Hypai 1 -> 91 s,
                // arm thin2-3, 2026-10-17: "next wave x4 after 8048 ms").
                (Ok(route), 1) | (Ok(route), 4) | (Ok(route), 6) | (Ok(route), 8) => (route.jumps as f32) <= arm_ceiling,
                // The thin single (kind 7) NEVER arms: its weight-3 answer
                // under the ceiling guillotined the dig behind it (arm thin-3:
                // Colonia -> Sol 55 -> 56 j, Spase -> Colonia 87 -> 88 j);
                // like the greedy cone it wins where nothing else answers.
                (Ok(_), 7) | (Ok(_), 9) => false,
                // The corridor variant arms against its own descent
                // anchor OR the general ceiling, whichever admits: a
                // forward route within slack of the same-ship descent
                // is actually near this corridor's optimum, however
                // loose the rim's euclid/field bounds are.
                (Ok(route), 5) => {
                    let anchor = corridor_anchor.load(std::sync::atomic::Ordering::Relaxed);
                    (route.jumps as f32) <= arm_ceiling || (anchor > 0 && route.jumps as u32 <= anchor)
                }
                (Ok(route), 0) => single_arms(repaired.load(std::sync::atomic::Ordering::Relaxed), route.jumps as f32, arm_ceiling, single_ceiling),
                _ => false,
            };
            if let Ok(route) = &res {
                // ED_INCUMBENT=1 (arm): the shared incumbent prunes every coarse
                // search (R3 Task 4). Off by default: measured a wash on three
                // threads (w7n-3 without it 5 / 15 / 8 vs thin6-3, w6-3 with it
                // 5 / 13 / 10, the difference two knife-edge plots moving in
                // opposite directions) and the pre-registered wall gains on
                // the Jongou pairs did not appear; the pruning risk (a coarse
                // hop count is an estimate) buys nothing measured.
                if std::env::var_os("ED_INCUMBENT").is_some() {
                    incumbent.fetch_min(route.jumps as u32, std::sync::atomic::Ordering::Relaxed);
                }
                (ctl.found)(route);
            }
            let (coarse_expansions, stall_remaining_ly) = (coarse.expansions.load(std::sync::atomic::Ordering::Relaxed), coarse.stall_ly());
            if debug {
                eprintln!("    variant outcome: wave x{cap_mult} #{i} {} {} {} ms, coarse {coarse_expansions} expansions, stall {stall_remaining_ly:.0} ly", variant_kind(i, witness, thin, variants.len(), bidi, retro, greedy_slots, offramp_slots, scan_twins, corridor_slot, total), if res.is_ok() { "route" } else { "none" }, variant_started.elapsed().as_millis());
            }
            (ctl.variant)(VariantOutcome {
                wave: cap_mult,
                kind: variant_kind(i, witness, thin, variants.len(), bidi, retro, greedy_slots, offramp_slots, scan_twins, corridor_slot, total),
                index: i,
                ok: res.is_ok(),
                jumps: res.as_ref().map(|r| r.jumps as u32).unwrap_or(0),
                stops: res.as_ref().map(|r| r.refuel_stops as u32).unwrap_or(0),
                ms: variant_started.elapsed().as_millis() as u64,
                coarse_expansions,
                stall_remaining_ly,
            });
            (res, arms)
        }, &order);
        variants_run += launched;
        variants_finished += finished;
        // A variant the grace rule cancelled is not the user's Stop.
        any_cancelled |= finished == 0 && results.iter().any(|r| matches!(r, Err(RouteError::Cancelled)));
        any_budget |= results.iter().any(|r| matches!(r, Err(RouteError::Budget)));
        if debug {
            for (i, r) in results.iter().enumerate() {
                let label = if lead > 0 && i < lead {
                    lead_kind(i, witness).to_string()
                } else {
                let i = i - lead;
                if i < variants.len() {
                    let (w, rf, b) = variants[i];
                    format!("single w{w} r{rf} b{b}")
                } else if bidi && i == variants.len() {
                    "bidi".to_string()
                } else if retro && i == total - lead - 1 {
                    "retro".to_string()
                } else if i == variants.len() + usize::from(bidi) {
                    "greedy single".to_string()
                } else if greedy_slots > 1 && i == variants.len() + usize::from(bidi) + 1 {
                    "greedy bidi".to_string()
                } else if scan_twins > 0 && i >= variants.len() + usize::from(bidi) + greedy_slots + offramp_slots && i < variants.len() + usize::from(bidi) + greedy_slots + offramp_slots + scan_twins {
                    format!("scan-twin {}", i - variants.len() - usize::from(bidi) - greedy_slots - offramp_slots)
                } else if corridor_slot && i == variants.len() + usize::from(bidi) + greedy_slots + offramp_slots + scan_twins {
                    "corridor".to_string()
                } else {
                    format!("offramp {}", i - variants.len() - usize::from(bidi) - greedy_slots)
                }
                };
                match r {
                    Ok(r) => eprintln!("    wave x{cap_mult} {label}: {} jumps, {} stops, {} boosted, {} ms", r.jumps, r.refuel_stops, r.boosted_jumps, r.elapsed_ms),
                    Err(e) => eprintln!("    wave x{cap_mult} {label}: {e:?}"),
                }
            }
        }
        let before = best.as_ref().map(&judge);
        for r in results.into_iter().flatten() {
            if debug {
                eprintln!("    fold: {} j / {} stops judged {}", r.jumps, r.refuel_stops, judge(&r));
            }
            if best.as_ref().is_none_or(|b| judge(&r) < judge(b)) {
                best = Some(r);
            }
        }
        let improved = best.as_ref().map(judge) != before;
        let _ = improved;
        let wave_took = wave_started.elapsed();
        // Another wave only inside a budget, with room for a wave that will
        // take longer than this one, and while a coarse still stalls on its
        // allowance. A non-improving wave no longer breaks the loop (14a):
        // with the seeded clock it runs to its natural end instead of being
        // guillotined, and the escalations are what the budget is for --
        // Wongi -> Beagle's 190-jump route lives at the x16 allowance, and
        // the old rule only ever reached it by scheduling luck.
        // The estimate must match the escalation: cap_mult quadruples and
        // stall-bound topology (the rim) scales wave time with the cap --
        // measured Wongi -> Beagle E 3.4 s -> 13.7 s (x4.0), M 2.6 s ->
        // 9.5 s (x3.7). The old x3 launched a wave 3 that needed >43 s
        // into 43 s of budget: doomed on arrival, guillotined at the cap,
        // 43 wasted seconds on every 60 s rim ascent (and the ledgered
        // Oevasy budget burn). x4 stops after the wave that actually
        // delivers. ED_WAVE_COST for the harness.
        // Deeper caps are MORE stall-bound, so the scaling worsens with
        // each escalation: measured x4.0 then unfinishable (E), x3.7
        // then >=5.1 (M, whose x16 wave mostly finished and improved
        // nothing -- the rim bound is too loose for the prize check to
        // stop a small ship). First escalation prices at the base so
        // mid-budget plots keep their wave 2; deeper ones at 1.5x it.
        let wave_cost: f32 = std::env::var("ED_WAVE_COST").ok().and_then(|v| v.parse().ok()).unwrap_or(4.0);
        let wave_cost = if cap_mult >= 4 { wave_cost * 1.5 } else { wave_cost };
        let room = deadline.is_some_and(|d| {
            d.saturating_duration_since(std::time::Instant::now()) > wave_took.mul_f32(wave_cost)
        });
        // 19b: prize-aware continuation -- keep escalating only while the
        // remaining treasure can pay for the excavation. The field bound
        // is the floor of what any dig could reach; the prize is the gap
        // to it priced in pilot seconds (40 s/jump), and the next wave
        // costs at least what this one did. The e86 Spase return dug
        // 99 -> 87 j on a 480 s prize for ~25 s of waves (19:1); a 2-jump
        // gap must not buy the same excavation. ED_PRIZE_K scales the
        // break-even bar (search-harness parameter).
        let prize_left = match (&best, ship_bound) {
            (Some(b), Some(bound)) => (b.jumps as f32 - bound).max(0.0) * 40.0,
            _ => f32::INFINITY,
        };
        let prize_k: f32 = req.prize_k.or_else(|| std::env::var("ED_PRIZE_K").ok().and_then(|v| v.parse().ok())).unwrap_or(1.0);
        // On by default (arm w7-3 vs w6-3: 2 better / 26 same / 0 worse,
        // Wongi -> Blaa Hypai 99.9 -> 6.7 s); ED_NO_BOUND_STOP=1 ablates.
        let bound_stop = std::env::var_os("ED_NO_BOUND_STOP").is_none();
        if !room || !stalled.load(std::sync::atomic::Ordering::Relaxed) || cancelled() || cap_mult >= 64 || prize_left < wave_took.as_secs_f32() * prize_k || wave_stops_at_bound(best.as_ref().map(|b| b.jumps), arm_ceiling, bound_stop) {
            break;
        }
        cap_mult *= 4;
        if debug {
            eprintln!("    next wave x{cap_mult} after {} ms (best {:?})", started.elapsed().as_millis(), best.as_ref().map(|b| b.jumps));
        }
    }
    // The cell-graph fallback: only when the whole portfolio came up
    // empty. Benched as a SEED it was a net negative -- 0.2-11 s added
    // to every plot for chains that refine into worse routes (Colonia ->
    // Spase: 301 jumps at 49 boosted) -- but on plots where the
    // portfolio returns NOTHING (real highway gaps beyond the widen
    // radius), a rough route beats an error. Costs nothing when a route
    // exists or the sidecar is absent.
    if best.is_none() {
        if let Some(Ok(route)) = plan_graph_first(g, neutrons, req, ctl) {
            if debug {
                eprintln!("    graph-first fallback: {} jumps, {} stops, {} ms", route.jumps, route.refuel_stops, route.elapsed_ms);
            }
            best = Some(route);
        }
    }
    if best.is_none() && (any_cancelled || any_budget) {
        return Err(if user_cancelled() { RouteError::Cancelled } else { RouteError::Budget });
    }
    let mut best = best.ok_or(RouteError::NoRoute)?;
    best.elapsed_ms = started.elapsed().as_millis() as u64;
    best.variants_run = variants_run;
    best.variants_finished = variants_finished;
    Ok(best)
}

/// Does any highway star lie within `radius` of the segment `a` -> `b`?
/// Walks only the sub-index cells the corridor's bounding box touches (a
/// few dozen at 250 ly cells for a 1,000 ly plot), so a plot with no
/// highway nearby pays a handful of binary searches and nothing else.
/// With `white_dwarfs` false only neutrons count: a plot that may not
/// boost off a white dwarf must not spin up the neutron-first variant for
/// a shortcut it is not allowed to take.
pub fn neutron_in_corridor(neutrons: &Galaxy, a: [f32; 3], b: [f32; 3], radius: f32, white_dwarfs: bool) -> bool {
    let cell_ly = neutrons.cell_ly;
    let lo = crate::format::cell_of_with([a[0].min(b[0]) - radius, a[1].min(b[1]) - radius, a[2].min(b[2]) - radius], cell_ly);
    let hi = crate::format::cell_of_with([a[0].max(b[0]) + radius, a[1].max(b[1]) + radius, a[2].max(b[2]) + radius], cell_ly);
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let r2 = radius * radius;
    // Squared distance from `p` to the segment.
    let seg_d2 = |p: [f32; 3]| -> f32 {
        let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
        let t = if len2 > 0.0 { ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / len2).clamp(0.0, 1.0) } else { 0.0 };
        let q = [ap[0] - t * ab[0], ap[1] - t * ab[1], ap[2] - t * ab[2]];
        q[0] * q[0] + q[1] * q[1] + q[2] * q[2]
    };
    // A morton-ordered walk on a v3 sub-index skips the corridor box's
    // empty cells in one jump each; pre-v3 falls back to per-cell probes.
    let mut found = false;
    neutrons.for_each_cell_in_box(lo, hi, |_cx, _cy, _cz, start, count| {
        for i in start..start + count {
            if !white_dwarfs && neutrons.class(&neutrons.record(i)) == crate::StarClass::WhiteDwarf {
                continue;
            }
            if seg_d2(neutrons.pos_of(i)) <= r2 {
                found = true;
                return std::ops::ControlFlow::Break(());
            }
        }
        std::ops::ControlFlow::Continue(())
    });
    found
}

/// Whether a plot needs the neutron-first planner alongside the exact one:
/// under the long-route threshold, and a neutron within one supercharged
/// jump of the straight line between the ends (the corridor is `range x
/// neutron boost` wide, the most a shortcut can reach off the line).
fn wants_neutron_variant(g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest) -> bool {
    if !req.supercharge || neutrons.count == 0 {
        return false;
    }
    let range = match req.fuel {
        Some(m) => m.reach(m.capacity, 1.0),
        None => req.range_ly.max(1.0),
    };
    neutron_in_corridor(neutrons, g.pos_of(req.from), g.pos_of(req.to), range * req.boost.neutron, req.boost.white_dwarf > 1.0)
}

/// The entry point every caller plots through. Over `LONG_ROUTE_LY` the
/// neutron-first planner; under it the exact planner -- and, when a
/// neutron sits in the plot's corridor, the neutron-first planner at the
/// same time, the better route under the usual tie-break (fewer jumps,
/// then fewer boosts, then fewer ly) being the answer. The default
/// weighted exact search can walk right past a neutron shortcut when a
/// plain chain exists (its f falls monotonically along the chain), and a
/// 1,000 ly plot is exactly where a x4 hop or two matters most.
///
/// With no neutron sub-index, or none in the corridor, this is `plan`:
/// same expansions, same time. Cancellation and the time budget apply to
/// both searches; once one has a route the other gets a short grace
/// period (it may still win) and is then dropped.
pub fn plan_best(g: &Galaxy, neutrons: Option<&Galaxy>, req: &RouteRequest, ctl: &Control) -> Result<Route, RouteError> {
    // A request already withdrawn (the server's client went away while
    // the plot waited for a slot) never starts; the in-flight checks
    // below (router.rs expansion loop, the coarse and refine phases)
    // stop one that is withdrawn later.
    if (ctl.cancelled)() {
        return Err(RouteError::Cancelled);
    }
    let mut route = plan_best_inner(g, neutrons, req, ctl)?;
    // Min-fuel is a rewrite over the winning route, not a different
    // search: the planners plan (and prove feasibility) topping up
    // everywhere, then the stops the remaining legs never need are
    // dropped. See [`crate::router::minimize_refuels`].
    if req.min_fuel {
        if let Some(m) = &req.fuel {
            crate::router::minimize_refuels(m, &req.boost, req.injection.map(|(mult, _, _)| mult), &mut route, req.start_fuel);
        }
        // Item 34 rung 1, DEFAULT ON since the matrix blessed it
        // (2026-09-03: 13 better / 2 worse / 39 same, -2,083 fitted s,
        // wall free; the wins land exactly on cross-dry-then-drink).
        // ED_CROSSING_REPLAN=0 is the opt-out, per the knob convention.
        if std::env::var("ED_CROSSING_REPLAN").ok().as_deref() != Some("0") {
            crossing_replan(g, neutrons, req, ctl, &mut route);
        }
    } else if let Some(m) = &req.fuel {
        // Item 39: an eager plan still labels its comfort top-ups so the
        // HUD can say "fuel available, not needed" instead of "fuel here".
        crate::router::mark_optional_stops(m, &req.boost, req.injection.map(|(mult, _, _)| mult), &mut route, req.start_fuel);
    }
    // Legs record the secondary on the hop; the route-level count is
    // derived here so every path (exact search, legs, min-fuel rewrite)
    // reports the same headline number.
    route.secondary_boosts = route.hops.iter().filter(|h| h.via_secondary.is_some()).count();
    Ok(route)
}

/// Item 34 rung 1: the crossing re-plan pass — cross dry, then drink.
///
/// The v1 min-fuel rewrite minimises stops over a FIXED hop chain, so a
/// desert crossing planned at eager full-tank mass keeps its short wet
/// hops even after the pre-crossing top-up is deleted (measured: the
/// Spase -> Colonia crossing at 8 x 66 ly wet where 5 x 77 flies dry —
/// the ship is 72 ly wet and 77.5 ly at 13 t). This pass finds each
/// plain run (3+ consecutive unboosted, uninjected hops), anchors a
/// segment a few hops either side — wide enough that the re-plan may
/// MOVE the crossing point, not just re-step it (rung 0: hop count is
/// placement-bound, step length is dryness-bound) — and re-plans the
/// segment from the route's actual lean tank at the entry anchor. The
/// spliced candidate is re-flown through the real fuel formula, gets
/// its own stop rewrite, and replaces the route only when the seconds
/// judge strictly improves. Returns whether anything was accepted.
fn crossing_replan(g: &Galaxy, neutrons: Option<&Galaxy>, req: &RouteRequest, ctl: &Control, route: &mut Route) -> bool {
    const MIN_RUN: usize = 3;
    const ANCHOR_SLACK: usize = 4;
    let Some(m) = req.fuel.as_ref() else { return false };
    if route.hops.len() < MIN_RUN + 1 {
        return false;
    }
    let scoop_rate = if m.scoop_rate > 0.0 { m.scoop_rate } else { 1.0 };
    let inj = req.injection.map(|(mult, _, _)| mult);
    // Maximal plain runs (hop i arrived unboosted and uninjected), by
    // arrival index, longest first: the widest desert is the payer.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for i in 1..route.hops.len() {
        let plain = !route.hops[i].boosted && route.hops[i].injection.is_none();
        match (plain, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if i - s >= MIN_RUN {
                    runs.push((s, i - 1));
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        if route.hops.len() - s >= MIN_RUN {
            runs.push((s, route.hops.len() - 1));
        }
    }
    runs.sort_by_key(|(s, e)| std::cmp::Reverse(e - s));
    let mut improved = false;
    for (run_s, run_e) in runs {
        // Anchor a few hops out so the segment planner may relocate the
        // crossing, and re-index against the CURRENT route (an earlier
        // acceptance moved everything).
        if improved {
            break; // one accepted splice per plot: runs were indexed on the old chain.
        }
        let lo = run_s.saturating_sub(1 + ANCHOR_SLACK);
        let hi = (run_e + ANCHOR_SLACK).min(route.hops.len() - 1);
        if hi <= lo + 1 {
            continue;
        }
        let entry_fuel = route.hops[lo].fuel_after.unwrap_or(req.start_fuel);
        let sub = RouteRequest {
            from: route.hops[lo].idx,
            to: route.hops[hi].idx,
            start_fuel: entry_fuel,
            min_fuel: false, // the whole candidate gets its own rewrite below
            time_budget_ms: 3_000.min(if req.time_budget_ms > 0 { req.time_budget_ms } else { u64::MAX }),
            max_expansions: 200_000,
            ..req.clone()
        };
        let Ok(seg) = plan_best_inner(g, neutrons, &sub, ctl) else { continue };
        if seg.hops.len() < 2 || seg.hops.first().map(|h| h.idx) != Some(route.hops[lo].idx) {
            continue;
        }
        // Splice, rebuild the aggregates, re-fly with the real formula
        // from the true departure tank, then re-run the stop rewrite so
        // the candidate is judged in the same lean currency as the base.
        let mut cand = route.clone();
        cand.hops.splice(lo + 1..=hi, seg.hops.into_iter().skip(1));
        cand.jumps = cand.hops.len() - 1;
        cand.boosted_jumps = cand.hops.iter().filter(|h| h.boosted).count();
        cand.injections = cand.hops.iter().filter(|h| h.injection.is_some()).count();
        let mut total = 0.0f32;
        for i in 0..cand.hops.len() {
            total += cand.hops[i].distance_ly;
            cand.hops[i].total_ly = total;
        }
        cand.total_ly = total;
        if !refuel_hops(g, m, req, &mut cand, req.start_fuel) {
            continue;
        }
        crate::router::minimize_refuels(m, &req.boost, inj, &mut cand, req.start_fuel);
        if route_score(&cand, scoop_rate, req) < route_score(route, scoop_rate, req) {
            *route = cand;
            improved = true;
        }
    }
    improved
}

fn plan_best_inner(g: &Galaxy, neutrons: Option<&Galaxy>, req: &RouteRequest, ctl: &Control) -> Result<Route, RouteError> {
    let straight = dist(g.pos_of(req.from), g.pos_of(req.to));
    let Some(neutrons) = neutrons else { return plan(g, req, ctl) };
    if req.supercharge && straight > LONG_ROUTE_LY {
        return plan_long(g, neutrons, req, ctl);
    }
    if !wants_neutron_variant(g, neutrons, req) {
        return plan(g, req, ctl);
    }
    let started = std::time::Instant::now();
    let deadline = (req.time_budget_ms > 0).then(|| started + std::time::Duration::from_millis(req.time_budget_ms));
    let user_cancelled = ctl.cancelled;
    // Set by whichever search finishes with a route first: (when, ms it took).
    let first_done: std::sync::Mutex<Option<(std::time::Instant, u64)>> = std::sync::Mutex::new(None);
    let note_done = |ok: bool| {
        if ok {
            let mut f = first_done.lock().unwrap_or_else(|e| e.into_inner());
            if f.is_none() {
                *f = Some((std::time::Instant::now(), started.elapsed().as_millis() as u64));
            }
        }
    };
    // Once a route exists the other search has three times as long as the
    // winner took (at least a second) to beat it, then it is dropped: a
    // 50 ms exact plot must not wait seconds for a highway variant.
    let past_grace = || {
        let f = *first_done.lock().unwrap_or_else(|e| e.into_inner());
        f.is_some_and(|(at, took)| at.elapsed() > std::time::Duration::from_millis((took * 3).max(1_000)))
    };
    let stop = || user_cancelled() || deadline.is_some_and(|d| std::time::Instant::now() > d) || past_grace();
    let exact_finished = std::sync::atomic::AtomicBool::new(false);
    // Progress: the exact search reports while it runs; the neutron
    // variant's numbers are on another scale (coarse expansions over
    // neutrons) so it takes over only after the exact search is done.
    let quiet_progress = |n: u64, remaining: f32| {
        if exact_finished.load(std::sync::atomic::Ordering::Relaxed) {
            (ctl.progress)(n, remaining);
        }
    };
    let exact_ctl = Control { cancelled: &stop, progress: ctl.progress, stage: ctl.stage, found: ctl.found, legs: ctl.legs, variant: ctl.variant, trace: ctl.trace };
    let neutron_ctl = Control { cancelled: &stop, progress: &quiet_progress, stage: &|_, _, _| {}, found: ctl.found, legs: ctl.legs, variant: ctl.variant, trace: ctl.trace };
    // The exact search gets a thread of its own: the neutron-first planner
    // fans its variants and leg refinements over the rayon pool, and an
    // exact search queued behind them (measured: 44 ms alone, 290 ms
    // through `rayon::join`) would make every short plot pay for the
    // highway check.
    let (exact, highway) = std::thread::scope(|scope| {
        let exact = scope.spawn(|| {
            let r = plan(g, req, &exact_ctl);
            if let Ok(route) = &r {
                (ctl.found)(route);
            }
            note_done(r.is_ok());
            exact_finished.store(true, std::sync::atomic::Ordering::Relaxed);
            r
        });
        let highway = plan_long(g, neutrons, req, &neutron_ctl);
        note_done(highway.is_ok());
        let exact = exact.join().unwrap_or(Err(RouteError::NoRoute));
        (exact, highway)
    });
    if std::env::var_os("ED_PLOT_DEBUG").is_some() {
        let show = |r: &Result<Route, RouteError>| match r {
            Ok(r) => format!("{} jumps, {} boosted, {:.0} ly, {} ms", r.jumps, r.boosted_jumps, r.total_ly, r.elapsed_ms),
            Err(e) => format!("{e:?}"),
        };
        eprintln!("  mid-range: exact {} | neutron-first {}", show(&exact), show(&highway));
    }
    // One judge (R2): the planner's seconds, then jumps, then light-years.
    let rank = |r: &Route| (route_score(r, req.fuel.map(|m| m.scoop_rate).unwrap_or(0.0), req), r.jumps, (r.total_ly * 10.0) as u64);
    let mut best = match (exact, highway) {
        (Ok(a), Ok(b)) => Ok(if rank(&b) < rank(&a) { b } else { a }),
        (Ok(a), Err(_)) | (Err(_), Ok(a)) => Ok(a),
        (Err(a), Err(b)) => Err(if user_cancelled() {
            RouteError::Cancelled
        } else if matches!(a, RouteError::Budget | RouteError::Cancelled) || matches!(b, RouteError::Budget | RouteError::Cancelled) {
            // A deadline stop is a budget, not the user's Stop.
            RouteError::Budget
        } else {
            RouteError::NoRoute
        }),
    };
    if let Ok(r) = best.as_mut() {
        r.elapsed_ms = started.elapsed().as_millis() as u64;
    }
    best
}

/// The cell-graph-first coarse planner (ROUTING-NEXT 5.3): the chain is
/// a shortest path over `graph250.bin` -- which, unlike the neutron A*,
/// HAS edges across highway gaps -- and the shared refine machinery
/// flies it. The far-gap stall was a reachability failure (the search's
/// edges reach ~1,200 ly, real gaps reach 11,000); this planner routes
/// on the graph that can cross them. Runs only when the sidecar exists
/// (the app does not build it yet); `None` = does not apply, and the
/// portfolio proceeds without it. `ED_NO_CGRAPH=1` disables it for A/B.
fn plan_graph_first(g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, ctl: &Control) -> Option<Result<Route, RouteError>> {
    if std::env::var_os("ED_NO_CGRAPH").is_some() {
        return None;
    }
    let started = std::time::Instant::now();
    let graph = neutrons.cell_graph()?;
    let start_pos = g.pos_of(req.from);
    let goal_pos = g.pos_of(req.to);
    let from = crate::cgraph::nearest_cell(neutrons, start_pos, crate::cgraph::R_GAP)?;
    let to = crate::cgraph::nearest_cell(neutrons, goal_pos, crate::cgraph::R_GAP)?;
    let path = graph.shortest_cell_path(neutrons, from, to)?;
    if path.len() > 2_000 {
        return None; // a chain that long would drown the refiner
    }
    if (ctl.cancelled)() {
        return Some(Err(RouteError::Cancelled));
    }
    let fuel_model = req.fuel.map(|m| crate::fuel::FuelModel { reserve: m.reserve.max(m.max_fuel_per_jump * RESERVE_FRACTION), ..m });
    let req = &RouteRequest { fuel: fuel_model, ..req.clone() };
    let start_fuel = match fuel_model {
        Some(m) => req.start_fuel.clamp(0.0, m.capacity),
        None => 0.0,
    };
    let capacity = fuel_model.map(|m| m.capacity).unwrap_or(0.0);
    // Representative highway star per path cell, thinned to every second
    // cell (cells are 250 ly, a boosted hop 400+): nearest record to the
    // cell centre, mapped back to the full index by name.
    let mut waypoints: Vec<(u32, f32)> = vec![(req.from, start_fuel)];
    let cell_ly = neutrons.cell_ly;
    for (k, &cell) in path.iter().enumerate() {
        if k % 2 == 1 && k + 1 != path.len() {
            continue;
        }
        let (key, start, count) = neutrons.cell_entry(cell as usize);
        let (cx, cy, cz) = crate::format::morton_cell_of(key);
        let centre = [(cx as f32 + 0.5) * cell_ly, (cy as f32 + 0.5) * cell_ly, (cz as f32 + 0.5) * cell_ly];
        let mut best: Option<(f32, u32)> = None;
        for r in start..start + count {
            let d = dist(neutrons.pos_of(r), centre);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, r));
            }
        }
        let Some((_, rep)) = best else { continue };
        let Some(full) = g.find(neutrons.name(&neutrons.record(rep))) else { continue };
        if full != req.from && waypoints.last().map(|&(w, _)| w) != Some(full) {
            // The coarse fuel assumption legs are refined under: full
            // tank less a jump; the fuel rounds correct what is off.
            waypoints.push((full, (capacity - fuel_model.map(|m| m.max_fuel_per_jump).unwrap_or(0.0)).max(0.0)));
        }
    }
    if waypoints.last().map(|&(w, _)| w) != Some(req.to) {
        waypoints.push((req.to, 0.0));
    }
    if waypoints.len() < 2 {
        return None;
    }
    let straight = dist(start_pos, goal_pos);
    Some(refine_waypoints(g, req, ctl, waypoints, start_fuel, fuel_model, path.len() as u64, straight, started))
}

/// Once-per-plot topography test for the ALT oracle: sample the
/// bound/euclid ratio toward `target` at eight points along the
/// start->goal line. UNIFORM inflation (the thin rim, Wongi->Beagle,
/// ratios <= 1.15) scales h by a near-constant factor, reorders nothing
/// under weighted A*, and only costs -- measured at +25% wall with
/// IDENTICAL expansions under every consult-side gating scheme tried.
/// NON-uniform inflation -- a desert dominating the line, the void
/// topology where the controlled bench measured ~2x -- is what reorders
/// the frontier, and only that pays for the per-push consult. A long
/// real leg dilutes even a real desert below the threshold (Colonia->
/// Spase peaks at 1.15 over 31 kly, and the A/B there shows identical
/// wall either way), which is the threshold working, not failing.
///
/// A sample point with NO cell is not silence -- it is the loudest
/// signal on the line. A contiguous empty run of >= 2 cells (~500 ly)
/// FLANKED by occupied samples is a wall: something the frontier must
/// steer around, which is precisely where the oracle pays -- and where
/// the ratio test alone is blind, because a wall with a cheap detour
/// inflates surviving shore samples by almost nothing (measured ~1.04
/// on the Mac's synthetic, whose 2x this rule protects; it is also
/// what halves Colonia->Spase, 20.6 s / 265 j vs ~38 s / 300 j
/// without). A LONE empty cell is grid noise the search crosses with a
/// plain jump (treating one as a wall put Wongi->Beagle back at +25%),
/// and a run that reaches the line's end unflanked is the rim fading
/// into thinness -- nothing to steer around, and the uniform-overhead
/// case this precheck exists to kill.
pub(crate) fn alt_line_is_nonuniform(
    neutrons: &Galaxy,
    alt: &crate::alt::AltOracle,
    start_pos: [f32; 3],
    goal_pos: [f32; 3],
    target_pos: [f32; 3],
    target_cell: usize,
    full_range: f32,
) -> bool {
    let line = [goal_pos[0] - start_pos[0], goal_pos[1] - start_pos[1], goal_pos[2] - start_pos[2]];
    let len = (line[0] * line[0] + line[1] * line[1] + line[2] * line[2]).sqrt().max(1.0);
    let step = [line[0] / len * 250.0, line[1] / len * 250.0, line[2] / len * 250.0];
    let (mut lo_r, mut hi_r) = (f32::INFINITY, 0.0f32);
    let (mut seen, mut pending_wall, mut wall) = (false, false, false);
    for k in 1..=8 {
        let t = k as f32 / 9.0;
        let p = [
            start_pos[0] + line[0] * t,
            start_pos[1] + line[1] * t,
            start_pos[2] + line[2] * t,
        ];
        let Some(c) = neutrons.cell_index_of_pos(p) else {
            // A lone empty 250 ly cell on a real line is grid noise the
            // search shrugs off with a plain jump or two (mid-disk
            // occupancy is not contiguous; treating one as a wall put
            // Wongi->Beagle right back at +25%). A wall is a CONTIGUOUS
            // empty run >= 2 cells (~500 ly, the axis deserts' measured
            // width): walk the line both ways from the sample until an
            // occupied cell appears.
            if seen && !pending_wall {
                let mut run = 1u32;
                for dir in [1.0f32, -1.0] {
                    for s in 1..=6 {
                        let q = [
                            p[0] + step[0] * s as f32 * dir,
                            p[1] + step[1] * s as f32 * dir,
                            p[2] + step[2] * s as f32 * dir,
                        ];
                        if neutrons.cell_index_of_pos(q).is_some() {
                            break;
                        }
                        run += 1;
                    }
                }
                pending_wall = run >= 2;
            }
            continue;
        };
        seen = true;
        wall |= pending_wall;
        let e = dist(p, target_pos).max(full_range);
        let r = alt.lower_bound_ly(c, target_cell).max(e) / e;
        lo_r = lo_r.min(r);
        hi_r = hi_r.max(r);
    }
    // A wall only counts with the oracle's corroboration: some sampled
    // bound >= 1% above euclid. Wongi->Beagle's line has genuine 500 ly
    // gaps, yet the oracle barely prices them (measured hi 1.0031) --
    // a wall the oracle cannot see cannot reorder the frontier, only
    // bill for the consults -- while the walls that pay bind clearly
    // (Colonia->Spase 1.0215/1.0315 by side, the Mac's skirtable
    // synthetic ~1.04).
    let verdict = (wall && hi_r > 1.01) || (hi_r > 0.0 && hi_r / lo_r.max(1.0) > 1.3);
    if std::env::var_os("ED_ALT_TRACE").is_some() {
        eprintln!(
            "alt precheck: target [{:.0} {:.0} {:.0}] lo {lo_r:.5} hi {hi_r:.5} wall {wall} -> {verdict}",
            target_pos[0], target_pos[1], target_pos[2]
        );
    }
    verdict
}

/// Bidirectional coarse search (ROUTING-NEXT item 12, the user's
/// design): expand from BOTH ends and meet in the middle. The matrix
/// measured hard directions at up to 250x their reverses (Wongi→Beagle
/// 10.9 s vs 89 ms, with a 30-jump-worse route) because a frontier
/// climbing into thinness balloons while one descending into density
/// converges; meeting in the middle means the uphill half only ever
/// covers half the distance. The directed-edge problem dissolves here:
/// the backward frontier explores predecessors, and edge u→v prices by
/// boost(u) -- the CANDIDATE's boost, which the scan knows per
/// candidate. Both half-chains stay forward-valid and stitch without
/// the +16% inflation that killed flip-then-refine.
///
/// Fuel-light by design: states are nodes, not (node, tank) -- the
/// backward half cannot know its tank, so neither half pretends to, and
/// [`refine_waypoints`]'s fuel rounds settle the stitched chain (their
/// job already). The fuel-aware single-direction variants stay in the
/// portfolio beside this one; the tie-break keeps whichever is better.
fn plan_long_bidi(g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, ctl: &Control, coarse_weight: f32, cap_mult: u64, greedy: bool, lean_scan: bool, fields: [Option<&crate::cgraph::GoalField>; 2]) -> Result<Route, RouteError> {
    // The harness weight override reaches bidi too (2026-10-17: the three
    // bidi variants ran at 1.5 inside every ED_COARSE_W=3 A/B and flooded
    // the core while the singles crossed).
    let coarse_weight: f32 = std::env::var("ED_COARSE_W").ok().and_then(|v| v.parse().ok()).unwrap_or(coarse_weight);

    use crate::alt::ordered::F32;
    let started = std::time::Instant::now();
    let fuel_model = req.fuel.map(|m| crate::fuel::FuelModel { reserve: m.reserve.max(m.max_fuel_per_jump * RESERVE_FRACTION), ..m });
    let req = &RouteRequest { fuel: fuel_model, ..req.clone() };
    let full_range = match fuel_model {
        Some(m) => m.reach(m.capacity, 1.0),
        None => req.range_ly.max(1.0),
    };
    let neutron_boost = if req.supercharge { req.boost.neutron } else { 1.0 };
    let node_boost = |idx: u32| -> f32 {
        if !req.supercharge {
            return 1.0;
        }
        req.boost.for_class(neutrons.class(&neutrons.record(idx)))
    };
    let highway_ok = |idx: u32| -> bool {
        req.boost.white_dwarf > 1.0 || neutrons.class(&neutrons.record(idx)) != crate::StarClass::WhiteDwarf
    };
    let start_rec = g.record(req.from);
    let start_pos = start_rec.pos();
    let start_boost = crate::router::boost_at(g, req, req.from, g.class(&start_rec));
    let goal_pos = g.record(req.to).pos();
    let straight = dist(start_pos, goal_pos);

    // Frontier 0 walks START -> GOAL, frontier 1 walks GOAL -> START by
    // predecessor; both push toward the other's origin.
    let ends = [start_pos, goal_pos];
    let mut best_g: [HashMap<u32, f32>; 2] = [HashMap::default(), HashMap::default()];
    let mut parent: [HashMap<u32, u32>; 2] = [HashMap::default(), HashMap::default()];
    let mut open: [BinaryHeap<std::cmp::Reverse<(F32, u32)>>; 2] = [BinaryHeap::new(), BinaryHeap::new()];
    best_g[0].insert(START, 0.0);
    best_g[1].insert(GOAL, 0.0);
    open[0].push(std::cmp::Reverse((F32(straight / (full_range * neutron_boost) * coarse_weight), START)));
    open[1].push(std::cmp::Reverse((F32(straight / (full_range * neutron_boost) * coarse_weight), GOAL)));
    let pos_of = |idx: u32| -> [f32; 3] {
        match idx {
            START => start_pos,
            GOAL => goal_pos,
            i => neutrons.pos_of(i),
        }
    };
    // The gap-aware ALT bound serves both frontiers: it knows what
    // euclid cannot -- that a desert on the line costs plain jumps.
    // The pos->cell lookup is memoised per morton key (the consult's
    // real cost is one binary search per pushed candidate).
    let alt = if std::env::var_os("ED_NO_ALT").is_some() { None } else { neutrons.alt() };
    let end_cells = [neutrons.cell_index_of_pos(start_pos), neutrons.cell_index_of_pos(goal_pos)];
    let cell_cache: std::cell::RefCell<HashMap<u64, Option<u32>>> = std::cell::RefCell::new(HashMap::default());
    // Same uniformity precheck as the single search; both ends are
    // probed because each frontier consults toward its own target -- a
    // goal deep in sparse space has no cell at all (end_cells[1] =
    // None), yet the backward frontier still consults toward the start.
    let alt = alt.filter(|a| {
        (0..2).any(|side| {
            end_cells[side].is_some_and(|gc| {
                alt_line_is_nonuniform(neutrons, a, start_pos, goal_pos, ends[side], gc, full_range)
            })
        })
    });
    // Plain-jump units per light-year (R2): the cheaper of a plain jump
    // per `full_range` and a supercharged hop at BOOST_UNITS per
    // `full_range x boost`.
    let unit_per_ly = if units_knob(&COARSE_UNITS, "ED_UNITS_COARSE") { (1.0 / full_range).min(BOOST_UNITS / (full_range * neutron_boost)) } else { 1.0 / (full_range * neutron_boost) };
    let h_of = |p: [f32; 3], target_side: usize| -> f32 {
        let mut d = dist(p, ends[target_side]);
        let mut cell = None;
        if alt.is_some() || fields[target_side].is_some() {
            let (cx, cy, cz) = crate::format::cell_of_with(p, neutrons.cell_ly);
            let key = crate::format::morton_cell_key(cx, cy, cz);
            cell = *cell_cache
                .borrow_mut()
                .entry(key)
                .or_insert_with(|| neutrons.cell_index(cx, cy, cz).map(|i| i as u32));
        }
        if let (Some(alt), Some(tc), Some(pc)) = (alt, end_cells[target_side], cell) {
            d = d.max(alt.lower_bound_ly(pc as usize, tc));
        }
        // Boosted hops to go, in plain-jump units (R2): a plain jump is a
        // unit per `full_range`, a supercharged one `BOOST_UNITS` per
        // `full_range x boost`; the cheaper per light-year bounds below.
        let mut j = d * unit_per_ly;
        // The goal field's bound is already in (reference) jumps -- each
        // at least one unit -- and knows the desert crossings cost plain
        // jumps; take whichever bound is tighter.
        if let (Some(f), Some(pc)) = (fields[target_side], cell) {
            if let Some(fj) = f.jumps_to_goal(pc as usize) {
                j = j.max(fj);
            }
        }
        j * coarse_weight
    };
    // Best stitched total and its meeting node.
    let mut meet: Option<(f32, u32)> = None;
    let mut meet_seen_at: u64 = 0;
    let meet_settle: u64 = std::env::var("ED_MEET_SETTLE").ok().and_then(|v| v.parse().ok()).unwrap_or(800);
    let cap: u64 = 30_000 * cap_mult;
    let mut expansions: u64 = 0;
    // Per-side depth diagnostics: how far each frontier actually got
    // (distance to ITS target), printed under ED_PLOT_DEBUG at exit --
    // "never met" can mean needle-vs-haystack OR frontiers 50 kly
    // apart drowning in breadth; the fix differs.
    let mut exps = [0u64; 2];
    let (mut pops, mut stale) = ([0u64; 2], [0u64; 2]);
    let mut best_rem = [straight; 2];
    // Each side's closest-approach position: the tip of that frontier
    // toward the other end -- and therefore the point the OTHER side's
    // cone should intercept once it comes within a few reaches (18b).
    let mut tip: [[f32; 3]; 2] = [start_pos, goal_pos];
    let mut cands: Vec<(f32, u32, [f32; 3], f32)> = Vec::with_capacity(2048);
    let mut bucket: HashMap<u64, usize> = HashMap::default();
    // Item 18a: each side's pushed nodes indexed by sub-index cell.
    // A meet needs a node in BOTH parent chains; requiring the exact
    // same star delays it (measured: 21k expansions to the first
    // shared star on Wongi -> Beagle, long past the grace axe). Cells
    // are 250 ly and boosted reach is 467: when an expanded node's
    // cell NEIGHBOURHOOD holds an opposing node, one priced bridging
    // relax turns "same haystack" into a legitimate shared node.
    let mut cell_nodes: [HashMap<u64, u32>; 2] = [HashMap::default(), HashMap::default()];
    let cell_key_of = |p: [f32; 3]| -> (i32, i32, i32) { crate::format::cell_of_with(p, neutrons.cell_ly) };
    // Item 13: the greedy cone, per frontier -- the forward side can
    // stand in the core while the backward side rides the rim, so each
    // side keeps its own last-scan density. ED_GREEDY=1 forces it on
    // for every variant; ED_GREEDY_FLOOR overrides the engage floor
    // (test rigs whose synthetics sit under the real-galaxy floor).
    let greedy = greedy || std::env::var_os("ED_GREEDY").is_some();
    // Reach-cubed normalization, same as the single search: one floor
    // value means the same stellar density for every drive.
    let greedy_floor: u64 = std::env::var("ED_GREEDY_FLOOR").ok().and_then(|v| v.parse().ok()).unwrap_or(GREEDY_SCAN_FLOOR);
    let greedy_floor = ((greedy_floor as f32 * ((full_range * neutron_boost) / crate::cgraph::REF_BOOSTED_REACH_LY).powi(3)).max(1.0)) as u64;
    let (cone_angles, cone_cap, cone_band_lo) = cone_knobs();
    let (refuel_k, deadend_f) = refuel_knobs();
    // B_* knobs for the search harness: the starvation-guard ratio and
    // offset (shipped 3x + 64) and the meet-settle window (shipped 800).
    let bidi_ratio: u64 = std::env::var("ED_BIDI_RATIO").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let bidi_offset: u64 = std::env::var("ED_BIDI_OFFSET").ok().and_then(|v| v.parse().ok()).unwrap_or(64);
    let mut last_scan = [0u64; 2];
    let mut greedy_cands: Vec<(f32, u32, [f32; 3], f32)> = Vec::new();

    while !(open[0].is_empty() && open[1].is_empty()) {
        if meet.is_some() && expansions - meet_seen_at > meet_settle {
            break;
        }
        if expansions > cap {
            break;
        }
        if expansions.is_multiple_of(20) {
            if (ctl.cancelled)() {
                return Err(RouteError::Cancelled);
            }
            if req.max_expansions > 0 && expansions > req.max_expansions {
                return Err(RouteError::Budget);
            }
        }
        if expansions > 0 && expansions.is_multiple_of(2_000) && std::env::var_os("ED_PLOT_DEBUG").is_some() {
            eprintln!(
                "    bidi x{cap_mult} progress: {expansions} exps at {} ms; fwd {} exps within {:.0} ly of goal (open {}), bwd {} exps within {:.0} ly of start (open {})",
                started.elapsed().as_millis(), exps[0], best_rem[0], open[0].len(), exps[1], best_rem[1], open[1].len()
            );
        }
        // Expand whichever frontier's head is more promising -- but never
        // let one side starve: measured on Wongi -> Beagle, the pure
        // best-f rule gave the forward side 20,997 of 21,078 expansions
        // (the backward side moved 81 and stayed 63 kly out), so "bidi"
        // degenerated into a single search that met the other frontier's
        // shore. The two sides' f scales are not comparable (each h aims
        // at its own target across different terrain). A bounded ratio
        // keeps the choice adaptive while both frontiers actually move.
        let side = match (open[0].peek(), open[1].peek()) {
            (Some(a), Some(b)) => {
                if exps[0] > exps[1] * bidi_ratio + bidi_offset {
                    1
                } else if exps[1] > exps[0] * bidi_ratio + bidi_offset {
                    0
                } else {
                    usize::from(a.0 .0 .0 > b.0 .0 .0)
                }
            }
            (Some(_), None) => 0,
            (None, Some(_)) => 1,
            (None, None) => break,
        };
        let std::cmp::Reverse((F32(f), cur)) = open[side].pop().unwrap();
        pops[side] += 1;
        let g_cur = best_g[side].get(&cur).copied().unwrap_or(f32::INFINITY);
        let h_cur = f - g_cur;
        if h_cur < -1.0 {
            stale[side] += 1;
            continue; // stale entry from an improved g
        }
        expansions += 1;
        exps[side] += 1;
        let here = pos_of(cur);
        (ctl.trace)("coarse", here, g_cur);
        let target = ends[1 - side];
        let remaining = dist(here, target);
        if remaining < best_rem[side] {
            best_rem[side] = remaining;
            tip[side] = here;
        }
        // The scan radius covers the strongest possible departure boost:
        // forward that is the CURRENT node's, backward the candidates'.
        let depart_boost = if side == 0 {
            if cur == START { start_boost } else { node_boost(cur) }
        } else {
            neutron_boost
        };
        let sentinel = cur == START || cur == GOAL;
        let bridges = if sentinel { MAX_END_BRIDGE_JUMPS } else { MAX_BRIDGE_JUMPS * 3 };
        let scan = full_range * depart_boost + full_range * MAX_BRIDGE_JUMPS as f32;
        let corridor = remaining + full_range * 2.0;
        let toward = Some((target, corridor));
        let mut greedy_picks = 0usize;
        if greedy && last_scan[side] >= greedy_floor && remaining > scan && !sentinel {
            // 18b, cone-at-haystack: once the OPPOSING frontier's tip is
            // within a few reaches, the cone aims at it instead of the
            // far endpoint -- a guided intercept exactly where the meet
            // otherwise fails by diffusion. Far apart the two aims are
            // collinear and this changes nothing; the gate keeps a
            // mid-range lateral wander from misleading the cone.
            let enemy_tip = tip[1 - side];
            let tip_d = dist(here, enemy_tip);
            let (aim, aim_d) = if tip_d > scan && tip_d < full_range * depart_boost * 6.0 {
                (enemy_tip, tip_d)
            } else {
                (target, remaining)
            };
            let dirv = [
                (aim[0] - here[0]) / aim_d,
                (aim[1] - here[1]) / aim_d,
                (aim[2] - here[2]) / aim_d,
            ];
            let cell = neutrons.cell_ly;
            greedy_cands.clear();
            for &theta_deg in cone_angles.iter() {
                let (sin_t, cos_t) = theta_deg.to_radians().sin_cos();
                let (r_lo, r_hi) = (full_range * depart_boost * cone_band_lo, full_range * depart_boost);
                let center = [
                    here[0] + dirv[0] * r_hi * 0.9,
                    here[1] + dirv[1] * r_hi * 0.9,
                    here[2] + dirv[2] * r_hi * 0.9,
                ];
                let half = r_hi * (0.1 + sin_t) + cell;
                let c = |v: f32| (v / cell).floor() as i32;
                for cx in c(center[0] - half)..=c(center[0] + half) {
                    for cy in c(center[1] - half)..=c(center[1] + half) {
                        for cz in c(center[2] - half)..=c(center[2] + half) {
                            let Some((s0, n)) = neutrons.cell_range(cx, cy, cz) else { continue };
                            for n_idx in s0..s0 + n {
                                if cur == n_idx || !highway_ok(n_idx) {
                                    continue;
                                }
                                let n_pos = neutrons.pos_of(n_idx);
                                let d = dist(here, n_pos);
                                if d < r_lo || d > r_hi {
                                    continue;
                                }
                                let along = (n_pos[0] - here[0]) * dirv[0]
                                    + (n_pos[1] - here[1]) * dirv[1]
                                    + (n_pos[2] - here[2]) * dirv[2];
                                if along < d * cos_t {
                                    continue;
                                }
                                let to_target = dist(n_pos, target);
                                if to_target > corridor {
                                    continue;
                                }
                                greedy_cands.push((to_target, n_idx, n_pos, d));
                            }
                        }
                    }
                }
                if !greedy_cands.is_empty() {
                    break;
                }
            }
            if greedy_cands.len() > cone_cap {
                greedy_cands.select_nth_unstable_by(cone_cap, |a, b| a.0.total_cmp(&b.0));
                greedy_cands.truncate(cone_cap);
            }
            greedy_picks = greedy_cands.len();
        }
        cands.clear();
        if greedy_picks > 0 {
            cands.extend(greedy_cands.iter().copied());
        } else {
            neutrons.for_each_within_toward(here, scan, toward, |n_idx, d| {
                if n_idx == cur || !highway_ok(n_idx) {
                    return;
                }
                let n_pos = neutrons.pos_of(n_idx);
                let refuels = !lean_scan && refills_on_arrival(neutrons, n_idx);
                let to_t = dist(n_pos, target);
                let bonus = if refuels {
                    let full = full_range * neutron_boost * refuel_k;
                    if through_stop(neutrons, n_pos, target, full_range * neutron_boost) { full } else { full * deadend_f }
                } else {
                    0.0
                };
                cands.push((to_t - bonus, n_idx, n_pos, d));
            });
            last_scan[side] = cands.len() as u64;
            // Thinning parity with the single search (the missing piece
            // that made bidi's expansions an order of magnitude more
            // expensive: it relaxed EVERY scanned candidate, hundreds per
            // expansion in dense space, and the meet landed at ~21k
            // expansions -- long after the fast singles armed the grace
            // axe). Per bucket: the most forward and the nearest;
            // refuelling neutrons always kept.
            let bucket_ly = full_range * 0.75;
            if cands.len() > 64 {
                bucket.clear();
                let mut keep: Vec<(f32, u32, [f32; 3], f32)> = Vec::with_capacity(cands.len() / 4);
                for &c in cands.iter() {
                    let refuels = !lean_scan && refills_on_arrival(neutrons, c.1);
                    if refuels {
                        keep.push(c);
                        continue;
                    }
                    let cell = |v: f32| (v / bucket_ly).floor() as i64 as u64 & 0x1f_ffff;
                    let k = (cell(c.2[0]) << 42) | (cell(c.2[1]) << 21) | cell(c.2[2]);
                    match bucket.get(&k) {
                        Some(&i) => {
                            let (fwd, near) = (i, i + 1);
                            if c.0 < keep[fwd].0 {
                                keep[fwd] = c;
                            }
                            if c.3 < keep[near].3 {
                                keep[near] = c;
                            }
                        }
                        None => {
                            bucket.insert(k, keep.len());
                            keep.push(c);
                            keep.push(c);
                        }
                    }
                }
                keep.dedup_by_key(|c| c.1);
                std::mem::swap(&mut cands, &mut keep);
            }
        }
        let mut pushed = 0usize;
        let relax = |n_idx: u32, n_pos: [f32; 3], d: f32, best_g: &mut [HashMap<u32, f32>; 2], parent: &mut [HashMap<u32, u32>; 2], open: &mut [BinaryHeap<std::cmp::Reverse<(F32, u32)>>; 2], cell_nodes: &mut [HashMap<u64, u32>; 2], max_bridge: u32, meet: &mut Option<(f32, u32)>, meet_seen_at: &mut u64, expansions: u64, cost_override: Option<f32>| -> bool {
            // The DEPARTING side's boost prices the edge: forward that is
            // `cur`, backward it is the candidate. A gap crossing brings
            // its own price (plain jumps across a highway void) that
            // edge_cost_max could never grant.
            let eb = if side == 0 { depart_boost } else { node_boost(n_idx) };
            let Some(c) = cost_override.or_else(|| edge_cost_max(d, full_range, eb, max_bridge).map(|(c, _)| c)) else {
                return false;
            };
            let ng = g_cur + c;
            if best_g[side].get(&n_idx).is_some_and(|&bg| bg <= ng) {
                return false;
            }
            best_g[side].insert(n_idx, ng);
            parent[side].insert(n_idx, cur);
            let h = h_of(n_pos, 1 - side);
            open[side].push(std::cmp::Reverse((F32(ng + h), n_idx)));
            // Register this side's presence in the node's cell (18a).
            let (cx, cy, cz) = cell_key_of(n_pos);
            let ck = crate::format::morton_cell_key(cx, cy, cz);
            let e = cell_nodes[side].entry(ck).or_insert(n_idx);
            if *e != n_idx && best_g[side].get(e).is_none_or(|&eg| ng < eg) {
                *e = n_idx;
            }
            if let Some(&og) = best_g[1 - side].get(&n_idx) {
                let total = ng + og;
                if meet.is_none_or(|(bt, _)| total < bt) {
                    if meet.is_none() {
                        *meet_seen_at = expansions;
                    }
                    *meet = Some((total, n_idx));
                }
            }
            true
        };
        for &(_, n_idx, n_pos, d) in cands.iter() {
            pushed += relax(n_idx, n_pos, d, &mut best_g, &mut parent, &mut open, &mut cell_nodes, bridges, &mut meet, &mut meet_seen_at, expansions, None) as usize;
        }
        // Item 18a, the cell-level meet: when the opposing frontier holds
        // a node in this cell's neighbourhood, bridge to it with a priced
        // hop -- it becomes a node in BOTH parent chains and the ordinary
        // meet logic takes over. 27 hash lookups per expansion.
        {
            let (cx, cy, cz) = cell_key_of(here);
            for dx in -1..=1i32 {
                for dy in -1..=1i32 {
                    for dz in -1..=1i32 {
                        let ck = crate::format::morton_cell_key(cx + dx, cy + dy, cz + dz);
                        let Some(&b) = cell_nodes[1 - side].get(&ck) else { continue };
                        if b == cur || best_g[side].contains_key(&b) {
                            continue;
                        }
                        let b_pos = pos_of(b);
                        let d = dist(here, b_pos);
                        pushed += relax(b, b_pos, d, &mut best_g, &mut parent, &mut open, &mut cell_nodes, bridges, &mut meet, &mut meet_seen_at, expansions, None) as usize;
                    }
                }
            }
        }
        if pushed == 0 {
            // Mirror of the single-direction widen: nothing pushable in
            // the highway's reach, rescan a longer ordinary run. At a
            // sentinel the run is the END allowance, as the single search
            // grants its first edge: the rim-to-rim stress route
            // (2026-10-17, Ood Fleau -> Sphiesi) has its nearest neutron
            // 1,498 ly from the start and 1,138 ly from the goal, and
            // with the sentinels excluded here bidi died after one
            // expansion per side ("no meet after 2 expansions") on the
            // one crossing only it can solve. The scan stays cheap: it
            // runs only when the short scan pushed nothing.
            let far = if sentinel { full_range * depart_boost + full_range * MAX_END_BRIDGE_JUMPS as f32 } else { scan + full_range * (MAX_BRIDGE_JUMPS * 3) as f32 };
            let widen_bridges = if sentinel { MAX_END_BRIDGE_JUMPS } else { MAX_BRIDGE_JUMPS * 6 };
            cands.clear();
            neutrons.for_each_within_toward(here, far, toward, |n_idx, d| {
                if n_idx == cur || d <= scan || !highway_ok(n_idx) {
                    return;
                }
                cands.push((0.0, n_idx, neutrons.pos_of(n_idx), d));
            });
            let mut widened = 0usize;
            for &(_, n_idx, n_pos, d) in cands.iter() {
                widened += relax(n_idx, n_pos, d, &mut best_g, &mut parent, &mut open, &mut cell_nodes, widen_bridges, &mut meet, &mut meet_seen_at, expansions, None) as usize;
            }
            if widened == 0 {
                // 5.3-v2, the desert widen: even the widened scan found
                // nothing -- this frontier stands at a highway void
                // wider than any bridge allowance. Keep doubling the
                // scan until the far shore appears (the morton walk
                // makes empty-space scans nearly free -- this is what
                // the crossover work bought), and relax the nearest few
                // shore stars priced as the plain-jump crossing they
                // are. Both frontiers do this, so they meet mid-desert.
                // First tried via the cell graph's gap edges: measured
                // useless on the real Spase desert, because gap edges
                // only join FOREIGN components and the core and far-arm
                // highways are one component joined the long way round;
                // deserts inside a component got no edges at all.
                let mut lo_r = far;
                let mut hi_r = far * 2.0;
                let max_r = full_range * 160.0;
                let mut shore: Vec<(f32, u32, [f32; 3])> = Vec::new();
                while shore.is_empty() && lo_r < max_r {
                    neutrons.for_each_within_toward(here, hi_r.min(max_r), toward, |n_idx, d| {
                        if n_idx == cur || d <= lo_r || !highway_ok(n_idx) {
                            return;
                        }
                        shore.push((d, n_idx, neutrons.pos_of(n_idx)));
                    });
                    lo_r = hi_r;
                    hi_r *= 2.0;
                }
                shore.sort_by(|a, b| a.0.total_cmp(&b.0));
                for &(d, n_idx, n_pos) in shore.iter().take(4) {
                    let crossing = (d / full_range).ceil().max(1.0);
                    relax(n_idx, n_pos, d, &mut best_g, &mut parent, &mut open, &mut cell_nodes, 0, &mut meet, &mut meet_seen_at, expansions, Some(crossing));
                }
            }
        }
    }
    let Some((_, meet_node)) = meet else {
        if std::env::var_os("ED_PLOT_DEBUG").is_some() {
            eprintln!(
                "    bidi x{cap_mult}: no meet after {expansions} expansions (fwd {} exps, {} nodes, reached within {:.0} ly of goal; bwd {} exps, {} nodes, within {:.0} ly of start; straight {:.0}, gap ~{:.0} ly)",
                exps[0], best_g[0].len(), best_rem[0], exps[1], best_g[1].len(), best_rem[1], straight,
                (best_rem[0] + best_rem[1] - straight).max(0.0)
            );
        }
        return Err(RouteError::NoRoute);
    };
    // Stitch: START..meet from the forward parents (reversed), then
    // meet..GOAL from the backward parents (already goal-ward).
    let dbg = std::env::var_os("ED_PLOT_DEBUG").is_some();
    let mut fwd: Vec<u32> = Vec::new();
    let mut at = meet_node;
    while at != START {
        fwd.push(at);
        let Some(&p) = parent[0].get(&at) else {
            if dbg { eprintln!("    bidi: forward parent chain broken at {at}"); }
            return Err(RouteError::NoRoute);
        };
        at = p;
    }
    fwd.reverse();
    let mut chain: Vec<u32> = fwd;
    let mut at = meet_node;
    while at != GOAL {
        let Some(&p) = parent[1].get(&at) else {
            if dbg { eprintln!("    bidi: backward parent chain broken at {at}"); }
            return Err(RouteError::NoRoute);
        };
        at = p;
        if at != GOAL {
            chain.push(at);
        }
    }
    if dbg {
        let mp = pos_of(meet_node);
        eprintln!(
            "    bidi: met at node {meet_node} [{:.0} {:.0} {:.0}], chain {} nodes, {expansions} expansions (fwd {} exps/{} pops/{} stale to {:.0} ly out, open {}; bwd {} exps/{} pops/{} stale to {:.0} ly out, open {}), {} ms",
            mp[0], mp[1], mp[2], chain.len(), exps[0], pops[0], stale[0], best_rem[0], open[0].len(), exps[1], pops[1], stale[1], best_rem[1], open[1].len(), started.elapsed().as_millis()
        );
    }
    let start_fuel = match fuel_model {
        Some(m) => req.start_fuel.clamp(0.0, m.capacity),
        None => 0.0,
    };
    let capacity = fuel_model.map(|m| m.capacity).unwrap_or(0.0);
    let mid_fuel = (capacity - fuel_model.map(|m| m.max_fuel_per_jump).unwrap_or(0.0)).max(0.0);
    let mut waypoints: Vec<(u32, f32)> = vec![(req.from, start_fuel)];
    for &n in &chain {
        let Some(full) = g.find(neutrons.name(&neutrons.record(n))) else { continue };
        if full != req.from && waypoints.last().map(|&(w, _)| w) != Some(full) {
            waypoints.push((full, mid_fuel));
        }
    }
    if waypoints.last().map(|&(w, _)| w) != Some(req.to) {
        waypoints.push((req.to, 0.0));
    }
    if waypoints.len() < 2 {
        return Err(RouteError::NoRoute);
    }
    let r = refine_waypoints(g, req, ctl, waypoints, start_fuel, fuel_model, expansions, straight, started);
    if dbg {
        match &r {
            Ok(r) => eprintln!("    bidi: refined to {} jumps, {} stops", r.jumps, r.refuel_stops),
            Err(e) => eprintln!("    bidi: refine FAILED: {e:?}"),
        }
    }
    r
}

/// The retrograde variant (un-parked 2026-09-02): plan the DESCENDING
/// direction (goal -> start), reverse the skeleton, refine forward with
/// the real fuel. Where descent converges, its coarse skeleton is
/// better-shaped than anything an ascending frontier builds -- measured
/// on Wongi -> Beagle Mandalay: 313 j descending vs 363 ascending over
/// the same stars, and even bidi's meet-chain is ~90% ascending
/// scaffold. Runs as ONE portfolio member under the tie-break (never
/// arms the grace clock): it does not need to be faster everywhere, it
/// needs to win where descent is easy and lose silently elsewhere.
/// The reversed skeleton's fuel assumptions are wrong by construction
/// (scoop positions differ by direction); refine_waypoints' fuel rounds
/// and leg merges repair that, the same pipeline that flies bidi's
/// fuel-blind chains. ED_NO_RETRO=1 removes it for A/B.
fn plan_long_retro(g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, ctl: &Control, cap_mult: u64, field_back: Option<&crate::cgraph::GoalField>) -> Result<Route, RouteError> {
    let started = std::time::Instant::now();
    let full_tank = req.fuel.map(|m| m.capacity).unwrap_or(0.0);
    let rev = RouteRequest { from: req.to, to: req.from, start_fuel: full_tank, ..req.clone() };
    let none = std::sync::atomic::AtomicBool::new(false);
    let back = plan_long_with(g, neutrons, &rev, ctl, COARSE_WEIGHT, RESERVE_FRACTION, 0.75, cap_mult, false, false, false, None, &none, &CoarseStats::default(), field_back, false, &NO_BANS, &NO_INCUMBENT)?;
    let fuel_model = req.fuel.map(|m| crate::fuel::FuelModel { reserve: m.reserve.max(m.max_fuel_per_jump * RESERVE_FRACTION), ..m });
    let req = &RouteRequest { fuel: fuel_model, ..req.clone() };
    let start_fuel = fuel_model.map(|m| req.start_fuel.clamp(0.0, m.capacity)).unwrap_or(0.0);
    let capacity = fuel_model.map(|m| m.capacity).unwrap_or(0.0);
    let mid_fuel = (capacity - fuel_model.map(|m| m.max_fuel_per_jump).unwrap_or(0.0)).max(0.0);
    // The skeleton is the boost-eligible highway stars only: plain
    // pass-through stars are the refiner's business (and a x1.0 white
    // dwarf flown through must not become a coarse waypoint). The
    // `boosted` hop flag marks arrivals, not boost sources, and flips
    // meaning under reversal -- membership does not.
    let keep_star = |name: &str| -> bool {
        neutrons.find(name).is_some_and(|i| {
            req.boost.white_dwarf > 1.0 || neutrons.class(&neutrons.record(i)) != crate::StarClass::WhiteDwarf
        })
    };
    let mut waypoints: Vec<(u32, f32)> = vec![(req.from, start_fuel)];
    // Keep every RETRO_STRIDE-th highway star: pinning every one forces
    // the forward refine to satisfy fuel at waypoints placed for
    // DOWNHILL scooping (measured: full pinning turned a 313 j descent
    // into a 365 j / 127 stop forward slog). A loose skeleton keeps the
    // descent's macro-geometry and leaves the fuel micro-structure to
    // the refiner. ED_RETRO_STRIDE overrides.
    let stride: usize = std::env::var("ED_RETRO_STRIDE").ok().and_then(|v| v.parse().ok()).unwrap_or(3).max(1);
    let mut kept = 0usize;
    for h in back.hops.iter().rev() {
        if !keep_star(&h.name) {
            continue;
        }
        kept += 1;
        if !(kept - 1).is_multiple_of(stride) {
            continue;
        }
        let Some(idx) = g.find(&h.name) else { continue };
        if idx != req.from && idx != req.to && waypoints.last().map(|&(w, _)| w) != Some(idx) {
            waypoints.push((idx, mid_fuel));
        }
    }
    if waypoints.last().map(|&(w, _)| w) != Some(req.to) {
        waypoints.push((req.to, 0.0));
    }
    if waypoints.len() < 2 {
        return Err(RouteError::NoRoute);
    }
    let straight = dist(g.pos_of(req.from), g.pos_of(req.to));
    refine_waypoints(g, req, ctl, waypoints, start_fuel, fuel_model, back.expansions, straight, started)
}

/// Rung 3 of the 24c ladder (item 27 follow-up, user-greenlit): the
/// descending corridor prior. On a thin-goal ascent the descending
/// search threads the rim from a tiny opening frontier in ~1 s while
/// the ascending frontier pays 12-17 s for the same knowledge. Plan
/// the DESCENDING direction first, keep only its corridor GEOMETRY
/// (a subsampled polyline -- NOT the hop skeleton, whose downhill
/// scoop positions fuel-poisoned the retro variant), then run the
/// forward plan with the polyline as a heuristic guide. Gated by the
/// off-ramp mirror signal (goal-side thin, start-side dense), the
/// same cells where the reverse plan is known-cheap. One portfolio
/// slot; ED_NO_CORRIDOR=1 removes it. The anchor (the reverse
/// route ACHIEVED jumps x slack) is stored for the arming rule:
/// unlike the euclid/field ceilings, hopeless at the rim, a
/// same-ship descent is a real achieved figure for this corridor.
fn plan_long_corridor(
    g: &Galaxy,
    neutrons: &Galaxy,
    req: &RouteRequest,
    ctl: &Control,
    cap_mult: u64,
    fields: [Option<&crate::cgraph::GoalField>; 2],
    anchor: &std::sync::atomic::AtomicU32,
) -> Result<Route, RouteError> {
    let (start_pos, goal_pos) = (g.pos_of(req.from), g.pos_of(req.to));
    let straight = dist(start_pos, goal_pos);
    let boost = if req.supercharge { req.boost.neutron } else { 1.0 };
    let reach = req.fuel.map(|m| m.reach(m.capacity, 1.0)).unwrap_or(req.range_ly.max(1.0)) * boost;
    // Engage only where the mirror gate says the goal side is the thin
    // side -- everywhere else the slot loses silently and cheaply.
    if on_ramp(neutrons, fields[1], goal_pos, start_pos, straight, reach, true).is_none() {
        return Err(RouteError::Cancelled);
    }
    let full_tank = req.fuel.map(|m| m.capacity).unwrap_or(0.0);
    let rev = RouteRequest { from: req.to, to: req.from, start_fuel: full_tank, ..req.clone() };
    let none = std::sync::atomic::AtomicBool::new(false);
    let back = plan_long_with(g, neutrons, &rev, ctl, COARSE_WEIGHT, RESERVE_FRACTION, 0.75, cap_mult, false, false, false, None, &none, &CoarseStats::default(), fields[0], false, &NO_BANS, &NO_INCUMBENT)?;
    let slack: f32 = std::env::var("ED_CORRIDOR_SLACK").ok().and_then(|v| v.parse().ok()).unwrap_or(1.05);
    anchor.store(((back.jumps as f32) * slack + 2.0).ceil() as u32, std::sync::atomic::Ordering::Relaxed);
    // Pins: the descent's own highway stars, reversed to start -> goal
    // order, spaced >= ED_CORRIDOR_SEG apart. The h-guide form measured
    // inert (a polyline pull cannot redirect the scan's lane choice at
    // any subsample density), and retro's exact-refine pins fly plain
    // between waypoints (stride 10-60 measured 856-911 j, ~0 boosts).
    // This is the middle lever: SEGMENTED COARSE planning -- each pin
    // pair gets a full plan_long_with with total freedom inside the
    // segment and the previous segment's real end fuel, so the descent
    // contributes exactly its macro-geometry and nothing else.
    // Pin only the goal-side TAIL: the mid-corridor lanes measured FREE
    // (equal jumps at 2-3 kly lateral separation, item 23), so body pins
    // pure constraint; the descent's transferable knowledge is which rim
    // entry chains, and that lives in the last stretch.
    let seg_ly: f32 = std::env::var("ED_CORRIDOR_SEG").ok().and_then(|v| v.parse().ok()).unwrap_or(3_000.0);
    let tail_ly: f32 = std::env::var("ED_CORRIDOR_TAIL").ok().and_then(|v| v.parse().ok()).unwrap_or(12_000.0);
    let keep_star = |name: &str| -> bool {
        neutrons.find(name).is_some_and(|i| {
            req.boost.white_dwarf > 1.0 || neutrons.class(&neutrons.record(i)) != crate::StarClass::WhiteDwarf
        })
    };
    let mut pins: Vec<(u32, [f32; 3])> = Vec::new();
    let mut last_pos = start_pos;
    for h in back.hops.iter().rev() {
        if !keep_star(&h.name) {
            continue;
        }
        let to_goal = dist(h.pos, goal_pos);
        if to_goal <= tail_ly && to_goal >= seg_ly && dist(h.pos, last_pos) >= seg_ly {
            pins.push((h.idx, h.pos));
            last_pos = h.pos;
        }
    }
    let mut fuel = req.start_fuel;
    let mut whole: Option<Route> = None;
    let mut at = req.from;
    let ends: Vec<u32> = pins.iter().map(|&(i, _)| i).chain(std::iter::once(req.to)).collect();
    for &pin in &ends {
        let sreq = RouteRequest { from: at, to: pin, start_fuel: fuel, ..req.clone() };
        // Segments run field-less: the plot's goal field prices toward
        // the FINAL goal and would poison a segment's heuristic.
        let leg = plan_long_with(g, neutrons, &sreq, ctl, COARSE_WEIGHT, RESERVE_FRACTION, 0.75, cap_mult, false, false, false, None, &none, &CoarseStats::default(), None, false, &NO_BANS, &NO_INCUMBENT)?;
        fuel = leg.hops.last().and_then(|h| h.fuel_after).unwrap_or(full_tank);
        at = pin;
        whole = Some(match whole {
            None => leg,
            Some(mut w) => {
                w.hops.extend(leg.hops.into_iter().skip(1));
                w.jumps += leg.jumps;
                w.total_ly += leg.total_ly;
                w.boosted_jumps += leg.boosted_jumps;
                w.refuel_stops += leg.refuel_stops;
                w.expansions += leg.expansions;
                w.injections += leg.injections;
                w
            }
        });
    }
    let mut route = whole.ok_or(RouteError::NoRoute)?;
    route.straight_ly = straight;
    Ok(route)
}

fn plan_long_with(g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, ctl: &Control, coarse_weight: f32, reserve_f: f32, bucket_f: f32, cap_mult: u64, greedy: bool, off_ramp_on: bool, lean_scan: bool, corridor: Option<&[([f32; 3], f32)]>, stalled: &std::sync::atomic::AtomicBool, stats: &CoarseStats, field: Option<&crate::cgraph::GoalField>, thin: bool, banned: &std::sync::Mutex<Vec<(u32, u32)>>, incumbent: &std::sync::atomic::AtomicU32) -> Result<Route, RouteError> {

    let started = std::time::Instant::now();
    // Keep a hop's worth of fuel in hand: a route may never arrive at a
    // neutron (no scooping there) with an empty tank, since the way out
    // is a jump. The reserve is enforced by every leg planner below.
    let reserve_f: f32 = std::env::var("ED_RESERVE_F").ok().and_then(|v| v.parse().ok()).unwrap_or(reserve_f);
    let fuel_model = req.fuel.map(|m| crate::fuel::FuelModel { reserve: m.reserve.max(m.max_fuel_per_jump * reserve_f), ..m });
    let req = &RouteRequest { fuel: fuel_model, ..req.clone() };
    // Coarse edges are sized with the same margin the legs will be held to,
    // or every edge at the limit fails refinement and turns into a detour.
    let full_range = match fuel_model {
        Some(m) => m.reach(m.capacity, 1.0),
        None => req.range_ly.max(1.0),
    };
    let range_at = |fuel: f32| -> f32 {
        match fuel_model {
            Some(m) => m.reach(fuel, 1.0),
            None => req.range_ly.max(1.0),
        }
    };
    // The strongest boost on the highway: heuristic and reach radii use it.
    // Each expanded node supercharges by its own class (a white dwarf in
    // the sub-index gives x1.5, not x4).
    let neutron_boost = if req.supercharge { req.boost.neutron } else { 1.0 };
    let node_boost = |idx: u32| -> f32 {
        if !req.supercharge {
            return 1.0;
        }
        req.boost.for_class(neutrons.class(&neutrons.record(idx)))
    };
    // A white dwarf that cannot boost (profile x1.0) is no waypoint: as a
    // plain hop it would only pad the coarse graph and the expansion count.
    let white_dwarfs = req.boost.white_dwarf > 1.0;
    let highway_ok = |idx: u32| -> bool { white_dwarfs || neutrons.class(&neutrons.record(idx)) != crate::StarClass::WhiteDwarf };
    let true_goal_pos = g.record(req.to).pos();
    // Destinations off the highway (Beagle Point is thousands of ly past the
    // last neutron) are reached through a gateway: the neutron nearest the
    // destination within an ordinary-jump run, found once here. The coarse
    // search aims at the gateway and the final approach is one exact leg.
    let gateway: Option<u32> = {
        let near = full_range * neutron_boost + full_range * MAX_BRIDGE_JUMPS as f32;
        let mut any_near = false;
        neutrons.for_each_within(true_goal_pos, near, |n, _| any_near |= highway_ok(n));
        if any_near {
            None
        } else {
            let mut best: Option<(u32, f32)> = None;
            neutrons.for_each_within(true_goal_pos, full_range * MAX_END_BRIDGE_JUMPS as f32, |n, d| {
                if highway_ok(n) && best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((n, d));
                }
            });
            best.map(|(n, _)| n)
        }
    };
    let goal_pos = match gateway {
        Some(n) => neutrons.pos_of(n),
        None => true_goal_pos,
    };
    let debug = std::env::var_os("ED_PLOT_DEBUG").is_some();
    if debug {
        eprintln!("    gateway: {:?} ({:.0} ly from the destination)", gateway.map(|n| neutrons.name(&neutrons.record(n)).to_string()), dist(goal_pos, true_goal_pos));
    }
    let start_rec = g.record(req.from);
    let start_pos = start_rec.pos();
    let start_boost = crate::router::boost_at(g, req, req.from, g.class(&start_rec));
    let straight = dist(start_pos, goal_pos);
    let start_fuel = match fuel_model {
        Some(m) => req.start_fuel.clamp(0.0, m.capacity),
        None => 0.0,
    };
    let steps = fuel_model.map(|m| fuel_steps(m.capacity)).unwrap_or(1.0);
    let quant = |fuel: f32| -> u64 {
        match fuel_model {
            Some(m) if m.capacity > 0.0 => ((fuel / m.capacity) * steps).round() as u64,
            _ => 0,
        }
    };
    let key = |idx: u32, fuel: f32| -> u64 { ((idx as u64) << 8) | quant(fuel) };

    let pos_of = |idx: u32| -> [f32; 3] {
        match idx {
            GOAL => goal_pos,
            START => start_pos,
            i => neutrons.pos_of(i),
        }
    };
    let coarse_weight: f32 = std::env::var("ED_COARSE_W").ok().and_then(|v| v.parse().ok()).unwrap_or(coarse_weight);
    // The ALT landmark bound knows where the highway detours (voids, the
    // rim); the straight line does not, and pays for it in widened
    // rescans. Highest of the two admissible bounds wins; the table is
    // absent = straight line alone, as ever. ED_NO_ALT=1 disables it
    // for A/B measurement.
    let alt = if std::env::var_os("ED_NO_ALT").is_some() { None } else { neutrons.alt() };
    let alt_goal = alt.and_then(|_| neutrons.cell_index_of_pos(goal_pos));
    // (An earlier trial fed the goal FIELD in here and was reverted --
    // reachability, not guidance, was the failure then. The desert
    // widen changed that; the gap-aware ALT bound below is the second
    // trial, measured ~2x on void topology with routes unchanged.)
    // Candidates cluster in cells, so the pos->cell-index lookup -- the
    // consult's real cost, one binary search per pushed candidate -- is
    // memoised per morton key for the life of this search.
    let cell_cache: std::cell::RefCell<HashMap<u64, Option<u32>>> = std::cell::RefCell::new(HashMap::default());
    let cell_of_cached = move |p: [f32; 3]| -> Option<usize> {
        let (cx, cy, cz) = crate::format::cell_of_with(p, neutrons.cell_ly);
        let key = crate::format::morton_cell_key(cx, cy, cz);
        (*cell_cache
            .borrow_mut()
            .entry(key)
            .or_insert_with(|| neutrons.cell_index(cx, cy, cz).map(|i| i as u32)))
        .map(|i| i as usize)
    };
    let alt = alt.filter(|a| {
        alt_goal.is_some_and(|gc| alt_line_is_nonuniform(neutrons, a, start_pos, goal_pos, goal_pos, gc, full_range))
    });
    // The cell graph backs the field-aware widen: when the scan cannot
    // advance the field, its gap edges say where the crossing lands.
    let cell_graph = field.and_then(|_| neutrons.cell_graph());
    // Item 24c rung 1: the opening heuristic learns one detour -- the
    // cheapest credible field entry near the start. Applied as a raise
    // (max), not a bound: the slab tax lives in euclid-dominated states
    // the field cannot see (thin cells fail the keep filter), and the
    // ladder's literal min() form is inert by the triangle inequality.
    // Only states still farther from the goal than the ramp feel it;
    // past the ramp the heuristic is untouched.
    let ramp = on_ramp(neutrons, field, start_pos, goal_pos, straight, full_range * neutron_boost, false);
    // Item 23 residual: the OFF-ramp, 24c's exact mirror. A goal in
    // thinning space (the Beagle rim) punishes an approach that commits
    // to the wrong entry: the ascending Mandalay flew its last 3.8 kly
    // at 0% boost / 55 jumps while a chain-bearing entry (the
    // descending route's exit, via Puwee LM-M d7-0) sat 15 jumps
    // cheaper within a 0.2% detour. Same candidate walk with the ends
    // swapped, so the gradient gate inverts for free: on-ramp engages
    // INTO concentration, off-ramp engages INTO thinning -- mutually
    // exclusive by construction. ED_OFFRAMP=0 disables independently.
    let off_ramp = off_ramp_on
        .then(|| on_ramp(neutrons, field, goal_pos, start_pos, straight, full_range * neutron_boost, true))
        .flatten()
        .map(|(c, fj, _)| (c, fj, dist(c, goal_pos)));
    // Plain-jump units per light-year (R2): the cheaper of a plain jump
    // per `full_range` and a supercharged hop at BOOST_UNITS per
    // `full_range x boost`.
    let unit_per_ly = if units_knob(&COARSE_UNITS, "ED_UNITS_COARSE") { (1.0 / full_range).min(BOOST_UNITS / (full_range * neutron_boost)) } else { 1.0 / (full_range * neutron_boost) };
    let h = |p: [f32; 3]| {
        let mut d = dist(p, goal_pos);
        let cell = (alt.is_some() || field.is_some()).then(|| cell_of_cached(p)).flatten();
        if let (Some(alt), Some(goal_cell), Some(from_cell)) = (alt, alt_goal, cell) {
            d = d.max(alt.lower_bound_ly(from_cell, goal_cell));
        }
        // Plain-jump units (R2): the cheaper of a plain jump per
        // `full_range` and a supercharged hop at `BOOST_UNITS` per
        // `full_range x boost`, per light-year; the ramp jump counts are
        // at least a unit each.
        let mut j = d * unit_per_ly;
        if let Some((rp, rj, r_to_goal)) = ramp {
            if dist(p, goal_pos) > r_to_goal {
                j = j.max(dist(p, rp) * unit_per_ly + rj);
            }
        }
        if let Some((rp, rj, r_gd)) = off_ramp {
            // Corridor-wide, not an annulus: a raise that switches on at
            // a radius leaves an h cliff the frontier stalls against
            // (measured: M ascent 336 -> 363 j with the annulus).
            if dist(p, goal_pos) > r_gd {
                j = j.max(dist(p, rp) * unit_per_ly + rj);
            }
        }
        // Rung 3: the descending corridor prior. Estimate cost-to-goal
        // as the best "join the descent's path here and follow it" over
        // the subsampled polyline (point, remaining-length pairs) --
        // smooth by construction (a min over all points), no annulus
        // cliff. States far off the descent's corridor are punished by
        // their perpendicular distance; states on it inherit the
        // descent's measured macro-geometry.
        if let Some(cor) = corridor {
            let mut est = f32::MAX;
            for &(cp, suffix) in cor {
                let e = dist(p, cp) + suffix;
                if e < est {
                    est = e;
                }
            }
            j = j.max(est / (full_range * neutron_boost));
        }
        // The goal field's bound is already in (reference) jumps and
        // knows the desert crossings cost plain jumps -- take whichever
        // bound is tighter. (Tried unweighted-field h when the 18d
        // honest pricing tightened the bound: measured inert on the
        // Spase corridor both ways, reverted.)
        if let (Some(f), Some(from_cell)) = (field, cell) {
            if let Some(fj) = f.jumps_to_goal(from_cell) {
                j = j.max(fj);
            }
        }
        j * coarse_weight
    };

    let mut best_g: HashMap<u64, f32> = HashMap::default();
    let mut parent: HashMap<u64, (u64, bool)> = HashMap::default(); // (parent state, via scoop stop)
    let injection = req.injection;
    // Non-dominated (cost, fuel) per neutron: same node, no fewer jumps, no more fuel = worthless.
    let mut pareto: HashMap<u32, Vec<(f32, f32)>> = HashMap::default();
    let mut open = BinaryHeap::new();
    let start_key = key(START, start_fuel);
    best_g.insert(start_key, 0.0);
    open.push(Open { f: h(start_pos), g: 0.0, idx: START, fuel: start_fuel, inj: 0, hops: 0 });

    let mut expansions: u64 = 0;
    let mut best_remaining = straight;
    let mut goal_key: Option<u64> = None;
    // Where the highway thins out the heuristic (one boosted hop per step)
    // is far too optimistic and the search stops converging. Track the
    // closest state reached; after a long stall the plan ends there and
    // the exact planner flies the rest.
    let mut best_key: Option<u64> = None;
    let mut last_improvement: u64 = 0;
    // The goal edge (a long ordinary run) costs far more than the heuristic
    // credits the states around it, so once one is on the heap the search
    // would grind through the whole open list before popping it. A goal
    // that has been reachable for this many expansions is taken as found.
    let mut goal_seen: Option<(f32, u64, u64)> = None; // (g, key, expansions when first seen)
    let goal_settle: u64 = std::env::var("ED_GOAL_SETTLE").ok().and_then(|v| v.parse().ok()).unwrap_or(2_000);
    let mut scanned: u64 = 0;
    let mut relaxed: u64 = 0;
    // ED_COARSE_TIMERS: cumulative time per section, measured once per
    // expansion (not per candidate) so the timers cannot distort the loop.
    let timers = std::env::var_os("ED_COARSE_TIMERS").is_some();
    let mut t_scan = std::time::Duration::ZERO;
    let mut t_thin = std::time::Duration::ZERO;
    let mut t_relax = std::time::Duration::ZERO;
    let mut t_pop = std::time::Duration::ZERO;
    let mut t_widen = std::time::Duration::ZERO;
    let mut widens: u64 = 0;
    let mut t_mark = std::time::Instant::now();
    let mut cands: Vec<(f32, u32, [f32; 3], f32)> = Vec::with_capacity(4096);
    let bucket_ly: f32 = std::env::var("ED_COARSE_BUCKET_LY").ok().and_then(|v| v.parse().ok()).unwrap_or(full_range * bucket_f);
    // ED_COARSE_SECTOR_KEEP > 0 replaces the bucket thinning with sector
    // k-best (R1 A/B knob; the matrix decides the default).
    let sector_keep: usize = std::env::var("ED_COARSE_SECTOR_KEEP").ok().and_then(|v| v.parse().ok()).unwrap_or(COARSE_SECTOR_KEEP);
    let mut bucket: HashMap<u64, usize> = HashMap::default();
    let goal_run_bridge: f32 = std::env::var("ED_GOAL_RUN_BRIDGE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.6);
    let fanout: usize = std::env::var("ED_COARSE_FANOUT").ok().and_then(|v| v.parse().ok()).unwrap_or(if thin { THIN_FANOUT } else { COARSE_FANOUT });
    // Item 13: density-adaptive greedy cone. In dense space the full
    // scan sees thousands of near-equivalent candidates per expansion
    // and RELAX dominates coarse time (43.8 of 65 ms on Colonia->
    // Sag A*, 1,918 relaxed per expansion). When the last full scan
    // says the shelf is full, walk only the cells of a narrow goalward
    // cone in the outer reach band and relax the best few. A cone miss
    // falls back to the full machinery, whose scan count re-decides
    // density -- sparse space never enters. ED_GREEDY=1 forces it on
    // for every variant; ED_GREEDY_FLOOR overrides the engage floor
    // (test rigs whose synthetics sit under the real-galaxy floor).
    let greedy = greedy || std::env::var_os("ED_GREEDY").is_some();
    // The floor is an absolute candidate COUNT against a scan whose
    // volume scales with reach cubed -- unnormalized, the same 512 was
    // effectively ~3.7x stricter for a x4 Mandalay (30% of the x6
    // scan volume), an accidental per-ship policy (user-spotted,
    // 2026-09-02). Normalize by (ship boosted reach / reference)^3 so
    // one knob means the same stellar density for every drive.
    let greedy_floor: u64 = std::env::var("ED_GREEDY_FLOOR").ok().and_then(|v| v.parse().ok()).unwrap_or(GREEDY_SCAN_FLOOR);
    let greedy_floor = ((greedy_floor as f32 * ((full_range * neutron_boost) / crate::cgraph::REF_BOOSTED_REACH_LY).powi(3)).max(1.0)) as u64;
    let (cone_angles, cone_cap, cone_band_lo) = cone_knobs();
    let (refuel_k, deadend_f) = refuel_knobs();
    let mut last_scan_count: u64 = 0;
    let mut greedy_exps: u64 = 0;
    let mut greedy_scanned: u64 = 0;
    let mut greedy_cands: Vec<(f32, u32, [f32; 3], f32)> = Vec::new();
    let stall_expansions: u64 = std::env::var("ED_COARSE_STALL").ok().and_then(|v| v.parse().ok()).unwrap_or(20_000 * cap_mult);
    // Small-tank / low-boost ships (x4, 32 t) make the coarse graph far
    // wider; past this many expansions the closest neutron reached is good
    // enough and the exact planner finishes from there.
    let coarse_expansion_cap: u64 = std::env::var("ED_COARSE_CAP").ok().and_then(|v| v.parse().ok()).unwrap_or(30_000 * cap_mult);
    // ED_COARSE_TRACE=1 (instrument, 2026-10-17, the rim pair): where the
    // search is every 5,000 expansions and, at a stall, the frontier and
    // the refusals -- [edge too far to bridge, no fundable candidate,
    // beaten at the state, pareto-dominated].
    let trace = std::env::var_os("ED_COARSE_TRACE").is_some();
    // ED_GOAL_BRIDGE (study knob, 2026-10-17): the plain-jump run the last
    // coarse edge may hand the exact planner; the start end allows
    // MAX_END_BRIDGE_JUMPS, the goal end MAX_GOAL_BRIDGE_JUMPS.
    let goal_bridge: u32 = std::env::var("ED_GOAL_BRIDGE").ok().and_then(|v| v.parse().ok()).unwrap_or(MAX_GOAL_BRIDGE_JUMPS);
    // ED_COARSE_FAR_BRIDGES (study knob, 2026-10-17): the plain jumps a
    // widened mid-route scan may bridge (default MAX_BRIDGE_JUMPS x 3).
    let far_bridges: u32 = std::env::var("ED_COARSE_FAR_BRIDGES").ok().and_then(|v| v.parse().ok()).unwrap_or(MAX_BRIDGE_JUMPS * 3);
    let mut trace_counts = [0u64; 4];
    let mut trace_refused: std::collections::HashSet<u32> = std::collections::HashSet::new();
    (ctl.stage)("coarse", 0, 0);
    // How far out the search settled, for the variant outcome (0 = goal).
    let mut stall_at: f32 = 0.0;
    // Candidates the last scan produced before thinning (the widen rule),
    // and the cells whose dominated expansions already widened once.
    let mut scan_cands: usize = 0;
    let mut widened_cells: rustc_hash::FxHashSet<u64> = rustc_hash::FxHashSet::default();
    let widen_once_per_cell = std::env::var("ED_COARSE_WIDEN_ONCE_PER_CELL").ok().as_deref() == Some("1");

    let mut pops: u64 = 0;
    let mut stale_pops: u64 = 0;
    let mut max_open: usize = 0;
    let (mut fwidens, mut fw_relaxed, mut fw_pushed): (u64, u64, u64) = (0, 0, 0);
    while let Some(cur) = open.pop() {
        pops += 1;
        max_open = max_open.max(open.len() + 1);
        let cur_key = key(cur.idx, cur.fuel);
        if best_g.get(&cur_key).is_some_and(|&bg| bg < cur.g) {
            stale_pops += 1;
            continue;
        }
        if cur.idx == GOAL {
            goal_key = Some(cur_key);
            break;
        }
        expansions += 1;
        if debug && expansions.is_multiple_of(25_000) {
            eprintln!("    coarse progress: {expansions} expansions, best remaining {best_remaining:.0} ly, open {}, f {:.1} g {:.1} fuel {:.0}", open.len(), cur.f, cur.g, cur.fuel);
        }
        if expansions.is_multiple_of(20) {
            if (ctl.cancelled)() {
                return Err(RouteError::Cancelled);
            }
            (ctl.progress)(expansions, best_remaining);
            if req.max_expansions > 0 && expansions > req.max_expansions {
                return Err(RouteError::Budget);
            }
        }
        let here = pos_of(cur.idx);
        (ctl.trace)("coarse", here, cur.g);
        let boost = if cur.idx == START { start_boost } else { node_boost(cur.idx) };
        let remaining = dist(here, goal_pos);
        // Progress is measured in the field's units when a field exists:
        // a desert detour starts LATERALLY, so euclid-remaining sits flat
        // for thousands of ly of real progress along the arc, and the
        // stall guillotine would fire on exactly the walk the field
        // guides. The fallback handoff inherits the same measure -- the
        // exact planner continues from the best node on the arc, not the
        // closest wall-hugger.
        let here_cell = field.and_then(|_| cell_of_cached(here));
        let here_field = match (field, here_cell) {
            (Some(f), Some(c)) => f.jumps_to_goal(c),
            _ => None,
        };
        // The measure must be ONE unit system: max(field, euclid) in
        // jumps -- the same lower bound h uses. An euclid-only fallback
        // where the field has no value (thin cells) is optimistic, and a
        // frontier starting from a thin end records a minimum that
        // field-covered progress can never beat: last_improvement
        // freezes and the stall guillotine fires spuriously (measured:
        // Spase -> Colonia 107 j / 43 s against Colonia -> Spase's
        // 91 j / 1.1 s on the same corridor).
        let measure = match field {
            Some(_) => here_field
                .unwrap_or(0.0)
                .max(remaining / (full_range * neutron_boost)),
            None => remaining,
        };
        if measure < best_remaining {
            best_remaining = measure;
            best_key = Some(cur_key);
            last_improvement = expansions;
        }
        if let Some((_, gk, seen)) = goal_seen {
            if expansions - seen > goal_settle {
                if debug {
                    eprintln!("    coarse: goal reachable since expansion {seen}; settling after {expansions}");
                }
                goal_key = Some(gk);
                break;
            }
        }
        if (expansions - last_improvement > stall_expansions || expansions > coarse_expansion_cap) && best_key.is_some() && cur.idx != START {
            if debug {
                eprintln!("    coarse stalled at measure {best_remaining:.0} ({}) after {expansions} expansions; the exact planner takes it from there", if field.is_some() { "field jumps" } else { "ly" });
                if field.is_some() {
                    eprintln!("    field widen: fired {fwidens}, relaxed {fw_relaxed}, pushed {fw_pushed}; stalled here [{:.0} {:.0} {:.0}] remaining {remaining:.0} field {:?}", here[0], here[1], here[2], here_field);
                }
            }
            stalled.store(true, std::sync::atomic::Ordering::Relaxed);
            stall_at = if field.is_some() { remaining } else { best_remaining };
            goal_key = best_key;
            if trace {
                let bp = pos_of((best_key.unwrap_or(cur_key) >> 8) as u32);
                eprintln!("    TRACE stall: exp {expansions} open {} best_remaining {best_remaining:.0} at [{:.0} {:.0} {:.0}] (from here [{:.0} {:.0} {:.0}] remaining {remaining:.0}, fuel {:.1}, g {:.1}); refused: too far {} / no fuel {} / beaten {} / pareto {}", open.len(), bp[0], bp[1], bp[2], here[0], here[1], here[2], cur.fuel, cur.g, trace_counts[0], trace_counts[1], trace_counts[2], trace_counts[3]);
                let mut top: Vec<&Open> = open.iter().collect();
                top.sort_by(|a, b| a.f.partial_cmp(&b.f).unwrap_or(std::cmp::Ordering::Equal));
                for o in top.iter().take(12) {
                    let p = pos_of(o.idx);
                    eprintln!("      open f {:.1} g {:.1} fuel {:.1} inj {} at [{:.0} {:.0} {:.0}] remaining {:.0}", o.f, o.g, o.fuel, o.inj, p[0], p[1], p[2], dist(p, goal_pos));
                }
            }
            break;
        }
        let range_here = range_at(cur.fuel);
        let direct_radius = range_here * boost;
        // Coarse edges a refinement leg could not fly, this plot (R3 Task 3,
        // LazySP): read once per expansion, refused in relax.
        let bans: Vec<(u32, u32)> = banned.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if trace && expansions.is_multiple_of(5_000) {
            eprintln!("    TRACE exp {expansions} open {} best_remaining {best_remaining:.0} here [{:.0} {:.0} {:.0}] remaining {remaining:.0} fuel {:.1} g {:.1} inj {} refused {:?}", open.len(), here[0], here[1], here[2], cur.fuel, cur.g, cur.inj, trace_counts);
        }

        // Try an edge to `n_idx` at distance `d`. Two flavours: straight
        // (burning fuel), or via a scoop stop when fuel would not allow it
        // (one more jump, tank refilled short of the target).
        let mut relax = |n_idx: u32, n_pos: [f32; 3], d: f32, best_g: &mut HashMap<u64, f32>, parent: &mut HashMap<u64, (u64, bool)>, open: &mut BinaryHeap<Open>, pareto: &mut HashMap<u32, Vec<(f32, f32)>>, max_bridge: u32| -> bool {
            if !bans.is_empty() && bans.iter().any(|&(a, b)| a == cur.idx && b == n_idx) {
                return false;
            }
            // An edge the ship cannot bridge may still be bridged with one
            // injected (unboosted, x1.25-x2) jump in the run, priced as
            // three jumps like the exact planner does; the injection never
            // helps the supercharged first hop, which it does not stack with.
            let (c, edge_bridges, injected) = match edge_cost_max(d, range_here, boost, max_bridge) {
                Some((c, b)) => (c, b, false),
                None => match injection {
                    Some((mult, _, max)) if cur.inj < max => match edge_cost_max(d - range_here * (mult - 1.0), range_here, boost, max_bridge) {
                        Some((c, b)) => (c + INJECTION_COARSE_PENALTY, b, true),
                        None => {
                            trace_counts[0] += 1;
                            return false;
                        }
                    },
                    _ => {
                        trace_counts[0] += 1;
                        return false;
                    }
                },
            };
            let candidates: [(f32, f32, bool); 2] = match fuel_model {
                None => [(c, 0.0, false), (f32::NAN, 0.0, false)],
                Some(m) => {
                    // Straight: burn for the boosted hop (bridging jumps burn
                    // roughly a max jump each).
                    // The injected jump burns like a bridging one.
                    let bridges = edge_bridges as f32 + if injected { 1.0 } else { 0.0 };
                    let burn = m.fuel_for(d.min(range_here * boost), cur.fuel, boost) + bridges * m.max_fuel_per_jump;
                    let refills = n_idx != GOAL && refills_on_arrival(neutrons, n_idx);
                    // The last edge is an ordinary run the refiner flies with
                    // scoop stops as needed; only the first boosted hop has to
                    // be fundable from the tank, or a small-tank ship stalls
                    // hundreds of ly short of the goal for the whole budget.
                    let goal_run = n_idx == GOAL && bridges > 0.0 && m.fuel_for(d.min(range_here * boost), cur.fuel, boost) <= cur.fuel - m.reserve;
                    let straight = if goal_run {
                        // Priced above a neutron path of the same length: the
                        // run scoops on the way, and where the highway still
                        // reaches, the highway is better.
                        (hop_units(boost) + bridges * goal_run_bridge, m.reserve, false)
                    } else if bridges >= 1.0 && m.fuel_for(d.min(range_here * boost), cur.fuel, boost) <= cur.fuel - m.reserve && d <= range_here * boost + full_range * bridges + if injected { full_range * (injection.map(|i| i.0).unwrap_or(1.0) - 1.0) } else { 0.0 } {
                        // A leg with ordinary jumps in it scoops on the way (the
                        // exact leg planner picks the star): only the boosted
                        // hop has to be fundable, and the tank counts as full
                        // less the last jump. Where no scoopable star exists,
                        // refinement and the fuel rounds put it right.
                        (c, if refills { m.capacity } else { m.capacity - m.max_fuel_per_jump }, false)
                    } else if cur.fuel - burn >= m.reserve && d <= range_here * boost + full_range * bridges + if injected { full_range * (injection.map(|i| i.0).unwrap_or(1.0) - 1.0) } else { 0.0 } {
                        (c, if refills { m.capacity } else { cur.fuel - burn }, false)
                    } else {
                        (f32::NAN, 0.0, false)
                    };
                    // Scoop stop: supercharge to a scoopable star within reach,
                    // refill, then ordinary jumps on. Feasible when the target
                    // is within boosted reach plus one full-tank jump.
                    // The hop to the scoop star burns fuel too: with an empty
                    // tank there is no hop, whatever the range figure says.
                    // Priced as what it is (R2): the hop, the onward jump
                    // and a full stop's overhead at the request's stop
                    // weight; the tonnes are below the coarse resolution.
                    let first_hop = d.min(range_here * boost);
                    let can_reach_scoop = m.fuel_for(first_hop, cur.fuel, boost) <= cur.fuel - m.reserve;
                    let via = if can_reach_scoop && d <= range_here * boost + full_range {
                        let onward = (d - range_here * boost).max(0.0).min(full_range);
                        let after = m.capacity - m.fuel_for(onward.max(full_range * 0.5), m.capacity, 1.0);
                        (if units_knob(&VIA_UNITS, "ED_UNITS_VIA") { hop_units(boost) + 1.0 + STOP_UNITS * req.stop_weight } else { 2.0 }, after, true)
                    } else {
                        (f32::NAN, 0.0, true)
                    };
                    [straight, via]
                }
            };
            let mut any = false;
            let would_improve = trace && n_idx != GOAL && dist(n_pos, goal_pos) < best_remaining;
            if candidates.iter().all(|c| c.0.is_nan()) {
                trace_counts[1] += 1;
                if would_improve && trace_refused.insert(n_idx) {
                    eprintln!("    TRACE refused (no fuel) {} at {:.0} ly from goal, d {d:.0}, fuel here {:.1}, bridges {:.0}, injected {injected}", neutrons.name(&neutrons.record(n_idx)), dist(n_pos, goal_pos), cur.fuel, edge_bridges);
                }
            }
            for (cost, fuel_after, via_scoop) in candidates {
                if cost.is_nan() {
                    continue;
                }
                // The shared incumbent (R3 Task 4): a chain that cannot beat
                // the fewest jumps any variant has finished with, even at a
                // boosted hop per reach from here, is not worth a state. A
                // twentieth of slack and two jumps keep the refinement's
                // own jumps from pruning a route that would tie.
                let n_hops = cur.hops + 1 + edge_bridges + u32::from(injected) + u32::from(via_scoop);
                let inc = incumbent.load(std::sync::atomic::Ordering::Relaxed);
                if inc != u32::MAX && n_idx != GOAL {
                    let floor = (dist(n_pos, goal_pos) / (full_range * neutron_boost)).ceil() as u32;
                    if n_hops + floor > inc + inc / 20 + 2 {
                        trace_counts[2] += 1;
                        continue;
                    }
                }
                let ng = cur.g + cost;
                let nk = key(n_idx, fuel_after);
                if best_g.get(&nk).is_some_and(|&bg| bg <= ng) {
                    trace_counts[2] += 1;
                    if would_improve && trace_refused.insert(n_idx) {
                        eprintln!("    TRACE refused (beaten at the state) {} at {:.0} ly from goal, d {d:.0}, g {ng:.1} vs {:.1}", neutrons.name(&neutrons.record(n_idx)), dist(n_pos, goal_pos), best_g[&nk]);
                    }
                    continue;
                }
                if n_idx != GOAL {
                    let e = pareto.entry(n_idx).or_default();
                    if e.iter().any(|&(c0, f0)| c0 <= ng && f0 >= fuel_after - 0.5) {
                        trace_counts[3] += 1;
                        if would_improve && trace_refused.insert(n_idx) {
                            eprintln!("    TRACE refused (pareto) {} at {:.0} ly from goal, d {d:.0}, g {ng:.1} fuel {fuel_after:.1}", neutrons.name(&neutrons.record(n_idx)), dist(n_pos, goal_pos));
                        }
                        continue;
                    }
                    e.retain(|&(c0, f0)| !(ng <= c0 && fuel_after >= f0));
                    e.push((ng, fuel_after));
                }
                best_g.insert(nk, ng);
                parent.insert(nk, (cur_key, via_scoop));
                open.push(Open { f: ng + h(n_pos), g: ng, idx: n_idx, fuel: fuel_after, inj: cur.inj + (injected && !via_scoop) as u32, hops: n_hops });
                if n_idx == GOAL {
                    goal_seen = Some(match goal_seen {
                        Some((g0, k0, s0)) if g0 <= ng => (g0, k0, s0),
                        Some((_, _, s0)) => (ng, nk, s0),
                        None => (ng, nk, expansions),
                    });
                }
                any = true;
            }
            any
        };

        // Straight to the goal when one edge covers it; the last edge may be
        // a long ordinary run (cost: one jump per full-tank range, scooping
        // on the way is the refiner's business).
        if remaining <= direct_radius + full_range * goal_bridge as f32 {
            relax(GOAL, goal_pos, remaining, &mut best_g, &mut parent, &mut open, &mut pareto, goal_bridge);
        }

        // Neutrons reachable in one boosted jump first; widen to bridging
        // distance only when there are none, so the plan prefers pure
        // highway hops (and the refiner gets one-jump legs).
        let mut pushed = 0usize;
        let corridor = remaining + full_range * 2.0;
        // Only cells that can get closer to the goal than we are now (plus
        // slack for the corridor): the far half of the sphere is never taken.
        let toward = Some((goal_pos, remaining + full_range * 2.0));
        // The greedy cone (item 13): engaged only while the last full
        // scan proved the shelf full. Walk the cells of a goalward cone
        // at the outer reach band, widen the half-angle before giving
        // up; the pick is the same key the full scan sorts by. Best-
        // effort by construction -- a miss just runs the full scan.
        let mut greedy_hit = false;
        if greedy && last_scan_count >= greedy_floor && remaining > direct_radius {
            let dirv = [
                (goal_pos[0] - here[0]) / remaining,
                (goal_pos[1] - here[1]) / remaining,
                (goal_pos[2] - here[2]) / remaining,
            ];
            let cell = neutrons.cell_ly;
            greedy_cands.clear();
            for &theta_deg in cone_angles.iter() {
                let (sin_t, cos_t) = theta_deg.to_radians().sin_cos();
                let (r_lo, r_hi) = (direct_radius * cone_band_lo, direct_radius);
                let center = [
                    here[0] + dirv[0] * r_hi * 0.9,
                    here[1] + dirv[1] * r_hi * 0.9,
                    here[2] + dirv[2] * r_hi * 0.9,
                ];
                let half = r_hi * (0.1 + sin_t) + cell;
                let c = |v: f32| (v / cell).floor() as i32;
                for cx in c(center[0] - half)..=c(center[0] + half) {
                    for cy in c(center[1] - half)..=c(center[1] + half) {
                        for cz in c(center[2] - half)..=c(center[2] + half) {
                            // cell_range is (first record, COUNT)
                            let Some((s0, n)) = neutrons.cell_range(cx, cy, cz) else { continue };
                            for n_idx in s0..s0 + n {
                                greedy_scanned += 1;
                                if cur.idx == n_idx || !highway_ok(n_idx) {
                                    continue;
                                }
                                let n_pos = neutrons.pos_of(n_idx);
                                let d = dist(here, n_pos);
                                if d < r_lo || d > r_hi {
                                    continue;
                                }
                                let along = (n_pos[0] - here[0]) * dirv[0]
                                    + (n_pos[1] - here[1]) * dirv[1]
                                    + (n_pos[2] - here[2]) * dirv[2];
                                if along < d * cos_t {
                                    continue;
                                }
                                let to_goal = dist(n_pos, goal_pos);
                                if to_goal > corridor {
                                    continue;
                                }
                                let refuels = !lean_scan && refills_on_arrival(neutrons, n_idx);
                                let bonus = if refuels {
                                    let full = full_range * boost * refuel_k;
                                    if through_stop(neutrons, n_pos, goal_pos, full_range * boost) { full } else { full * deadend_f }
                                } else {
                                    0.0
                                };
                                greedy_cands.push((to_goal - bonus, n_idx, n_pos, d));
                            }
                        }
                    }
                }
                if !greedy_cands.is_empty() {
                    break;
                }
            }
            if greedy_cands.len() > cone_cap {
                greedy_cands.select_nth_unstable_by(cone_cap, |a, b| a.0.total_cmp(&b.0));
                greedy_cands.truncate(cone_cap);
            }
            for &(_, n_idx, n_pos, d) in greedy_cands.iter() {
                relaxed += 1;
                if relax(n_idx, n_pos, d, &mut best_g, &mut parent, &mut open, &mut pareto, MAX_BRIDGE_JUMPS) {
                    pushed += 1;
                }
            }
            greedy_hit = pushed > 0;
            greedy_exps += u64::from(greedy_hit);
            if timers {
                t_scan += t_mark.elapsed();
                t_mark = std::time::Instant::now();
            }
        }
        // 18d: an UNCOVERED cell (thin, chain-floor-excluded) prices as
        // infinity, so any candidate stepping onto the covered component
        // counts as field progress and the widen below can guide a
        // wandering frontier back to the highway (measured: the Spase ->
        // Colonia return stalled in field-less cells with the widen
        // structurally unable to fire -- `field None` at every stall).
        let hf_eff = match (field, here_field) {
            (Some(_), Some(hf)) => Some(hf),
            (Some(_), None) => Some(f32::INFINITY),
            (None, _) => None,
        };
        let mut field_advances = greedy_hit || hf_eff.is_none();
        if !greedy_hit {
        cands.clear();
        if timers { t_pop += t_mark.elapsed(); t_mark = std::time::Instant::now(); }
        let scanned_before = scanned;
        neutrons.for_each_within_toward(here, direct_radius + full_range, toward, |n_idx, d| {
            scanned += 1;
            if cur.idx == n_idx || d < full_range * 0.5 || !highway_ok(n_idx) {
                return;
            }
            let n_pos = neutrons.pos_of(n_idx);
            let to_goal = dist(n_pos, goal_pos);
            if to_goal > corridor {
                return;
            }
            // Refuelling neutrons rank first: they are the highway's fuel
            // stops. A min-fuel plan wants pure progress instead -- it
            // will not stop unless the tank makes it.
            let refuels = !lean_scan && refills_on_arrival(neutrons, n_idx);
            let bonus = if refuels {
                let full = full_range * boost * refuel_k;
                if through_stop(neutrons, n_pos, goal_pos, full_range * boost) { full } else { full * deadend_f }
            } else {
                0.0
            };
            cands.push((to_goal - bonus, n_idx, n_pos, d));
        });
        last_scan_count = scanned - scanned_before;
        scan_cands = cands.len();
        if timers { t_scan += t_mark.elapsed(); t_mark = std::time::Instant::now(); }
        // Thin clusters: of the neutrons sharing a cell, only the one that
        // makes the most progress gets an edge (a refuelling one always
        // does). Neighbours a few ly apart are the same choice to the
        // coarse plan, and in the core they are most of the candidates.
        if sector_keep > 0 && cands.len() > 64 {
            thin_by_sector(&mut cands, here, &|idx: u32| !lean_scan && refills_on_arrival(neutrons, idx), sector_keep);
        } else if bucket_ly > 0.0 && cands.len() > 64 {
            bucket.clear();
            let mut keep: Vec<(f32, u32, [f32; 3], f32)> = Vec::with_capacity(cands.len() / 4);
            for &c in cands.iter() {
                let refuels = !lean_scan && refills_on_arrival(neutrons, c.1);
                if refuels {
                    keep.push(c);
                    continue;
                }
                let cell = |v: f32| (v / bucket_ly).floor() as i64 as u64 & 0x1f_ffff;
                let k = (cell(c.2[0]) << 42) | (cell(c.2[1]) << 21) | cell(c.2[2]);
                // Per cell: the most forward candidate and the nearest one
                // (least fuel burnt getting there) -- both are real choices.
                match bucket.get(&k) {
                    Some(&i) => {
                        let (fwd, near) = (i, i + 1);
                        if c.0 < keep[fwd].0 {
                            keep[fwd] = c;
                        }
                        if c.3 < keep[near].3 {
                            keep[near] = c;
                        }
                    }
                    None => {
                        bucket.insert(k, keep.len());
                        keep.push(c);
                        keep.push(c);
                    }
                }
            }
            keep.dedup_by_key(|c| c.1);
            std::mem::swap(&mut cands, &mut keep);
        }
        if cands.len() > fanout {
            cands.select_nth_unstable_by(fanout, |a, b| a.0.total_cmp(&b.0));
            cands.truncate(fanout);
        }
        if timers { t_thin += t_mark.elapsed(); t_mark = std::time::Instant::now(); }
        for &(_, n_idx, n_pos, d) in cands.iter() {
            relaxed += 1;
            if relax(n_idx, n_pos, d, &mut best_g, &mut parent, &mut open, &mut pareto, MAX_BRIDGE_JUMPS) {
                pushed += 1;
                // Does anything we pushed actually advance the field?
                // At a desert shore the scan is full of thin stars that
                // relax fine but lead nowhere -- the widen trigger below
                // must see through them.
                if !field_advances {
                    if let (Some(f), Some(hf)) = (field, hf_eff) {
                        let cf = cell_of_cached(n_pos).and_then(|c| f.jumps_to_goal(c));
                        if cf.is_some_and(|cf| cf < hf - 1.0 / 64.0) {
                            field_advances = true;
                        }
                    }
                }
            }
        }
        if timers { t_relax += t_mark.elapsed(); t_mark = std::time::Instant::now(); }
        } // !greedy_hit: the full scan/thin/relax machinery
        let mut widened: u64 = 0;
        // A void is a scan that found NOTHING; an expansion whose candidates
        // were all already known is not one. Until 2026-10-17 the rule was
        // `pushed == 0` alone, and in dense regions half of all expansions
        // paid the 3x-radius rescan for dominated neighbours (R0 timers:
        // 22,700 of 44,000 pops on the rim-to-rim pair, 6.3 of 9 s).
        let widen_cell = {
            let q = |v: f32| (v / NEUTRON_CELL_LY).floor() as i64 as u64 & 0x1f_ffff;
            (q(here[0]) << 42) | (q(here[1]) << 21) | q(here[2])
        };
        let cell_widened_before = scan_cands > 0 && !widened_cells.insert(widen_cell);
        if widen_due(pushed as u64, scan_cands, cell_widened_before, widen_once_per_cell) {
            // Nothing in the highway's reach: widen to a longer ordinary run
            // (rim of the galaxy) rather than back the search up. In thin
            // topology these rescans dominate coarse time (70% on the
            // synthetic void galaxy); the fix that measured was the morton
            // walk inside for_each_within_toward. Measured and rejected
            // here: an aggregate-oracle "any star in the sphere" pre-check
            // -- a widen that pushes nothing still SEES candidates (they
            // fail highway_ok, the corridor bound or relax), so the sphere
            // is almost never dark and the check was pure overhead.
            let far = if cur.idx == START { direct_radius + full_range * MAX_END_BRIDGE_JUMPS as f32 } else { direct_radius + full_range * far_bridges as f32 };
            neutrons.for_each_within_toward(here, far, toward, |n_idx, d| {
                if cur.idx == n_idx || d <= direct_radius + full_range || !highway_ok(n_idx) {
                    return;
                }
                let n_pos = neutrons.pos_of(n_idx);
                if dist(n_pos, goal_pos) > corridor {
                    return;
                }
                relax(n_idx, n_pos, d, &mut best_g, &mut parent, &mut open, &mut pareto, if cur.idx == START { MAX_END_BRIDGE_JUMPS } else { far_bridges });
            });
            widened += 1;
        }
        // Field widen (item 16): the scan pushed something, but nothing
        // that advances the goal field -- a desert shore full of thin
        // stars that lead nowhere. Follow the cell graph's own edges out
        // of this cell (the exact crossings the field priced, up to
        // R_GAP) toward lower field values, and relax their cells' stars
        // with a bridge allowance matched to each gap. This is what lets
        // the coarse REALIZE a crossing the field promised: the sphere
        // widen above tops out around 1.3 kly. Fuel does the final say --
        // a crossing the tank cannot fund is refused by relax as ever.
        if !field_advances && pushed > 0 {
            if let (Some(cg), Some(hc), Some(hf), Some(f)) = (cell_graph, here_cell, hf_eff, field) {
                fwidens += 1;
                for &(ncell, _) in cg.neighbours(hc) {
                    let nc = ncell as usize;
                    if f.jumps_to_goal(nc).is_none_or(|nf| nf >= hf) {
                        continue;
                    }
                    let Some((s0, n)) = neutrons.cell_records(nc) else { continue };
                    for n_idx in s0..s0 + n {
                        if cur.idx == n_idx || !highway_ok(n_idx) {
                            continue;
                        }
                        let n_pos = neutrons.pos_of(n_idx);
                        let d = dist(here, n_pos);
                        if d <= direct_radius + full_range {
                            continue; // the ordinary scan already offered it
                        }
                        if dist(n_pos, goal_pos) > corridor {
                            continue;
                        }
                        let bridge = (((d - direct_radius) / full_range).ceil() as u32).saturating_add(1);
                        fw_relaxed += 1;
                        fw_pushed += u64::from(relax(n_idx, n_pos, d, &mut best_g, &mut parent, &mut open, &mut pareto, bridge));
                    }
                }
                widened += 1;
            }
        }
        if timers && widened > 0 { t_widen += t_mark.elapsed(); t_mark = std::time::Instant::now(); }
        widens += widened;
    }
    stats.record(expansions, stall_at, widens);
    LAST_WIDENS.with(|c| c.set(widens));
    let Some(gk) = goal_key else { return Err(RouteError::NoRoute) };
    if std::env::var_os("ED_PLOT_DEBUG").is_some() {
        eprintln!("    coarse: {expansions} expansions, {} states, {} ms; {scanned} neutrons scanned, {relaxed} relaxed ({:.0}/{:.0} per expansion)", best_g.len(), started.elapsed().as_millis(), scanned as f64 / expansions.max(1) as f64, relaxed as f64 / expansions.max(1) as f64);
        if greedy {
            eprintln!("    greedy cone: {greedy_exps}/{expansions} expansions greedy, {greedy_scanned} cone-scanned");
        }
        if timers {
            eprintln!("    coarse timers: scan {:.1} ms, thin+select {:.1} ms, relax {:.1} ms, pop+overhead {:.1} ms, widen {:.1} ms ({widens} widens); pops {pops} ({stale_pops} stale), max open {max_open}", t_scan.as_secs_f64() * 1e3, t_thin.as_secs_f64() * 1e3, t_relax.as_secs_f64() * 1e3, t_pop.as_secs_f64() * 1e3, t_widen.as_secs_f64() * 1e3);
        }
    }

    // Coarse chain of states, start .. goal.
    let mut chain = vec![gk];
    let mut k = gk;
    while let Some(&(p, _)) = parent.get(&k) {
        chain.push(p);
        k = p;
    }
    chain.reverse();

    let full_idx = |state: u64| -> Option<u32> {
        match (state >> 8) as u32 {
            START => Some(req.from),
            GOAL => match gateway {
                Some(n) => g.find(neutrons.name(&neutrons.record(n))),
                None => Some(req.to),
            },
            i => g.find(neutrons.name(&neutrons.record(i))),
        }
    };
    let fuel_of = |state: u64| -> f32 {
        match fuel_model {
            Some(m) => (state & 0xff) as f32 / steps * m.capacity,
            None => 0.0,
        }
    };
    let mut waypoints: Vec<(u32, f32)> = chain.iter().filter_map(|&s| full_idx(s).map(|i| (i, fuel_of(s)))).collect();
    // The sub-index star behind each waypoint, for banning a dead edge.
    let mut wp_sub: Vec<Option<u32>> = chain.iter().filter_map(|&s| full_idx(s).map(|_| Some((s >> 8) as u32))).collect();
    let stalled = goal_key != Some(key(GOAL, 0.0)) && (gk >> 8) as u32 != GOAL;
    stats.coarse_done.store(!stalled, std::sync::atomic::Ordering::Relaxed);
    if stalled {
        // The chain ends at the closest neutron reached; the rest is exact
        // -- when the rest is a leg the exact planner can take. A stall
        // tens of thousands of ly out is not that: on the rim-to-rim
        // stress route (2026-10-17) every variant capped out 45,000 ly
        // short, the exact planner was handed 45,000 ly, never returned,
        // and the x4 wave that exists for stalls never started (no route
        // in 600 s on 24 threads). Past this many boosted reaches the
        // variant fails fast instead, so the wave ends and the next one
        // gets the allowance. ED_STALL_HANDOFF_REACHES tunes it.
        let end = waypoints.last().map(|w| w.0).unwrap_or(req.from);
        let remaining = dist(g.pos_of(end), true_goal_pos);
        let handoff_reaches: f32 = std::env::var("ED_STALL_HANDOFF_REACHES").ok().and_then(|v| v.parse().ok()).unwrap_or(if thin { THIN_HANDOFF_REACHES } else { STALL_HANDOFF_REACHES });
        if remaining > full_range * neutron_boost * handoff_reaches {
            if std::env::var_os("ED_PLOT_DEBUG").is_some() {
                eprintln!("    coarse stall {remaining:.0} ly out exceeds the exact hand-off ({handoff_reaches} reaches): variant fails fast");
            }
            return Err(RouteError::NoRoute);
        }
        // (Inserting the gateway neutron here was measured negative on
        // 2026-10-17: the long hand-off leg arrives dry and the approach
        // leg then fails from 0 t -- `docs/benches/2026-10-17-rim-to-rim-stress.csv`.)
        waypoints.push((req.to, 0.0));
        wp_sub.push(None);
    } else if gateway.is_some() {
        // Final approach: gateway -> destination, planned like any other leg
        // (the exact planner handles the long ordinary run and its scoops).
        waypoints.push((req.to, 0.0));
        wp_sub.push(None);
    }
    let mut failed_leg = None;
    let refined = refine_waypoints_reporting(g, req, ctl, waypoints, start_fuel, fuel_model, expansions, straight, started, &mut failed_leg);
    if let (Err(_), Some(i)) = (&refined, failed_leg) {
        // LazySP (R3 Task 3): the coarse edge whose leg could not be flown
        // is banned for the plot; the caller re-plans around it.
        if let (Some(Some(a)), Some(Some(b))) = (wp_sub.get(i), wp_sub.get(i + 1)) {
            // The sentinels are real coarse nodes: an edge INTO the goal or
            // OUT of the start can be the dead one.
            if *a != GOAL && *b != START {
                banned.lock().unwrap_or_else(|e| e.into_inner()).push((*a, *b));
                if std::env::var_os("ED_PLOT_DEBUG").is_some() {
                    let label = |x: u32| if x == GOAL { "goal".to_string() } else if x == START { "start".to_string() } else { neutrons.name(&neutrons.record(x)).to_string() };
                    eprintln!("    banned coarse edge {} -> {} (leg {i} could not be flown); the variant re-plans", label(*a), label(*b));
                }
            }
        }
    }
    refined
}

/// Refine a waypoint chain into a flyable route: plan every leg with the
/// fuel-aware weighted planner, settle fuel leg to leg, stitch. Shared
/// by the neutron-first coarse search and the cell-graph-first planner —
/// however the chain was found, this is how it is flown. `doc(hidden)`
/// pub so benches can refine externally-derived chains (the reverse-
/// planning experiment); not a stable API.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn refine_waypoints(
    g: &Galaxy,
    req: &RouteRequest,
    ctl: &Control,
    waypoints: Vec<(u32, f32)>,
    start_fuel: f32,
    fuel_model: Option<crate::fuel::FuelModel>,
    coarse_expansions: u64,
    straight: f32,
    started: std::time::Instant,
) -> Result<Route, RouteError> {
    let mut failed = None;
    refine_waypoints_reporting(g, req, ctl, waypoints, start_fuel, fuel_model, coarse_expansions, straight, started, &mut failed)
}

/// `refine_waypoints` that also names the first leg that could not be
/// planned (R3 Task 3): the caller bans that coarse edge and re-plans. The
/// first pass stops its sibling legs on the first failure instead of
/// finishing every one of them first.
#[allow(clippy::too_many_arguments)]
pub fn refine_waypoints_reporting(
    g: &Galaxy,
    req: &RouteRequest,
    ctl: &Control,
    waypoints: Vec<(u32, f32)>,
    start_fuel: f32,
    fuel_model: Option<crate::fuel::FuelModel>,
    coarse_expansions: u64,
    straight: f32,
    started: std::time::Instant,
    failed_leg: &mut Option<usize>,
) -> Result<Route, RouteError> {
    let debug = std::env::var_os("ED_PLOT_DEBUG").is_some();
    let legs = waypoints.len().saturating_sub(1) as u32;
    for (i, (idx, _)) in waypoints.iter().enumerate() {
        (ctl.trace)("chain", g.pos_of(*idx), i as f32);
    }

    // Refine every leg with the fuel-aware weighted planner, all legs at
    // once: each starts from the coarse plan's fuel estimate for its
    // waypoint. The estimates are then checked in order against what the
    // previous leg actually left in the tank; a leg that would start with
    // less than it assumed is re-planned from the real figure (rare: the
    // coarse plan is conservative), so the stitched route is exact.
    use rayon::prelude::*;
    let done = std::sync::atomic::AtomicU32::new(0);
    (ctl.stage)("refine", 0, legs);
    let leg_of = |from: u32, to: u32, start_fuel: f32| RouteRequest { from, to, weight: LEG_WEIGHT, thorough: false, max_expansions: leg_expansion_cap(dist(g.pos_of(from), g.pos_of(to))), time_budget_ms: leg_time_budget_ms(req.time_budget_ms), start_fuel, ..req.clone() };
    let windows: Vec<[(u32, f32); 2]> = waypoints.windows(2).map(|w| [w[0], w[1]]).collect();
    let plan_legs = |jobs: &[(usize, f32)], use_memo: bool| -> Vec<(usize, Result<Route, RouteError>)> {
        // The first failure stops the siblings: a dead leg's 25 s is paid
        // once, not once per leg still queued behind it.
        let abort = std::sync::atomic::AtomicBool::new(false);
        // Hundreds of one-hop legs (the witness) go to the pool in a few
        // chunks, not one task each: a thousand tiny tasks flooded the
        // box's three workers (2026-10-18, arm w4-3).
        let min_len = if jobs.len() > 200 { jobs.len() / 4 } else { 1 };
        jobs.par_iter()
            .with_min_len(min_len)
            .map(|&(i, fuel)| {
                if abort.load(std::sync::atomic::Ordering::Relaxed) {
                    return (i, Err(RouteError::Cancelled));
                }
                let t = std::time::Instant::now();
                let r = refine_leg(g, req, ctl, &leg_of, windows[i][0].0, windows[i][1].0, fuel, use_memo);
                if r.is_err() {
                    abort.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if std::env::var_os("ED_PLOT_DEBUG").is_some() {
                    match &r {
                        Ok(r) => eprintln!("    leg {i:>2}: {} jumps {} expansions {} ms (fuel {fuel:.0})", r.jumps, r.expansions, t.elapsed().as_millis()),
                        Err(e) => eprintln!("    leg {i:>2}: FAILED {e:?} {} -> {} ({:.0} ly, fuel {fuel:.0}) {} ms", g.name(&g.record(windows[i][0].0)), g.name(&g.record(windows[i][1].0)), dist(g.pos_of(windows[i][0].0), g.pos_of(windows[i][1].0)), t.elapsed().as_millis()),
                    }
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                (ctl.stage)("refine", n.min(legs), legs);
                if let Ok(r) = &r {
                    for h in &r.hops {
                        (ctl.trace)("leg", h.pos, i as f32);
                    }
                }
                (i, r)
            })
            .collect()
    };
    let mut assumed: Vec<f32> = windows.iter().enumerate().map(|(i, w)| if i == 0 { start_fuel } else { w[0].1 }).collect();
    let mut planned: Vec<Option<Route>> = vec![None; windows.len()];
    let first_pass = plan_legs(&windows.iter().enumerate().map(|(i, _)| (i, assumed[i])).collect::<Vec<_>>(), true);
    // The first REAL failure names the leg; the siblings it aborted report
    // Cancelled and are not it.
    if let Some((i, e)) = first_pass.iter().find_map(|(i, r)| match r {
        Err(e) if !matches!(e, RouteError::Cancelled) => Some((*i, e.clone_err())),
        _ => None,
    }) {
        *failed_leg = Some(i);
        return Err(e);
    }
    for (i, r) in first_pass {
        planned[i] = Some(r?);
    }
    if debug {
        eprintln!("    refine: {legs} legs planned at {} ms", started.elapsed().as_millis());
    }
    // Fly the chain with real fuel. A leg planned from a tank estimate more
    // than a few tonnes off the truth (or that is no longer feasible) is
    // re-planned from the real figure, all such legs at once; a couple of
    // rounds settle it.
    if let Some(m) = fuel_model {
        for round in 0..8 {
            if debug {
                eprintln!("    refine: settle round {round} at {} ms", started.elapsed().as_millis());
            }
            let mut f = start_fuel;
            let mut redo: Vec<(usize, f32)> = Vec::new();
            for i in 0..windows.len() {
                let leg = planned[i].as_mut().unwrap();
                let ok = refuel_hops(g, &m, req, leg, f);
                if !ok || (f - assumed[i]).abs() > 8.0 {
                    redo.push((i, f));
                    assumed[i] = f;
                }
                if let Some(last) = leg.hops.last() {
                    if let Some(x) = last.fuel_after {
                        f = x;
                    }
                }
            }
            if redo.is_empty() {
                break;
            }
            if debug {
                eprintln!("    fuel round {round}: {} legs re-planned at {} ms", redo.len(), started.elapsed().as_millis());
            }
            for (i, r) in plan_legs(&redo, false) {
                match r {
                    Ok(l) => planned[i] = Some(l),
                    // A leg that cannot be re-planned from the real tank
                    // (a fuel-blind coarse chain can pin consecutive
                    // max-reach hops with no scoop in reach -- measured:
                    // bidi's Beagle chain re-planning a 425 ly boosted
                    // hop from 5 t) is NOT fatal here: keep the old leg
                    // and let the final pass merge it with its
                    // predecessor, whose exact planner finds the scoop.
                    Err(RouteError::Cancelled) => return Err(RouteError::Cancelled),
                    Err(_) => {}
                }
            }
        }
        // Final pass: consistent fuel figures along the whole route; a leg
        // that still does not fly is re-planned in order from the real tank.
        if debug {
            eprintln!("    refine: final pass at {} ms", started.elapsed().as_millis());
        }
        let mut f = start_fuel;
        let mut prev_start = start_fuel;
        // Every leg was planned with the whole injection allowance; along
        // the route each leg gets what the ones before left.
        let mut inj_left: u32 = req.injection.map(|(_, _, n)| n).unwrap_or(0);
        for i in 0..windows.len() {
            let Some(leg) = planned[i].as_mut() else { continue };
            let leg_start = f;
            let used = leg.hops.iter().filter(|h| h.injection.is_some()).count() as u32;
            if used > inj_left {
                let allowed = req.injection.filter(|_| inj_left > 0).map(|(m, n, _)| (m, n, inj_left));
                if debug {
                    eprintln!("    final pass: leg {i} uses {used} injections, {inj_left} left; re-planning");
                }
                let fresh = plan(g, &RouteRequest { injection: allowed, ..leg_of(windows[i][0].0, windows[i][1].0, f) }, ctl)?;
                *leg = fresh;
            }
            inj_left = inj_left.saturating_sub(leg.hops.iter().filter(|h| h.injection.is_some()).count() as u32);
            if !refuel_hops(g, &m, req, leg, f) {
                if debug {
                    eprintln!("    refine: final pass re-plans leg {i} ({} -> {}, {:.0} ly, fuel {f:.0}) at {} ms", g.name(&g.record(windows[i][0].0)), g.name(&g.record(windows[i][1].0)), dist(g.pos_of(windows[i][0].0), g.pos_of(windows[i][1].0)), started.elapsed().as_millis());
                }
                let fresh = match refine_leg(g, req, ctl, &leg_of, windows[i][0].0, windows[i][1].0, f, false) {
                    Ok(l) => l,
                    Err(_) if i > 0 && planned[i - 1].is_some() => {
                        // Cannot be flown from what the previous leg left: merge
                        // the two legs and let the exact planner find a scoop.
                        let f0 = prev_start;
                        let merged = plan(g, &leg_of(windows[i - 1][0].0, windows[i][1].0, f0), ctl)?;
                        if debug {
                            eprintln!("    final pass: legs {} and {i} merged ({} jumps)", i - 1, merged.jumps);
                        }
                        planned[i - 1] = Some(merged);
                        planned[i] = None;
                        let mut ff = f0;
                        if let Some(l) = planned[i - 1].as_mut() {
                            refuel_hops(g, &m, req, l, ff);
                            if let Some(x) = l.hops.last().and_then(|h| h.fuel_after) {
                                ff = x;
                            }
                        }
                        f = ff;
                        continue;
                    }
                    Err(e) => {
                        if debug {
                            eprintln!("    final pass: leg {i} {} -> {} ({:.0} ly) could not be re-planned from {f:.0} t: {e:?}", g.name(&g.record(windows[i][0].0)), g.name(&g.record(windows[i][1].0)), dist(g.pos_of(windows[i][0].0), g.pos_of(windows[i][1].0)));
                        }
                        return Err(e);
                    }
                };
                let planned_hops: Vec<String> = fresh.hops.iter().map(|h| format!("{} {:.1}ly b={} sc={} f={:?} cls={:?}", h.name, h.distance_ly, h.boosted, h.scoopable, h.fuel_after, h.class)).collect();
                *leg = fresh;
                if !refuel_hops(g, &m, req, leg, f) {
                    if std::env::var_os("ED_PLOT_DEBUG").is_some() {
                        eprintln!("    final pass: leg {i} does not fly from {f:.0} t; planned: {}", planned_hops.join(" | "));
                    }
                    return Err(RouteError::NoRoute);
                }
            }
            if let Some(x) = leg.hops.last().and_then(|h| h.fuel_after) {
                f = x;
            }
            prev_start = leg_start;
        }
    }
    if debug {
        eprintln!("    refine: stitch at {} ms", started.elapsed().as_millis());
    }
    let mut route: Option<Route> = None;
    let mut total_expansions = coarse_expansions;
    for leg in planned.into_iter().flatten() {
        total_expansions += leg.expansions;
        route = Some(match route {
            None => leg,
            Some(mut acc) => {
                let offset = acc.total_ly;
                for (j, mut h) in leg.hops.into_iter().enumerate() {
                    if j == 0 {
                        continue; // same system as the previous leg's last hop
                    }
                    h.total_ly += offset;
                    acc.hops.push(h);
                }
                acc.total_ly += leg.total_ly;
                acc.boosted_jumps += leg.boosted_jumps;
                acc.refuel_stops += leg.refuel_stops;
                acc.jumps = acc.hops.len().saturating_sub(1);
                acc
            }
        });
    }
    let mut route = route.ok_or(RouteError::NoRoute)?;
    route.straight_ly = straight;
    route.injections = route.hops.iter().filter(|h| h.injection.is_some()).count();
    route.expansions = total_expansions;
    route.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(route)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::router::NO_LEG_MEMO;
    use crate::fuel::{BoostProfile, FuelModel};
    use crate::import::{import_reader, subset_cells};
    use crate::StarClass;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    pub(crate) const K: &str = "K (Yellow-Orange) Star";
    const L: &str = "L (Brown dwarf) Star";
    pub(crate) const N: &str = "Neutron Star";
    const WD: &str = "White Dwarf (DA) Star";

    /// A synthetic index plus its neutron sub-index, written through the
    /// crate's own importer so the tests exercise the real on-disk path.
    pub(crate) struct Field {
        pub(crate) _dir: tempfile::TempDir,
        pub(crate) g: Galaxy,
        pub(crate) neutrons: Galaxy,
    }

    pub(crate) fn build(stars: &[(String, f32, f32, f32, &str)]) -> Field {
        build_with(stars, highway_star)
    }

    fn build_with(stars: &[(String, f32, f32, f32, &str)], highway: impl Fn(StarClass) -> bool) -> Field {
        let mut lines = vec!["[".to_string()];
        for (i, (name, x, y, z, sub)) in stars.iter().enumerate() {
            lines.push(format!(
                r#"{{"id64":{},"name":"{name}","coords":{{"x":{x},"y":{y},"z":{z}}},"bodies":[{{"type":"Star","subType":"{sub}","mainStar":true}}]}},"#,
                i + 1
            ));
        }
        lines.push("]".into());
        let dir = tempfile::tempdir().unwrap();
        import_reader(Box::new(std::io::Cursor::new(lines.join("\n").into_bytes())), dir.path(), &mut |_| {}).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let ndir = neutron_dir(dir.path());
        subset_cells(&g, &ndir, NEUTRON_CELL_LY, |r| highway(StarClass::from_code(r.class))).unwrap();
        let neutrons = Galaxy::open(&ndir).unwrap();
        Field { _dir: dir, g, neutrons }
    }

    /// Item 34 rung 1: the crossing re-plan pass. A route whose desert
    /// crossing was planned at wet-tank reach (37.5 ly steps) gets its
    /// plain run re-planned from the LEAN post-rewrite fuel state --
    /// where the longer 50 ly lane is suddenly legal -- and the spliced
    /// route wins the judge with one fewer jump. Cross dry, then drink.
    #[test]
    fn the_crossing_replan_recrosses_lean_and_wins() {
        // Toy drive with a violent wet/dry contrast: 100 t hull, 100 t
        // tank. Wet reach ~40.2 ly; at the 45 t lean entry, ~52.6 ly.
        let m = crate::fuel::FuelModel::from_loadout(100.0, 100.0, 8.0, 5, false, false, 75.0, 0.0, 0.0);
        let f = build(&[
            ("Entry".into(), 0.0, 0.0, 0.0, "K (Yellow-Orange) Star"),
            // The wet chain the eager plan used (unscoopable desert).
            ("W1".into(), 37.5, 0.0, 0.0, "L (Brown dwarf) Star"),
            ("W2".into(), 75.0, 0.0, 0.0, "L (Brown dwarf) Star"),
            ("W3".into(), 112.5, 0.0, 0.0, "L (Brown dwarf) Star"),
            // The lean lane only a light ship can ride.
            ("L1".into(), 50.0, 0.0, 0.0, "L (Brown dwarf) Star"),
            ("L2".into(), 100.0, 0.0, 0.0, "L (Brown dwarf) Star"),
            ("Shore".into(), 150.0, 0.0, 0.0, "K (Yellow-Orange) Star"),
        ]);
        let idx = |name: &str| f.g.find(name).unwrap();
        let hop = |name: &str, d: f32, fuel: Option<f32>| {
            let i = idx(name);
            let r = f.g.record(i);
            crate::router::Hop {
                idx: i, id64: r.id64, name: name.into(), pos: r.pos(),
                class: f.g.class(&r), scoopable: f.g.class(&r).scoopable(),
                distance_ly: d, boosted: false, total_ly: 0.0,
                fuel_after: fuel, refuel: false, fuel_optional: false, injection: None,
                synthesized: false,
                via_secondary: None,
            }
        };
        let hops = vec![
            hop("Entry", 0.0, Some(45.0)),
            hop("W1", 37.5, Some(39.0)),
            hop("W2", 37.5, Some(33.5)),
            hop("W3", 37.5, Some(28.5)),
            hop("Shore", 37.5, Some(24.0)),
        ];
        let mut route = crate::router::Route {
            highway_pending: false,
            range_ly: m.range_at(m.capacity), hops, jumps: 4, total_ly: 150.0,
            straight_ly: 150.0, boosted_jumps: 0, expansions: 0, elapsed_ms: 0,
            refuel_stops: 0, injections: 0, secondary_boosts: 0, ship_id: None, ship: None,
            variants_run: 0, variants_finished: 0, ship_has_scoop: None,
            fsd_integrity: None, integrity_loss_per_boost: None, ship_has_afmu: None,
        };
        let req = RouteRequest {
            from: idx("Entry"), to: idx("Shore"), range_ly: m.range_at(m.capacity),
            thorough: true, fuel: Some(m), start_fuel: 45.0, min_fuel: true,
            ..Default::default()
        };
        let improved = crossing_replan(&f.g, Some(&f.neutrons), &req, &crate::router::Control::none(), &mut route);
        assert!(improved, "the lean re-plan must beat the wet crossing");
        assert_eq!(route.jumps, 3, "50 ly lane: Entry -> L1 -> L2 -> Shore");
        assert!(route.hops.iter().any(|h| h.name == "L1"), "rides the lean lane");
        assert!(route.hops.iter().all(|h| h.fuel_after.is_some()), "ledger re-fit after the splice");
    }

    /// Item 17: route quality is pilot time, not a lexicographic
    /// (jumps, stops) tuple. The commander's cockpit numbers: a jump is
    /// ~40 s, a fuel stop is 1-2 minutes -- so a stop prices at two
    /// jumps, and 57 jumps / 10 stops LOSES to 58 jumps / 2 stops.
    #[test]
    fn a_stop_heavy_route_loses_to_a_slightly_longer_clean_one() {
        // Journal-fit seconds: 30 t per stop at the 8A's 1.245 t/s.
        let r = |j: usize, st: usize| time_units(j, 0, 0, 0, st, st as f32 * 30.0, 1.245, 1.0, None, None);
        assert!(r(58, 2) < r(57, 10), "58 j / 2 stops is minutes faster in the chair");
        assert!(r(57, 2) < r(58, 2), "fewer jumps still wins at equal stops");
        assert!(r(57, 2) < r(57, 3), "fewer stops still wins at equal jumps");
        // A mid-tank top-up is cheaper than a deep fill, fitted or not:
        // the one currency (R2) prices the tonnes over the scoop rate in
        // the public model too (item 42's flat public stop is superseded).
        assert!(time_units(57, 0, 0, 0, 2, 20.0, 1.245, 1.0, None, Some(36.0)) < time_units(57, 0, 0, 0, 2, 120.0, 1.245, 1.0, None, Some(36.0)));
        assert!(time_units(57, 0, 0, 0, 2, 20.0, 1.245, 1.0, None, None) < time_units(57, 0, 0, 0, 2, 120.0, 1.245, 1.0, None, None), "the public stop prices its tonnes too");
    }

    /// R2 (2026-10-17), one currency: the judge prices a plain jump at 53 s
    /// and a boosted hop at 73 s, so a route with fewer jumps but many more
    /// supercharged hops can lose: 10 plain + 10 boosted (1,260 s) beats
    /// 4 plain + 15 boosted (1,307 s) although the second is a jump shorter.
    #[test]
    fn the_judge_prices_a_boosted_hop_above_a_plain_one() {
        let req = RouteRequest { stop_weight: 1.0, ..Default::default() };
        let a = synthetic_route(10, 10, &[]);
        let b = synthetic_route(4, 15, &[]);
        assert!(a.jumps > b.jumps);
        let (sa, sb) = (route_score_public(&a, &req), route_score_public(&b, &req));
        assert!(sa < sb, "fewer jumps is not fewer seconds: {sa} vs {sb} deciseconds");
        assert_eq!(sa, 12_600, "deciseconds: 10 x 53 + 10 x 73");
    }

    /// A full stop costs its 120 s approach plus the tonnes over the scoop
    /// rate (124.5 t at the Caspian's 1.245 t/s is 100 s), fitted or not.
    #[test]
    fn a_full_stop_costs_the_overhead_plus_the_tonnes() {
        let mut m = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
        m.scoop_rate = 1.245;
        let req = RouteRequest { stop_weight: 1.0, fuel: Some(m), ..Default::default() };
        let dry = synthetic_route(10, 0, &[]);
        let full = synthetic_route(10, 0, &[(5, 124.5, true)]);
        let diff = route_score_public(&full, &req) as i64 - route_score_public(&dry, &req) as i64;
        assert!((diff - 2_200).abs() <= 1, "deciseconds: 120 s + 100 s, got {diff}");
    }

    /// A top-up in passing (the tank rose at a star the plan did not mark
    /// as a stop) costs 6 s plus its tonnes, not a stop's 120 s.
    #[test]
    fn a_passing_scoop_costs_six_seconds_plus_its_tonnes() {
        let mut m = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
        m.scoop_rate = 1.245;
        let req = RouteRequest { stop_weight: 1.0, fuel: Some(m), ..Default::default() };
        let dry = synthetic_route(10, 0, &[]);
        let sip = synthetic_route(10, 0, &[(5, 12.45, false)]);
        let diff = route_score_public(&sip, &req) as i64 - route_score_public(&dry, &req) as i64;
        assert!((diff - 160).abs() <= 1, "deciseconds: 6 s + 10 s, got {diff}");
    }

    /// `plain` plain hops then `boosted` boosted ones from the origin, with a
    /// fuel ledger that starts full, burns 5 t a hop and, at each `(hop,
    /// tonnes, stop)` in `scoops`, shows `tonnes` more than the previous
    /// arrival (a marked stop when `stop`). The judge reads only the
    /// fields the real planner fills.
    fn synthetic_route(plain: usize, boosted: usize, scoops: &[(usize, f32, bool)]) -> Route {
        let mut hops = Vec::new();
        let mut fuel = 128.0f32;
        for i in 0..=(plain + boosted) {
            let scoop = scoops.iter().find(|s| s.0 == i);
            match scoop {
                Some(&(_, t, _)) => fuel += t,
                None if i > 0 => fuel -= 5.0,
                None => {}
            }
            hops.push(crate::router::Hop {
                idx: i as u32, id64: i as u64, name: format!("S{i}"), pos: [i as f32 * 50.0, 0.0, 0.0], class: StarClass::K, scoopable: scoop.is_some(),
                distance_ly: if i == 0 { 0.0 } else { 50.0 }, boosted: i > plain, total_ly: i as f32 * 50.0, fuel_after: Some(fuel),
                refuel: scoop.is_some_and(|s| s.2), synthesized: false, fuel_optional: false, injection: None, via_secondary: None,
            });
        }
        Route {
            range_ly: 72.0, jumps: plain + boosted, total_ly: (plain + boosted) as f32 * 50.0, straight_ly: 0.0, boosted_jumps: boosted, expansions: 0, elapsed_ms: 0,
            refuel_stops: scoops.iter().filter(|s| s.2).count(), injections: 0, secondary_boosts: 0, ship_id: None, ship: None, variants_run: 0, variants_finished: 0,
            ship_has_scoop: None, fsd_integrity: None, integrity_loss_per_boost: None, ship_has_afmu: None, highway_pending: false, hops,
        }
    }

    /// Item 29 rank-agreement study: ED_JUDGE=fuel judges on
    /// replenished tonnes alone. The crafted pair pins the exact
    /// disagreement the study exists to measure: the seconds model
    /// prefers fewer jumps at a bigger fill; the gauge prefers the
    /// smaller fill however many jumps carry it.
    #[test]
    fn the_study_fuel_judge_ranks_on_tonnes_alone() {
        assert!(fuel_units(40.0) < fuel_units(41.0));
        assert_eq!(fuel_units(40.0), fuel_units(40.0), "equal tonnes tie; callers keep the first");
        // 60 j / 90 t vs 70 j / 40 t: the seconds model takes the short route...
        assert!(time_units(60, 0, 0, 0, 3, 90.0, 1.245, 1.0, None, None) < time_units(70, 0, 0, 0, 3, 40.0, 1.245, 1.0, None, None));
        // ...the gauge takes the thrifty one. That gap IS the study.
        assert!(fuel_units(40.0) < fuel_units(90.0));
    }

    /// Item 20: the jumps-vs-refuels dial. stop_weight scales the whole
    /// refuel term; 0 judges by flying time alone (the try-hard preset
    /// flips the item-17 verdict back: fewer jumps wins no matter the
    /// stops), and a heavy weight trades jumps away for fewer stops.
    #[test]
    fn the_stop_weight_dial_moves_the_jumps_vs_refuels_tradeoff() {
        // Pinned against the FITTED decomposed model (70 s / 36 s +
        // tonnage) -- the dial's semantics predate the item-42 flat
        // unfitted default and must survive it.
        let r = |j: usize, st: usize, w: f32| time_units(j, 0, 0, 0, st, st as f32 * 30.0, 1.245, w, Some(70.0), Some(36.0));
        // Fitted model (1.0): the clean 58 j beats the stop-heavy 57 j.
        assert!(r(58, 2, 1.0) < r(57, 10, 1.0));
        // Try hard (0.0): stops are free, 57 j wins outright.
        assert!(r(57, 10, 0.0) < r(58, 2, 0.0));
        // Stop-averse (3.0): two extra stops cost ~120 fitted seconds,
        // under the 140 s of two extra jumps -- so the fitted model
        // prefers 57 j / 4 stops, and weight 3 (~360 s) flips it.
        assert!(r(59, 2, 1.0) > r(57, 4, 1.0), "fitted model prefers the shorter route");
        assert!(r(59, 2, 3.0) < r(57, 4, 3.0), "weight 3 flips it");
    }

    /// Item 28: per-commander time models ride the request. A pilot
    /// whose journals say jumps are slow (120 s) trades differently
    /// from one who chains them at 45 s — the same two routes flip.
    /// None must be byte-identical to the built-in defaults.
    #[test]
    fn per_commander_times_ride_the_request_and_none_is_the_default() {
        let r = |j: usize, st: usize, tj: Option<f32>, ov: Option<f32>| time_units(j, 0, 0, 0, st, st as f32 * 30.0, 1.245, 1.0, tj, ov);
        // None = the public model: 53 s a plain jump, 120 s a stop plus the
        // tonnes over the scoop rate (R2 one currency; item 42's flat
        // tonnage-blind 60/120 is superseded). Closed form so the pin is
        // arithmetic, not echo.
        let public = ((58.0f32 * T_PLAIN_S + 6.0 * T_STOP_S + 180.0 / 1.245) * 10.0) as i64;
        assert!((r(58, 6, None, None) as i64 - public).abs() <= 1, "None = the public model: {} vs {public}", r(58, 6, None, None));
        assert_ne!(r(58, 6, None, None), r(58, 6, Some(70.0), Some(36.0)), "the old fitted built-ins are no longer the default");
        // Slow-jumping pilot: two extra stops beat four extra jumps.
        assert!(r(58, 6, Some(120.0), None) < r(62, 4, Some(120.0), None));
        // Jump-chaining speedrunner at 45 s: the same comparison flips
        // once stops carry a real 120 s public-default overhead.
        assert!(r(58, 6, Some(45.0), Some(120.0)) > r(62, 4, Some(45.0), Some(120.0)));
    }

    /// Item 14a, the reset: a finish that strictly improves the best tuple
    /// renews the grace window, so an actively-improving wave is not
    /// guillotined mid-climb; a worse or merely-tying finish renews
    /// nothing. Pure clock arithmetic -- no threads, no sleeps.
    #[test]
    fn the_grace_clock_resets_on_improvement_and_only_on_improvement() {
        let grace = std::time::Duration::from_millis(100);
        let ms = std::time::Duration::from_millis;
        let t0 = std::time::Instant::now();
        let mut clock = GraceClock::new(None);
        // Armed by the first credible finish (to_first 100 ms -> the
        // effective window is 100 ms in both directions).
        clock.finish(t0, ms(100), time_units(100, 0, 0, 0, 10, 0.0, 1.25, 1.0, None, None), true);
        assert!(!clock.expired(t0 + ms(90), grace, None));
        assert!(clock.expired(t0 + ms(150), grace, None));
        // An improvement at t0+80 renews the window from t0+80.
        clock.finish(t0 + ms(80), ms(180), time_units(90, 0, 0, 0, 8, 0.0, 1.25, 1.0, None, None), true);
        assert!(!clock.expired(t0 + ms(150), grace, None), "the reset must outlive the original window");
        assert!(clock.expired(t0 + ms(200), grace, None));
        // A worse finish and a tie renew nothing.
        clock.finish(t0 + ms(170), ms(270), time_units(95, 0, 0, 0, 9, 0.0, 1.25, 1.0, None, None), true);
        clock.finish(t0 + ms(175), ms(275), time_units(90, 0, 0, 0, 8, 0.0, 1.25, 1.0, None, None), true);
        assert!(clock.expired(t0 + ms(200), grace, None), "neither a worse finish nor a tie may renew the window");
        // A non-arming kind (a greedy twin) that IMPROVES still renews:
        // resetting only delays cancellation, it never causes one.
        let mut clock = GraceClock::new(None);
        clock.finish(t0, ms(100), time_units(100, 0, 0, 0, 10, 0.0, 1.25, 1.0, None, None), true);
        clock.finish(t0 + ms(80), ms(180), time_units(80, 0, 0, 0, 8, 0.0, 1.25, 1.0, None, None), false);
        assert!(!clock.expired(t0 + ms(150), grace, None), "an improving greedy finish must renew the window");
    }

    /// Item 14a, the escalation seed: a wave past the first exists only to
    /// improve on the standing best, so a finish that merely re-confirms
    /// it must not arm the guillotine -- that coin-flip is exactly why
    /// Wongi -> Beagle came back 215 or 190 depending on scheduling.
    #[test]
    fn an_escalation_wave_arms_only_on_a_finish_that_beats_the_seed() {
        let grace = std::time::Duration::from_millis(100);
        let ms = std::time::Duration::from_millis;
        let t0 = std::time::Instant::now();
        let mut clock = GraceClock::new(Some(time_units(215, 0, 0, 0, 40, 0.0, 1.25, 1.0, None, None)));
        // A tie of the standing best: no clock.
        clock.finish(t0, ms(100), time_units(215, 0, 0, 0, 40, 0.0, 1.25, 1.0, None, None), true);
        assert!(!clock.expired(t0 + ms(10_000), grace, None), "a re-confirmation must not arm the clock");
        // A genuine improvement arms it.
        clock.finish(t0 + ms(200), ms(300), time_units(190, 0, 0, 0, 40, 0.0, 1.25, 1.0, None, None), true);
        assert!(clock.expired(t0 + ms(1_000), grace, None));
        // The first wave has no seed: its first credible finish arms
        // regardless of score (the status quo, unchanged).
        let mut wave_one = GraceClock::new(None);
        wave_one.finish(t0, ms(100), time_units(215, 0, 0, 0, 40, 0.0, 1.25, 1.0, None, None), true);
        assert!(wave_one.expired(t0 + ms(1_000), grace, None));
    }

    /// R1 (2026-10-17): the portfolio starts its variants in a declared
    /// order as workers come free. On one worker that is exactly the order;
    /// on the box's three the first three families start first. Rayon's own
    /// range splitting started the slow diggers ahead of the 1.2 s finders
    /// (Jongou pair: 6 s on the PC, no first route in 100 s on the box).
    #[test]
    fn variants_start_in_the_declared_order() {
        let dummy = || -> Route { serde_json::from_value(serde_json::json!({"range_ly":0.0,"hops":[],"jumps":1,"total_ly":0.0,"straight_ly":0.0,"boosted_jumps":0,"expansions":0,"elapsed_ms":0,"refuel_stops":0,"injections":0,"secondary_boosts":0})).unwrap() };
        let ran = std::sync::Mutex::new(Vec::new());
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let (results, finished, started) = pool.install(|| run_variants(6, None, std::time::Duration::ZERO, None, &|| false, |_: &Route| 0, |i, _| { ran.lock().unwrap().push(i); (Ok(dummy()), true) }, &[3, 0, 5, 1, 2, 4]));
        assert_eq!((finished, started), (6, 6));
        assert!(results.iter().all(|r| r.is_ok()));
        assert_eq!(ran.into_inner().unwrap(), vec![3, 0, 5, 1, 2, 4], "one worker: the declared order, nothing else");
        // Results come back by variant index whatever the start order.
        let (results, _, _) = pool.install(|| run_variants(3, None, std::time::Duration::ZERO, None, &|| false, |r: &Route| r.jumps as u64, |i, _| (Ok(Route { jumps: i + 10, ..dummy() }), true), &[2, 0, 1]));
        assert_eq!(results.iter().map(|r| r.as_ref().unwrap().jumps).collect::<Vec<_>>(), vec![10, 11, 12]);
    }

    /// The thin family (2026-10-17, the rim pair): a fanout-capped, weight-3
    /// single and a weight-3 bidi lead the portfolio at indices 0 and 1, so
    /// rayon's range split starts them first on any thread count; every
    /// other family keeps its place behind them. ED_NO_THIN=1 restores the
    /// old numbering for A/B.
    #[test]
    fn the_thin_family_leads_the_portfolio() {
        // nb 6 singles, bidi, no retro, 2 greedy, 2 offramp, 6 twins, no corridor: 17 + 2 thin.
        assert_eq!(variant_kind(0, 0, 2, 6, true, false, 2, 2, 6, false, 19), "thin single");
        assert_eq!(variant_kind(1, 0, 2, 6, true, false, 2, 2, 6, false, 19), "thin bidi");
        assert_eq!(variant_kind(2, 0, 2, 6, true, false, 2, 2, 6, false, 19), "single");
        assert_eq!(variant_kind(8, 0, 2, 6, true, false, 2, 2, 6, false, 19), "bidi");
        assert_eq!(variant_kind(9, 0, 2, 6, true, false, 2, 2, 6, false, 19), "greedy single");
        assert_eq!(variant_kind(18, 0, 2, 6, true, true, 2, 2, 5, false, 19), "retro", "the last slot is still the retro one");
        assert_eq!(variant_kind(0, 0, 0, 6, true, false, 2, 2, 6, false, 17), "single", "ED_NO_THIN: the old numbering");
        assert_eq!(&KIND_ORDER[..3], &["witness", "thin single", "thin bidi"]);
        assert_eq!(&variant_order(19, 0, 2, 6, true, false, 2, 2, 6, false)[..2], &[0, 1]);
        // With the witness on it leads, and the thin pair follows at 1 and 2.
        assert_eq!(variant_kind(0, 1, 2, 6, true, false, 2, 2, 6, false, 20), "witness");
        assert_eq!(variant_kind(1, 1, 2, 6, true, false, 2, 2, 6, false, 20), "thin single");
        assert_eq!(variant_kind(3, 1, 2, 6, true, false, 2, 2, 6, false, 20), "single");
        assert_eq!(&variant_order(20, 1, 2, 6, true, false, 2, 2, 6, false)[..3], &[0, 1, 2]);
    }

    /// R3 Task 2: the witness is the answer when every optimiser fails. A K
    /// chain to an island 100 ly past its end (an injection enters it) with
    /// one neutron far away: the coarse variants find nothing, the witness
    /// does, and the plot returns its route instead of "no route".
    #[test]
    fn the_witness_route_is_the_answer_when_every_variant_fails() {
        let mut stars: Vec<_> = (0..5).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        stars.push(star("Island", 340.0, 0.0, 0.0, K));
        stars.push(star("N0", 0.0, 3000.0, 0.0, N));
        let f = build(&stars);
        let r = RouteRequest { injection: Some((2.0, "premium", 22)), time_budget_ms: 20_000, ..req(&f, "K0", "Island", 72.0) };
        let route = plan_long(&f.g, &f.neutrons, &r, &Control::none()).unwrap();
        assert_eq!(names(&route).last(), Some(&"Island"));
        assert_eq!(route.injections, 1, "{:?}", names(&route));
        // Contiguous, with the injected hop allowed its x2 (assert_contiguous
        // knows boosts, not injections).
        for w in route.hops.windows(2) {
            let d = dist(w[0].pos, w[1].pos);
            let allowed = r.range_ly * if w[1].injection.is_some() { 2.0 } else { 1.0 } + 0.01;
            assert!(d <= allowed, "{} -> {}: {d:.1} ly over {allowed:.1}", w[0].name, w[1].name);
        }
    }

    /// R3 Task 6, the bound-gap stop (default on, ED_NO_BOUND_STOP=1 ablates): no escalation
    /// wave is launched while the standing best is under the credibility
    /// ceiling -- the gap to the bound is inside the slack, so no dig can
    /// pay (Wongi -> Blaa Hypai, w6-3: a 38 j best under a 38.7 ceiling
    /// escalated to the x4 wave because the first wave ended 50 ms before
    /// the grace window did, and ran 100 s).
    #[test]
    fn no_wave_is_launched_while_the_best_is_under_the_ceiling() {
        assert!(wave_stops_at_bound(Some(38), 38.7, true));
        assert!(!wave_stops_at_bound(Some(39), 38.7, true), "above the ceiling the waves go on");
        assert!(!wave_stops_at_bound(None, 38.7, true), "no route yet: dig");
        assert!(!wave_stops_at_bound(Some(38), 38.7, false), "knob off: the old rule");
    }

    /// Whether the base single's route arms the grace clock: always, unless
    /// it came from a LazySP repair above the credibility ceiling (a rescue
    /// where the coarse graph lied is not a credible optimum: the repaired
    /// 425 j rim route armed at 6 s over the 412 j dig; a repaired 38 j
    /// Wongi -> Blaa Hypai under its ceiling is as good as the unrepaired
    /// one and ran 100 s for nothing when every repair was refused).
    /// ED_SINGLE_CEILING=1 (arm) gates the single by the ceiling like the
    /// bidi: on the rim the singles then never arm and the plot digs.
    #[test]
    fn the_base_single_arms_unless_repaired_above_the_ceiling() {
        assert!(single_arms(false, 425.0, 246.0, false), "an unrepaired route arms whatever the ceiling");
        assert!(!single_arms(true, 425.0, 246.0, false), "a repair above the ceiling does not");
        assert!(single_arms(true, 38.0, 38.7, false), "a repair under the ceiling does");
        assert!(!single_arms(false, 425.0, 246.0, true), "ceiling-gated: above does not arm");
        assert!(single_arms(false, 38.0, 38.7, true), "ceiling-gated: under arms");
    }

    /// A route that needed a repair (the variant re-ran after a leg banned a
    /// coarse edge) is a rescue, not a credible optimum: `with_repair`
    /// reports it so the caller never lets it arm the grace clock (arm
    /// w5p-3, 2026-10-18: the repaired base single's 425 j rim route armed
    /// at 6 s and guillotined the 412 j dig; Spase -> Colonia 87 -> 92 j).
    #[test]
    fn a_repaired_route_is_reported_as_repaired() {
        let banned = std::sync::Mutex::new(Vec::new());
        let repaired = std::sync::atomic::AtomicBool::new(false);
        let r = with_repair(&banned, &repaired, || Ok::<u32, RouteError>(1));
        assert!(r.is_ok() && !repaired.load(std::sync::atomic::Ordering::Relaxed), "a first-run route is not a repair");
        let mut runs = 0;
        let r = with_repair(&banned, &repaired, || {
            runs += 1;
            if runs == 1 {
                banned.lock().unwrap().push((1, 2));
                Err(RouteError::NoRoute)
            } else {
                Ok(2u32)
            }
        });
        assert!(r.is_ok() && repaired.load(std::sync::atomic::Ordering::Relaxed), "a route after a ban re-run is a repair");
    }

    /// R3 Task 4: the shared incumbent prunes the coarse search without
    /// costing a route that beats it: with the incumbent above the best
    /// route the plan is unchanged; with it at the best route the chain
    /// survives the slack.
    #[test]
    fn incumbent_pruning_keeps_every_route_that_beats_the_incumbent() {
        let f = highway();
        let r = req(&f, "K0", "K3000", 72.0);
        let stalled = std::sync::atomic::AtomicBool::new(false);
        let plan = |incumbent: u32| {
            let inc = std::sync::atomic::AtomicU32::new(incumbent);
            plan_long_with(&f.g, &f.neutrons, &r, &Control::none(), COARSE_WEIGHT, RESERVE_FRACTION, 0.75, 1, false, false, false, None, &stalled, &CoarseStats::default(), None, false, &NO_BANS, &inc)
        };
        let base = plan(u32::MAX).unwrap();
        let loose = plan(base.jumps as u32 + 5).unwrap();
        assert_eq!(names(&loose), names(&base), "an incumbent the route beats changes nothing");
        // An incumbent the route only ties: the slack keeps the chain (the
        // coarse hop count is an estimate; a far-below incumbent may prune
        // it and leave a detour the plot's judge then discards).
        match plan(base.jumps as u32) {
            Ok(r) => assert!(r.jumps <= base.jumps, "a route that ties the incumbent survives the slack: {} vs {}", r.jumps, base.jumps),
            Err(_) => {}
        }
    }

    /// R3 Task 3 (LazySP): a coarse edge whose leg the exact planner cannot
    /// fly is banned for the plot and the variant re-plans around it. A, B,
    /// C are neutrons on a line with nothing between B and C (600 ly: a
    /// one-bridge coarse edge with no star to bridge on), and E, F a detour
    /// at 400 ly hops. The coarse search takes B -> C (2.25 against 3); the
    /// refinement fails, the edge is banned, the re-run goes B -> E -> F -> C.
    #[test]
    fn a_failed_leg_bans_its_edge_and_the_variant_re_plans() {
        // A, B, C in a line; nothing between B and C (500 ly: a one-bridge
        // coarse edge priced 2.25 with no star to bridge on). The detour E,
        // F runs 400 ly above the line at boosted hops (3.0). The coarse
        // search takes B -> C, the refinement fails, the edge is banned and
        // the re-run goes B -> E -> F -> C.
        let stars = vec![
            star("A", 0.0, 0.0, 0.0, N),
            star("B", 400.0, 0.0, 0.0, N),
            // C is a K star: the goal is one coarse node (a neutron goal is
            // also a sub-index node, and the dead edge would need two bans).
            star("C", 900.0, 0.0, 0.0, K),
            star("E", 400.0, 400.0, 0.0, N),
            star("F", 800.0, 400.0, 0.0, N),
        ];
        let f = build(&stars);
        let r = RouteRequest { boost: BoostProfile { neutron: 6.0, white_dwarf: 1.5 }, ..req(&f, "A", "C", 72.0) };
        let banned = std::sync::Mutex::new(Vec::new());
        let stalled = std::sync::atomic::AtomicBool::new(false);
        let plan = || plan_long_with(&f.g, &f.neutrons, &r, &Control::none(), COARSE_WEIGHT, RESERVE_FRACTION, 0.75, 1, false, false, false, None, &stalled, &CoarseStats::default(), None, false, &banned, &NO_INCUMBENT);
        let first = plan();
        assert!(first.is_err(), "the B -> C leg cannot be flown: {:?}", first.map(|r| names(&r).iter().map(|s| s.to_string()).collect::<Vec<_>>()));
        assert_eq!(banned.lock().unwrap().len(), 1, "the dead edge is banned");
        let second = plan().expect("a route once the dead edge is banned");
        assert_eq!(names(&second), vec!["A", "B", "E", "F", "C"], "banned {:?}", banned.lock().unwrap());
    }

    /// The witness is refined EAGERLY, whatever the request's fuel mode: the
    /// feasibility search assumed a refill at every scoopable star, and the
    /// min-fuel refinement stripped the stops it relied on (the reverse
    /// Jongou witnesses, 2026-10-18: a neutron hop reached with 7 t).
    #[test]
    fn the_witness_is_refined_with_eager_stops() {
        let r = RouteRequest { min_fuel: true, ..Default::default() };
        assert!(!witness_refinement_request(&r).min_fuel);
        assert_eq!(witness_refinement_request(&r).range_ly, r.range_ly, "everything else is the request's");
    }

    /// The witness's route reaches the early-answer hook like any other
    /// variant's, so a plot whose optimisers are still digging has an
    /// answer on the wire in seconds.
    #[test]
    fn the_witness_arrives_as_an_early_answer() {
        let mut stars: Vec<_> = (0..5).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        stars.push(star("Island", 340.0, 0.0, 0.0, K));
        stars.push(star("N0", 0.0, 3000.0, 0.0, N));
        let f = build(&stars);
        let r = RouteRequest { injection: Some((2.0, "premium", 22)), time_budget_ms: 20_000, ..req(&f, "K0", "Island", 72.0) };
        let seen = std::sync::Mutex::new(Vec::new());
        let found = |route: &Route| seen.lock().unwrap().push(route.hops.last().map(|h| h.name.clone()).unwrap_or_default());
        let ctl = Control { found: &found, ..Control::none() };
        plan_long(&f.g, &f.neutrons, &r, &ctl).unwrap();
        let seen = seen.into_inner().unwrap();
        assert!(seen.iter().any(|n| n == "Island"), "the witness's route was offered as an answer: {seen:?}");
    }

    /// The thin pair is a quick-answer family: on the box's three threads a
    /// thin variant that has not answered by a third of the budget holds a
    /// worker the deep digs need (arm thin2-3, 2026-10-17: Colonia -> Sol
    /// 55 -> 56 j, Spase -> Colonia 87 -> 88 j, Wongi -> Blaa Hypai 1 -> 98 s
    /// wall), so it yields there; 30 s when the plot has no budget.
    #[test]
    fn the_thin_pair_yields_its_worker_at_a_third_of_the_budget() {
        assert_eq!(thin_budget_ms(100_000), 33_333);
        assert_eq!(thin_budget_ms(3_000), 1_000);
        assert_eq!(thin_budget_ms(0), 30_000, "no budget: the thin pair still yields at 30 s");
    }

    /// The goal field is wanted when the straight line looks non-uniform OR
    /// when forced (ED_FIELD=1, 2026-10-17): the reverse Jongou approach
    /// leaves the straight line 6 kly south and the precheck, which looks
    /// along that line, never fired.
    #[test]
    fn the_goal_field_is_wanted_when_forced_or_nonuniform() {
        assert!(field_wanted(false, true));
        assert!(field_wanted(true, false));
        assert!(!field_wanted(false, false));
    }

    /// The thin cap covers the coarse phase only: a thin variant whose
    /// chain has reached the goal finishes refining it (the rim reverse
    /// with the field: chain done in 1,307 expansions, cut at 33 s while
    /// refining, the wave's answer 462 j instead of 447).
    #[test]
    fn the_thin_cap_ends_at_the_coarse_phase() {
        assert!(thin_yields(true, false), "past the deadline, chain not done: yield");
        assert!(!thin_yields(true, true), "past the deadline, chain done: finish the refinement");
        assert!(!thin_yields(false, false));
    }

    /// A plot end off the highway snaps onto the cell graph within the
    /// plain run the coarse search allows that end (60 jumps), not 8 x the
    /// range: Jongou AA-A d0's nearest neutron is 873 ly out and the goal
    /// field toward it was never built (2026-10-17, "goal fields built in
    /// 0 ms"), so the reverse Jongou search had no arm map to follow.
    #[test]
    fn a_plot_end_snaps_onto_the_cell_graph_within_its_plain_run() {
        assert_eq!(end_snap_ly(72.0), 72.0 * MAX_END_BRIDGE_JUMPS as f32);
        assert!(end_snap_ly(72.0) > 873.0, "Jongou's 873 ly to the nearest neutron is inside the snap");
    }

    /// A refinement leg's expansion cap scales with its length (2026-10-17,
    /// the reverse Jongou route): one 1,209 ly leg the coarse chain had
    /// assumed bridgeable was not flyable, and at the old flat 5,000,000 cap
    /// the exact planner spent 97.8 of the plot's 100 s proving it. The
    /// longest legitimate leg on the record took 27,471 expansions.
    #[test]
    fn a_refinement_legs_expansion_cap_scales_with_its_length() {
        assert_eq!(leg_expansion_cap(1209.0), 1_209_000);
        assert_eq!(leg_expansion_cap(40.0), 200_000, "a floor for the short legs");
        assert!(leg_expansion_cap(514.0) > 218_774, "a legitimate 742 ly rim leg took 218,774 expansions; a 514 ly one over 205,000 (the 400-an-ly cap failed it)");
    }

    /// The cap that matters is time: a quarter of the plot's budget per leg
    /// (2026-10-17: the unflyable 1,209 ly Jongou leg fails at 25 s instead
    /// of 98; every legitimate leg on the record finished under 8 s). No
    /// plot budget, no leg budget.
    #[test]
    fn a_refinement_leg_gets_a_quarter_of_the_plots_time() {
        assert_eq!(leg_time_budget_ms(100_000), 25_000);
        assert_eq!(leg_time_budget_ms(30_000), 20_000, "the interactive lane's 30 s: a quarter would fail a legitimate 742 ly rim leg (7.8 s alone)");
        assert_eq!(leg_time_budget_ms(3_000), 3_000, "never more than the plot has");
        assert_eq!(leg_time_budget_ms(0), 0);
    }

    /// Spec D1: an injection is 1,800 s in the one currency, the judge
    /// included (until 2026-10-17 the judge counted an injected hop as a
    /// plain jump while both searches priced it at 1,800 s).
    #[test]
    fn an_injection_costs_its_eighteen_hundred_seconds() {
        let req = RouteRequest { stop_weight: 1.0, ..Default::default() };
        let plain = synthetic_route(10, 0, &[]);
        let injected = Route { injections: 1, ..synthetic_route(10, 0, &[]) };
        assert_eq!(route_score_public(&injected, &req) - route_score_public(&plain, &req), 18_000, "deciseconds");
    }

    /// A scoop250.bin whose record count is not the sub-index's is a stale
    /// side file from another build: it answers for the wrong stars, so it
    /// is ignored.
    #[test]
    fn a_stale_scoop_file_is_ignored() {
        let f = highway();
        let bytes = vec![10u8; f.neutrons.count + 1];
        crate::scoop_side::write(&f.neutrons.dir.join(crate::scoop_side::SCOOP_SIDE_FILE), &bytes).unwrap();
        let sub = Galaxy::open(&f.neutrons.dir).unwrap();
        assert_eq!(sub.nearest_scoop_ly(0), None, "one byte too many: not this sub-index's file");
        std::fs::remove_file(f.neutrons.dir.join(crate::scoop_side::SCOOP_SIDE_FILE)).unwrap();
    }

    /// The declared start order is a knob, off by default (2026-10-17, the
    /// three-thread box emulation: arm r1-3 lost 8 of 28 routes to the
    /// shipped split order). Off, the wave starts in rayon's range order,
    /// exactly as before R1.
    #[test]
    fn the_declared_start_order_is_a_knob_off_by_default() {
        assert!(!declared_order_on(None));
        assert!(!declared_order_on(Some("0")));
        assert!(declared_order_on(Some("1")));
        let dummy = || -> Route { serde_json::from_value(serde_json::json!({"range_ly":0.0,"hops":[],"jumps":1,"total_ly":0.0,"straight_ly":0.0,"boosted_jumps":0,"expansions":0,"elapsed_ms":0,"refuel_stops":0,"injections":0,"secondary_boosts":0})).unwrap() };
        let ran = std::sync::Mutex::new(Vec::new());
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let (results, finished, started) = pool.install(|| run_variants(5, None, std::time::Duration::ZERO, None, &|| false, |_: &Route| 0, |i, _| { ran.lock().unwrap().push(i); (Ok(dummy()), true) }, &[]));
        assert_eq!((finished, started), (5, 5));
        assert!(results.iter().all(|r| r.is_ok()));
        assert_eq!(ran.into_inner().unwrap(), vec![0, 1, 2, 3, 4], "no order given: the range, as rayon splits it (one worker: in index order)");
    }

    /// Once one variant has a route the others get the grace period and are
    /// then cancelled: a plot does not wait for its slowest variant. Variant
    /// 1 here never finishes on its own -- it only returns when told to
    /// stop -- so without the rule this test would hang.
    #[test]
    fn a_slow_variant_is_cancelled_once_another_has_a_route() {
        let dummy = || Route { highway_pending: false, range_ly: 0.0, hops: vec![], jumps: 1, total_ly: 0.0, straight_ly: 0.0, boosted_jumps: 0, expansions: 0, elapsed_ms: 0, refuel_stops: 0, injections: 0, secondary_boosts: 0, ship_id: None, ship: None, variants_run: 0, variants_finished: 0, ship_has_scoop: None, fsd_integrity: None, integrity_loss_per_boost: None, ship_has_afmu: None };
        let started = std::time::Instant::now();
        let (results, finished, _) = run_variants(3, Some(std::time::Duration::from_millis(50)), std::time::Duration::ZERO, None, &|| false, |r: &Route| route_score(r, 0.0, &RouteRequest::default()), |i, check| {
            if i == 1 {
                while !check() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                return (Err(RouteError::Cancelled), false);
            }
            (Ok(dummy()), true)
        }, &[]);
        assert_eq!(finished, 2);
        assert!(matches!(results[1], Err(RouteError::Cancelled)));
        assert!(results[0].is_ok() && results[2].is_ok());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // Without a grace period every variant is waited for; the user's
        // own cancel still reaches them.
        let (results, finished, _) = run_variants(2, None, std::time::Duration::ZERO, None, &|| true, |r: &Route| route_score(r, 0.0, &RouteRequest::default()), |_, check| if check() { (Err(RouteError::Cancelled), false) } else { (Ok(dummy()), true) }, &[]);
        assert_eq!(finished, 0);
        assert!(results.iter().all(|r| matches!(r, Err(RouteError::Cancelled))));
    }

    /// A white dwarf in the highway sub-index is a coarse waypoint with its
    /// own x1.5 boost: the plan crosses a gap only a white-dwarf hop can
    /// bridge, and every hop respects that multiplier (a x4 assumption
    /// would have proposed an unflyable direct edge).
    #[test]
    fn white_dwarf_highway_waypoint_boosts_by_its_own_class() {
        let stars = vec![
            star("K0", 0.0, 0.0, 0.0, K),
            star("WD", 40.0, 0.0, 0.0, WD),
            star("Beyond", 120.0, 0.0, 0.0, K),
            star("Island", 165.0, 0.0, 0.0, K),
        ];
        let neutron_only = build_with(&stars, |c| c == StarClass::Neutron);
        let with_wd = build(&stars);
        assert_eq!(neutron_only.neutrons.count, 0);
        assert_eq!(with_wd.neutrons.count, 1);
        let r = req(&with_wd, "K0", "Island", 60.0);
        // Coarse waypoints are reported through the trace hook: the white
        // dwarf must be one of them.
        let chain = std::sync::Mutex::new(Vec::new());
        let trace = |phase: &'static str, pos: [f32; 3], _: f32| {
            if phase == "chain" {
                chain.lock().unwrap().push(pos);
            }
        };
        let ctl = Control { cancelled: &|| false, progress: &|_, _| {}, stage: &|_, _, _| {}, found: &|_| {}, legs: &NO_LEG_MEMO, variant: &|_| {}, trace: &trace };
        let route = plan_long(&with_wd.g, &with_wd.neutrons, &r, &ctl).unwrap();
        let chain = chain.into_inner().unwrap();
        assert!(chain.iter().any(|p| (p[0] - 40.0).abs() < 0.01), "WD is a coarse waypoint: {chain:?}");
        let _ = neutron_only;
        assert_contiguous(&route, &r);
        assert_eq!(names(&route), vec!["K0", "WD", "Beyond", "Island"]);
        let hop = &route.hops[2];
        assert!(hop.boosted && hop.distance_ly > 60.0 && hop.distance_ly <= 90.0, "WD -> Beyond is a x1.5 hop: {hop:?}");
        assert_eq!(route.boosted_jumps, 1);
    }

    pub(crate) fn star(name: &str, x: f32, y: f32, z: f32, sub: &'static str) -> (String, f32, f32, f32, &'static str) {
        (name.to_string(), x, y, z, sub)
    }

    /// The commander's Mandalay (see `fuel.rs`): 32 t tank, ~72 ly on a
    /// full tank, standard x4 / x1.5 supercharge.
    fn mandalay() -> FuelModel {
        FuelModel::from_loadout(319.0, 32.0, 5.2, 5, true, false, 77.9, 10.5, 0.0)
    }

    fn names(r: &Route) -> Vec<&str> {
        r.hops.iter().map(|h| h.name.as_str()).collect()
    }

    fn req(f: &Field, from: &str, to: &str, range: f32) -> RouteRequest {
        RouteRequest { from: f.g.find(from).unwrap(), to: f.g.find(to).unwrap(), range_ly: range, ..Default::default() }
    }

    /// Every hop is a real jump from the previous one, within the reach the
    /// request allows, and the totals are the sums of the parts.
    fn assert_contiguous(r: &Route, req: &RouteRequest) {
        assert_eq!(r.hops.first().unwrap().idx, req.from, "route must start at the origin");
        assert_eq!(r.hops.last().unwrap().idx, req.to, "route must end at the destination");
        assert_eq!(r.jumps, r.hops.len() - 1);
        assert_eq!(r.hops[0].distance_ly, 0.0);
        let mut total = 0.0f32;
        let mut boosted = 0usize;
        for w in r.hops.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            assert_ne!(a.idx, b.idx, "hop repeats {}", b.name);
            let d = dist(a.pos, b.pos);
            assert!((d - b.distance_ly).abs() < 0.01, "{} -> {}: hop says {:.2} ly, positions say {d:.2}", a.name, b.name, b.distance_ly);
            total += d;
            assert!((total - b.total_ly).abs() < 0.05, "{}: cumulative {:.2} vs summed {total:.2}", b.name, b.total_ly);
            let boost = if b.boosted { b.via_secondary.map_or_else(|| req.boost.for_class(a.class), |(c, _)| req.boost.for_class(c)) } else { 1.0 };
            let reach = match req.fuel {
                Some(m) => m.reach(a.fuel_after.expect("fuel figures with a fuel model"), boost),
                None => req.range_ly * boost,
            };
            assert!(d <= reach + 0.01, "{} -> {} is {d:.1} ly but reach is {reach:.1} (boost {boost})", a.name, b.name);
            if b.boosted && b.via_secondary.is_none() {
                assert!(a.class == StarClass::Neutron || a.class == StarClass::WhiteDwarf, "{} boosted a jump", a.name);
            }
            boosted += b.boosted as usize;
        }
        assert!((total - r.total_ly).abs() < 0.05, "total_ly {:.2} vs summed {total:.2}", r.total_ly);
        assert_eq!(boosted, r.boosted_jumps);
    }

    /// Re-fly the route with the fuel model: every jump must be affordable
    /// from what the previous hop left, the tank is never negative, refuel
    /// stops are scoopable stars, and the reported figures match.
    fn assert_fuel_consistent(r: &Route, m: &FuelModel, boost: &BoostProfile, start_fuel: f32) {
        let mut f = start_fuel;
        assert!((r.hops[0].fuel_after.unwrap() - start_fuel).abs() < 0.05);
        for w in r.hops.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            let bst = if b.boosted { b.via_secondary.map_or_else(|| boost.for_class(a.class), |(c, _)| boost.for_class(c)) } else { 1.0 };
            let left = m.jump(b.distance_ly, f, bst).unwrap_or_else(|| panic!("{} -> {} ({:.1} ly, boost {bst}) is not fundable from {f:.1} t", a.name, b.name, b.distance_ly));
            assert!(left >= 0.0);
            f = if b.scoopable { m.capacity } else { left };
            let reported = b.fuel_after.unwrap();
            assert!(reported >= 0.0, "{}: negative tank {reported}", b.name);
            assert!((reported - f).abs() < 0.05, "{}: reports {reported:.2} t, re-simulation says {f:.2}", b.name);
            if b.refuel {
                assert!(b.scoopable, "{} is a refuel stop but not scoopable", b.name);
            }
            assert_eq!(b.refuel, b.scoopable && left < m.capacity, "{}: refuel flag", b.name);
        }
        assert_eq!(r.refuel_stops, r.hops.iter().filter(|h| h.refuel).count());
    }

    /// K stars every 50 ly along x from 0 to 3000, and a neutron highway
    /// 230 ly apart (10 ly off the line) from x=50 to x=2810.
    fn highway() -> Field {
        let mut stars = Vec::new();
        for i in 0..=60 {
            stars.push(star(&format!("K{}", i * 50), (i * 50) as f32, 0.0, 0.0, K));
        }
        for i in 0..13 {
            stars.push(star(&format!("N{i}"), 50.0 + 230.0 * i as f32, 0.0, 10.0, N));
        }
        build(&stars)
    }

    /// 25 x 25 neutrons 200 ly apart in the x/z plane, and an island K star
    /// 5,000 ly above them that nothing can reach.
    fn field() -> Field {
        let mut stars = Vec::new();
        for i in 0..25 {
            for j in 0..25 {
                stars.push(star(&format!("N{i}-{j}"), (i * 200) as f32, 0.0, (j * 200) as f32, N));
            }
        }
        stars.push(star("Island", 2400.0, 5000.0, 2400.0, K));
        build(&stars)
    }

    // ---- (a) neutron x4, (b) white dwarf x1.5 -------------------------

    /// Line of K stars 20 ly apart, a boosting star at x=40, and an island
    /// `off` ly to the side of it that only the boosted hop can reach.
    fn island(boost_star: &'static str, off: f32) -> Field {
        let mut stars: Vec<_> = (0..=10).map(|i| star(&format!("S{i}"), (i * 20) as f32, 0.0, 0.0, if i == 2 { boost_star } else { K })).collect();
        stars.push(star("Island", 40.0, off, 0.0, K));
        build(&stars)
    }

    /// With `white_dwarf = 1.0` in the profile (the commander's opt-out) a
    /// white dwarf on the highway is no coarse waypoint -- the plan never
    /// names it -- and the corridor check does not count it, so a plain
    /// plot does not spin up the neutron-first variant for it.
    #[test]
    fn a_white_dwarf_that_cannot_boost_is_not_a_coarse_waypoint() {
        let stars = vec![
            star("K0", 0.0, 0.0, 0.0, K),
            star("WD", 40.0, 0.0, 0.0, WD),
            star("K1", 80.0, 0.0, 0.0, K),
            star("K2", 120.0, 0.0, 0.0, K),
            star("Island", 165.0, 0.0, 0.0, K),
        ];
        let f = build(&stars);
        assert_eq!(f.neutrons.count, 1, "the white dwarf is on the highway by default");
        let no_wd = RouteRequest { boost: BoostProfile { neutron: 4.0, white_dwarf: 1.0 }, ..req(&f, "K0", "Island", 60.0) };
        let chain = std::sync::Mutex::new(Vec::new());
        let trace = |phase: &'static str, pos: [f32; 3], _: f32| {
            if phase == "chain" {
                chain.lock().unwrap().push(pos);
            }
        };
        let ctl = Control { cancelled: &|| false, progress: &|_, _| {}, stage: &|_, _, _| {}, found: &|_| {}, legs: &NO_LEG_MEMO, variant: &|_| {}, trace: &trace };
        let route = plan_long(&f.g, &f.neutrons, &no_wd, &ctl).unwrap();
        let chain = chain.into_inner().unwrap();
        assert!(!chain.iter().any(|p| (p[0] - 40.0).abs() < 0.01), "WD must not be a coarse waypoint at x1.0: {chain:?}");
        assert_contiguous(&route, &no_wd);
        assert_eq!(route.boosted_jumps, 0, "{:?}", names(&route));
        // The same field with the boost allowed does take it (the pin above).
        let with_wd = req(&f, "K0", "Island", 60.0);
        assert!(plan_long(&f.g, &f.neutrons, &with_wd, &Control::none()).unwrap().boosted_jumps >= 1);
        // Corridor: the white dwarf sits 40 ly from the start, on the line.
        let a = [0.0, 0.0, 0.0];
        let b = [165.0, 0.0, 0.0];
        assert!(neutron_in_corridor(&f.neutrons, a, b, 5.0, true));
        assert!(!neutron_in_corridor(&f.neutrons, a, b, 5.0, false), "a white dwarf that cannot boost is not in the corridor");
    }

    #[test]
    fn neutron_on_the_line_gives_a_four_times_hop() {
        let f = island(N, 100.0);
        let r = req(&f, "S0", "Island", 30.0);
        let route = plan(&f.g, &r, &Control::none()).unwrap();
        assert_contiguous(&route, &r);
        assert_eq!(names(&route), ["S0", "S1", "S2", "Island"], "S2 is the neutron");
        let last = route.hops.last().unwrap();
        assert!(last.boosted && (last.distance_ly - 100.0).abs() < 0.01);
        assert!(last.distance_ly > 3.0 * r.range_ly && last.distance_ly <= 4.0 * r.range_ly, "the hop is only possible at x4");
        assert_eq!(route.boosted_jumps, 1);
        // A weaker drive (x3) cannot make the 100 ly hop; no boost at all cannot either.
        let weak = RouteRequest { boost: BoostProfile { neutron: 3.0, white_dwarf: 1.5 }, ..r.clone() };
        assert!(matches!(plan(&f.g, &weak, &Control::none()), Err(RouteError::NoRoute)));
        let plain = RouteRequest { supercharge: false, ..r.clone() };
        assert!(matches!(plan(&f.g, &plain, &Control::none()), Err(RouteError::NoRoute)));
    }

    #[test]
    fn white_dwarf_gives_a_one_and_a_half_times_hop() {
        let f = island(WD, 44.0);
        let r = req(&f, "S0", "Island", 30.0);
        let route = plan(&f.g, &r, &Control::none()).unwrap();
        assert_contiguous(&route, &r);
        assert_eq!(names(&route), ["S0", "S1", "S2", "Island"], "S2 is the white dwarf");
        let last = route.hops.last().unwrap();
        assert!(last.boosted && (last.distance_ly - 44.0).abs() < 0.01);
        assert_eq!(route.hops[2].class, StarClass::WhiteDwarf);
        assert_eq!(route.boosted_jumps, 1);
        // x1.4 falls 2 ly short: the multiplier is exactly the profile's.
        let weak = RouteRequest { boost: BoostProfile { neutron: 4.0, white_dwarf: 1.4 }, ..r.clone() };
        assert!(matches!(plan(&f.g, &weak, &Control::none()), Err(RouteError::NoRoute)));
    }

    // ---- tie-break: fewer jumps, then fewer boosts, then fewer ly ------

    #[test]
    fn tie_break_prefers_fewer_boosts_then_fewer_light_years() {
        // Three two-jump ways from Start to Goal (115 ly at 60 ly range):
        //   Mid    K at (57.5,  3): 115.16 ly, unboosted
        //   Wide   K at (57.5, -8): 116.10 ly, unboosted
        //   Pulsar N at (50,   0): 115.00 ly, second hop boosted (65 > 60)
        // Fewest ly is the pulsar, but a boost only earns its keep when it
        // removes a jump, so Mid wins; Wide loses to Mid on ly.
        let f = build(&[star("Start", 0.0, 0.0, 0.0, K), star("Goal", 115.0, 0.0, 0.0, K), star("Mid", 57.5, 3.0, 0.0, K), star("Wide", 57.5, -8.0, 0.0, K), star("Pulsar", 50.0, 0.0, 0.0, N)]);
        for thorough in [false, true] {
            let r = RouteRequest { thorough, ..req(&f, "Start", "Goal", 60.0) };
            let route = plan(&f.g, &r, &Control::none()).unwrap();
            assert_contiguous(&route, &r);
            assert_eq!(names(&route), ["Start", "Mid", "Goal"], "thorough={thorough}");
            assert_eq!((route.jumps, route.boosted_jumps), (2, 0));
        }
        // Take Mid and Wide away and the boosted path is the only two-jump one.
        let f = build(&[star("Start", 0.0, 0.0, 0.0, K), star("Goal", 115.0, 0.0, 0.0, K), star("Pulsar", 50.0, 0.0, 0.0, N)]);
        let r = req(&f, "Start", "Goal", 60.0);
        let route = plan(&f.g, &r, &Control::none()).unwrap();
        assert_eq!(names(&route), ["Start", "Pulsar", "Goal"]);
        assert_eq!((route.jumps, route.boosted_jumps), (2, 1));
    }

    #[test]
    fn fewer_jumps_beats_fewer_boosts() {
        // Goal 200 ly out: four 50 ly hops unboosted, or K0 -> N (50 ly) -> Goal (150 ly boosted).
        // Exact modes (thorough, or weight 1.0): the two-jump boosted path.
        // The default weighted search (1.3, plain-range heuristic) is not
        // asked here: along a chain of 50 ly hops its f strictly falls, so
        // it walks the K line to the goal before ever expanding the neutron.
        let f = build(&[star("K0", 0.0, 0.0, 0.0, K), star("K50", 50.0, 0.0, 0.0, K), star("K100", 100.0, 0.0, 0.0, K), star("K150", 150.0, 0.0, 0.0, K), star("Goal", 200.0, 0.0, 0.0, K), star("N", 50.0, 5.0, 0.0, N)]);
        for r in [RouteRequest { thorough: true, ..req(&f, "K0", "Goal", 60.0) }, RouteRequest { weight: 1.0, ..req(&f, "K0", "Goal", 60.0) }] {
            let route = plan(&f.g, &r, &Control::none()).unwrap();
            assert_contiguous(&route, &r);
            assert_eq!(names(&route), ["K0", "N", "Goal"], "thorough={} weight={}", r.thorough, r.weight);
            assert_eq!((route.jumps, route.boosted_jumps), (2, 1));
        }
    }

    // ---- (c) max_dry_jumps --------------------------------------------

    #[test]
    fn max_dry_jumps_is_respected_and_a_longer_dry_run_is_rejected() {
        // K0, three brown dwarfs, K4: 20 ly apart. At 25 ly every star is visited.
        let f = build(&[star("K0", 0.0, 0.0, 0.0, K), star("L1", 20.0, 0.0, 0.0, L), star("L2", 40.0, 0.0, 0.0, L), star("L3", 60.0, 0.0, 0.0, L), star("K4", 80.0, 0.0, 0.0, K)]);
        let r = RouteRequest { max_dry_jumps: 3, ..req(&f, "K0", "K4", 25.0) };
        let route = plan(&f.g, &r, &Control::none()).unwrap();
        assert_eq!(names(&route), ["K0", "L1", "L2", "L3", "K4"]);
        assert_eq!(route.hops.iter().filter(|h| !h.scoopable).count(), 3);
        let tight = RouteRequest { max_dry_jumps: 2, ..r.clone() };
        assert!(matches!(plan(&f.g, &tight, &Control::none()), Err(RouteError::NoRoute)), "three dry arrivals in a row must be refused");
        // With 45 ly of range the dwarfs can be skipped two at a time: K0 -> L2 -> K4, one dry arrival.
        let longer = RouteRequest { range_ly: 45.0, max_dry_jumps: 2, ..r.clone() };
        let route = plan(&f.g, &longer, &Control::none()).unwrap();
        assert_contiguous(&route, &longer);
        let longest_dry = route.hops.iter().skip(1).fold((0, 0), |(run, max), h| if h.scoopable { (0, max) } else { (run + 1, max.max(run + 1)) }).1;
        assert!(longest_dry <= 2, "{:?}", names(&route));
        assert_eq!(names(&route), ["K0", "L2", "K4"]);
    }

    // ---- (d) no route, (e) cancellation, (f) budget --------------------

    #[test]
    fn a_gap_wider_than_the_range_is_no_route_for_both_planners() {
        let f = build(&[star("K0", 0.0, 0.0, 0.0, K), star("K50", 50.0, 0.0, 0.0, K), star("Far", 150.0, 0.0, 0.0, K)]);
        assert_eq!(f.neutrons.count, 0, "no highway at all");
        let r = req(&f, "K0", "Far", 60.0);
        assert!(matches!(plan(&f.g, &r, &Control::none()), Err(RouteError::NoRoute)));
        assert!(matches!(plan_long(&f.g, &f.neutrons, &r, &Control::none()), Err(RouteError::NoRoute)));
    }

    #[test]
    fn an_unreachable_island_exhausts_the_highway_and_is_no_route() {
        let f = field();
        assert_eq!(f.neutrons.count, 625);
        let r = req(&f, "N0-0", "Island", 60.0);
        let started = std::time::Instant::now();
        assert!(matches!(plan_long(&f.g, &f.neutrons, &r, &Control::none()), Err(RouteError::NoRoute)));
        assert!(matches!(plan(&f.g, &r, &Control::none()), Err(RouteError::NoRoute)));
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "the search must terminate");
    }

    #[test]
    fn cancellation_is_returned_before_any_progress_is_reported() {
        let f = field();
        let r = req(&f, "N0-0", "Island", 60.0);
        let progress_calls = AtomicU64::new(0);
        let progress = |_: u64, _: f32| {
            progress_calls.fetch_add(1, AtomicOrdering::Relaxed);
        };
        let ctl = Control { cancelled: &|| true, progress: &progress, stage: &|_, _, _| {}, found: &|_| {}, legs: &NO_LEG_MEMO, variant: &|_| {}, trace: &|_, _, _| {} };
        // The island is unreachable: without cancellation the answer would
        // be NoRoute; the user's Stop must win over that.
        assert!(matches!(plan_long(&f.g, &f.neutrons, &r, &ctl), Err(RouteError::Cancelled)));
        assert!(matches!(plan(&f.g, &r, &ctl), Err(RouteError::Cancelled)));
        assert_eq!(progress_calls.load(AtomicOrdering::Relaxed), 0, "cancel is checked before the first progress report");
    }

    #[test]
    fn the_expansion_budget_returns_budget_not_no_route() {
        let f = field();
        let r = RouteRequest { max_expansions: 10, ..req(&f, "N0-0", "Island", 60.0) };
        assert!(matches!(plan(&f.g, &r, &Control::none()), Err(RouteError::Budget)));
        // Every coarse variant runs out of budget: the answer is Budget, not
        // "no route possible" (which the user reads as an unjumpable gap).
        assert!(matches!(plan_long(&f.g, &f.neutrons, &r, &Control::none()), Err(RouteError::Budget)));
    }

    #[test]
    fn the_time_budget_stops_the_search_without_a_panic() {
        let f = field();
        let r = RouteRequest { time_budget_ms: 1, ..req(&f, "N0-0", "Island", 60.0) };
        match plan_long(&f.g, &f.neutrons, &r, &Control::none()) {
            Err(RouteError::Budget) | Err(RouteError::NoRoute) => {}
            other => panic!("expected Budget (or NoRoute if a variant finished in time): {other:?}"),
        }
    }

    // ---- (h) plan_long stitching, (i) agreement with the exact planner --

    #[test]
    fn long_range_stitches_the_highway_into_one_contiguous_route() {
        let f = highway();
        assert_eq!(f.neutrons.count, 13);
        let r = req(&f, "K0", "K3000", 60.0);
        let stage_calls = AtomicU64::new(0);
        let stage = |_: &str, _: u32, _: u32| {
            stage_calls.fetch_add(1, AtomicOrdering::Relaxed);
        };
        let ctl = Control { cancelled: &|| false, progress: &|_, _| {}, stage: &stage, found: &|_| {}, legs: &NO_LEG_MEMO, variant: &|_| {}, trace: &|_, _, _| {} };
        let route = plan_long(&f.g, &f.neutrons, &r, &ctl).unwrap();
        assert_contiguous(&route, &r);
        // By hand: K0 -> N0 (51 ly), twelve 230 ly neutron hops, N12 -> K3000 (190.3 ly).
        assert_eq!(names(&route), ["K0", "N0", "N1", "N2", "N3", "N4", "N5", "N6", "N7", "N8", "N9", "N10", "N11", "N12", "K3000"]);
        assert_eq!((route.jumps, route.boosted_jumps), (14, 13));
        let expect = dist([0.0, 0.0, 0.0], [50.0, 0.0, 10.0]) + 12.0 * 230.0 + dist([2810.0, 0.0, 10.0], [3000.0, 0.0, 0.0]);
        assert!((route.total_ly - expect).abs() < 0.1, "{} vs {expect}", route.total_ly);
        assert!((route.straight_ly - 3000.0).abs() < 0.01);
        assert_eq!(route.refuel_stops, 0, "no fuel model, no refuel stops");
        assert!(route.hops.iter().all(|h| h.fuel_after.is_none()));
        assert!(stage_calls.load(AtomicOrdering::Relaxed) > 0, "coarse/refine stages are reported");
    }

    #[test]
    fn long_range_matches_the_exact_planner_on_the_highway() {
        // The exact planner in its admissible mode (thorough: weight 1, a
        // heuristic that credits the x4) finds the 14-jump highway route; the
        // coarse neutron search must not do worse than the optimum.
        let f = highway();
        let r = req(&f, "K0", "K3000", 60.0);
        let long = plan_long(&f.g, &f.neutrons, &r, &Control::none()).unwrap();
        let exact = plan(&f.g, &RouteRequest { thorough: true, ..r.clone() }, &Control::none()).unwrap();
        assert_contiguous(&exact, &r);
        assert_eq!(exact.jumps, 14, "{:?}", names(&exact));
        assert_eq!(long.jumps, exact.jumps, "long {:?} vs exact {:?}", names(&long), names(&exact));
        assert_eq!(long.boosted_jumps, exact.boosted_jumps);
    }

    #[test]
    fn short_routes_are_below_the_long_range_threshold_and_agree() {
        assert_eq!(LONG_ROUTE_LY, 1_500.0, "callers dispatch on this; moving it changes every plot");
        let f = highway();
        // 500 ly: the callers use the exact planner; the long-range one must not disagree.
        let r = req(&f, "K0", "K500", 60.0);
        assert!(dist(f.g.pos_of(r.from), f.g.pos_of(r.to)) < LONG_ROUTE_LY);
        let exact = plan(&f.g, &RouteRequest { thorough: true, ..r.clone() }, &Control::none()).unwrap();
        let long = plan_long(&f.g, &f.neutrons, &r, &Control::none()).unwrap();
        assert_contiguous(&exact, &r);
        assert_contiguous(&long, &r);
        // K0 -> N0 -> N1 (x=280) -> K500 (220 ly boosted): 3 jumps either way.
        assert_eq!(exact.jumps, 3, "{:?}", names(&exact));
        assert_eq!(long.jumps, exact.jumps, "long {:?} vs exact {:?}", names(&long), names(&exact));
    }

    // ---- mid-range plots: the neutron shortcut must not be missed ------

    /// The highway cut at 1,000 ly: K stars every 50 ly to K1000, neutrons
    /// at x = 50, 280, 510, 740 (10 ly off the line). Well under
    /// `LONG_ROUTE_LY`, so the app used the exact planner alone -- which at
    /// weight 1.3 walks the K chain (its f falls monotonically along it)
    /// and never expands a neutron: 20 jumps where six will do.
    fn mid_highway() -> Field {
        let mut stars = Vec::new();
        for i in 0..=20 {
            stars.push(star(&format!("K{}", i * 50), (i * 50) as f32, 0.0, 0.0, K));
        }
        for i in 0..4 {
            stars.push(star(&format!("N{i}"), 50.0 + 230.0 * i as f32, 0.0, 10.0, N));
        }
        build(&stars)
    }

    /// R1 (2026-10-17, from the R0 timers): the widen rescan is for a true
    /// void -- a scan that found nothing -- not for an expansion whose every
    /// candidate was already known; half of all expansions on the rim pair
    /// fired it for dominated neighbours. The synthetic fields are too
    /// regular to reproduce domination, so the rule itself is pinned here
    /// and the counts come from the timers CSV.
    #[test]
    fn the_widen_fires_whenever_nothing_was_pushed_and_once_per_cell_only_by_the_knob() {
        assert!(widen_due(0, 0, false, false), "nothing seen: a void, widen");
        assert!(widen_due(0, 37, true, false), "shipped rule: candidates all known, widen every time (the long bridges)");
        assert!(!widen_due(3, 37, false, false), "something pushed: no widen");
        assert!(widen_due(0, 37, false, true), "knob: first time in this cell, widen");
        assert!(!widen_due(0, 37, true, true), "knob: this cell widened already, not again");
        assert!(widen_due(0, 0, true, true), "knob: a void widens regardless");
    }

    /// R1 sector thinning (2026-10-17): per direction sector the coarse
    /// scan keeps the candidates that make the most progress, the nearest
    /// one (least fuel) and the best refuel option; everything else in the
    /// same direction is the same choice to the coarse plan. Candidates are
    /// (score = lower is more progress, index, position, distance).
    #[test]
    fn sector_thinning_keeps_a_refuel_candidate() {
        let here = [0.0, 0.0, 0.0];
        let mut cands: Vec<(f32, u32, [f32; 3], f32)> = vec![
            (-400.0, 1, [400.0, 0.0, 0.0], 400.0),
            (-350.0, 2, [350.0, 5.0, 0.0], 350.0),
            (-300.0, 3, [300.0, -5.0, 0.0], 300.0),
            (-200.0, 4, [200.0, 0.0, 0.0], 200.0),
            (-90.0, 5, [0.0, 90.0, 0.0], 90.0),
        ];
        thin_by_sector(&mut cands, here, &|idx: u32| idx == 3, 1);
        let kept: Vec<u32> = cands.iter().map(|c| c.1).collect();
        assert!(kept.contains(&1), "the most progress in +x stays: {kept:?}");
        assert!(kept.contains(&3), "the refuel option stays although it is third on progress: {kept:?}");
        assert!(kept.contains(&4), "the nearest in +x stays (least fuel): {kept:?}");
        assert!(kept.contains(&5), "the only candidate in +y stays: {kept:?}");
        assert!(!kept.contains(&2), "the second-best plain +x candidate goes: {kept:?}");
        assert_eq!(kept.len(), 4);
    }

    /// R2 Task 2, corrected the same day: the coarse search that carries
    /// fuel in its state refills for free only where the companion flag
    /// says a scoopable star sits at arrival. A scoopable star 10 ly away
    /// is a detour and a stop, not a refill: read as one it cost the
    /// matrix 2-8 jumps a route (arm r2-judgeonly vs r1: 14 of 28 worse,
    /// stops down and jumps up on the same routes).
    #[test]
    fn a_free_refill_is_a_companion_not_a_neighbour() {
        let f = highway();
        let mut bytes = vec![crate::scoop_side::NONE; f.neutrons.count as usize];
        bytes[0] = 10;
        crate::scoop_side::write(&f.neutrons.dir.join(crate::scoop_side::SCOOP_SIDE_FILE), &bytes).unwrap();
        let sub = Galaxy::open(&f.neutrons.dir).unwrap();
        assert!(refuels_here(&sub, 0, 72.0), "the byte says a scoop is within reach...");
        assert_eq!(refills_on_arrival(&sub, 0), sub.flags(0) & crate::format::FLAG_SCOOP_NEARBY != 0, "...but a free refill is the companion flag alone");
        std::fs::remove_file(f.neutrons.dir.join(crate::scoop_side::SCOOP_SIDE_FILE)).unwrap();
    }

    /// R2 Task 2 (2026-10-17): the refuel signal reads the scoop byte against
    /// the ship's plain reach at the current tank; without the file it is
    /// the companion flag, as before.
    #[test]
    fn refuel_reach_follows_the_tank() {
        let f = highway();
        let mut bytes = vec![crate::scoop_side::NONE; f.neutrons.count as usize];
        bytes[0] = 60;
        bytes[1] = 10;
        crate::scoop_side::write(&f.neutrons.dir.join(crate::scoop_side::SCOOP_SIDE_FILE), &bytes).unwrap();
        let sub = Galaxy::open(&f.neutrons.dir).unwrap();
        assert!(refuels_here(&sub, 0, 72.0), "60 ly is within a 72 ly reach");
        assert!(!refuels_here(&sub, 0, 50.0), "not within a 50 ly reach at a low tank");
        assert!(refuels_here(&sub, 1, 50.0));
        assert!(!refuels_here(&sub, 2, 72.0), "255: nothing within 254 ly");
        std::fs::remove_file(f.neutrons.dir.join(crate::scoop_side::SCOOP_SIDE_FILE)).unwrap();
        let bare = Galaxy::open(&f.neutrons.dir).unwrap();
        assert_eq!(refuels_here(&bare, 0, 72.0), bare.flags(0) & crate::format::FLAG_SCOOP_NEARBY != 0, "no file: the flag answers");
    }

    /// R2 (2026-10-17): scoop250.bin, the nearest scoopable star per
    /// highway star, is optional at run time and built from the full index.
    #[test]
    fn a_sub_index_without_the_scoop_file_plans_as_before() {
        let f = highway();
        // The sub-index build writes the file; a sub-index from before R2 has none.
        let _ = std::fs::remove_file(f.neutrons.dir.join(crate::scoop_side::SCOOP_SIDE_FILE));
        let sub = Galaxy::open(&f.neutrons.dir).unwrap();
        assert_eq!(sub.nearest_scoop_ly(0), None, "no file: unknown, the flag path answers");
        let r = req(&f, "K0", "K3000", 60.0);
        assert!(plan_best(&f.g, Some(&f.neutrons), &r, &Control::none()).is_ok());
    }

    #[test]
    fn nearest_scoop_bytes_measure_the_full_index() {
        // highway(): K stars every 50 ly on the line, neutrons 10 ly off it
        // at x = 50 + 230 i: every neutron has a K star within ~10-27 ly.
        let f = highway();
        let bytes = crate::import::nearest_scoop_bytes(&f.g, &f.neutrons);
        assert_eq!(bytes.len(), f.neutrons.count as usize);
        assert!(bytes.iter().all(|&b| b <= 27), "{bytes:?}");
        // Written beside the sub-index, the same figures come back by record index.
        crate::scoop_side::write(&f.neutrons.dir.join(crate::scoop_side::SCOOP_SIDE_FILE), &bytes).unwrap();
        let reopened = Galaxy::open(&f.neutrons.dir).unwrap();
        assert_eq!(reopened.nearest_scoop_ly(0), Some(bytes[0]));
    }

    /// R1 leg memo (2026-10-17): six variants recomputed one 68 s hand-off
    /// leg on the rim-to-rim route. A remembered leg serves a later request
    /// only when the tank at departure is at least what it was planned from
    /// (more fuel never breaks a leg; less may). Bucketing by 4 t instead
    /// made the settle rounds re-plan the same leg eight times and never
    /// converge: Beagle Point -> Sol became no route (r1 arm).
    #[test]
    fn the_leg_memo_serves_a_leg_only_from_at_least_the_fuel_it_was_planned_from() {
        let memo = LegMemo::for_plot();
        let five: Route = serde_json::from_value(serde_json::json!({"range_ly":50.0,"hops":[],"jumps":5,"total_ly":0.0,"straight_ly":0.0,"boosted_jumps":0,"expansions":0,"elapsed_ms":0,"refuel_stops":0,"injections":0,"secondary_boosts":0})).unwrap();
        memo.put(LegKey::new(1, 2, 22, 5.1), 120.0, Ok(five.clone()));
        assert_eq!(memo.get(&LegKey::new(1, 2, 22, 5.1), 120.0, 0).unwrap().unwrap().jumps, 5, "the same tank: a hit");
        assert_eq!(memo.get(&LegKey::new(1, 2, 22, 5.1), 128.0, 0).unwrap().unwrap().jumps, 5, "more fuel: a candidate the settle rounds verify, a hit");
        memo.put(LegKey::new(7, 8, 22, 5.1), 100.0, Err(RouteError::Budget));
        assert!(memo.get(&LegKey::new(7, 8, 22, 5.1), 90.0, 0).is_none(), "a budget failure is the clock's, not the leg's: not kept");
        assert!(memo.get(&LegKey::new(1, 2, 22, 5.1), 119.0, 0).is_none(), "less fuel: not the same leg");
        assert!(memo.get(&LegKey::new(1, 2, 21, 5.1), 128.0, 0).is_none(), "injections left are part of the leg");
        assert!(memo.get(&LegKey::new(1, 3, 22, 5.1), 128.0, 0).is_none(), "another destination");
        assert!(memo.get(&LegKey::new(1, 2, 22, 2.7), 128.0, 0).is_none(), "another reserve is another variant's leg");
        // A later, leaner plan replaces nothing: both are kept, the best fit answers.
        let three: Route = Route { jumps: 3, ..five.clone() };
        memo.put(LegKey::new(1, 2, 22, 5.1), 100.0, Ok(three));
        assert_eq!(memo.get(&LegKey::new(1, 2, 22, 5.1), 110.0, 0).unwrap().unwrap().jumps, 3, "the plan from the lower tank serves 110 t");
        assert_eq!(memo.get(&LegKey::new(1, 2, 22, 5.1), 125.0, 0).unwrap().unwrap().jumps, 5, "the plan from the higher tank serves 125 t");
        // A failure runs the other way: impossible from 100 t means impossible
        // from 90 t, and says nothing about 128 t.
        memo.put(LegKey::new(4, 5, 22, 5.1), 100.0, Err(RouteError::NoRoute));
        assert!(matches!(memo.get(&LegKey::new(4, 5, 22, 5.1), 90.0, 0), Some(Err(RouteError::NoRoute))), "less fuel: the failure stands");
        assert!(memo.get(&LegKey::new(4, 5, 22, 5.1), 128.0, 0).is_none(), "more fuel: not known to fail");
        NO_LEG_MEMO.put(LegKey::new(1, 2, 22, 5.1), 128.0, Err(RouteError::NoRoute));
        assert!(NO_LEG_MEMO.get(&LegKey::new(1, 2, 22, 5.1), 128.0, 0).is_none(), "the shared no-memo never remembers");
    }

    /// R0 observability (2026-10-17): every portfolio variant reports what
    /// it did, so the server can count outcomes per kind and the matrix can
    /// say which variant answered first on which route class.
    #[test]
    fn every_variant_reports_an_outcome() {
        let f = highway();
        let r = req(&f, "K0", "K3000", 60.0);
        let seen = std::sync::Mutex::new(Vec::<VariantOutcome>::new());
        let none = Control::none();
        let ctl = Control { legs: &NO_LEG_MEMO, variant: &|o: VariantOutcome| seen.lock().unwrap().push(o), ..none };
        let route = plan_best(&f.g, Some(&f.neutrons), &r, &ctl).unwrap();
        let seen = seen.into_inner().unwrap();
        assert!(!seen.is_empty(), "the portfolio ran and said nothing");
        assert!(seen.iter().all(|o| !o.kind.is_empty()), "every outcome names its variant");
        assert!(seen.iter().any(|o| o.ok && o.jumps == route.jumps as u32), "the winner is among the outcomes: {seen:?}");
        assert!(seen.iter().all(|o| o.wave >= 1));
    }

    #[test]
    fn a_mid_range_plot_takes_the_neutron_shortcut() {
        let f = mid_highway();
        assert_eq!(f.neutrons.count, 4);
        let r = req(&f, "K0", "K1000", 60.0);
        assert!(dist(f.g.pos_of(r.from), f.g.pos_of(r.to)) < LONG_ROUTE_LY, "this is the sub-threshold path");
        let exact = plan(&f.g, &r, &Control::none()).unwrap();
        assert_eq!(exact.jumps, 20, "the exact default planner walks the chain: {:?}", names(&exact));
        let route = plan_best(&f.g, Some(&f.neutrons), &r, &Control::none()).unwrap();
        assert_contiguous(&route, &r);
        // K0 -> N0 -> N1 -> N2 -> N3 -> K950 -> K1000: four x4 hops.
        assert!(route.jumps <= 6, "{} jumps: {:?}", route.jumps, names(&route));
        assert!(route.boosted_jumps >= 1, "{:?}", names(&route));
        assert_eq!(names(&route), ["K0", "N0", "N1", "N2", "N3", "K950", "K1000"]);
        assert_eq!((route.jumps, route.boosted_jumps), (6, 4));
        // Without supercharging there is no shortcut to take: the chain.
        let plain = RouteRequest { supercharge: false, ..r.clone() };
        assert_eq!(plan_best(&f.g, Some(&f.neutrons), &plain, &Control::none()).unwrap().jumps, 20);
    }

    #[test]
    fn the_corridor_check_finds_neutrons_near_the_line_only() {
        let f = mid_highway();
        let n = &f.neutrons;
        // N1 is at (280, 0, 10): 10 ly off the x axis.
        assert!(neutron_in_corridor(n, [0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], 10.0, true));
        assert!(!neutron_in_corridor(n, [0.0, 0.0, 0.0], [1000.0, 0.0, 0.0], 9.0, true));
        // A segment far above the highway, and one alongside but short of it.
        assert!(!neutron_in_corridor(n, [0.0, 600.0, 0.0], [1000.0, 600.0, 0.0], 240.0, true));
        assert!(neutron_in_corridor(n, [0.0, 600.0, 0.0], [1000.0, 600.0, 0.0], 601.0, true), "N1 is 600.08 ly below the line");
        assert!(!neutron_in_corridor(n, [1200.0, 0.0, 0.0], [2000.0, 0.0, 0.0], 240.0, true));
        assert!(neutron_in_corridor(n, [1200.0, 0.0, 0.0], [2000.0, 0.0, 0.0], 470.0, true), "N3 at x=740 is 460 ly from the near end");
        // A degenerate segment is a sphere.
        assert!(neutron_in_corridor(n, [50.0, 0.0, 0.0], [50.0, 0.0, 0.0], 10.5, true));
        assert!(!neutron_in_corridor(n, [50.0, 0.0, 0.0], [50.0, 0.0, 0.0], 9.5, true));
    }

    #[test]
    fn no_neutron_in_the_corridor_is_the_exact_planner_unchanged() {
        // The chain only, no highway: `plan_best` must be `plan` -- the same
        // hops and the same number of expansions, whether or not a (empty
        // or irrelevant) neutron index is offered.
        let stars: Vec<_> = (0..=20).map(|i| star(&format!("K{}", i * 50), (i * 50) as f32, 0.0, 0.0, K)).collect();
        let f = build(&stars);
        assert_eq!(f.neutrons.count, 0);
        let r = req(&f, "K0", "K1000", 60.0);
        let exact = plan(&f.g, &r, &Control::none()).unwrap();
        for neutrons in [None, Some(&f.neutrons)] {
            let best = plan_best(&f.g, neutrons, &r, &Control::none()).unwrap();
            assert_eq!(names(&best), names(&exact));
            assert_eq!(best.expansions, exact.expansions);
        }
        // Neutrons exist but none within a boosted hop of the line: still exact.
        let f = mid_highway();
        let r = RouteRequest { boost: BoostProfile { neutron: 4.0, white_dwarf: 1.5 }, ..req(&f, "K0", "K1000", 60.0) };
        let far = build(&{
            let mut v = stars.clone();
            v.push(star("Faraway", 500.0, 1000.0, 0.0, N));
            v
        });
        let rf = req(&far, "K0", "K1000", 60.0);
        assert_eq!(far.neutrons.count, 1);
        assert!(!wants_neutron_variant(&far.g, &far.neutrons, &rf));
        let best = plan_best(&far.g, Some(&far.neutrons), &rf, &Control::none()).unwrap();
        let exact = plan(&far.g, &rf, &Control::none()).unwrap();
        assert_eq!(names(&best), names(&exact));
        assert_eq!(best.expansions, exact.expansions);
        assert!(wants_neutron_variant(&f.g, &f.neutrons, &r), "the highway is in the corridor");
    }

    #[test]
    fn a_mid_range_plot_still_honours_cancel_and_budget() {
        let f = mid_highway();
        let r = req(&f, "K0", "K1000", 60.0);
        assert!(wants_neutron_variant(&f.g, &f.neutrons, &r));
        let ctl = Control { cancelled: &|| true, progress: &|_, _| {}, stage: &|_, _, _| {}, found: &|_| {}, legs: &NO_LEG_MEMO, variant: &|_| {}, trace: &|_, _, _| {} };
        // Both searches are tiny here; either returns Cancelled or finishes
        // before its first check -- a finished route must never be dropped
        // for a cancel that came after it.
        match plan_best(&f.g, Some(&f.neutrons), &r, &ctl) {
            Ok(_) | Err(RouteError::Cancelled) => {}
            other => panic!("{other:?}"),
        }
        // A budget both searches blow is Budget, not "no route": a 20 x 20
        // grid of K stars (the exact search floods it before its first
        // budget check at 200 expansions), a neutron in the corridor, and an
        // island 1,200 ly up that nothing reaches.
        let mut stars = Vec::new();
        for i in 0..20 {
            for j in 0..20 {
                stars.push(star(&format!("G{i}-{j}"), (i * 40) as f32, 0.0, (j * 40) as f32, K));
            }
        }
        stars.push(star("Pulsar", 40.0, 120.0, 40.0, N)); // on the line to the island
        stars.push(star("Island", 400.0, 1200.0, 400.0, K));
        let dense = build(&stars);
        let starved = RouteRequest { max_expansions: 1, ..req(&dense, "G0-0", "Island", 60.0) };
        assert!(dist(dense.g.pos_of(starved.from), dense.g.pos_of(starved.to)) < LONG_ROUTE_LY);
        assert!(wants_neutron_variant(&dense.g, &dense.neutrons, &starved));
        assert!(matches!(plan(&dense.g, &starved, &Control::none()), Err(RouteError::Budget)));
        assert!(matches!(plan_best(&dense.g, Some(&dense.neutrons), &starved, &Control::none()), Err(RouteError::Budget)));
        // And with no budget, the honest answer for the island.
        let open = RouteRequest { max_expansions: 0, ..starved.clone() };
        assert!(matches!(plan_best(&dense.g, Some(&dense.neutrons), &open, &Control::none()), Err(RouteError::NoRoute)));
        // Over the threshold the neutron-first planner alone is used, as before.
        let hw = highway();
        let long = req(&hw, "K0", "K3000", 60.0);
        let best = plan_best(&hw.g, Some(&hw.neutrons), &long, &Control::none()).unwrap();
        assert_eq!((best.jumps, best.boosted_jumps), (14, 13));
    }

    // ---- (g) fuel model -----------------------------------------------

    #[test]
    fn fuel_plan_refuels_only_at_scoopable_stars_and_never_strands_the_ship() {
        let f = highway();
        let m = mandalay();
        let boost = BoostProfile::default();
        let r = RouteRequest { fuel: Some(m), start_fuel: m.capacity, boost, range_ly: m.range_at(m.capacity), ..req(&f, "K0", "K3000", 60.0) };
        let route = plan_long(&f.g, &f.neutrons, &r, &Control::none()).unwrap();
        assert_contiguous(&route, &r);
        assert_fuel_consistent(&route, &m, &boost, m.capacity);
        // Thirteen boosted hops burn ~38 t on a 32 t tank: at least one scoop.
        assert!(route.refuel_stops >= 1, "{:?}", names(&route));
        assert!(route.hops.iter().filter(|h| h.refuel).all(|h| h.class == StarClass::K));
        assert!(route.boosted_jumps >= 12, "the highway is still flown: {} boosted", route.boosted_jumps);
        assert!(route.jumps <= 18, "{} jumps: {:?}", route.jumps, names(&route));
        // The exact planner on the same request obeys the same fuel physics.
        let exact = plan(&f.g, &r, &Control::none()).unwrap();
        assert_contiguous(&exact, &r);
        assert_fuel_consistent(&exact, &m, &boost, m.capacity);
    }

    #[test]
    fn a_low_tank_routes_to_a_scoopable_star_first() {
        // Goal 120 ly out. A brown dwarf sits on the line at 60 ly; a K star
        // 20 ly off it. With 4 t aboard the dwarf is a dead end (no fuel to
        // go on), so the plan must take the K star and scoop.
        let f = build(&[star("Start", 0.0, 0.0, 0.0, K), star("Dwarf", 60.0, 0.0, 0.0, L), star("Scoop", 60.0, 20.0, 0.0, K), star("Goal", 120.0, 0.0, 0.0, K)]);
        let m = mandalay();
        let boost = BoostProfile::default();
        let r = RouteRequest { fuel: Some(m), start_fuel: 4.0, boost, range_ly: m.range_at(m.capacity), ..req(&f, "Start", "Goal", 60.0) };
        assert!(m.jump(60.0, 4.0, 1.0).is_some(), "the dwarf is in reach on 4 t");
        let route = plan(&f.g, &r, &Control::none()).unwrap();
        assert_contiguous(&route, &r);
        assert_fuel_consistent(&route, &m, &boost, 4.0);
        assert_eq!(names(&route), ["Start", "Scoop", "Goal"]);
        assert!(route.hops[1].refuel && route.hops[1].scoopable);
        assert_eq!(route.hops[1].fuel_after, Some(m.capacity));
        // The K goal is topped up on arrival too, so it counts as a stop.
        assert_eq!(route.refuel_stops, 2);
    }

    /// Past the slice, a single (it has coarse stats) yields unless its
    /// coarse chain reached the goal -- the exact-leg refinement of a
    /// STALLED chain is what the slice exists to cut (the Jongou XM-W
    /// trace: five singles 26-27 s refining a 10,000 ly stall while the
    /// scan twins that carry the 402-jump route waited); a bidi has no
    /// coarse stats and keeps the refining exemption instead.
    #[test]
    fn a_stalled_single_yields_at_the_slice_even_while_refining() {
        assert!(heavy_yields(true, true, false, true), "a single refining a stalled chain yields");
        assert!(!heavy_yields(true, true, true, false), "a single whose chain reached the goal finishes");
        assert!(!heavy_yields(true, false, false, true), "a bidi in refinement finishes");
        assert!(heavy_yields(true, false, false, false), "a bidi still searching yields");
        assert!(!heavy_yields(false, true, false, true), "nothing yields before the slice");
    }

    /// The heavy variants' first-wave slice (the maintainer's "hop between
    /// techniques on a stall"): an eighth of the budget, first wave only,
    /// ED_SLICE_MS overriding (0 = off).
    #[test]
    fn the_heavy_slice_is_an_eighth_of_the_budget_in_the_first_wave() {
        assert_eq!(heavy_slice_ms(100_000, true, None), Some(12_500), "the long lane");
        assert_eq!(heavy_slice_ms(30_000, true, None), Some(3_750), "the interactive lane");
        assert_eq!(heavy_slice_ms(0, true, None), Some(12_500), "no budget: the long lane's slice");
        assert_eq!(heavy_slice_ms(100_000, false, None), None, "escalation waves have no slice");
        assert_eq!(heavy_slice_ms(100_000, true, Some(0)), None, "ED_SLICE_MS=0 turns it off");
        assert_eq!(heavy_slice_ms(100_000, true, Some(20_000)), Some(20_000), "the knob's value stands");
    }

    /// "Spend a little" (the maintainer, 2026-10-18): the first wave is not
    /// guillotined before the quality floor -- ten seconds, a tenth of a
    /// short budget -- so the slow diggers get a worker on the plots the
    /// quick variants answer in a second (eight of the 28 pinned routes
    /// ended 1 to 7 jumps over the external reference in 0.6 to 6.8 s with
    /// 90 s of budget unused). Escalation waves keep their own clocks and
    /// no floor; ED_QUALITY_FLOOR_MS overrides, 0 turns it off.
    #[test]
    fn the_quality_floor_is_the_first_waves_and_a_tenth_of_the_budget() {
        use std::time::Duration;
        let ms = Duration::from_millis;
        assert_eq!(quality_floor(100_000, true, None), ms(10_000), "the long lane: ten seconds");
        assert_eq!(quality_floor(30_000, true, None), ms(3_000), "the interactive lane: a tenth of its budget");
        assert_eq!(quality_floor(0, true, None), ms(10_000), "no budget: the full floor");
        assert_eq!(quality_floor(100_000, false, None), Duration::ZERO, "escalation waves have no floor");
        assert_eq!(quality_floor(100_000, true, Some(0)), Duration::ZERO, "ED_QUALITY_FLOOR_MS=0 turns it off");
        assert_eq!(quality_floor(100_000, true, Some(20_000)), ms(20_000), "the knob's value stands, uncapped");
    }

    /// An armed clock waits for the floor and cuts at once past it; the
    /// floor never arms a clock by itself.
    #[test]
    fn the_grace_clock_does_not_expire_before_the_quality_floor() {
        use std::time::{Duration, Instant};
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        let mut clock = GraceClock::new(None);
        clock.finish(t0 + ms(300), ms(300), 1_000, true);
        let grace = ms(1_000);
        assert!(clock.expired(t0 + ms(900), grace, None), "no floor: the 500 ms window has passed");
        let floor = Some(t0 + ms(10_000));
        assert!(!clock.expired(t0 + ms(900), grace, floor), "under the floor the window waits");
        assert!(!clock.expired(t0 + ms(9_999), grace, floor));
        assert!(clock.expired(t0 + ms(10_001), grace, floor), "past the floor the expired window cuts at once");
        let unarmed = GraceClock::new(None);
        assert!(!unarmed.expired(t0 + ms(20_000), grace, floor), "the floor never arms a clock");
    }

    /// The measured shape: fast plots stop waiting on losers (a 337 ms
    /// first route gets the 500 ms floor, not the configured second),
    /// slower firsts get proportional room, slow plots keep the full
    /// configured window, and a caller asking for LESS than the floor is
    /// honoured.
    #[test]
    fn grace_adapts_to_time_to_first_within_the_configured_cap() {
        use std::time::Duration;
        let ms = Duration::from_millis;
        assert_eq!(effective_grace(ms(1_000), ms(337)), ms(500), "fast first: the floor");
        assert_eq!(effective_grace(ms(1_000), ms(800)), ms(800), "proportional");
        // A slow first finisher EXTENDS the window past the configured
        // value: a quick mediocre variant (bidi on Sol->Sagittarius A*)
        // must not cancel a better route that needs its proportional
        // share -- the 114-jump Mandalay route missed a capped window by
        // 15 ms and shipped 136 jumps instead.
        assert_eq!(effective_grace(ms(1_000), ms(1_739)), ms(1_739), "slow first: proportional past the cap");
        assert_eq!(effective_grace(ms(1_000), ms(9_000)), ms(4_000), "but never more than 4x configured");
        assert_eq!(effective_grace(ms(200), ms(50)), ms(200), "a small configured grace stays authoritative below the floor");
    }

    #[test]
    fn a_min_fuel_plan_skips_the_topups_the_route_does_not_need() {
        // Three K stars in a line, full tank: the default plan tops up at
        // both scoopable arrivals; the min-fuel plan makes the same jumps
        // and never stops, and its fuel projection is the honest no-scoop
        // drain, never below one max-effort escape jump.
        let f = build(&[star("A", 0.0, 0.0, 0.0, K), star("B", 50.0, 0.0, 0.0, K), star("C", 100.0, 0.0, 0.0, K)]);
        let m = mandalay();
        let boost = BoostProfile::default();
        let base = RouteRequest { fuel: Some(m), start_fuel: m.capacity, boost, range_ly: m.range_at(m.capacity), ..req(&f, "A", "C", 60.0) };
        let eager = plan_best(&f.g, Some(&f.neutrons), &base, &Control::none()).unwrap();
        assert!(eager.refuel_stops >= 1, "the default plan tops up in passing: {:?}", names(&eager));
        let lean = plan_best(&f.g, Some(&f.neutrons), &RouteRequest { min_fuel: true, ..base }, &Control::none()).unwrap();
        assert_eq!(names(&lean), names(&eager), "min-fuel changes stops, not the route");
        assert_eq!(lean.refuel_stops, 0, "{:?}", lean.hops.iter().map(|h| (h.name.clone(), h.refuel, h.fuel_after)).collect::<Vec<_>>());
        assert!(lean.hops.iter().all(|h| !h.refuel));
        let drained: Vec<f32> = lean.hops.iter().map(|h| h.fuel_after.unwrap()).collect();
        assert!(drained.windows(2).all(|w| w[1] < w[0]), "no scoop, so the tank drains: {drained:?}");
        assert!(drained.iter().all(|&t| t >= m.max_fuel_per_jump), "always one escape jump in hand: {drained:?}");
    }

    #[test]
    fn a_min_fuel_plan_scoops_at_the_last_star_before_a_dry_stretch() {
        // Two K stars, then three unscoopable L dwarfs, then a K goal, 50 ly
        // apart, leaving with 12 t of 32: skipping every scoop dips under
        // the escape-jump floor mid-dry-stretch, so the plan stops exactly
        // once, at B -- the last scoopable before the stretch -- and not at
        // the goal, which the default plan would also top up.
        let f = build(&[
            star("A", 0.0, 0.0, 0.0, K),
            star("B", 50.0, 0.0, 0.0, K),
            star("C", 100.0, 0.0, 0.0, L),
            star("D", 150.0, 0.0, 0.0, L),
            star("E", 200.0, 0.0, 0.0, L),
            star("F", 250.0, 0.0, 0.0, K),
        ]);
        let m = mandalay();
        let boost = BoostProfile::default();
        let base = RouteRequest { fuel: Some(m), start_fuel: 12.0, boost, range_ly: m.range_at(m.capacity), ..req(&f, "A", "F", 60.0) };
        let lean = plan_best(&f.g, Some(&f.neutrons), &RouteRequest { min_fuel: true, ..base }, &Control::none()).unwrap();
        assert_eq!(names(&lean), ["A", "B", "C", "D", "E", "F"]);
        let stops: Vec<&str> = lean.hops.iter().filter(|h| h.refuel).map(|h| h.name.as_str()).collect();
        assert_eq!(stops, ["B"], "one stop, at the last scoopable before the dry stretch");
        assert_eq!(lean.refuel_stops, 1);
        assert_eq!(lean.hops[1].fuel_after, Some(m.capacity), "the one stop fills the tank");
        assert!(lean.hops.iter().skip(1).all(|h| h.fuel_after.unwrap() >= m.max_fuel_per_jump - 0.05), "the floor holds once a scoop can provide it: {:?}", lean.hops.iter().map(|h| h.fuel_after).collect::<Vec<_>>());
    }

    /// Item 12: the bidirectional variant alone finds the same-quality
    /// route as the single-direction portfolio on the clean highway, and
    /// on a fan fixture -- a dense thicket of neutrons at the goal end
    /// that balloons the uphill frontier -- it meets in the middle with
    /// far fewer expansions than the single-direction search from the
    /// thin end.
    #[test]
    fn the_bidirectional_variant_matches_quality_and_meets_in_the_middle() {
        let hw = highway();
        let r = req(&hw, "K0", "K3000", 60.0);
        let none = std::sync::atomic::AtomicBool::new(false);
        let single = plan_long_with(&hw.g, &hw.neutrons, &r, &Control::none(), COARSE_WEIGHT, RESERVE_FRACTION, 0.5, 1, false, false, false, None, &none, &CoarseStats::default(), None, false, &NO_BANS, &NO_INCUMBENT).unwrap();
        let both = plan_long_bidi(&hw.g, &hw.neutrons, &r, &Control::none(), COARSE_WEIGHT, 1, false, false, [None, None]).unwrap();
        assert_eq!(both.hops.first().unwrap().name, "K0");
        assert_eq!(both.hops.last().unwrap().name, "K3000");
        assert!(
            both.jumps <= single.jumps + 1,
            "bidi must not degrade the highway: {} vs {}",
            both.jumps, single.jumps
        );

        // The fan: a thin line from Start eastward, ending in a dense
        // neutron thicket around the goal. Uphill (thin -> dense) the
        // single search wades through the whole fan; bidi's backward
        // frontier starts inside it and meets the thin line early.
        // Geometry flyable at range 70: the first neutron 60 ly from the
        // K start (plain hop), 180 ly line and fan spacing (boosted
        // x4 = 280), the K goal 60 ly off a fan-edge neutron.
        let mut stars = vec![star("Start", 0.0, 0.0, 0.0, K)];
        for i in 0..10 {
            stars.push(star(&format!("Ln{i}"), 60.0 + i as f32 * 180.0, 0.0, 0.0, N));
        }
        for i in 0..14 {
            for j in 0..14 {
                stars.push(star(&format!("F{i}x{j}"), 1_800.0 + i as f32 * 180.0, 0.0, (j as f32 - 6.5) * 180.0, N));
            }
        }
        stars.push(star("Goal", 4_200.0, 0.0, 90.0, K));
        let fan = build(&stars);
        let rf = req(&fan, "Start", "Goal", 70.0);
        let single = plan_long_with(&fan.g, &fan.neutrons, &rf, &Control::none(), COARSE_WEIGHT, RESERVE_FRACTION, 0.5, 1, false, false, false, None, &none, &CoarseStats::default(), None, false, &NO_BANS, &NO_INCUMBENT).unwrap();
        let both = plan_long_bidi(&fan.g, &fan.neutrons, &rf, &Control::none(), COARSE_WEIGHT, 1, false, false, [None, None]).unwrap();
        assert_eq!(both.hops.first().unwrap().name, "Start");
        assert_eq!(both.hops.last().unwrap().name, "Goal");
        assert!(both.jumps <= single.jumps + 1, "{} vs {}", both.jumps, single.jumps);
        // The EXPANSION win cannot be shown at fixture scale (settle
        // windows swamp toy graphs; a toy fan is beelined either way) --
        // that claim lives with the full-index gates in ROUTING-NEXT 12:
        // Wongi->Beagle <=2 s at <=196 jumps, Blaa Hypai <=1 s,
        // Sol->Sagittarius A* Mandalay <=1 s.
    }

    /// The retrograde variant plans the descent, reverses the skeleton,
    /// and refines forward: the result must be a valid START -> GOAL
    /// route flown with the real (forward) fuel.
    #[test]
    fn the_retrograde_variant_flies_forward_from_a_reversed_descent() {
        let mut stars: Vec<(String, f32, f32, f32, &str)> = Vec::new();
        for i in 0..12 {
            stars.push(star(&format!("N{i}"), i as f32 * 250.0, 0.0, 10.0, N));
        }
        stars.push(star("Start", 0.0, 0.0, 0.0, K));
        stars.push(star("Goal", 2_750.0, 0.0, 0.0, K));
        let f = build(&stars);
        let r = req(&f, "Start", "Goal", 70.0);
        let route = plan_long_retro(&f.g, &f.neutrons, &r, &Control::none(), 1, None).unwrap();
        assert_eq!(route.hops.first().unwrap().name, "Start");
        assert_eq!(route.hops.last().unwrap().name, "Goal");
        assert!(route.boosted_jumps >= 5, "the chain flies boosted: {}", route.boosted_jumps);
    }

    /// The graph-first planner crosses a highway gap the neutron search
    /// can only stall against: two neutron arms, a 3,100 ly void between
    /// them bridged by plain K stars (flyable, just unboosted), a K goal
    /// past the east arm. Without graph250.bin the portfolio behaves as
    /// today; with it, the graph-first seed still finds a route and the
    /// tie-break keeps whichever is better -- never worse.
    #[test]
    fn a_graph_first_seed_crosses_the_gap_and_never_worsens_the_route() {
        let mut stars: Vec<(String, f32, f32, f32, &str)> = Vec::new();
        for i in 0..8 {
            stars.push(star(&format!("WN{i}"), i as f32 * 250.0, 0.0, 10.0, N));
        }
        for i in 0..62 {
            stars.push(star(&format!("B{i}"), 2_000.0 + i as f32 * 50.0, 0.0, 0.0, K));
        }
        for i in 0..8 {
            stars.push(star(&format!("EN{i}"), 5_100.0 + i as f32 * 250.0, 0.0, 10.0, N));
        }
        stars.push(star("Start", 0.0, 0.0, 0.0, K));
        stars.push(star("Goal", 7_000.0, 0.0, 0.0, K));
        let f = build(&stars);
        // Range 70: arm hops (250 ly) fly boosted (280), bridge hops
        // (50 ly) fly plain, and the widen radius (~910 ly) cannot see
        // across the 3,100 ly void -- but the DESERT WIDEN (5.3-v2)
        // doubles the scan until the far shore appears, no sidecar
        // needed, so the bidi variant crosses and the whole portfolio
        // routes what used to be NoRoute.
        let r = req(&f, "Start", "Goal", 70.0);
        let solo = plan_long_bidi(&f.g, &f.neutrons, &r, &Control::none(), COARSE_WEIGHT, 1, false, false, [None, None])
            .expect("the desert widen must cross the void");
        assert_eq!(solo.hops.first().unwrap().name, "Start");
        assert_eq!(solo.hops.last().unwrap().name, "Goal");
        assert!(solo.boosted_jumps >= 8, "arms flown boosted: {}", solo.boosted_jumps);
        let after = plan_long(&f.g, &f.neutrons, &r, &Control::none()).unwrap();
        assert_eq!(after.hops.first().unwrap().name, "Start");
        assert_eq!(after.hops.last().unwrap().name, "Goal");
        assert!(after.boosted_jumps >= 8, "both arms are flown boosted: {}", after.boosted_jumps);
        assert!(after.jumps < 90, "the bridge is ~62 plain hops plus the arms: {}", after.jumps);
    }

    /// The precheck deciding whether a plot consults the ALT oracle: a
    /// desert dominating the line reads as a non-uniform bound/euclid
    /// profile (keep ALT), a clean corridor reads uniform (the oracle
    /// is pure overhead there -- drop it). Pinned because a regression
    /// to always-false shows up in no bench except as a silently
    /// missing ~2x on void topology.
    #[test]
    fn the_alt_precheck_keeps_the_oracle_for_a_desert_and_drops_it_on_a_corridor() {
        // Two neutron arms, a 2,350 ly void between: the graph prices
        // the crossing in plain jumps, so bounds from the far arm back
        // to the start cell dwarf euclid while near-arm bounds track it.
        let mut stars: Vec<(String, f32, f32, f32, &str)> = Vec::new();
        for i in 0..8 {
            stars.push(star(&format!("WN{i}"), i as f32 * 250.0, 0.0, 10.0, N));
        }
        for i in 0..8 {
            stars.push(star(&format!("EN{i}"), 5_100.0 + i as f32 * 250.0, 0.0, 10.0, N));
        }
        stars.push(star("Start", 0.0, 0.0, 0.0, K));
        stars.push(star("Goal", 7_000.0, 0.0, 0.0, K));
        let f = build(&stars);
        let graph = crate::cgraph::build(&f.neutrons).unwrap();
        let alt = crate::alt::build(&f.neutrons, &graph).unwrap();
        let (start, goal) = ([0.0, 0.0, 0.0], [7_000.0, 0.0, 0.0]);
        let start_cell = f.neutrons.cell_index_of_pos(start).expect("WN0 shares the start cell");
        assert!(
            alt_line_is_nonuniform(&f.neutrons, &alt, start, goal, start, start_cell, 70.0),
            "a desert on the line must read as non-uniform"
        );

        // The clean highway: every sampled bound tracks euclid.
        let hw = highway();
        let graph = crate::cgraph::build(&hw.neutrons).unwrap();
        let alt = crate::alt::build(&hw.neutrons, &graph).unwrap();
        let target = [50.0 + 230.0 * 12.0, 0.0, 10.0]; // N12, the far end of the lane
        let target_cell = hw.neutrons.cell_index_of_pos(target).expect("N12's cell");
        assert!(
            !alt_line_is_nonuniform(&hw.neutrons, &alt, [0.0, 0.0, 0.0], target, target, target_cell, 60.0),
            "a uniform corridor must drop the oracle"
        );
    }

    /// A wall the frontier can SKIRT is where the ratio test alone is
    /// blind (the Mac's synthetic, caught in cross-machine validation):
    /// a bypass lane makes the detour cheap, so surviving shore samples
    /// inflate by almost nothing -- yet the void still sits on the
    /// line, its samples have no cell at all, and steering around it is
    /// exactly where ALT's 2x lives. An interior missing-cell run
    /// flanked by occupied samples must keep the oracle on its own.
    #[test]
    fn the_alt_precheck_reads_a_cell_less_gap_on_the_line_as_the_desert_it_is() {
        let mut stars: Vec<(String, f32, f32, f32, &str)> = Vec::new();
        for i in 0..8 {
            stars.push(star(&format!("WN{i}"), i as f32 * 250.0, 0.0, 10.0, N));
        }
        for i in 0..8 {
            stars.push(star(&format!("EN{i}"), 5_100.0 + i as f32 * 250.0, 0.0, 10.0, N));
        }
        // The bypass: a lane offset 1,100 ly in y, reached by stepped
        // entries. Since the 18d honest-edge pricing, "mild" means the
        // STAR pairs along the ladder stay within boosted reach (470) --
        // a 550 ly step is honestly ~1.9 jumps now and would inflate
        // the shore ratios past the threshold this fixture must stay
        // under. The lane itself stays at 1,100 ly: far enough that no
        // single diagonal skirts the wall.
        for (n, y) in [("A1", 440.0), ("A2", 880.0)] {
            stars.push(star(n, 1_750.0, y, 10.0, N));
        }
        for (n, y) in [("B1", 440.0), ("B2", 880.0)] {
            stars.push(star(n, 5_100.0, y, 10.0, N));
        }
        let mut i = 0;
        let mut x = 1_750.0;
        while x <= 5_100.0 {
            stars.push(star(&format!("BP{i}"), x, 1_100.0, 10.0, N));
            i += 1;
            x += 250.0;
        }
        let f = build(&stars);
        let graph = crate::cgraph::build(&f.neutrons).unwrap();
        let alt = crate::alt::build(&f.neutrons, &graph).unwrap();
        let (start, goal) = ([0.0, 0.0, 10.0], [6_850.0, 0.0, 10.0]);
        let target_cell = f.neutrons.cell_index_of_pos(goal).expect("EN7's cell");
        // The fixture only pins the gap rule if the ratio rule really is
        // blind here: a west-shore sample's bound must stay dilute.
        let shore = f.neutrons.cell_index_of_pos([1_522.0, 0.0, 10.0]).expect("west-arm cell");
        let e = dist([1_522.0, 0.0, 10.0], goal);
        assert!(
            alt.lower_bound_ly(shore, target_cell) < e * 1.3,
            "the bypass must keep shore ratios under the threshold, or this fixture tests nothing"
        );
        assert!(
            alt_line_is_nonuniform(&f.neutrons, &alt, start, goal, goal, target_cell, 70.0),
            "a skirtable wall must keep ALT via its cell-less samples, not its diluted ratios"
        );
    }

    // ---- (j) neutron sub-index discovery -------------------------------

    #[test]
    fn neutron_dir_is_named_for_its_grid_and_must_exist_with_the_right_version() {
        assert_eq!(neutron_dir(Path::new("/idx")), Path::new("/idx/boost250"));
        let dir = tempfile::tempdir().unwrap();
        let missing = neutron_dir(dir.path());
        assert!(!Galaxy::exists(&missing));
        assert!(Galaxy::open(&missing).is_err());

        let f = highway();
        let ndir = neutron_dir(f.g.dir.as_path());
        assert!(Galaxy::exists(&ndir));
        assert_eq!(f.neutrons.cell_ly, NEUTRON_CELL_LY);
        assert_eq!(f.neutrons.count, 13);
        // Every sub-index record resolves back into the full index by name.
        for i in 0..f.neutrons.count as u32 {
            let rec = f.neutrons.record(i);
            assert_eq!(f.neutrons.class(&rec), StarClass::Neutron);
            let full = f.g.find(f.neutrons.name(&rec)).expect("neutron name resolves in the full index");
            assert_eq!(f.g.record(full).id64, rec.id64);
            assert_eq!(f.g.pos_of(full), rec.pos());
        }

        // A future format version is refused, not misread.
        let copy = tempfile::tempdir().unwrap();
        for p in Galaxy::paths(&ndir) {
            std::fs::copy(&p, copy.path().join(p.file_name().unwrap())).unwrap();
        }
        let stars = copy.path().join("stars.bin");
        let mut bytes = std::fs::read(&stars).unwrap();
        bytes[4..8].copy_from_slice(&(crate::format::VERSION + 1).to_le_bytes());
        std::fs::write(&stars, bytes).unwrap();
        let err = match Galaxy::open(copy.path()) {
            Ok(_) => panic!("a future format version must be refused"),
            Err(e) => e.to_string(),
        };
        assert!(err.contains("version"), "{err}");
    }

    // ---- real-index regressions (skipped without the fixture) ----------

    /// The populated EDGX v2 index (145,579 systems), gitignored at the
    /// repo root. Override with `EDDA_GALAXY_FIXTURE`.
    fn populated_fixture() -> Option<std::path::PathBuf> {
        if let Some(p) = std::env::var_os("EDDA_GALAXY_FIXTURE") {
            let p = std::path::PathBuf::from(p);
            return Galaxy::exists(&p).then_some(p);
        }
        Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().map(|a| a.join(".data-fixtures/galaxy_populated")).find(|p| Galaxy::exists(p))
    }

    struct Populated {
        _tmp: Option<tempfile::TempDir>,
        g: Galaxy,
        neutrons: Galaxy,
    }

    fn populated() -> Option<Populated> {
        let Some(dir) = populated_fixture() else {
            eprintln!("skipping: populated galaxy fixture not found (set EDDA_GALAXY_FIXTURE)");
            return None;
        };
        let g = Galaxy::open(&dir).unwrap();
        let ndir = neutron_dir(&dir);
        let (tmp, neutrons) = if Galaxy::exists(&ndir) {
            (None, Galaxy::open(&ndir).unwrap())
        } else {
            let tmp = tempfile::tempdir().unwrap();
            subset_cells(&g, tmp.path(), NEUTRON_CELL_LY, |r| highway_star(StarClass::from_code(r.class))).unwrap();
            let n = Galaxy::open(tmp.path()).unwrap();
            (Some(tmp), n)
        };
        Some(Populated { _tmp: tmp, g, neutrons })
    }

    /// The Explorer Mk II as `examples/bench.rs` models it.
    fn explorer(g: &Galaxy, from: &str, to: &str) -> RouteRequest {
        let model = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
        RouteRequest {
            from: g.find(from).unwrap_or_else(|| panic!("unknown {from}")),
            to: g.find(to).unwrap_or_else(|| panic!("unknown {to}")),
            range_ly: model.range_at(model.capacity),
            supercharge: true,
            boost: BoostProfile::MK2_SCO,
            fuel: Some(model),
            start_fuel: model.capacity,
            ..Default::default()
        }
    }

    /// White dwarfs are opt-in: the request as every default caller sends it.
    fn without_white_dwarfs(r: RouteRequest) -> RouteRequest {
        RouteRequest { boost: BoostProfile { white_dwarf: 1.0, ..r.boost }, ..r }
    }

    /// Measured on the populated fixture (`examples/bench.rs`, Explorer Mk II,
    /// white dwarfs off).
    struct Pinned {
        jumps: usize,
        ly: f32,
        boosted: usize,
        refuels: usize,
        expansions: u64,
    }

    fn pin(p: &Populated, from: &str, to: &str, want: Pinned) -> Route {
        let Pinned { jumps, ly, boosted, refuels, expansions } = want;
        let r = without_white_dwarfs(explorer(&p.g, from, to));
        assert!(dist(p.g.pos_of(r.from), p.g.pos_of(r.to)) <= LONG_ROUTE_LY, "{from} -> {to} is a short plot");
        let route = plan(&p.g, &r, &Control::none()).unwrap();
        assert_contiguous(&route, &r);
        assert_fuel_consistent(&route, r.fuel.as_ref().unwrap(), &r.boost, r.start_fuel);
        assert_eq!(route.jumps, jumps, "{from} -> {to} jumps: {:?}", names(&route));
        assert!((route.total_ly - ly).abs() <= 0.5, "{from} -> {to} flew {:.1} ly, expected {ly}", route.total_ly);
        assert_eq!(route.boosted_jumps, boosted, "{from} -> {to} boosted");
        assert_eq!(route.refuel_stops, refuels, "{from} -> {to} refuel stops");
        assert_eq!(route.expansions, expansions, "{from} -> {to} expansions");
        route
    }

    /// Opted in, the plot boosts off the D-class MY Apodis and saves a jump.
    #[test]
    #[ignore = "needs the populated galaxy fixture; run with --ignored"]
    fn populated_wongi_to_deciat_with_white_dwarfs_is_two_jumps_via_my_apodis() {
        let Some(p) = populated() else { return };
        let started = std::time::Instant::now();
        let r = explorer(&p.g, "Wongi", "Deciat");
        let route = plan_best(&p.g, Some(&p.neutrons), &r, &Control::none()).unwrap();
        assert_contiguous(&route, &r);
        assert_fuel_consistent(&route, r.fuel.as_ref().unwrap(), &r.boost, r.start_fuel);
        assert_eq!(route.jumps, 2, "{:?}", names(&route));
        // The one boosted hop leaves MY Apodis, a D-class white dwarf.
        let boosted: Vec<&str> = route.hops.iter().zip(route.hops.iter().skip(1)).filter(|(_, next)| next.boosted).map(|(from, _)| from.name.as_str()).collect();
        assert_eq!(boosted, vec!["MY Apodis"], "{:?}", names(&route));
        assert_eq!(route.boosted_jumps, 1);
        // A regression guard, not a benchmark.
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "Wongi -> Deciat took {:?}", started.elapsed());
    }

    /// The default plot (white dwarfs off) is the three-jump plain chain.
    #[test]
    #[ignore = "needs the populated galaxy fixture; run with --ignored"]
    fn populated_wongi_to_deciat_is_three_jumps_in_402_expansions() {
        let Some(p) = populated() else { return };
        let started = std::time::Instant::now();
        pin(&p, "Wongi", "Deciat", Pinned { jumps: 3, ly: 158.0, boosted: 0, refuels: 3, expansions: 402 });
        // A regression guard, not a benchmark: this is 54 ms in release.
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "Wongi -> Deciat took {:?}", started.elapsed());
    }

    #[test]
    #[ignore = "needs the populated galaxy fixture; run with --ignored"]
    fn populated_wongi_to_maia_is_seven_jumps_in_7_expansions() {
        let Some(p) = populated() else { return };
        pin(&p, "Wongi", "Maia", Pinned { jumps: 7, ly: 492.0, boosted: 0, refuels: 7, expansions: 7 });
    }

    #[test]
    #[ignore = "needs the populated galaxy fixture; run with --ignored"]
    fn populated_wongi_to_sol_is_two_jumps_in_16_expansions() {
        let Some(p) = populated() else { return };
        pin(&p, "Wongi", "Sol", Pinned { jumps: 2, ly: 118.0, boosted: 0, refuels: 2, expansions: 16 });
    }

    #[test]
    #[ignore = "needs the populated galaxy fixture; run with --ignored"]
    fn populated_deciat_to_sol_is_two_jumps_in_2_expansions() {
        let Some(p) = populated() else { return };
        pin(&p, "Deciat", "Sol", Pinned { jumps: 2, ly: 132.0, boosted: 0, refuels: 2, expansions: 2 });
    }

    #[test]
    #[ignore = "needs the populated galaxy fixture; run with --ignored"]
    fn populated_wongi_to_maia_at_37_6_ly_is_fourteen_jumps() {
        // `examples/route.rs`: no fuel model, default boost, 37.6 ly.
        let Some(p) = populated() else { return };
        let r = RouteRequest { from: p.g.find("Wongi").unwrap(), to: p.g.find("Maia").unwrap(), range_ly: 37.6, ..Default::default() };
        let route = plan(&p.g, &r, &Control::none()).unwrap();
        assert_contiguous(&route, &r);
        assert_eq!(route.jumps, 14, "{:?}", names(&route));
        assert!((route.total_ly - 503.1).abs() <= 0.5, "{}", route.total_ly);
        assert_eq!(route.boosted_jumps, 0);
        assert_eq!(route.expansions, 15);
    }

    #[test]
    #[ignore = "needs the populated galaxy fixture; run with --ignored"]
    fn populated_wongi_to_colonia_has_no_route_on_the_bubble_only_index() {
        let Some(p) = populated() else { return };
        let r = explorer(&p.g, "Wongi", "Colonia");
        assert!(dist(p.g.pos_of(r.from), p.g.pos_of(r.to)) > LONG_ROUTE_LY);
        let started = std::time::Instant::now();
        assert!(matches!(plan_long(&p.g, &p.neutrons, &r, &Control::none()), Err(RouteError::NoRoute)));
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "took {:?}", started.elapsed());
    }

    // ---- full-galaxy regressions (the gaming PC only) --------------------

    /// The full EDGX v2 index with its `boost250/` sub-index, named by
    /// `EDDA_GALAXY_FULL_INDEX`. Measured 2026-08-30 on the PC with
    /// `examples/bench.rs` (199 M systems, 3,853,782 highway stars).
    /// Default (white dwarfs off): Explorer Mk II Wongi -> Colonia 58
    /// jumps in 1,320 ms, Sol -> Sagittarius A* 69 jumps in 596 ms, Wongi
    /// -> Maia 4 jumps; Mandalay Wongi -> Colonia 93 jumps in 1,285 ms.
    /// Opted in: 57 jumps in 1,208 ms, 66 in 310 ms; Mandalay 94 in 929
    /// ms. The pins are ceilings, so a better route passes; a reintroduced
    /// lock in the hot loop or broken stitching does not. Their time limits
    /// assume the machine to themselves: run with `--test-threads=1`.
    /// The long-range planner (the pins' path) honours the switch at the
    /// origin: departing a black-hole system whose neutron sits 3 ls out
    /// gets the x4 hop when the run is allowed, and not when it is not.
    /// (2026-09-11: the probe found the first jump out of Sphoetz BQ-Y f127
    /// unboosted at 10,000 ls — the switch had only reached the exact
    /// search.)
    #[test]
    fn plan_best_boosts_out_of_a_black_hole_with_a_neutron_secondary() {
        let json = r#"[
{"id64":1,"name":"Hole","coords":{"x":0,"y":0,"z":0},"bodies":[{"type":"Star","subType":"Black Hole","mainStar":true},{"type":"Star","subType":"Neutron Star","mainStar":false,"distanceToArrival":3.0}]},
{"id64":2,"name":"Far","coords":{"x":100,"y":0,"z":0},"bodies":[{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}]}
]"#;
        let dir = tempfile::tempdir().unwrap();
        crate::import::import_reader(Box::new(std::io::Cursor::new(json.as_bytes().to_vec())), dir.path(), &mut |_| {}).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        let base = RouteRequest { from: g.find("Hole").unwrap(), to: g.find("Far").unwrap(), range_ly: 30.0, supercharge: true, ..Default::default() };
        assert!(plan_best(&g, None, &base, &Control::none()).is_err(), "off: 100 ly is beyond a 30 ly ship");
        let on = RouteRequest { secondary_boost_ls: 10.0, ..base };
        let route = plan_best(&g, None, &on, &Control::none()).unwrap();
        assert_eq!((route.jumps, route.boosted_jumps, route.secondary_boosts), (1, 1, 1));
        assert_eq!(route.hops[1].via_secondary, Some((crate::StarClass::Neutron, 3.0)));
    }

    fn full_index() -> Option<(Galaxy, Galaxy)> {
        let Some(dir) = std::env::var_os("EDDA_GALAXY_FULL_INDEX") else {
            eprintln!("skipping: set EDDA_GALAXY_FULL_INDEX to the full galaxy index");
            return None;
        };
        let dir = std::path::PathBuf::from(dir);
        let ndir = neutron_dir(&dir);
        assert!(Galaxy::exists(&ndir), "full index needs its neutron sub-index at {}", ndir.display());
        Some((Galaxy::open(&dir).unwrap(), Galaxy::open(&ndir).unwrap()))
    }

    /// The Mandalay as `examples/bench.rs --ship mandalay` models it: a
    /// standard drive, x4 neutron / x1.5 white dwarf.
    fn mandalay_request(g: &Galaxy, from: &str, to: &str) -> RouteRequest {
        // bench.rs's loadout, not `mandalay()` above: the pin matches the
        // measurement (5.0 t max fuel per jump; the 5.2 t model plots 95).
        let model = FuelModel::from_loadout(319.2, 32.0, 5.0, 5, true, false, 77.86, 10.5, 0.0);
        RouteRequest {
            from: g.find(from).unwrap_or_else(|| panic!("unknown {from}")),
            to: g.find(to).unwrap_or_else(|| panic!("unknown {to}")),
            range_ly: model.range_at(model.capacity),
            supercharge: true,
            boost: BoostProfile::default(),
            fuel: Some(model),
            start_fuel: model.capacity,
            ..Default::default()
        }
    }

    fn full_pin(g: &Galaxy, neutrons: &Galaxy, r: &RouteRequest, label: &str, max_jumps: usize, within: std::time::Duration) -> Route {
        let started = std::time::Instant::now();
        let route = plan_best(g, Some(neutrons), r, &Control::none()).unwrap();
        let took = started.elapsed();
        assert_contiguous(&route, r);
        assert_fuel_consistent(&route, r.fuel.as_ref().unwrap(), &r.boost, r.start_fuel);
        assert!(route.jumps <= max_jumps, "{label} took {} jumps", route.jumps);
        assert!(took < within, "{label} took {took:?}");
        route
    }

    /// What "best" means is the judge's, not the planner's (maintainer,
    /// 2026-09-10: "we need pins for the various settings. minimize
    /// jumps, minimize time, etc."). A pin that asserts a jump count under
    /// the default judge measures the judge as much as the planner: item 42
    /// (2026-09-03, the flat public model of 60 s a jump and 120 s a stop)
    /// turned the Mandalay Wongi -> Colonia plan from 93 jumps / 26 stops
    /// into 95 / 23, four minutes faster by the contract, and the jump pin
    /// went stale unnoticed because the full-index pins only run by hand.
    /// So a route is pinned once per judge, each number owned by the one
    /// thing that can move it: jumps under the fewest-jumps preset (the
    /// planner's), seconds under the flat model in default mode (the
    /// judge's).
    #[derive(Clone, Copy)]
    enum Judge {
        /// `stop_weight` 0: flying time alone, absolute fewest jumps — the
        /// Try-harder preset.
        FewestJumps,
        /// The public server's defaults: the flat model, 60 s a jump and
        /// 120 s a stop, passed explicitly as the website does.
        FlatPublic,
    }

    fn judged(r: RouteRequest, judge: Judge) -> RouteRequest {
        match judge {
            Judge::FewestJumps => RouteRequest { stop_weight: 0.0, ..r },
            Judge::FlatPublic => RouteRequest { stop_weight: 1.0, t_jump_s: Some(60.0), stop_overhead_s: Some(120.0), ..r },
        }
    }

    /// Seconds a plan costs under the flat public model: the number the
    /// default judge minimises, so the number its pin asserts.
    fn flat_seconds(route: &Route) -> u32 {
        route.jumps as u32 * 60 + route.refuel_stops as u32 * 120
    }

    struct JudgePin {
        label: &'static str,
        judge: Judge,
        max_jumps: usize,
        /// Asserted only under FlatPublic; None where the seconds are not
        /// yet measured on the index the pins run on.
        max_flat_seconds: Option<u32>,
    }

    /// Every full-index pin, once per judge. Values measured on the PC's
    /// galaxy-95957318 (2026-09-07 build) at main, 2026-09-10; the
    /// fewest-jumps values are the pre-item-42 answers. A row's numbers
    /// are re-measured, not argued, when they move; the index id travels
    /// with them in docs/benches.
    /// Measurement, not a pin (maintainer, 2026-09-10: "1 and 2, go"):
    /// every full-index route under both judges with secondary boosts off,
    /// within 5,000 ls and within 10,000 ls. Prints jumps, boosts,
    /// secondary boosts, stops, flat seconds, and the supercruise seconds
    /// the runs cost under the cost model (150 s + 0.5 s per 1,000 ls),
    /// so the judge can be read either way. Pre-registered before the
    /// first run: at most two jumps saved on any pin, usually zero.
    /// Needs `boost.bin` beside the index (an import made after this
    /// change); without it every row equals the off row.
    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_pins_secondary_boosts_measure() {
        let Some((g, neutrons)) = full_index() else { return };
        let side = g.boost_secondary(g.find("Sol").unwrap()).is_some() || std::path::Path::new(&std::env::var("EDDA_GALAXY_FULL_INDEX").unwrap()).join(crate::boost_side::BOOST_SIDE_FILE).is_file();
        eprintln!("boost.bin beside the index: {side}");
        let routes: [(&str, fn(&Galaxy) -> RouteRequest); 6] = [
            ("Mandalay Wongi -> Colonia (no white dwarfs)", |g| without_white_dwarfs(mandalay_request(g, "Wongi", "Colonia"))),
            ("Mandalay Wongi -> Colonia (white dwarfs)", |g| mandalay_request(g, "Wongi", "Colonia")),
            ("explorer Wongi -> Colonia (no white dwarfs)", |g| without_white_dwarfs(explorer(g, "Wongi", "Colonia"))),
            ("explorer Wongi -> Colonia (white dwarfs)", |g| explorer(g, "Wongi", "Colonia")),
            ("explorer Sol -> Sagittarius A* (no white dwarfs)", |g| without_white_dwarfs(explorer(g, "Sol", "Sagittarius A*"))),
            ("explorer Sol -> Sagittarius A* (white dwarfs)", |g| explorer(g, "Sol", "Sagittarius A*")),
        ];
        println!("route,judge,secondary_ls,jumps,boosted,secondary_boosts,stops,flat_s,supercruise_s,wall_s");
        for (label, request) in routes {
            for judge in [Judge::FewestJumps, Judge::FlatPublic] {
                for allowed in [0.0f32, 5_000.0, 10_000.0] {
                    let r = RouteRequest { secondary_boost_ls: allowed, ..judged(request(&g), judge) };
                    let started = std::time::Instant::now();
                    let route = plan_best(&g, Some(&neutrons), &r, &Control::none()).unwrap();
                    let took = started.elapsed();
                    assert_contiguous(&route, &r);
                    assert_fuel_consistent(&route, r.fuel.as_ref().unwrap(), &r.boost, r.start_fuel);
                    // fold, not sum: f32::sum starts at -0.0 and prints "-0" for an empty route.
                    let supercruise_s: f32 = route.hops.iter().filter_map(|h| h.via_secondary).map(|(_, ls)| 150.0 + 0.5 * ls / 1000.0).fold(0.0, |a, b| a + b);
                    let judge_name = match judge { Judge::FewestJumps => "fewest_jumps", Judge::FlatPublic => "flat_public" };
                    println!("{label},{judge_name},{allowed},{},{},{},{},{},{supercruise_s:.0},{:.2}", route.jumps, route.boosted_jumps, route.secondary_boosts, route.refuel_stops, flat_seconds(&route), took.as_secs_f64());
                }
            }
        }
    }

    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_pins_hold_under_every_judge() {
        let Some((g, neutrons)) = full_index() else { return };
        let routes: [(&str, fn(&Galaxy) -> RouteRequest, &[JudgePin]); 8] = [
            (
                "Mandalay Wongi -> Colonia (no white dwarfs)",
                |g| without_white_dwarfs(mandalay_request(g, "Wongi", "Colonia")),
                &[
                    JudgePin { label: "fewest jumps", judge: Judge::FewestJumps, max_jumps: 93, max_flat_seconds: None },
                    JudgePin { label: "flat public", judge: Judge::FlatPublic, max_jumps: 95, max_flat_seconds: Some(8_460) },
                ],
            ),
            (
                "Mandalay Wongi -> Colonia (white dwarfs)",
                |g| mandalay_request(g, "Wongi", "Colonia"),
                &[
                    JudgePin { label: "fewest jumps", judge: Judge::FewestJumps, max_jumps: 94, max_flat_seconds: None },
                    JudgePin { label: "flat public", judge: Judge::FlatPublic, max_jumps: 96, max_flat_seconds: Some(8_520) },
                ],
            ),
            (
                "explorer Wongi -> Colonia (no white dwarfs)",
                |g| without_white_dwarfs(explorer(g, "Wongi", "Colonia")),
                &[
                    JudgePin { label: "fewest jumps", judge: Judge::FewestJumps, max_jumps: 58, max_flat_seconds: None },
                    JudgePin { label: "flat public", judge: Judge::FlatPublic, max_jumps: 58, max_flat_seconds: Some(4_200) },
                ],
            ),
            (
                "explorer Wongi -> Colonia (white dwarfs)",
                |g| explorer(g, "Wongi", "Colonia"),
                &[
                    JudgePin { label: "fewest jumps", judge: Judge::FewestJumps, max_jumps: 57, max_flat_seconds: None },
                    JudgePin { label: "flat public", judge: Judge::FlatPublic, max_jumps: 57, max_flat_seconds: Some(4_020) },
                ],
            ),
            (
                "explorer Sol -> Sagittarius A* (no white dwarfs)",
                |g| without_white_dwarfs(explorer(g, "Sol", "Sagittarius A*")),
                &[
                    JudgePin { label: "fewest jumps", judge: Judge::FewestJumps, max_jumps: 67, max_flat_seconds: None },
                    JudgePin { label: "flat public", judge: Judge::FlatPublic, max_jumps: 67, max_flat_seconds: Some(4_860) },
                ],
            ),
            (
                "explorer Sol -> Sagittarius A* (white dwarfs)",
                |g| explorer(g, "Sol", "Sagittarius A*"),
                &[
                    // 66 under the jumps judge on both the 09-01 and 09-07
                    // indexes; the default judge takes 67 jumps with 5 stops
                    // over 66 with 6 (4,620 s against 4,680 s).
                    JudgePin { label: "fewest jumps", judge: Judge::FewestJumps, max_jumps: 66, max_flat_seconds: None },
                    JudgePin { label: "flat public", judge: Judge::FlatPublic, max_jumps: 67, max_flat_seconds: Some(4_620) },
                ],
            ),
            (
                "explorer Wongi -> Maia",
                |g| explorer(g, "Wongi", "Maia"),
                &[
                    JudgePin { label: "fewest jumps", judge: Judge::FewestJumps, max_jumps: 3, max_flat_seconds: None },
                    JudgePin { label: "flat public", judge: Judge::FlatPublic, max_jumps: 3, max_flat_seconds: Some(300) },
                ],
            ),
            (
                "explorer Wongi -> Colonia, thorough (no white dwarfs, grace 1 s, budget 120 s)",
                |g| RouteRequest { thorough: true, grace_ms: 1_000, time_budget_ms: 120_000, ..without_white_dwarfs(explorer(g, "Wongi", "Colonia")) },
                &[
                    JudgePin { label: "fewest jumps", judge: Judge::FewestJumps, max_jumps: 58, max_flat_seconds: None },
                    JudgePin { label: "flat public", judge: Judge::FlatPublic, max_jumps: 58, max_flat_seconds: Some(4_200) },
                ],
            ),
        ];
        let mut failures = Vec::new();
        for (route_label, request, pins) in routes {
            for pin in pins {
                let r = judged(request(&g), pin.judge);
                let started = std::time::Instant::now();
                let route = plan_best(&g, Some(&neutrons), &r, &Control::none()).unwrap();
                let took = started.elapsed();
                assert_contiguous(&route, &r);
                assert_fuel_consistent(&route, r.fuel.as_ref().unwrap(), &r.boost, r.start_fuel);
                let secs = flat_seconds(&route);
                eprintln!("{route_label} / {}: {} jumps, {} boosted, {} stops, {secs} flat-s, {took:?}", pin.label, route.jumps, route.boosted_jumps, route.refuel_stops);
                if route.jumps > pin.max_jumps {
                    failures.push(format!("{route_label} / {}: {} jumps > {}", pin.label, route.jumps, pin.max_jumps));
                }
                if let Some(max) = pin.max_flat_seconds {
                    if secs > max {
                        failures.push(format!("{route_label} / {}: {secs} flat-s > {max}", pin.label));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "pins moved:\n{}", failures.join("\n"));
    }

    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_wongi_to_colonia_is_at_most_58_jumps_in_under_five_seconds() {
        let Some((g, neutrons)) = full_index() else { return };
        let r = without_white_dwarfs(explorer(&g, "Wongi", "Colonia"));
        let route = full_pin(&g, &neutrons, &r, "Wongi -> Colonia", 58, std::time::Duration::from_secs(5));
        assert!(route.boosted_jumps >= 1, "a Colonia plot without a single neutron boost");
    }

    /// The Route tab's "Try harder": the thorough portfolio with the grace
    /// rule answers in a few seconds because the slow variants (no
    /// thinning, low weight) are cancelled once the fast ones have a route.
    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_thorough_wongi_to_colonia_with_grace_stops_early_at_most_58_jumps() {
        let Some((g, neutrons)) = full_index() else { return };
        let r = RouteRequest { thorough: true, grace_ms: 1_000, time_budget_ms: 120_000, ..without_white_dwarfs(explorer(&g, "Wongi", "Colonia")) };
        let route = full_pin(&g, &neutrons, &r, "thorough Wongi -> Colonia", 58, std::time::Duration::from_secs(4));
        assert!(route.variants_run >= 18, "{} variants run", route.variants_run);
        assert!(route.variants_finished < route.variants_run, "the grace rule cancelled none of {} variants", route.variants_run);
        assert!(route.variants_finished >= 1);
    }

    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_wongi_to_colonia_with_white_dwarfs_is_at_most_57_jumps_in_under_five_seconds() {
        let Some((g, neutrons)) = full_index() else { return };
        let r = explorer(&g, "Wongi", "Colonia");
        full_pin(&g, &neutrons, &r, "Wongi -> Colonia (white dwarfs)", 57, std::time::Duration::from_secs(5));
    }

    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_sol_to_sagittarius_a_is_at_most_69_jumps_in_under_five_seconds() {
        let Some((g, neutrons)) = full_index() else { return };
        let r = without_white_dwarfs(explorer(&g, "Sol", "Sagittarius A*"));
        full_pin(&g, &neutrons, &r, "Sol -> Sagittarius A*", 69, std::time::Duration::from_secs(5));
    }

    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_sol_to_sagittarius_a_with_white_dwarfs_is_at_most_67_jumps_in_under_five_seconds() {
        // The clearest white-dwarf win: 69 -> 66 jumps, 7 -> 3 refuels.
        let Some((g, neutrons)) = full_index() else { return };
        let r = explorer(&g, "Sol", "Sagittarius A*");
        full_pin(&g, &neutrons, &r, "Sol -> Sagittarius A* (white dwarfs)", 67, std::time::Duration::from_secs(5));
    }

    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_mandalay_wongi_to_colonia_is_at_most_95_jumps_in_under_five_seconds() {
        let Some((g, neutrons)) = full_index() else { return };
        let r = without_white_dwarfs(mandalay_request(&g, "Wongi", "Colonia"));
        full_pin(&g, &neutrons, &r, "Mandalay Wongi -> Colonia", 95, std::time::Duration::from_secs(5));
    }

    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_mandalay_wongi_to_colonia_with_white_dwarfs_is_at_most_96_jumps_in_under_five_seconds() {
        // 93 without, 94 with: weighted-A* jitter on a larger coarse graph,
        // pinned so it cannot drift further.
        let Some((g, neutrons)) = full_index() else { return };
        let r = mandalay_request(&g, "Wongi", "Colonia");
        full_pin(&g, &neutrons, &r, "Mandalay Wongi -> Colonia (white dwarfs)", 96, std::time::Duration::from_secs(5));
    }

    #[test]
    #[ignore = "needs the full galaxy index; set EDDA_GALAXY_FULL_INDEX and run with --ignored"]
    fn full_wongi_to_maia_is_at_most_4_jumps_in_under_a_second() {
        let Some((g, neutrons)) = full_index() else { return };
        let r = explorer(&g, "Wongi", "Maia");
        let started = std::time::Instant::now();
        let route = plan_best(&g, Some(&neutrons), &r, &Control::none()).unwrap();
        let took = started.elapsed();
        assert_contiguous(&route, &r);
        assert!(route.jumps <= 4, "Wongi -> Maia took {} jumps", route.jumps);
        assert!(took < std::time::Duration::from_secs(1), "Wongi -> Maia took {took:?}");
    }
}
