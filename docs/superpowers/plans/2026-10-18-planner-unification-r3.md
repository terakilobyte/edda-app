# Planner Unification, Rung R3: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task (inline; the maintainer chose native execution for this programme). Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** every connected pair answers with a flyable route in seconds and never with "no route"; the optimisers then improve it, pruned by it, and stop when the bound says there is nothing left to find.

**Architecture:** A feasibility-first search over the full star index (state = star, jumps since the last scoop, injections used; injections in lexicographic buckets; a visited bitset per bucket) runs from t = 0 on its own thread and produces a witness: a flyable star sequence. The witness is refined into a route with its own segments as guaranteed fallbacks, published as the plot's incumbent (early answer, upper bound, the answer at the budget). A refinement leg that fails no longer kills its variant: the false coarse edge is banned for the plot and the variant's coarse search re-runs (LazySP); a failed leg's cost bound is memoised so no variant pays it twice. Every coarse search prunes by the shared incumbent. Measured per task on the 28-route matrix on both thread counts plus the rim pair and the reverse Jongou pair.

**Tech Stack:** Rust (ed-galaxy: new `feasible.rs`; `long_range.rs` plan_long / refine_waypoints / plan_long_with; `router.rs` LegMemo), the planner bench, `docs/benches/knobs/edda_matrix.sh`, `planner_before_after.py --judge seconds`.

**Spec:** `docs/superpowers/specs/2026-10-18-planner-holistic-review.md` (the synthesis and appendix C's design) over `docs/superpowers/specs/2026-10-17-planner-unification-design.md` (D2, D4).

## Global Constraints

- Everything in the R0/R1 and R2 plans' Global Constraints: no special-casing of any route, no third-party plotter named ("the external reference"), no AI attribution, worktree + separate `CARGO_TARGET_DIR` (`edda-target-limits`), tests silent, "Commander" only, the measurement doctrine, the boss flies before any release, no deploy without his word.
- Gate for every task that changes planner behaviour: `edda_matrix.sh` (28 routes, warm, ship `explorer`, premium 22, min-fuel, 100 s) on the PC and with `RAYON_NUM_THREADS=3`, rendered with `planner_before_after.py --judge seconds` against `thin6` (PC) and `thin6-3` / `before3` (three threads): no route with more jumps or more stops, pilot seconds not higher, wall within +10 % and +1 s (the two Jongou pairs and the rim pair at best of two because they answer at the cut); `cargo test -p ed-galaxy --release --lib` and `cargo test -p ed-api` green; `plot_fuzz` same-seed with 0 unflyable routes on every task that builds routes (Tasks 1, 2, 3).
- The target the maintainer set (2026-10-17): all 28 routes found, every route at or under the external reference's jumps, on three threads inside the budget. A task that moves toward it without regressing ships; the record keeps what did not.
- No compiles on this machine while a matrix arm runs (32 GB RAM, an 11 GB index: a build evicts it from the page cache and the wall figures lie).

## Review Focus

1. A pair that is disconnected at the ship's reach (an island nothing reaches even with every injection) must return `NoRoute` from the feasibility search in bounded time, not run to the budget (Task 1 test `a_disconnected_island_is_refused_once_the_frontier_drains`).
2. The witness must be flyable with the real fuel model: a 19th dry jump must never appear (Task 1 test `the_witness_never_runs_more_dry_jumps_than_the_tank_allows`); `plot_fuzz` replays every served route.
3. A witness that reaches an injection-only goal must spend an injection on the last hop and nowhere it is not needed (Task 1 test `an_island_goal_is_entered_with_one_injection_and_no_more`).
4. A refinement leg that fails must not cancel its siblings' results, and the same false edge must not be tried twice in a plot (Task 3 tests `a_failed_leg_bans_its_edge_and_the_variant_re_plans` and `a_failed_legs_bound_is_remembered`).
5. The incumbent must never make a plot worse: a variant pruned by the incumbent cannot lose a route the incumbent would not have beaten (Task 4 test `incumbent_pruning_keeps_every_route_that_beats_the_incumbent`).

---

### Task 1: The feasibility search (`feasible.rs`): a flyable witness in seconds, or a bounded "no"

**Files:**
- Create: `crates/ed-galaxy/src/feasible.rs`
- Modify: `crates/ed-galaxy/src/lib.rs` (`pub mod feasible;`)
- Test: `crates/ed-galaxy/src/feasible.rs` tests (fields built with the `long_range` test helpers; make `build`/`star`/`Field` `pub(crate)` in `long_range::tests` or duplicate the small JSON builder)

**Interfaces:**
- Consumes: `Galaxy::{count, pos_of, for_each_within, scoopable, class, record}`, `crate::StarClass`, `FuelModel::{reach, capacity, max_fuel_per_jump}`, `BoostProfile::for_class`, `RouteRequest::{from, to, range_ly, supercharge, boost, fuel, injection}`.
- Produces: `pub struct Witness { pub stars: Vec<u32>, pub injections: Vec<usize>, pub expansions: u64 }` (hop `i` is `stars[i-1] -> stars[i]`; `injections` lists hop indices that need one); `pub fn find_witness(g: &Galaxy, req: &RouteRequest, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Witness, RouteError>` (`NoRoute` when the frontier drains in every injection bucket, `Cancelled` when `cancelled()`); `pub fn dry_cap(m: Option<&FuelModel>) -> u8` (= `floor(capacity / max_fuel_per_jump)` capped at 63, 63 without a model); `pub fn feasible_reaches(req) -> Reaches { plain, boosted_neutron, boosted_wd, injected }` (full-tank figures: the fuel model's `reach(capacity, boost)` or `range_ly x boost`).

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::RouteRequest;
    // `build`, `star`, K, N: the long_range test helpers (make them pub(crate)).

    /// A chain of K stars 60 ly apart, 20 of them: a witness exists with no
    /// injection and at most one dry jump between scoops.
    #[test]
    fn a_plain_chain_has_a_witness_without_injections() {
        let stars: Vec<_> = (0..20).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("K0").unwrap(), to: f.g.find("K19").unwrap(), range_ly: 72.0, ..Default::default() };
        let w = find_witness(&f.g, &req, &|| false).unwrap();
        assert_eq!((w.stars.first(), w.stars.last()), (Some(&req.from), Some(&req.to)));
        assert!(w.injections.is_empty());
        assert_eq!(w.stars.len(), 20, "greedy on distance walks the chain: {:?}", w.stars.len());
    }

    /// An island K star 100 ly from the chain's end: only a x2 injection
    /// (144 ly at range 72) enters it, and the witness spends exactly one,
    /// on the last hop.
    #[test]
    fn an_island_goal_is_entered_with_one_injection_and_no_more() {
        let mut stars: Vec<_> = (0..5).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        stars.push(star("Island", 340.0, 0.0, 0.0, K));
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("K0").unwrap(), to: f.g.find("Island").unwrap(), range_ly: 72.0, injection: Some((2.0, "premium", 22)), ..Default::default() };
        let w = find_witness(&f.g, &req, &|| false).unwrap();
        assert_eq!(w.injections, vec![w.stars.len() - 1], "one injection, on the hop into the island");
        let none = RouteRequest { injection: None, ..req.clone() };
        assert!(matches!(find_witness(&f.g, &none, &|| false), Err(RouteError::NoRoute)), "without injections the island is unreachable, and the search says so");
    }

    /// Brown dwarfs (unscoopable) 60 ly apart for 25 hops, a Mandalay with
    /// a 32 t tank and 5.2 t a jump (6 dry jumps): no witness, because the
    /// 7th dry jump cannot be flown; with a K star every 5 hops the witness
    /// exists and never exceeds 6 dry jumps between scoops.
    #[test]
    fn the_witness_never_runs_more_dry_jumps_than_the_tank_allows() {
        let m = FuelModel::from_loadout(319.0, 32.0, 5.2, 5, true, false, 77.9, 10.5, 0.0);
        let dry: Vec<_> = (0..26).map(|i| star(&format!("D{i}"), i as f32 * 60.0, 0.0, 0.0, "L (Brown dwarf) Star")).collect();
        let f = build(&dry);
        let req = RouteRequest { from: f.g.find("D0").unwrap(), to: f.g.find("D25").unwrap(), range_ly: m.range_at(m.capacity), fuel: Some(m), start_fuel: 32.0, ..Default::default() };
        assert!(matches!(find_witness(&f.g, &req, &|| false), Err(RouteError::NoRoute)));
        let mixed: Vec<_> = (0..26).map(|i| star(&format!("S{i}"), i as f32 * 60.0, 0.0, 0.0, if i % 5 == 0 { K } else { "L (Brown dwarf) Star" })).collect();
        let f = build(&mixed);
        let req = RouteRequest { from: f.g.find("S0").unwrap(), to: f.g.find("S25").unwrap(), ..req };
        let w = find_witness(&f.g, &req, &|| false).unwrap();
        let mut run = 0;
        for &s in &w.stars[1..] { if f.g.scoopable(s) { run = 0 } else { run += 1; assert!(run <= dry_cap(req.fuel.as_ref()) as usize); } }
    }

    /// A goal nothing reaches (5,000 ly off, no injections): the frontier
    /// drains and the answer is NoRoute, with an expansion count that is
    /// the component's size, not a budget.
    #[test]
    fn a_disconnected_island_is_refused_once_the_frontier_drains() {
        let mut stars: Vec<_> = (0..10).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        stars.push(star("Far", 0.0, 5000.0, 0.0, K));
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("K0").unwrap(), to: f.g.find("Far").unwrap(), range_ly: 72.0, ..Default::default() };
        match find_witness(&f.g, &req, &|| false) {
            Err(RouteError::NoRoute) => {}
            other => panic!("{other:?}"),
        }
    }

    /// Neutrons 400 ly apart (boost x4 at range 72 reaches 288 -- too short;
    /// x6 reaches 432): the witness boosts only from neutrons and only when
    /// the profile allows it.
    #[test]
    fn boosted_hops_leave_neutrons_only() {
        let stars: Vec<_> = (0..6).map(|i| star(&format!("N{i}"), i as f32 * 400.0, 0.0, 0.0, N)).collect();
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("N0").unwrap(), to: f.g.find("N5").unwrap(), range_ly: 72.0, boost: BoostProfile { neutron: 6.0, white_dwarf: 1.5 }, ..Default::default() };
        assert_eq!(find_witness(&f.g, &req, &|| false).unwrap().stars.len(), 6);
        let weak = RouteRequest { boost: BoostProfile::default(), ..req.clone() };
        assert!(matches!(find_witness(&f.g, &weak, &|| false), Err(RouteError::NoRoute)));
    }
}
```

- [ ] **Step 2: Run, expect compile failure** (`cargo test -p ed-galaxy --release --lib -- feasible::` -> `cannot find function find_witness`).

- [ ] **Step 3: Implement**

```rust
//! The feasibility search: a flyable witness route in seconds, or a bounded
//! "no" (planner unification R3, 2026-10-18, the holistic review's spine).
//! State = (star, jumps since the last scoop, injections used). Fuel is
//! relaxed to feasibility with full-tank reaches everywhere and at most
//! `dry_cap` jumps between scoopable stars (a maximum jump burns at most
//! `max_fuel_per_jump`, so the witness always replays under the real model);
//! injections are lexicographic buckets, so none is spent until the
//! plain-and-boost frontier of the current bucket has drained; a visited
//! bitset per bucket keeps the search complete and lets a dead pocket (the
//! 5.7 kly lobe into Jongou) drain once. Greedy on straight-line distance
//! to the goal inside a bucket.
use crate::format::Galaxy;
use crate::router::{RouteError, RouteRequest};
use std::collections::BinaryHeap;

pub struct Witness { pub stars: Vec<u32>, pub injections: Vec<usize>, pub expansions: u64 }

pub struct Reaches { pub plain: f32, pub neutron: f32, pub white_dwarf: f32, pub injected: f32 }

pub fn feasible_reaches(req: &RouteRequest) -> Reaches {
    let at = |boost: f32| match req.fuel { Some(m) => m.reach(m.capacity, boost), None => req.range_ly.max(1.0) * boost };
    let (n, wd) = if req.supercharge { (req.boost.neutron, req.boost.white_dwarf) } else { (1.0, 1.0) };
    Reaches { plain: at(1.0), neutron: at(n), white_dwarf: at(wd), injected: at(req.injection.map(|(m, _, _)| m).unwrap_or(1.0)) }
}

pub fn dry_cap(m: Option<&crate::fuel::FuelModel>) -> u8 {
    match m { Some(m) if m.max_fuel_per_jump > 0.0 => ((m.capacity / m.max_fuel_per_jump).floor() as u8).clamp(1, 63), _ => 63 }
}

struct Open { key: F32Rev, star: u32, dry: u8 }   // ordered by distance to goal (min-heap)
// Bitset: Vec<u64> of ceil(count / 64) words per injection bucket, allocated lazily per bucket.
// Parent: HashMap<(u32, inj), (u32, inj, injected_hop)> for the witness only (the closed set is the bitset).
pub fn find_witness(g: &Galaxy, req: &RouteRequest, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Witness, RouteError> {
    let reaches = feasible_reaches(req);
    let cap = dry_cap(req.fuel.as_ref());
    let max_inj = req.injection.map(|(_, _, n)| n as usize).unwrap_or(0);
    let goal = g.pos_of(req.to);
    // bucket k: states reached with k injections; a bucket is searched to exhaustion before k+1 opens.
    // Within a bucket: greedy best-first on dist(star, goal); visited[k] bitset; dry carried in the heap entry
    // (a star re-entered with a smaller dry count is allowed once: visited is per (star) but a second entry
    // is pushed only if dry < best_dry[star]... keep a small u8 map for stars seen with dry > 0).
    // Successors of (s, dry, k): every star within `reach(s)` where reach = neutron/white_dwarf reach when
    // s is that class and the profile boosts, else plain; a successor t costs dry' = if g.scoopable(t) {0} else {dry+1},
    // refused when dry' > cap. Injected successors (plain reach x mult, boost does not stack) go to bucket k+1
    // when k < max_inj, carried in a side list and seeded when bucket k drains.
    // Return on popping `req.to`; NoRoute when every bucket up to max_inj has drained; Cancelled on `cancelled()`
    // checked every 256 pops.
    unimplemented!()
}
```
Write it in full: the heap per bucket, the bitset, the injected side list, the parent map, the reconstruction, the expansion counter. Keep it under 250 lines; no fuel arithmetic beyond `dry_cap`.

- [ ] **Step 4: Run the five tests to green; then the whole suite.**
- [ ] **Step 5: Bench the witness alone** with a new example `crates/ed-galaxy/examples/witness.rs` (`witness <index> <from> <to> [--inject premium N]`: prints stars, injections, expansions, ms) on the 28 pins + both reverse Jongou routes; record `docs/benches/2026-10-18-witness.csv` (route, witness found, hops, injections, expansions, ms). Pre-registered: a witness on all 28 routes and both reverse Jongou routes; under 10 s single-threaded on each; NoRoute on none.
- [ ] **Step 6: Commit** `git commit -am "Feasibility search: a flyable witness route in seconds (state star / dry jumps / injections, lexicographic injection buckets, a visited bitset per bucket), or a bounded no; the witness example and its record"`

---

### Task 2: The witness becomes the plot's incumbent from t = 0

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs` `plan_long` (around the wave loop and the dead `plan_graph_first` fallback), `refine_waypoints` (a `fallback: Option<&[u32]>` per leg: the witness segment), `run_variants` (an `incumbent: &Incumbent` shared with every variant's judge)
- Modify: `crates/ed-api/src/plot.rs` (the early answer already flows through `ctl.found`; the witness's route is published through it)
- Test: long_range.rs tests

**Interfaces:**
- Produces: `pub struct Incumbent { score: AtomicU64, jumps: AtomicU32, route: Mutex<Option<Route>> }` with `offer(&self, r: Route, judge) -> bool` (keeps the better by judge) and `jumps(&self) -> Option<u32>`; `fn witness_waypoints(g, w: &Witness, every_ly: f32) -> Vec<(u32, f32)>` (cut at witness stars every ~3 kly, always at a neutron when one is within the window, fuel estimate = capacity); `fn route_from_witness(g, req, w) -> Route` (hops from the stars with distances, boosted flags from the departure class, fuel by `refuel_hops`/`minimize_refuels`).

- [ ] **Step 1: Failing tests** -- `the_witness_route_is_the_answer_when_every_variant_fails` (a field whose only path needs an injected last hop into an island goal with no neutron chain: `plan_long` returns the witness-derived route, not NoRoute); `a_refinement_leg_with_a_witness_segment_never_fails` (a window whose exact leg is refused falls back to the witness hops); `the_incumbent_keeps_the_better_route` (offer two routes, the judge decides).
- [ ] **Step 2: Run, expect failure.**
- [ ] **Step 3: Implement.** In `plan_long`: before the first wave, `std::thread::scope` (or a dedicated rayon spawn on a one-thread pool) runs `find_witness` under the USER cancel only (not the deadline); when it returns, `route_from_witness` -> `incumbent.offer` -> `(ctl.found)(route)`; then `refine_waypoints(witness_waypoints(...), fallback = witness segments)` in the same thread offers the refined route. The waves' `judge`/fold take `incumbent` into account (fold the incumbent as a candidate). `plan_graph_first` is removed (dead; the witness replaces it). At the budget, `best` is at least the incumbent.
- [ ] **Step 4: Tests; `plot_fuzz` same-seed; the matrix on both thread counts vs thin6 / thin6-3; the rim pair; the reverse Jongou pair.** Pre-registered: both reverse Jongou routes return a route (any jump count) within 100 s on three threads; first-found on every row within 10 s on three threads; no row worse; the rim pair unchanged or better.
- [ ] **Step 5: Commit.**

---

### Task 3: LazySP repair: a false coarse edge is banned and the variant re-plans; a dead leg's bound is remembered

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs` `refine_waypoints` (return `Err(LegFailed { from, to, i })` carrying the failing edge; abort the first-pass `collect` on the first failure via an `AtomicBool`), `plan_long_with` and `plan_long_bidi` (`banned: &DeadEdges` checked in `relax`: `if banned.contains(cur_star, n_star) { return false }`), the dispatch in `plan_long` (on `LegFailed`: ban the edge, re-run the variant's coarse once more, up to 2 repairs per variant per wave)
- Modify: `crates/ed-galaxy/src/router.rs` `LegMemo::put` (a `Budget` failure is stored as `TooCostly { ms }`: a later lookup with less remaining budget skips the leg at once)
- Test: long_range.rs and router.rs tests

- [ ] **Step 1: Failing tests** -- `a_failed_leg_bans_its_edge_and_the_variant_re_plans` (a field with two coarse paths where the shorter one's middle edge is a void the exact planner refuses: the variant's second coarse run takes the longer path and the plot succeeds); `a_failed_legs_bound_is_remembered` (a `Budget` leg stored; a second `get` with a smaller remaining budget returns the failure without planning); `the_first_leg_failure_stops_the_sibling_legs` (the collect aborts: fewer legs planned than windows).
- [ ] **Step 2-3: Run red; implement.**
- [ ] **Step 4: Tests; the gate on both thread counts; the reverse Jongou pair with ED_PLOT_DEBUG** -- pre-registered: the dead 1,209 ly leg is tried once per plot; seconds in failed legs per plot under 1 s after the first; no row worse.
- [ ] **Step 5: Commit.**

---

### Task 4: Every coarse search prunes by the shared incumbent

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs` `plan_long_with` / `plan_long_bidi` (`Open` gains `jumps: u32`, the integer hop count along the chain; `relax` refuses a candidate when `jumps + 1 + ceil(dist(n, goal) / (full_range x boost)) >= incumbent.jumps()`), `run_variants` (passes `&incumbent`)
- Test: long_range.rs tests

- [ ] **Step 1: Failing test** `incumbent_pruning_keeps_every_route_that_beats_the_incumbent` (a field with a 10-jump route; incumbent at 12: the search still finds the 10; incumbent at 10: the search returns the incumbent's equal or NoRoute, never worse).
- [ ] **Step 2-3: Run red; implement.**
- [ ] **Step 4: Tests; the gate; per-plot coarse expansions with and without the incumbent (debug counter).** Pre-registered: expansions down on every budget-cut route; no row worse; wall down on the Jongou pairs.
- [ ] **Step 5: Commit.**

---

### Task 5: The widen's relax set is thinned (arm), the thin bidi gets its fanout (arm)

- [ ] Both as knobs first (`ED_WIDEN_THIN=1`, `ED_THIN_BIDI_FANOUT=32`), measured as arms against the Task 4 state; pre-registered: widen time down 5x (`ED_COARSE_TIMERS`), Beagle 191 j, Colonia -> Spase unchanged, rim forward within +5 j; the defaults follow the arms.

### Task 6: The bound-gap stop rule and resumed waves (arm)

- [ ] Stop a wave when `incumbent.jumps <= ceil(LB x (1 + e)) + pad` with LB the euclid floor or the field bound x ship ratio, `e` and `pad` knobs; waves resume each variant's open/best_g/parent instead of restarting. Pre-registered: the rim plots end within 10 s of their first route with the same jumps; Beagle's 18-vs-77 s knife-edge disappears; no row worse.

### Task 7: The winner tabulation and the after table

- [ ] `knobs/variant_first_found.py` extended with route class (straight ly band, end density band) and the winning family per class on the final arms; `docs/benches/2026-10-18-planner-matrix-r3-table.md`; the ROADMAP entry; the R4 plan named next (honest bridge pricing from the aggregate, hwgraph250, per-band field costs).

## What R3 does not do

- No highway adjacency file, no per-band field costs, no component labels beyond what Task 1's drained frontier gives (R4).
- No change to the one-currency prices inside the searches (knobs, measured negative).
