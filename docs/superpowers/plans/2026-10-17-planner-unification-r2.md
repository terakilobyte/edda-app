# Planner Unification, Rung R2: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One currency, pilot seconds, in the judge and in both searches, and the coarse search freed from carrying fuel in its state, so a crossing's coarse stage expands a fraction of the states it does today without producing an unflyable route.

**Architecture:** A new side file beside the highway sub-index, `scoop250.bin`, gives every highway star its nearest scoopable neighbour's distance in light years, which lets the coarse search ask "can the ship refuel within one plain jump here" instead of reading a flag that covers a tenth of the truth. With that answer the coarse state becomes (neutron, dry boosted hops) instead of (neutron, 32 fuel levels) and the scoop edges go; the leg planner places the stops it already places. Edge costs and the judge move from jumps to seconds: plain hop 53 s, boosted hop 73 s, passing scoop +6 s, full stop 120 s plus tonnes over the scoop rate, injection 1,800 s. Every task is gated by the 28-route matrix against the r1 arm and by the fuel fuzz harness.

**Tech Stack:** Rust (ed-galaxy: import.rs, format.rs, long_range.rs, router.rs; ed-api: galaxy_service.rs, routing.rs), the planner bench, `docs/benches/knobs/edda_matrix.sh`, `crates/ed-galaxy/examples/plot_fuzz.rs`, `fuel_replay.rs`.

**Spec:** `docs/superpowers/specs/2026-10-17-planner-unification-design.md` (D1, D3's scoop byte, the fuel-out-of-state proposal).

## Global Constraints

- Everything in the R0/R1 plan's Global Constraints (no special-casing, no third-party plotter named, no AI attribution, worktree + separate target dir, tests silent, "Commander" only, measurement doctrine).
- Gate for every task that changes planner behaviour: `docs/benches/knobs/edda_matrix.sh` (28 routes, warm, ship `explorer`, premium 22, budget 100 s) rendered against the r1 arm with `planner_before_after.py`: no route with more jumps or more stops, pilot seconds under the seconds judge not higher, wall within +10 % and +1 s; the two rim pairs and both Jongou pairs compared at best of two runs because they answer at the budget cut; `cargo test -p ed-galaxy --release --lib`, `cargo test -p ed-api` green; `plot_fuzz` same-seed with 0 unflyable routes on every task that touches fuel (Tasks 2, 4, 5, 6); `fuel_replay` on the final route of every matrix row that changed.
- Side files are optional at run time: a sub-index without `scoop250.bin` plans exactly as today (the flag path), and says so in the plot log (`scoop_side = false`).
- The new seconds constants are the fitted public defaults from the 2026-10-17 journal cut (130 journals): plain 53, boosted 73, passing scoop 6, full stop 120 + tonnes / 1.245 t/s (the Caspian's 7A scoop; the request's scoop rate when it carries one), injection 1,800. Per-commander fits stay an overlay through the request fields that exist (`t_jump_s`, `stop_overhead_s`).

## Review Focus

1. A highway star whose nearest scoopable star is exactly at plain reach must count as refuelable at full tank and not at a low tank (reach shrinks with mass): the byte is compared with the ship's reach at the current tank, not with a constant (Task 2 test `refuel_reach_follows_the_tank`).
2. A sub-index built before this change (no `scoop250.bin`) must still open, plan, and log the absence (Task 1 test `a_sub_index_without_the_scoop_file_plans_as_before`).
3. The dry-run cap must count boosted hops, not light years: eighteen 430 ly neutron hops drain a 128 t Caspian; eighteen 60 ly plain hops do not (Task 4 test `the_dry_cap_counts_max_fuel_hops`).
4. The seconds judge must rank a route with more plain jumps and fewer boosted ones correctly: 10 plain + 10 boosted (1,260 s) beats 5 plain + 16 boosted (1,433 s) even though the second has fewer jumps (Task 3 test `the_judge_prices_a_boosted_hop_above_a_plain_one`).
5. The exact leg planner at 32 fuel levels must still refuse a hop the tank cannot fund at the finer figure: a 6.8 t jump from 6.9 t with a 4 t quantum must not round into a free jump (Task 5 test `coarser_fuel_levels_never_round_a_dry_hop_into_a_flyable_one`).

---

### Task 1: `scoop250.bin`, the nearest-scoopable byte per highway star

**Files:**
- Create: `crates/ed-galaxy/src/scoop_side.rs` (writer, reader, `SCOOP_SIDE_FILE = "scoop250.bin"`, magic `EDSC`, version 1, count u64, then one u8 per sub-index record in record order: nearest scoopable star in whole light years, 255 = none within 254)
- Modify: `crates/ed-galaxy/src/lib.rs` (`pub mod scoop_side;`), `crates/ed-galaxy/src/format.rs:272-350` (a `scoop_side: OnceLock<Option<ScoopSide>>` beside `boost_side`, `pub fn nearest_scoop_ly(&self, idx: u32) -> Option<u8>`), `crates/ed-galaxy/src/import.rs:483-531` (`subset_cells_cancellable` builds it after the cells, best effort like `agg250.bin`), `crates/ed-api/src/routing.rs:98` (not listed: it lives in the highway dir the server builds itself)
- Test: `crates/ed-galaxy/src/scoop_side.rs` tests, `crates/ed-galaxy/src/long_range.rs` tests

**Interfaces:**
- Produces: `scoop_side::write(path, bytes: &[u8]) -> Result<()>`, `scoop_side::ScoopSide::open(path) -> Result<ScoopSide>`, `ScoopSide::get(idx: u32) -> Option<u8>`; `Galaxy::nearest_scoop_ly(idx) -> Option<u8>` (None when the file is absent); `import::nearest_scoop_bytes(full: &Galaxy, sub: &Galaxy) -> Vec<u8>` (for each sub record, scan the full index within 254 ly for the nearest star whose class is scoopable or carries `FLAG_SCOOP_NEARBY`; cell walk per star, parallel over records).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn the_scoop_file_round_trips_and_answers_by_record_index() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(SCOOP_SIDE_FILE);
    write(&path, &[30, 255, 0, 72]).unwrap();
    let side = ScoopSide::open(&path).unwrap();
    assert_eq!(side.get(0), Some(30));
    assert_eq!(side.get(1), Some(255), "255 is none within 254 ly");
    assert_eq!(side.get(3), Some(72));
    assert_eq!(side.get(4), None, "past the end: unknown");
}

#[test]
fn a_sub_index_without_the_scoop_file_plans_as_before() {
    let f = highway();
    assert_eq!(f.neutrons.nearest_scoop_ly(0), None);
    let r = req(&f, "K0", "K3000", 60.0);
    assert!(plan_best(&f.g, Some(&f.neutrons), &r, &Control::none()).is_ok());
}

#[test]
fn nearest_scoop_bytes_measure_the_full_index() {
    // highway(): a K star every 50 ly on the line, neutrons 10 ly off it:
    // every neutron's nearest scoopable star is within ~10-27 ly.
    let f = highway();
    let bytes = crate::import::nearest_scoop_bytes(&f.g, &f.neutrons);
    assert_eq!(bytes.len(), f.neutrons.count as usize);
    assert!(bytes.iter().all(|&b| b <= 27), "{bytes:?}");
}
```

- [ ] **Step 2: Run, expect compile failure**

Run: `cargo test -p ed-galaxy --release --lib -- scoop`
Expected: FAIL (module missing).

- [ ] **Step 3: Implement** the module on the `boost_side.rs` pattern (mmap, header check, `get` bounds-checked); `nearest_scoop_bytes` with `g.for_each_within(pos, 254.0, ...)` keeping the minimum distance over scoopable candidates (`StarClass::from_code(class_code).scoopable()` or `flags & FLAG_SCOOP_NEARBY != 0`), `rayon` over records; write it from `subset_cells_cancellable` after `write_index_cells` when the sub-index reopens (best effort, like `agg250.bin`); `Galaxy::nearest_scoop_ly` opening the file lazily from the sub-index dir.

- [ ] **Step 4: Tests + build the byte for the PC's index**

Run: `cargo test -p ed-galaxy --release --lib -- scoop nearest_scoop a_sub_index_without`
Then the one-off build for `D:\galos\index-boost-secondary\boost250` through `examples/sidecars.rs` (add a `--scoop` arm that calls `nearest_scoop_bytes` and writes the file); record its wall time and the byte histogram (share within 40 / 60 / 80 / 100 ly; the audit's sample said 63.5 / 83.2 / 92.1 / 94.5 %) in `docs/benches/2026-10-17-scoop-byte.csv`.

- [ ] **Step 5: Commit**

```bash
git commit -am "Highway sub-index: scoop250.bin carries every highway star's nearest scoopable neighbour in light years (built with the sub-index, optional at run time); the planner reads it through Galaxy::nearest_scoop_ly"
```

---

### Task 2: The coarse search's refuel signal comes from the byte

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs` at the three `FLAG_SCOOP_NEARBY` reads in `plan_long_with` (the scan's `refuels`, the bucket thinning's `refuels`, the relax closure's `refills`) and the equivalents in `plan_long_bidi`
- Test: long_range.rs tests

**Interfaces:**
- Produces: `fn refuels_here(neutrons: &Galaxy, idx: u32, reach_now_ly: f32) -> bool` = `nearest_scoop_ly(idx).map(|d| d != 255 && d as f32 <= reach_now_ly).unwrap_or(flags & FLAG_SCOOP_NEARBY != 0)`. `reach_now_ly` is the ship's plain reach at the current tank (`range_at(fuel)`), so a dry tank sees fewer refuel options than a full one.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn refuel_reach_follows_the_tank() {
    let f = highway();
    let dir = tempfile::tempdir().unwrap(); // a sub-index copy with a synthetic scoop file
    let sub = with_scoop_bytes(&f, &[/* N0 */ 60, /* N1 */ 10, /* rest */ 255, ...]);
    assert!(refuels_here(&sub, 0, 72.0), "60 ly is within a 72 ly reach");
    assert!(!refuels_here(&sub, 0, 50.0), "not within a 50 ly reach at a low tank");
    assert!(refuels_here(&sub, 1, 50.0));
    assert!(!refuels_here(&sub, 2, 72.0), "255: nothing within 254 ly");
}
```
(`with_scoop_bytes`: a test helper that copies the fixture's sub-index dir to a temp dir, writes `scoop250.bin` with the given bytes and opens it.)

- [ ] **Step 2: Run, expect compile failure**
- [ ] **Step 3: Implement** and replace the three reads (the scan keeps `!lean_scan &&`).
- [ ] **Step 4: Tests, then the matrix arm `r2-scoop` against r1** (expect: refuel stops equal or fewer, jumps unchanged; the coarse `states` count unchanged, this task changes the signal, not the state).
- [ ] **Step 5: Commit**

```bash
git commit -am "Coarse search: a highway star refuels when a scoopable star is within the ship's current plain reach (scoop250.bin), not when a companion sits within 1,500 ls (9 % of the truth)"
```

---

### Task 3: One currency, pilot seconds, in the judge and both searches

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs:579-650` (`time_units`, `route_score`, `scooped_tonnes`), `:165` (`edge_cost_max` callers: the coarse `relax` prices in seconds), the heuristic `h` in `plan_long_with` and `plan_long_bidi` (seconds per boosted reach); `crates/ed-galaxy/src/router.rs` (`cost()` at ~390-400 and the `Open` ordering; `INJECTION_PENALTY` becomes `INJECTION_SECONDS = 1_800.0`; the heuristic `h`); `crates/ed-api/src/plot.rs:471-472` (the server passes no per-jump override: the public defaults apply)
- Test: long_range.rs and router.rs tests

**Interfaces:**
- Produces: `pub const T_PLAIN_S: f32 = 53.0; pub const T_BOOST_S: f32 = 73.0; pub const T_PASS_SCOOP_S: f32 = 6.0; pub const T_STOP_S: f32 = 120.0; pub const SCOOP_RATE_DEFAULT: f32 = 1.245; pub const INJECTION_SECONDS: f32 = 1_800.0;` in `router.rs`; `time_units(plain, boosted, passing_scoops, stops, scooped_t, scoop_rate, stop_weight, t_jump_s, stop_overhead_s)`; `route_score` unchanged in signature; coarse edge cost in seconds (`edge_cost_max` returns seconds: boosted hop 73 + bridges × 53; scoop-via 73 + 53 + 120 × stop_weight); exact `cost()` in seconds with a lexicographic tie on (jumps, ly).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn the_judge_prices_a_boosted_hop_above_a_plain_one() {
    let req = RouteRequest { stop_weight: 1.0, ..Default::default() };
    let a = route_with(10, 10, 0); // plain, boosted, stops -> 10*53 + 10*73 = 1,260 s
    let b = route_with(5, 16, 0);  // 5*53 + 16*73 = 1,433 s, fewer jumps
    assert!(route_score_public(&a, &req) < route_score_public(&b, &req), "fewer jumps is not fewer seconds");
}

#[test]
fn a_full_stop_costs_the_overhead_plus_the_tonnes() {
    let req = RouteRequest { stop_weight: 1.0, fuel: Some(caspian()), ..Default::default() };
    let dry = route_with_stop(10, 0, /*scooped*/ 0.0);
    let full = route_with_stop(10, 1, 124.5); // 124.5 t at 1.245 t/s = 100 s + 120 s
    assert_eq!(route_score_public(&full, &req) - route_score_public(&dry, &req), 2_200, "deciseconds: 220 s");
}
```
(`route_with`, `route_with_stop`: test builders that set `hops` with `boosted` and `fuel_after` so `scooped_tonnes` and the counts come from the ledger, like the real planner's routes.)

In router.rs:
```rust
#[test]
fn the_exact_planner_prefers_two_plain_hops_to_one_boosted_detour_when_seconds_say_so() {
    // Start, goal 100 ly away; a neutron 60 ly off the line gives a boosted
    // second hop. Plain: 2 x 53 = 106 s; via the neutron: 53 + 73 = 126 s.
    ...
    assert_eq!(route.boosted_jumps, 0, "{:?}", names(&route));
}
```

- [ ] **Step 2: Run, expect failure** (today a boosted hop and a plain hop cost the same jump).
- [ ] **Step 3: Implement**: constants; `time_units` takes plain and boosted counts and passing scoops (a hop with `refuel == false` whose `fuel_after` rose); coarse edges and `h` in seconds (`h = remaining / (reach × boost) × T_BOOST_S × w`); exact `cost()` in seconds with the same tie-break; `INJECTION_SECONDS` replaces the 30-jump penalty in both searches; the server passes `None` for the two overrides so the public defaults apply there too (the per-commander fit still rides the request when the app sends one).
- [ ] **Step 4: Tests; `plot_fuzz` same-seed; the matrix arm `r2-seconds` against r1 under the SECONDS judge** (the renderer gains `--judge seconds` using 53/73/120 + tonnes, and the before/r1 rows are re-judged from their route files, not re-run). Pre-registered: pilot seconds equal or lower on every route; jumps may rise where plain hops replaced boosted ones, and that is the objective, not a regression; the CSV header says so.
- [ ] **Step 5: Commit**

```bash
git commit -am "One currency: pilot seconds (plain 53, boosted 73, passing scoop 6, stop 120 + tonnes over the scoop rate, injection 1,800) in the judge, the coarse edges and the exact planner; the server quotes the public defaults"
```

---

### Task 4: Fuel out of the coarse state

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs` `plan_long_with` (~2842-2850 the state key, ~3130-3200 the relax candidates, the reserve/bucket variant dimensions) and `plan_long_bidi` (already fuel-light: align its dry cap)
- Test: long_range.rs tests

**Interfaces:**
- Produces: coarse state key `(idx << 8) | dry_hops` where `dry_hops` is the count of consecutive boosted hops since the last refuelable star (`refuels_here` at the full-tank reach), capped at `dry_cap = floor(capacity / max_fuel_per_jump) - 1` (Caspian: 17; Mandalay: 5); an edge to a non-refuelable star at `dry_hops == dry_cap` is refused; the scoop-via edge is gone; the reserve and bucket variant dimensions remain as thinning knobs only. `fn dry_cap(m: &FuelModel) -> u8`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn the_dry_cap_counts_max_fuel_hops() {
    let caspian = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
    assert_eq!(dry_cap(&caspian), 17);
    let mandalay = FuelModel::from_loadout(319.2, 32.0, 5.0, 5, true, false, 77.86, 10.5, 0.0);
    assert_eq!(dry_cap(&mandalay), 5);
    // A chain of 20 neutrons 430 ly apart with no scoopable star near any
    // of them, then the goal: the fuel-blind coarse refuses it at 17 dry
    // hops and the plot says no route, as the tank would.
    let f = dry_chain(20);
    let r = RouteRequest { fuel: Some(caspian), start_fuel: 128.0, ..req(&f, "N0", "N20", 72.0) };
    assert!(matches!(plan_long_with(&f.g, &f.neutrons, &r, &Control::none(), COARSE_WEIGHT, RESERVE_FRACTION, 0.75, 1, false, false, false, None, &AtomicBool::new(false), None), Err(RouteError::NoRoute)));
    // The same chain with a K star 20 ly from N10: the cap resets there and the plot exists.
    let f = dry_chain_with_scoop_at(20, 10);
    assert!(plan_long_with(&f.g, &f.neutrons, &r2, ...).is_ok());
}
```

- [ ] **Step 2: Run, expect compile failure**
- [ ] **Step 3: Implement** (the key, the candidate pricing without the fuel branch, the dry counter in `Open`, `fuel_of` for waypoints becomes "full tank at a refuelable star, else capacity - dry_hops × max_fuel" as the estimate the refiner starts from).
- [ ] **Step 4: Tests; `plot_fuzz` same-seed (0 unflyable); the matrix arm `r2-fuelblind` against `r2-seconds`** (pre-registered: coarse states per variant 5-7x fewer in the debug line, first-found earlier on every crossing, seconds and stops within the gate).
- [ ] **Step 5: Commit**

```bash
git commit -am "Coarse search: the state is (neutron, dry boosted hops) with the tank's own cap instead of 32 fuel levels and scoop edges; the leg planner places the stops it already places"
```

---

### Task 5: The exact leg planner at 32 fuel levels

**Files:**
- Modify: `crates/ed-galaxy/src/router.rs` `plan()` (`quant` 255 -> `FUEL_LEVELS = 32`, and the dominance check that compares quantised fuel)
- Test: router.rs tests

- [ ] **Step 1: Failing test**

```rust
#[test]
fn coarser_fuel_levels_never_round_a_dry_hop_into_a_flyable_one() {
    // Start with 6.9 t, one 72 ly hop that burns 6.8 t, goal: flies.
    // Start with 6.7 t: must not (a 4 t quantum would round both to the same level).
    ...
}
```
The feasibility test is on the exact tonnes (`m.jump(d, cur.fuel, boost)`), the quantisation only on the state key; the test pins that order.

- [ ] **Step 2: Run** (passes today at 255 levels: this test guards the change; the RED is the next one)

```rust
#[test]
fn the_leg_planner_keys_its_states_on_thirty_two_levels() {
    assert_eq!(FUEL_LEVELS, 32);
    assert_eq!(quant_for_test(128.0, 128.0), 32);
    assert_eq!(quant_for_test(2.0, 128.0), 1);
}
```

- [ ] **Step 3: Implement**; **Step 4: tests, `plot_fuzz`, the standalone rim approach (expansions before/after), the matrix arm `r2-levels`**; **Step 5: Commit**.

---

### Task 6: The after table, the fuzz record and the ledger

- [ ] Run `plot_fuzz` same-seed and `fuel_replay` on every changed final route; file `docs/benches/2026-10-17-planner-matrix-r2.jsonl` and the rows; render before / r1 / r2 under both judges; ledger entry under Server with the headline; the R3 plan named next.

## What R2 does not do

- No change to the portfolio or the waves (R3).
- No highway adjacency file or component ids (R4).
- No change to the full index (the scoop byte is for highway stars; the exact planner keeps the class and the flag).
