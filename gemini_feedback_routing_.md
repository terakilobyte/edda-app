You are solving a genuinely nasty variant of the resource-constrained shortest path problem: a **~150M-node 3D spatial point cloud** spanning 85,000+ ly, where edge existence and cost depend non-linearly on a continuous state variable (fuel mass via [`FuelModel::range_at`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/fuel.rs#L171-L185)), supercharge multipliers create **4×–6× asymmetric wormholes** that don’t burn extra fuel, and sparse inter-arm voids break greedy Euclidean heuristics.

I reviewed the spatial index ([format.rs](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs)), exact leg planner ([router.rs](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs)), long-range highway portfolio ([long_range.rs](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs)), topology sidecars ([cgraph.rs](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs), [alt.rs](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/alt.rs), [agg.rs](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/agg.rs)), secondary planners ([carrier.rs](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/carrier.rs), [profit.rs](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-route/src/profit.rs), [engineer_tour.rs](file:///c:/Users/terak/ED-Claude/edda-app/src-tauri/src/engineer_tour.rs)), and your active unification spec ([2026-10-17-planner-unification-design.md](file:///c:/Users/terak/ED-Claude/edda-app/docs/superpowers/specs/2026-10-17-planner-unification-design.md)).

---

## 1. What You Are Doing Exceptionally Well

### A. Zero-Copy Morton Indexing with `BIGMIN` Range Skipping
* **Why it works:** Packing ~150M stars into a memory-mapped 29-byte [`StarRecord`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L15-L21) sorted by 63-bit Z-order curve ([`morton_cell_key`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L151-L157)) keeps spatially adjacent stars on adjacent OS pages.
* **The standout detail:** Your Tropf & Herzog [`morton_bigmin`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L174-L199) implementation paired with the empirically measured probe-vs-walk crossover (`PROBE_MAX_VOLUME = 216` in [`for_each_cell_in_box`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L682-L742)) and half-sphere goal pruning ([`for_each_within_toward`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L878-L942)) avoids allocating intermediate candidate vectors in the hot loop.

### B. Analytical Micro-Refiners Before Full A*
* In [`refine_leg_uncached`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L472-L510), you don’t blindly invoke [`router::plan`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs#L582) on every coarse highway pair. You cascade through:
  1. **0-search [`direct_leg`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L181-L237):** Checks single-hop boosted reach + `next_scoop` safety.
  2. **1-sphere [`scoop_leg`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L243-L320):** Analytically intersects the boosted shell from `a` with the plain-jump shell into `b`.
  3. **Bounded [`bridge_leg`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L329-L448):** Steps up to 4 hops to a nearby neutron launchpad when `a` isn’t a supercharger.
* This resolves >90% of highway legs in microseconds without touching an open-set heap.

### C. Post-Hoc Fuel-Mass Decoupling (`minimize_refuels` + `crossing_replan`)
* Coupling exact continuous fuel mass into the A* state space explodes the branching factor. Your two-phase insight is textbook operations research:
  1. Prove reachability assuming eager scooping at scoopable stars, then run backward-slack simulation in [`minimize_refuels`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs#L254-L355) to strip redundant scoops (relying on the monotonicity property that *skipping a scoop leaves the ship lighter and strictly increases jump range*).
  2. Follow up with [`crossing_replan`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L1725-L1854) ("cross dry, then drink") to re-plan plain-jump desert runs at lean-tank mass.

### D. Gap-Aware Coarse Topology (`CellGraph` + `GoalField`)
* Pure Euclidean heuristics fail catastrophically across the Abyssal Plain and inter-arm gaps. Building a 250 ly cell graph ([`cgraph.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs)) that uses union-find at build time to synthesize **foreign-component** and **beyond-reach** gap edges priced at plain-jump rates—and running a per-plot backward Dijkstra ([`GoalField`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs#L65-L162))—gives the star-level search true topological awareness around voids.

### E. Measurement Doctrine & Negative-Result Hygiene
* Your discipline in [ROADMAP.md](file:///c:/Users/terak/ED-Claude/edda-app/docs/ROADMAP.md) (recording *why* 57 ideas were buried alongside the exact commit and regression numbers) is rare and prevents circular re-invention.

---

## 2. Techniques You Are Missing (and Why the Search Space Still Fights Back)

Your active unification spec ([2026-10-17-planner-unification-design.md](file:///c:/Users/terak/ED-Claude/edda-app/docs/superpowers/specs/2026-10-17-planner-unification-design.md)) rightly identifies the three biggest architectural pain points: the 17-variant portfolio tax, fuel-quantized coarse state, and repeated 3D spatial scans (`hwgraph250.bin`).

However, several **algorithmic techniques from the large-scale route-planning literature** are missing from both the current code and the R0–R4 unification roadmap—particularly around *why* single-engine WA* floods in the core at `w=1.5` and floods at the rim at `w=3.0`.

### A. Missing Technique: Focal Search ($\text{A}^*_\epsilon$) / Potential-Driven Search Instead of Pure Weighted A*
* **The pathology you observed in R1 (`#75`):** In [`plan_long_with`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L2769), you use standard Weighted A* (`f = g + w * h`). Even with [`GoalField`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs#L65), the cell-graph edges are baked at build time using a fixed optimistic reference reach (`REF_BOOSTED_REACH_LY = 470.0` in [`cgraph.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs#L37)).
  * In the galactic core, real star-to-star traversal costs ~1.3×–2.2× more per ly than `h` predicts (due to ship range < 470 ly, bridge hops, and scoop detours).
  * In WA*, the moment $w \cdot \frac{\Delta h}{\Delta d} < \frac{\Delta g}{\Delta d}$, **every step forward increases $f(n)$**. The priority queue switches from depth-first progress toward the goal into a **breadth-first flood bubble** until all nodes in the पीछे (behind) cone reach that $f$-level—burning your 30,000-expansion cap 45,000 ly short.
  * Conversely, cranking `w=3.0` crosses the core, but at the sparse rim where plain bridges cost 4×–6× the highway rate, even `3.0 * h` underestimates $\Delta g$, or over-commits into a dead-end tendril.
* **The missing technique — Focal Search ($\text{A}^*_\epsilon$) / Explicit Estimation Search (EES):**
  * Decouple the **admissibility/quality bound** $f(n) = g(n) + h_{\text{admissible}}(n)$ from the **progress heuristic** $\hat{h}(n)$ (remaining topological cell-graph distance or calibrated jump count).
  * Maintain the open set ordered by $f(n)$ (with $f_{\min}$ at the head), and a **focal list** containing all open states with $f(n) \le w \cdot f_{\min}$, ordered by $\hat{h}(n)$ (or $\hat{f}(n) = g(n) + \hat{h}(n)$ where $\hat{h}$ uses the ship's *actual* empirical cost rate).
  * **Why it solves R2/R4's dilemma:** Inside the focal window $[f_{\min}, w \cdot f_{\min}]$, the search expands strictly by lowest $\hat{h}$ (closest to goal along the [`GoalField`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs#L65) gradient). It **cannot** flood laterally in the dense core while forward edges exist within the suboptimality bound $w$, yet if a sparse rim tendril genuinely dead-ends, $g(n)$ rises past $w \cdot f_{\min}$ and the focal window naturally shifts to the next corridor.

### B. Missing Technique: Query-Time Ship-Calibrated Edge Pricing in `GoalField`
* Right now, [`CellGraph::goal_field_where`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs#L89-L162) runs a per-plot Dijkstra in ~15–25 ms over 475k cells, summing static `u16` edge weights (`cost10`) baked for an idealized 470 ly ship.
* Because `GoalField` is *already* computed per plot at query time, storing static `cost10` on edges wastes the best property of a per-plot potential field!
* **How to improve it:** Store `(dist_ly: u16, min_pair_ly: u16, flags: u8)` on each edge in `graph250.bin`. During [`goal_field_where`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs#L89), price each edge dynamically using the **caller's actual `FuelModel`**, `boost_mult`, `allow_white_dwarf`, and `scoop_bonus`:
  * Highway edge ($d$): $\approx \lceil d / \text{actual\_boosted\_reach} \rceil \times (1 + \frac{1}{\text{tank\_jumps}} \cdot \text{scoop\_overhead})$.
  * Foreign/Beyond gap edge ($d$): $\approx (d / \text{actual\_plain\_range}) \times \text{plain\_weight}$.
  * Hard-filter edges where `min_pair_ly > actual_boosted_reach` (unless bridged at plain-jump cost).
* This turns `GoalField` from a loose reference lower bound into a **tight, ship-accurate consistent potential function** $\pi(c)$, making $w=1.3$ behave the same for a 35 ly hauler and an 85 ly Mandalay.

### C. Missing Technique: Hierarchical Plain-Gap Subdivision (Fixing the Coarse $\to$ `router::plan` Cliff)
* **The pathology:** When coarse highway search stalls or encounters a >4-hop plain bridge (`MAX_BRIDGE_JUMPS = 4` at [`long_range.rs:40`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L40)), [`plan_long_with`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L3573-L3580) appends `goal` directly onto the coarse waypoint list and hands the remaining gap (sometimes 2,000–45,000 ly!) as a single leg to [`refine_waypoints`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L3595) $\to$ [`router::plan`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs#L582).
* [`router::plan`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs#L582) expands the full 150M-star 50 ly grid with 255 fuel levels and a 500k expansion cap. A single stalled coarse variant handing a 10,000 ly gap to `router::plan` burns **60+ seconds** on one worker thread.
* **How to improve it:**
  1. **Enforce a hard maximum leg span before calling `router::plan`:** [`refine_leg_uncached`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L472) should never hand a leg longer than ~1,200 ly directly to fuel-quantized `router::plan`.
  2. **Intermediate Plain-Corridor Stepping:** When a coarse edge (or a `graph250` `EDGE_FOREIGN` / `EDGE_BEYOND` gap edge) spans $D > 500\text{ ly}$ of non-neutron space, subdivide it into ~400–600 ly scoopable anchor stars along the cell-graph shortest path (or run a fast, fuel-blind plain-star greedy/WA* stepper with `dry_hops`), then pass those short sub-legs into the parallel `refine_waypoints` pool.

### D. Missing Technique: 1D Dominance Array vs. Per-Node Heap-Allocated Pareto `Vec`
* In [`plan_long_with`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L2973-L2992) (and [`router::plan`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs#L625-L645)), dominance pruning maintains `pareto: FxHashMap<u32, Vec<(f32, f32)>>`—allocating a separate heap `Vec` for every visited star.
* Once you move to the D2 fuel-blind coarse state `(star_idx, dry_hops: u8)` (from your [R2 plan](file:///c:/Users/terak/ED-Claude/edda-app/docs/superpowers/plans/2026-10-17-planner-unification-r2.md)), `dry_hops` is a tiny integer in `0..=max_dry_hops` (typically $\le 10$).
* **How to improve it:**
  * A state `(u, dry)` dominates `(u, dry')` iff $g \le g'$ and $\text{dry} \le \text{dry}'$.
  * Instead of a `HashMap<u64, f32>` + `HashMap<u32, Vec<...>>`, each visited star only needs the **minimum `dry_hops` seen at the best `g`** (or a tiny inline `[f32; 8]` array indexed by `dry_hops` stored in a flat pool).
  * In fact, if `dry_hops` only constrains *feasibility* (doesn't change future jump range in the coarse pass), then the first time star `u` is popped at `dry_hops == 0` (freshly scooped), **no future arrival at `u` with any `dry_hops >= 0` and higher `g` can ever improve on it**. That collapses Pareto dominance at scooped stars to a single scalar `min_dry_popped: u8` check!

---

## 3. High-Leverage Micro-Architectural Improvements

Each of these is localized, directly testable against `route_bench` / `compare_spansh`, and orthogonal to your R2–R4 milestones:

### 1. Stop Decoding Full 29-Byte `StarRecord`s in `highway_ok` (Immediate Win)
In [`long_range.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L2103-L2108) and [`long_range.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L2813-L2818):
```rust
let highway_ok = |idx: u32| -> bool {
    allow_white_dwarf || neutrons.class(&neutrons.record(idx)) != StarClass::WhiteDwarf
};
```
Calling [`neutrons.record(idx)`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L483-L497) reads 29 bytes (`id64`, `name_off`, `x`, `y`, `z`, `flags`) from `stars.bin` for every single neighbor candidate before checking `best_g`.
* **Fix:** [`Galaxy`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L510-L515) already has a fast 1-byte reader [`neutrons.class_code(idx)`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L510-L515) (and in `for_each_within_toward`, the callback doesn't even need a second lookup if class/flags are checked via `class_code(idx) != StarClass::WhiteDwarf.code()`). Even better: when building `hwgraph250.bin` in R3, pack a 1-bit `is_wd` flag directly into the upper bit of the 32-bit target index (`u32` has 9 unused upper bits since $N_{\text{highway}} \approx 2.74\text{M} < 2^{22}$!). That makes WD filtering **zero memory reads** during edge iteration.

### 2. Pack Flag Bits into `hwgraph250.bin` Edge Targets (Sharpening R3 / D3)
Before you freeze the `hwgraph250.bin` layout in [2026-10-17-planner-unification-design.md](file:///c:/Users/terak/ED-Claude/edda-app/docs/superpowers/specs/2026-10-17-planner-unification-design.md#L121-L163):
* The highway sub-index has ~2.74M stars ($\le 2^{22}$).
* Instead of `edges_target: [u32; M]` + `edges_dist: [u16; M]` (6 bytes/edge) plus random lookups into `scoop250.bin`, `stars.bin`, and `GoalField` during the inner loop:
  * Pack each edge into a single aligned `u64` (8 bytes):
    * `target_idx: 22 bits`
    * `is_wd: 1 bit` (skip WDs before touching any per-target memory)
    * `target_has_local_scoop: 1 bit` (`scoop_near_ly <= full_range`, eliminating random reads into `scoop250.bin` during expansion!)
    * `dist_tenths_ly: 16 bits`
    * `cell_id: 24 bits` (or pre-grouped by direction sector)
* Because edges from a source are sorted by `dist_tenths_ly` ascending, you can still binary-search the cutoff `max_reach`, and every filter (`is_wd`, `target_has_local_scoop`, `dist`) is evaluated in registers from a single sequential cache line stream before ever touching `best_g[target_idx]`.

### 3. Eliminate Nested Rayon Work-Stealing Stalls (`run_variants` $\times$ `refine_waypoints`)
* Currently, [`run_variants`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L1040-L1086) spawns variants across the Rayon pool, and *inside* each variant, [`refine_waypoints`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L3624-L3648) runs `jobs.par_iter().map(...).collect()`.
* In Rayon's work-stealing scheduler, when Variant A finishes its coarse search in 80 ms and blocks in `.collect()` waiting on its 150 fast legs, its worker thread steals pending Rayon tasks—which can be a **multi-second stalled handoff leg from Variant B**! Variant A cannot return its finished route until that stolen task from Variant B completes.
* Removing the outer variant portfolio in R4 will naturally cure the worst of this, but in general: **never nest unbounded `par_iter` inside a latency-gated race**, and give `refine_leg`'s fallback [`router::plan`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L483-L492) a strict per-leg expansion budget (`max_expansions: 25_000` instead of `500_000`) when called inside `refine_waypoints`.

### 4. Replace `f32::powf` in `FuelModel::fuel_for` with a Specialized Fast Power or LUT
* [`FuelModel::fuel_for`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/fuel.rs#L194-L209) calls `ratio.powf(self.power)` on every candidate edge in [`router::plan`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs#L783) and `bridge_leg`.
* Only 8 exponent values exist in Elite Dangerous (`2.00, 2.15, 2.30, 2.45, 2.60, 2.75, 2.90, 2.5025`), and `ratio` is always in $[0, 1]$.
* A 256-entry piecewise-linear LUT precomputed in [`FuelModel::with_guardian`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/fuel.rs#L140) (or a 3-term minimax polynomial on $[0, 1]$) is accurate to $< 10^{-5}$ tons and replaces a ~15–20 cycle libm `powf` call with a 2-cycle table lookup or FMA chain.

### 5. Port `router.rs`’s Quickselect + Half-Sphere Scan to `carrier.rs`
* In [`carrier::search`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/carrier.rs#L248-L257), every carrier expansion calls `g.within(here, MAX_JUMP_LY)` (allocating a `Vec` of tens of thousands of stars in a 500 ly radius) and runs a full $O(K \log K)$ `cands.sort_by(...)` before `.take(fanout)`.
* [`router.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs#L714-L743) already solved this with reusable scratch buffers, [`for_each_within_toward`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/format.rs#L878), and $O(K)$ [`select_nth_unstable_by`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/router.rs#L735). Porting that exact pattern to [`carrier.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/carrier.rs#L248) will cut carrier plot allocations and core-region expansion time dramatically.

---

## Summary of Recommended Experiments (Under Measurement Doctrine)

| Priority | Hypothesis / Technique | Target | Gate Metric |
| :--- | :--- | :--- | :--- |
| **1 (R2/R4)** | **Ship-Calibrated Query-Time `GoalField`** (price `graph250` edges using caller's `FuelModel` + `boost_mult` during per-plot Dijkstra) | [`cgraph.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/cgraph.rs#L89) | Core + Rim-to-Rim completion at single weight without 30k-cap stall |
| **2 (R2/R4)** | **Focal Search ($\text{A}^*_\epsilon$)** (`f <= w * f_min` window ordered by calibrated `GoalField` $\hat{h}$) | [`long_range.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L3144) | Eliminates core lateral flooding while keeping jump count within 2% of 17-variant portfolio |
| **3 (R3)** | **Bit-Packed 64-bit `hwgraph250` Edges** (`target_idx:22 \| is_wd:1 \| has_scoop:1 \| dist:16 \| cell:24`) | `hwgraph.rs` / [`long_range.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs) | Expansion throughput $\ge 5\times$ vs 3D Morton scans; zero `stars.bin`/`scoop250.bin` cache misses in inner loop |
| **4** | **1D `min_dry_popped` Dominance** (replace `pareto: FxHashMap<u32, Vec<_>>` once coarse state is `dry_hops`) | [`long_range.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L2973) | Zero per-node heap allocations during search; lower memory traffic |
| **5** | **Plain-Gap Waypoint Subdivision** (never hand $>1,200\text{ ly}$ stalled/foreign gaps as a single leg to `router::plan`) | [`long_range.rs`](file:///c:/Users/terak/ED-Claude/edda-app/crates/ed-galaxy/src/long_range.rs#L3573) | Eliminates the 68s `legs_ms` tail on stalled outer-arm routes |
