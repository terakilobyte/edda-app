# Planner Unification, Rungs R0 and R1: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Instrument the planner, put every judge on one currency, and remove the measured waste (duplicate legs, injected scans on every expansion, unbounded fanout, variant scheduling on the box) so that every benched route plots the same or better on jumps, stops and pilot seconds, in less wall time, before the search itself is replaced (R2-R4, separate plans).

**Architecture:** The portfolio planner in `crates/ed-galaxy/src/long_range.rs` and the exact leg planner in `crates/ed-galaxy/src/router.rs` stay as they are structurally; this plan changes what they measure, what they price, what they recompute and in which order the server runs them (`crates/ed-api/src/plot.rs`). Every task is gated by a before/after run of the 28-route matrix with the same bench binary and recorded in `docs/benches/`.

**Tech Stack:** Rust (rayon, metrics facade, tracing), the planner bench example `crates/ed-galaxy/examples/bench.rs`, the full galaxy index on this PC (`D:\galos\index-boost-secondary`), bash knobs under `docs/benches/knobs/`, Python for the tables.

**Spec:** `docs/superpowers/specs/2026-10-17-planner-unification-design.md`

## Global Constraints

- No special-casing of particular routes; every change is a rule that applies to all plots.
- Never name the third-party plotter in commits, PR text, docs, CSVs or code comments; write "the external reference".
- No AI attribution trailers in commits or PRs.
- Rust edits happen in the worktree `.claude/worktrees/planner-rim` with `CARGO_TARGET_DIR=C:\Users\terak\ED-Claude\edda-target-limits`; never in the main tree.
- Tests must not speak; the player is "Commander" only (no user-facing strings change in this plan).
- Measurement doctrine: measure before and after with the same instrument; bench records are CSVs in `docs/benches/` with the verdict in the header; harnesses are files in `docs/benches/knobs/`; negatives are recorded, never deleted.
- Gate for every task that changes planner behaviour: on the 28-route matrix (`docs/benches/knobs/edda_matrix.sh`, ship `explorer`, premium injections 22, budget 100 s, this PC) no route gets more jumps or more stops than the baseline row, pilot seconds under the one judge do not rise, and wall time does not rise by more than 10 % on any route; `cargo test -p ed-galaxy --release` and `cargo test -p ed-api` pass; `plot_fuzz` same-seed reports 0 unflyable routes where the task touches fuel or legs.

## Review Focus

1. A plot request with no `stop_weight` from an old client must now be judged at 1.0, and a request that sends 0.0 explicitly must still be honoured (Task 1 test `a_missing_stop_weight_means_one_an_explicit_zero_stays_zero`).
2. A leg memo must never return a route planned from a different starting fuel to a leg that starts with less fuel (Task 4 test `the_leg_memo_keys_on_the_fuel_bucket`).
3. The lazy injected pass must still open an island origin whose nearest star is beyond plain reach (Task 5 test `an_island_origin_still_opens_with_an_injection`), and must not add an injection to a route that never needed one (the matrix gate: injections column unchanged).
4. Sector thinning must keep the scoopable candidate that refuels a dry tank even when it is not the farthest in its sector (Task 6 test `sector_thinning_keeps_a_refuel_candidate`).
5. Variant ordering must not starve a route class whose winner is a late variant: the matrix gate includes the desert pins (Wongi -> Beagle Point, Colonia -> Spase) where off-ramp and scan-twin variants have won before (Task 7, pre-registered in its measurement step).

---

### Task 0: The before table (measurement only, no code)

**Files:**
- Create: `docs/benches/knobs/edda_matrix.sh` (from the scratchpad script of the same name; env `EDDA_INDEX`, `EDDA_BENCH`)
- Create: `docs/benches/knobs/planner_matrix_routes.txt` (the 14 pairs, both directions are implied)
- Create: `docs/benches/knobs/planner_before_after.py` (renders a markdown table from two jsonl files)
- Create: `docs/benches/2026-10-17-planner-matrix-before.jsonl` (the shipped planner at main `4b79562`, bench built into `edda-target-baseline`)
- Create: `docs/benches/2026-10-17-planner-matrix.csv` (header = verdict, one row per route per arm; grows with every task)

**Interfaces:**
- Produces: `planner_before_after.py <before.jsonl> <after.jsonl> [--ref ref.jsonl]` prints a markdown table with columns route, straight ly, jumps before/after, stops before/after, pilot s before/after (60 s/jump + 120 s/stop + tonnes/1.245), first-found s, wall s, and a verdict column (`better`, `same`, `WORSE`) per route; exit code 1 if any route is `WORSE`.

- [ ] **Step 1: Copy the capture script and routes into knobs**

```bash
cp "$SCRATCH/before/edda_matrix.sh" docs/benches/knobs/edda_matrix.sh
cp "$SCRATCH/before/routes.txt" docs/benches/knobs/planner_matrix_routes.txt
```

- [ ] **Step 2: Write the table renderer**

```python
#!/usr/bin/env python
"""planner_before_after.py before.jsonl after.jsonl [--ref ref.jsonl]
One row per (from,to); verdict per route; exit 1 if any route is WORSE."""
import json, sys
T_JUMP, T_STOP, SCOOP_RATE = 60.0, 120.0, 1.245
def load(p):
    rows = {}
    for line in open(p, encoding="utf-8"):
        if not line.strip(): continue
        r = json.loads(line); rows[(r["from"], r["to"])] = r
    return rows
def seconds(r):
    if "jumps" not in r or r.get("jumps") is None: return None
    return r["jumps"] * T_JUMP + (r.get("stops") or 0) * T_STOP
def fmt(v, nd=0):
    return "-" if v is None else (f"{v:.{nd}f}" if isinstance(v, float) else str(v))
def verdict(b, a):
    if a.get("jumps") is None: return "WORSE" if b.get("jumps") is not None else "same"
    if b.get("jumps") is None: return "better"
    if a["jumps"] > b["jumps"] or (a.get("stops") or 0) > (b.get("stops") or 0): return "WORSE"
    if a["wall_s"] > b["wall_s"] * 1.10 and a["jumps"] == b["jumps"]: return "WORSE"
    if a["jumps"] < b["jumps"] or (a.get("stops") or 0) < (b.get("stops") or 0) or a["wall_s"] < b["wall_s"] * 0.9: return "better"
    return "same"
before, after = load(sys.argv[1]), load(sys.argv[2])
ref = load(sys.argv[4]) if len(sys.argv) > 4 and sys.argv[3] == "--ref" else {}
hdr = "| route | straight ly | jumps b/a | stops b/a | pilot s b/a | first s b/a | wall s b/a |" + (" ref jumps / s |" if ref else "") + " verdict |"
print(hdr); print("|" + "---|" * (hdr.count("|") - 1))
worse = 0
for key in sorted(before):
    b, a = before[key], after.get(key, {})
    v = verdict(b, a); worse += v == "WORSE"
    cells = [f"{key[0]} -> {key[1]}", fmt(b.get("straight_ly")),
             f"{fmt(b.get('jumps'))} / {fmt(a.get('jumps'))}", f"{fmt(b.get('stops'))} / {fmt(a.get('stops'))}",
             f"{fmt(seconds(b))} / {fmt(seconds(a))}",
             f"{fmt((b.get('first_found_ms') or 0)/1000,1)} / {fmt((a.get('first_found_ms') or 0)/1000,1)}",
             f"{fmt(b.get('wall_s'),1)} / {fmt(a.get('wall_s'),1)}"]
    if ref:
        r = ref.get(key, {}); cells.append(f"{fmt(r.get('jumps'))} / {fmt(r.get('ref_s'),0)}")
    cells.append(v)
    print("| " + " | ".join(cells) + " |")
sys.exit(1 if worse else 0)
```

- [ ] **Step 3: Run the renderer on the before file against itself**

Run: `python docs/benches/knobs/planner_before_after.py docs/benches/2026-10-17-planner-matrix-before.jsonl docs/benches/2026-10-17-planner-matrix-before.jsonl`
Expected: 28 rows, every verdict `same`, exit 0.

- [ ] **Step 4: Start the CSV with the baseline rows and a header that names the gate**

The header states: ship, injections, budget, PC, bench commit, the gate (no route worse on jumps/stops/seconds, wall within +10 %), and that the external reference's rows come from its public plotter with the same drive figures.

- [ ] **Step 5: Commit**

```bash
git add docs/benches/knobs/edda_matrix.sh docs/benches/knobs/planner_matrix_routes.txt docs/benches/knobs/planner_before_after.py docs/benches/2026-10-17-planner-matrix-before.jsonl docs/benches/2026-10-17-planner-matrix.csv
git commit -m "Bench: the planner matrix before table (28 routes, shipped planner at 4b79562) and the before/after renderer that fails on any route getting worse"
```

---

### Task 1: One judge on the wire (server)

**Files:**
- Modify: `crates/ed-api/src/plot.rs:467` (stop_weight default), `:716-718` (`better`), test at `:1594`
- Test: `crates/ed-api/src/plot.rs` (unit tests module)

**Interfaces:**
- Produces: `fn better<'a>(a: &'a Route, b: &'a Route, req: &RouteRequest) -> &'a Route` picking by `ed_galaxy::long_range::route_score_public(route, req)` (lower is better), ties by `(jumps, boosted_jumps)`.
- Produces: `pub fn route_score_public(r: &Route, req: &RouteRequest) -> u64` in long_range.rs wrapping the private `route_score` with `scoop_rate = req.fuel.map(|m| m.scoop_rate).unwrap_or(0.0)`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_missing_stop_weight_means_one_an_explicit_zero_stays_zero() {
    let api: ApiRouteRequest = serde_json::from_value(serde_json::json!({"from":"Sol","to":"Colonia","range_ly":50.0})).unwrap();
    assert_eq!(to_route_request(&api, 0, 1).stop_weight, 1.0);
    let api: ApiRouteRequest = serde_json::from_value(serde_json::json!({"from":"Sol","to":"Colonia","range_ly":50.0,"stop_weight":0.0})).unwrap();
    assert_eq!(to_route_request(&api, 0, 1).stop_weight, 0.0);
}

#[test]
fn the_early_answer_is_the_best_so_far_by_pilot_seconds() {
    let mk = |jumps: usize, stops: usize| Route { jumps, refuel_stops: stops, ..serde_json::from_value(serde_json::json!({"range_ly":50.0,"hops":[],"jumps":0,"total_ly":0.0,"straight_ly":0.0,"boosted_jumps":0,"expansions":0,"elapsed_ms":0,"refuel_stops":0,"injections":0,"secondary_boosts":0})).unwrap() };
    let req = RouteRequest { stop_weight: 1.0, t_jump_s: Some(60.0), stop_overhead_s: Some(120.0), ..Default::default() };
    // 200 jumps / 2 stops = 12,240 s beats 199 jumps / 4 stops = 12,420 s.
    assert_eq!(better(&mk(199, 4), &mk(200, 2), &req).jumps, 200);
    // Equal seconds: fewer jumps.
    assert_eq!(better(&mk(201, 1), &mk(199, 2), &req).jumps, 199);
}
```

(`to_route_request` is the existing conversion at `plot.rs:430-475`; if it is inline today, extract it with that name and signature `(api: &ApiRouteRequest, from: u32, to: u32) -> RouteRequest` as the first refactor step of this task.)

- [ ] **Step 2: Run them, expect failure**

Run: `cargo test -p ed-api a_missing_stop_weight the_early_answer_is_the_best_so_far_by_pilot_seconds`
Expected: FAIL (0.0 default; `better` takes two arguments).

- [ ] **Step 3: Implement**

`plot.rs:467` → `stop_weight: api.stop_weight.unwrap_or(1.0).clamp(0.0, 5.0),` with a comment: the doc at `:169` always said 1.0; the app's Route tab sends none; a served route has been ranked by jump count alone since the early answer landed (2026-10-17 audit).

`long_range.rs`, next to `route_score`:
```rust
/// The judge, for callers outside this module (the server's early-answer
/// picker): lower is better; the same units the portfolio ranks by.
pub fn route_score_public(r: &Route, req: &RouteRequest) -> u64 {
    route_score(r, req.fuel.map(|m| m.scoop_rate).unwrap_or(0.0), req)
}
```

`plot.rs:716`:
```rust
fn better<'a>(a: &'a Route, b: &'a Route, req: &RouteRequest) -> &'a Route {
    let (sa, sb) = (ed_galaxy::long_range::route_score_public(a, req), ed_galaxy::long_range::route_score_public(b, req));
    if (sb, b.jumps, b.boosted_jumps) < (sa, a.jumps, a.boosted_jumps) { b } else { a }
}
```
and update the two call sites in the early-answer loop (`plot.rs` ~1063 and ~1079) to pass `&req`. Delete the old test `the_early_answer_is_the_best_so_far_by_jumps_then_boosts`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ed-api && cargo test -p ed-galaxy --release`
Expected: all pass.

- [ ] **Step 5: Record and commit**

Ledger (Server section, today's entry): one sentence that the server judged by jumps alone since the early answer and now judges by the same seconds as bench and app. Commit:
```bash
git commit -am "Server plots judge by pilot seconds like the bench and the app: stop_weight defaults to 1.0 on the wire (the Route tab sends none) and the early-answer picker calls the planner's judge"
```

---

### Task 2: Per-variant, per-phase and per-plot observability (server)

**Files:**
- Modify: `crates/ed-galaxy/src/router.rs:329-341` (Control gains `variant: &dyn Fn(VariantOutcome) + Sync`), `crates/ed-galaxy/src/long_range.rs` run_variants / the debug `wave x{cap_mult} {label}` block (~1247-1275) emits a `VariantOutcome` per result; `crates/ed-api/src/plot.rs:985` (wire `stage` and `variant`, record metrics), `crates/ed-api/src/metrics.rs` (table rows).
- Test: `crates/ed-galaxy/src/long_range.rs` tests, `crates/ed-api/src/plot.rs` tests.

**Interfaces:**
- Produces:
```rust
#[derive(Debug, Clone, Copy)]
pub struct VariantOutcome { pub wave: u64, pub kind: &'static str, pub index: usize, pub ok: bool, pub jumps: u32, pub stops: u32, pub ms: u64, pub coarse_expansions: u64, pub stall_remaining_ly: f32 }
```
  `Control::none()` has a no-op `variant`. Metrics: `edda_route_variant_total{lane,kind,outcome}`, `edda_route_phase_seconds{lane,phase}` (coarse, refine, settle, post), `edda_route_first_found_seconds{lane}`, `edda_route_cpu_seconds{lane}` (sum of worker thread CPU via `cpu_time` per `PoolLease::install`). Tracing: one `info!` per variant with the same fields; one per plot with `first_found_ms`, `cpu_ms`, `variants_run`, `variants_finished`.

- [ ] **Step 1: Failing test in long_range.rs**

```rust
#[test]
fn every_variant_reports_an_outcome() {
    let (g, neutrons) = test_galaxy_with_highway(); // existing fixture used by the pin tests
    let seen = std::sync::Mutex::new(Vec::new());
    let ctl = Control { variant: &|o: VariantOutcome| seen.lock().unwrap().push(o), ..Control::none() };
    let _ = plan_best(&g, Some(&neutrons), &short_long_request(&g), &ctl);
    let seen = seen.into_inner().unwrap();
    assert!(!seen.is_empty(), "the portfolio ran and said nothing");
    assert!(seen.iter().all(|o| !o.kind.is_empty() && o.ms > 0 || !o.ok));
}
```
(`test_galaxy_with_highway` / `short_long_request`: name the existing fixtures in the tests module; if they are called differently, use those names, do not duplicate the fixture.)

- [ ] **Step 2: Run, expect compile failure (no `variant` field)**

- [ ] **Step 3: Implement** the struct, the field, `Control::none()`, the emission in the results loop (reuse the `label` computed for the debug print; compute `stall_remaining_ly` from the `stalled` path by returning it alongside the route in `plan_long_with`'s `Err`/`Ok` — simplest: a thread-local `LAST_STALL_LY` set where the stall line is printed and read in the results loop), the server wiring at `plot.rs:985` (`stage` records phase start/stop into `edda_route_phase_seconds`; `variant` increments the counter and logs), and the CPU accounting (`rayon` worker CPU via `libc::clock_gettime(CLOCK_THREAD_CPUTIME_ID)` sampled at task start/end inside the `map`, summed in an `AtomicU64`; on Windows the same via `GetThreadTimes` behind `cfg`).

- [ ] **Step 4: Run tests; then one plot with debug on the PC to see the lines**

Run: `cargo test -p ed-galaxy --release every_variant_reports_an_outcome && cargo test -p ed-api`
Then: `ED_PLOT_DEBUG=1 bench.exe $IDX Wongi Colonia --ship explorer --budget 30 --min-fuel --json 2>&1 | grep -c 'variant outcome'` → 17.

- [ ] **Step 5: Add the series to the metrics table and commit**

```bash
git commit -am "Planner observability: every variant reports its outcome (kind, wave, jumps, stops, ms, coarse expansions, stall distance); the server records phase seconds, first-found seconds, per-plot CPU seconds and a per-variant counter"
```

---

### Task 3: The coarse timers and the baseline re-cut under one judge (measurement only)

**Files:**
- Create: `docs/benches/2026-10-17-coarse-timers.csv`
- Modify: `docs/benches/2026-10-17-planner-matrix.csv` (rows for the `r0` arm: this branch after Tasks 1-2)

- [ ] **Step 1: Run the timers on the stress pair and three pins**

```bash
for r in "Ood Fleau ZJ-I d9-0|Sphiesi HX-L d7-0" "Sol|Beagle Point" "Colonia|Spase AA-A a108-0" "Wongi|Colonia"; do IFS='|' read -r F T <<< "$r"; ED_COARSE_TIMERS=1 ED_PLOT_DEBUG=1 ED_ONLY_BIDI= bench.exe $IDX "$F" "$T" --ship explorer --budget 100 --min-fuel --inject premium 22 --json 2> "$OUT/timers-$F.err" >/dev/null; grep 'coarse timers' "$OUT/timers-$F.err" | head -3; done
```
Record scan / thin+select / relax / pop / widen shares per route in the CSV with the verdict: which section dominates in the core and at the rim. This decides R4's first file (adjacency vs position arrays).

- [ ] **Step 2: Re-run the matrix with the Task 1-2 binary as arm `r0`**

```bash
docs/benches/knobs/edda_matrix.sh $BENCH r0 docs/benches/knobs/planner_matrix_routes.txt $OUT 100
python docs/benches/knobs/planner_before_after.py docs/benches/2026-10-17-planner-matrix-before.jsonl $OUT/r0.jsonl
```
Expected: exit 0 (Tasks 1-2 change no search; the table is `same` everywhere within noise). Append the rows to the matrix CSV.

- [ ] **Step 3: Commit**

```bash
git commit -am "Bench: coarse timers (where the millisecond goes) and the r0 matrix arm under one judge"
```

---

### Task 4: Leg memo per plot

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs:427` (`refine_leg`), `:3380` (`plan_legs`), `plan_best_inner` (owns the memo for the plot)
- Test: long_range.rs tests

**Interfaces:**
- Produces: `struct LegMemo(Mutex<FxHashMap<LegKey, Result<Route, RouteError>>>)`, `#[derive(Hash, Eq, PartialEq)] struct LegKey { from: u32, to: u32, fuel_bucket: u8, inj_left: u32 }`, `fuel_bucket = ((fuel / capacity) * 32.0).floor() as u8`. `refine_leg` takes `memo: &LegMemo` and returns a clone on hit; `plan_best_inner` creates one memo per plot and threads it through every variant (the wave loop and `refine_waypoints`).

- [ ] **Step 1: Failing test**

```rust
#[test]
fn the_leg_memo_keys_on_the_fuel_bucket() {
    let memo = LegMemo::default();
    let k_full = LegKey::new(1, 2, 128.0, 128.0, 22);
    let k_low = LegKey::new(1, 2, 10.0, 128.0, 22);
    assert_ne!(k_full, k_low, "a leg from a dry tank is a different leg");
    assert_eq!(LegKey::new(1, 2, 127.0, 128.0, 22), k_full, "within a bucket it is the same leg");
    memo.put(k_full.clone(), Ok(dummy_route(5)));
    assert_eq!(memo.get(&k_full).unwrap().unwrap().jumps, 5);
    assert!(memo.get(&k_low).is_none());
}
```

- [ ] **Step 2: Run, expect compile failure**

- [ ] **Step 3: Implement** the types, the lookup at the top of `refine_leg` (before `direct_leg`), the store before every `return`, the memo creation in `plan_best_inner` and its pass-through (`refine_waypoints` gains a `memo: &LegMemo` parameter; `plan_long_bidi`, `plan_long_with`, `plan_long_corridor`, `plan_long_retro` and `plan_graph_first` receive it). `Control` is unchanged.

- [ ] **Step 4: Tests and the measurement**

Run: `cargo test -p ed-galaxy --release`
Then the stress pair at weight 3 with debug: the count of `leg 220: 76 jumps 1132146 expansions` lines must drop from 6 to 1 (`grep -c '1132146 expansions'`), and the matrix arm `r1-memo` renders with no `WORSE`.

- [ ] **Step 5: Commit**

```bash
git commit -am "Planner: identical legs are planned once per plot (memo keyed on endpoints, fuel bucket and injections left); six variants recomputed one 68 s hand-off leg on the rim-to-rim route"
```

---

### Task 5: Injections searched only when the plain pass pushes nothing closer

**Files:**
- Modify: `crates/ed-galaxy/src/router.rs:555-620` (the two-pass loop in `plan`)
- Test: router.rs tests

- [ ] **Step 1: Failing test**

```rust
#[test]
fn an_island_origin_still_opens_with_an_injection() {
    // Origin, one star 80 ly away, goal 150 ly away; range 72; premium x2.
    let g = line_galaxy(&[0.0, 80.0, 150.0]);
    let req = RouteRequest { from: 0, to: 2, range_ly: 72.0, injection: Some((2.0, "premium", 2)), ..test_request() };
    let r = plan(&g, &req, &Control::none()).expect("the injected hop opens the island");
    assert_eq!(r.injections, 1);
}

#[test]
fn no_injection_is_searched_where_a_plain_hop_progresses() {
    let g = line_galaxy(&[0.0, 60.0, 120.0, 180.0]);
    let req = RouteRequest { from: 0, to: 3, range_ly: 72.0, injection: Some((2.0, "premium", 2)), ..test_request() };
    let r = plan(&g, &req, &Control::none()).unwrap();
    assert_eq!(r.injections, 0);
    assert!(r.expansions <= 4, "the injected sphere was scanned on plain expansions: {}", r.expansions);
}
```
(`line_galaxy` / `test_request`: the router tests already build tiny galaxies for the dry-jump tests; reuse those helpers' names.)

- [ ] **Step 2: Run, expect the second test to fail on `expansions`**

- [ ] **Step 3: Implement**: in pass 0 track `let mut progressed = false;` set when a relaxed candidate's `to_goal < remaining` is pushed; before pass 1: `if progressed { break; }`. Keep the goal special case (a goal within injected reach is always tried).

- [ ] **Step 4: Tests + measurement**: `cargo test -p ed-galaxy --release`; the Jongou pairs' `injections` column in the matrix arm `r1-inj` unchanged from baseline; the leg `Hypoe Groa NN-Q d6-0 -> Sphiesi HX-L d7-0` standalone expansions (8,591 today) recorded before/after.

- [ ] **Step 5: Commit**

```bash
git commit -am "Exact planner: the injected sphere is scanned only where the plain pass pushed nothing closer to the goal; injections stay a last resort in effort, not only in price"
```

---

### Task 6: Coarse fanout and sector k-best thinning

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs:77` (`COARSE_FANOUT`), `:3146-3185` (bucket thinning), `:2793` (fanout knob)
- Test: long_range.rs tests

**Interfaces:**
- Produces: `fn sector_of(here: [f32;3], there: [f32;3]) -> u8` (26 sectors by sign of each axis delta with a dead band of 0.3 × distance, so 0 is "ahead along the goal axis"), `fn thin_by_sector(cands: &mut Vec<Cand>, here: [f32;3], keep_per_sector: usize)` keeping per sector the farthest and the nearest-scoopable candidate; knob `ED_COARSE_SECTOR_KEEP` (default 2), `ED_COARSE_FANOUT` default 64.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn sector_thinning_keeps_a_refuel_candidate() {
    let here = [0.0, 0.0, 0.0];
    let mut cands = vec![
        cand([400.0, 0.0, 0.0], false), cand([350.0, 5.0, 0.0], false), cand([300.0, -5.0, 0.0], true), cand([200.0, 0.0, 0.0], false),
    ];
    thin_by_sector(&mut cands, here, 2);
    assert!(cands.iter().any(|c| c.pos[0] == 400.0), "the farthest stays");
    assert!(cands.iter().any(|c| c.refuels), "the scoopable stays even though it is not the farthest");
    assert_eq!(cands.len(), 2);
}
```

- [ ] **Step 2: Run, expect compile failure**

- [ ] **Step 3: Implement** behind the knobs with the shipped behaviour as the default until Step 4 says otherwise (`ED_COARSE_SECTOR_KEEP=0` = old bucket thinning).

- [ ] **Step 4: A/B on the matrix**: arms `r1-sector2` (`ED_COARSE_SECTOR_KEEP=2 ED_COARSE_FANOUT=64`) and `r1-sector4` against the current arm. Pre-registered expectation: coarse expansions per plot unchanged, ms per expansion down 3-10x in the core (timers), first-found earlier on every crossing, jumps within the gate. If the gate holds, make `2 / 64` the defaults; if not, record the negative and leave the knobs.

- [ ] **Step 5: Commit**

```bash
git commit -am "Coarse search: candidates are thinned per direction sector (farthest plus the refuel option) and the fanout cap is back on; measured on the 28-route matrix"
```

---

### Task 7: Variants in time-to-first order, staged; long lane sized for the box

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs:891-935` (`run_variants` takes an `order: &[usize]`), `~1200-1245` (the index → variant map gets a `VARIANT_ORDER` table), `crates/ed-api/src/plot.rs:568-569` (`LONG_THREADS`), `:60-70` (`LONG_CONCURRENCY`)
- Test: long_range.rs tests, plot.rs tests

**Interfaces:**
- Produces: `const VARIANT_ORDER: &[&str] = &["greedy single", "scan-twin 5", "single w1.5 r0.75 b0.75", "bidi", "single w1.3 r0.75 b0.5", /* rest in today's order */];` applied as a permutation before `into_par_iter`; staging: `run_variants` runs the first `stage = rayon::current_num_threads()` indices, and the rest only if no result arrived or the grace clock has not fired. `LONG_THREADS` 4 → 8 on an 8-core box (`EDDA_API_PLANNER_THREADS_LONG` stays the override), `LONG_CONCURRENCY` stays 2 (4 threads per plot).

- [ ] **Step 1: Failing test**

```rust
#[test]
fn variants_run_in_the_declared_order_and_the_tail_waits() {
    let ran = std::sync::Mutex::new(Vec::new());
    let (_, finished) = run_variants(6, None, None, &|| false, |_: &u32| 0, |i, _| { ran.lock().unwrap().push(i); (Ok(i as u32), true) }, &[3, 0, 5, 1, 2, 4], 2);
    assert_eq!(finished, 6);
    let ran = ran.into_inner().unwrap();
    assert_eq!(&ran[..2], &[3, 0], "the head stage runs first, in order");
}
```

- [ ] **Step 2: Run, expect compile failure**

- [ ] **Step 3: Implement** the permutation and the two-stage run (head = first `stage_size`, tail only when `results_so_far.iter().all(Err)` or when the grace has not fired and the budget has room); the order table from the measured first-found of each variant on the matrix (Task 2's per-variant outcomes give it; fill the constant from that CSV, do not guess).

- [ ] **Step 4: Measurement**: matrix arm `r1-order`; on the box, replay the 2026-10-12 rows (`stress_route.sh` with the Jongou bodies) after deploy: first route in single-digit seconds is the pre-registered expectation; canary bounds unchanged. The desert pins (Wongi -> Beagle Point, Colonia -> Spase, Wongi -> Spase) must keep their jumps and stops (Review Focus 5).

- [ ] **Step 5: Commit**

```bash
git commit -am "Portfolio: variants start in measured time-to-first order and the tail waits for the head; the long lane gets the box's cores (4 planner threads per plot on 8 vCPU)"
```

---

### Task 8: Approach legs planned from the sparser end

**Files:**
- Modify: `crates/ed-galaxy/src/long_range.rs:427` (`refine_leg`: before the `exact` fallback), new `fn plan_reversed(g, req, ctl, leg_of, from, to, fuel) -> Option<Route>` that plans `to -> from` and flips hops, then re-simulates fuel with `refuel_hops`
- Test: long_range.rs tests

- [ ] **Step 1: Failing test**

```rust
#[test]
fn a_reversed_leg_flies_forward_after_the_flip() {
    let g = line_galaxy(&[0.0, 60.0, 120.0, 180.0]);
    let req = test_request_with_fuel(&g);
    let r = plan_reversed(&g, &req, &Control::none(), &leg_of_test(&req), 0, 3, 128.0).unwrap();
    assert_eq!(r.hops.first().unwrap().idx, 0);
    assert_eq!(r.hops.last().unwrap().idx, 3);
    assert!(r.hops.iter().all(|h| h.fuel_after.unwrap() >= 0.0));
}
```

- [ ] **Step 2: Run, expect compile failure**

- [ ] **Step 3: Implement**: density test = count of stars within one reach of each end via `g.for_each_within` (cheap, two cell walks); when the destination's count is below a quarter of the origin's, plan reversed; keep the forward plan as the fallback when the reversed one fails `refuel_hops`.

- [ ] **Step 4: Measurement**: the standalone final approach (`Hypoe Groa NN-Q d6-0 -> Sphiesi HX-L d7-0`) expansions 8,591 → ~300; matrix arm `r1-reverse` with no `WORSE`; `plot_fuzz` same-seed 0 unflyable.

- [ ] **Step 5: Commit**

```bash
git commit -am "Refinement: an approach leg into a sparse end is planned from that end and flipped (30x fewer expansions measured on the rim approach); fuel is re-simulated forward"
```

---

### Task 9: The after table and the ledger

- [ ] **Step 1:** `edda_matrix.sh` with the final R1 binary as arm `r1`; `planner_before_after.py before r1 --ref ref.jsonl` → the table in `docs/benches/2026-10-17-planner-matrix.csv`'s header and in the PR body; exit 0 is the gate.
- [ ] **Step 2:** Ledger entry under Server: R0-R1 done, with the table's headline (routes better / same / worse, first-found and wall deltas), and the next plan (R2) named.
- [ ] **Step 3:** The maintainer flies the dev app on the change (house rule) before any release; deploy the API on his word; replay the box numbers into the CSV.

---

## Plans that follow (separate documents)

- R2: one currency in the edges and the fuel-blind coarse state (`scoop250.bin`, seconds-priced edges, dry-run cap, generalised crossing replan).
- R3: the anytime restarting weighted A* with the realistically priced goal field, progress-rate stall, restart-from-stall, 2-3 families.
- R4: `hwgraph250.bin`, priced long bridges, component ids.
