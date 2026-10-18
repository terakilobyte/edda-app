# The planner against the literature: a holistic review (2026-10-18)

Ordered by the maintainer after the R2 rung and the rim pair landed
(2026-10-17): "we need to holistically analyze what we're doing against
literature and known techniques ... if we're leaving anything on the
table, if we've not explored some things we should have, and why we can't
at least find even 'bad' routes on those two reverse plots." Three
fresh-context analysts read the code, the spec, the ledger and the traces
(no code was run); their reports are the appendices. This section is the
synthesis and the decisions it feeds into the R3 and R4 plans. Where the
analysts disagree or guess, it says so.

## What the day's measurements established (the premises)

- The portfolio planner finds 26 of the 28 pinned routes; both rim
  directions now plot under the external reference's jump count. Two
  routes, the reverse Jongou pair, return no route at all.
- Where it fails, it fails in one of two ways: a coarse search that cannot
  make progress (the bulge choke, the straight-line dead end 5.8 kly from
  an island goal), or a coarse chain whose edge the exact planner cannot
  fly, which then eats the budget proving a negative (the 1,209 ly leg).
- Time-to-first-route is short (6-21 s on three threads for the rim) and
  time-to-final is the budget, because nothing arms the grace clock on a
  route far above the jump floor and the wave policy is a timing
  knife-edge.
- The goal field (a Dijkstra lower bound over the cell graph) steers a
  stalled search correctly (Colonia -> Spase 50 s -> 1.3 s) and misprices
  the Caspian's long plain runs (rim +6 / +13 jumps), because it is priced
  at a reference ship and then multiplied by the search weight.

## Findings that both the literature analyst and the auditor reached

1. **The coarse graph promises edges the exact planner cannot always fly,
   and a failed edge is treated as a failed plot.** Hierarchical planning
   calls the missing guarantee the downward refinement property (Bacchus &
   Yang 1994); HPA* (Botea, Muller & Schaeffer 2004) builds abstract edges
   from real low-level paths; LazySP (Dellin & Srinivasa 2016) and Lazy
   Weighted A* (Cohen, Phillips & Likhachev 2014) solve the optimistic
   graph, check the solution's edges, reprice the failures and solve
   again. In `refine_waypoints` one failed leg returns `Err` for the whole
   variant, its sibling legs run to completion first (no short-circuit),
   the dead edge is never fed back to any coarse search, and the leg memo
   stores `NoRoute` but not `Budget`, so every variant pays the same dead
   leg's cap. **Decision: LazySP repair + a bounded, memoised leg check
   is the first R3 task.**
2. **No existence-first answer exists, and the one fallback that does
   (`plan_graph_first`) runs only after every wave and under the
   deadline-wrapped cancel, so on the plots that need it it returns
   Cancelled at once.** The literature's shape: component labels per
   reach band for an instant "no route" (union-find the cell graph already
   computes and discards), then a greedy best-first on distance-to-go
   (Thayer & Ruml 2009) over certified or lazily checked edges with fuel
   relaxed to feasibility (the electric-vehicle charging-stop framing:
   Baum et al. 2019, Storandt & Funke 2012), started from the sparser
   end (fail-first, Haralick & Elliott 1980), as the incumbent that an
   anytime search (ANA*, van den Berg et al. 2011; Potential Search, Stern
   et al. 2011) then improves and prunes against. **Decision: the
   feasibility pass starts at t = 0 on its own worker and is the R3
   spine; its result is the fallback answer at the budget.**
3. **The exact leg planner proves negatives over an unbounded space.**
   Fuel quantised to 1/255 (D3 says 32), a `toward` cut that drops any
   cell needing more than a one-range retreat (so its `NoRoute` is not a
   fact, yet is memoised as one), and no cost bound. Bounded-cost search
   (Stern et al. 2011) with U = coarse price x slack + c makes a failure a
   proof of "costs more than U" in seconds; a fuel-blind BFS precheck over
   systems makes a dead gap milliseconds. **Decision: both go into the
   R3 leg task; the 32-level quantisation is R2 Task 5, run after.**
4. **The portfolio has no shared incumbent.** Nineteen variants search
   alone and compare at the end; the grace clock reads the best score for
   timing only. Portfolio theory (Gomes & Selman 2001; Luby et al. 1993;
   Helmert et al. 2011) says share the bound and slice time; the pulse
   algorithm (Lozano & Medaglia 2013) prunes resource-constrained labels
   with a global bound. **Decision: an atomic incumbent owned by
   `run_variants`, pruning coarse states by jumps + floor; waves resume
   instead of restarting.**
5. **The stop rule.** The arm ceiling + grace clock + prize-aware waves
   are hand-made; ARA*'s rule (Likhachev et al. 2003) stops when incumbent
   <= (1 + e) x lower bound, and the reference-priced field is a proven
   lower bound for every ship. **Decision: the bound-gap stop replaces
   the ceiling once a tight bound exists; measured on the matrix first,
   because the field is off by default and the euclid floor is loose.**
6. **The widen rescan relaxes the whole annulus unthinned** (R0 timers: 70
   % of a variant's coarse time; the probable source of the 201 M beaten
   refusals). The two cheaper widen rules measured negative because they
   removed the long bridges; thinning the annulus's relax set keeps them.
   **Decision: an R3 arm, pre-registered: widen time down 5x, Beagle 191
   j, Colonia -> Spase unchanged, rim forward within +5 j.**
7. **The weight, the 30,000 cap and the hand-off stand in for "the
   heuristic is lying here".** Weighted A* fails when h tracks the cost
   badly (Wilt & Ruml 2012); focal search / EES (Pearl & Kim 1982; Thayer
   & Ruml 2011) separate guidance from the bound, and the search can learn
   its own jumps-per-ly rate per region (Thayer, Dionne & Ruml 2011). The
   field's fault is reference-ship pricing times the weight; CRP-style
   per-band costs (Delling et al. 2017) fix the pricing. **Decision:
   R4 territory, after the R3 spine is measured; the auditor and the
   analyst both rank it last and say R3 may deliver most of it.**

## Things on the table nobody had tried

- The thin bidi is not thin (no fanout cap in `plan_long_bidi`); it is
  the regular bidi at weight 3 and it is what wins the rim reverse.
  Measure a capped one as its own arm.
- `highway_ok` decodes the full record per candidate when only the class
  is needed; `fuel_for` is recomputed up to four times per relax;
  `full_idx` maps waypoints by name with allocations. Each small, each a
  counter away from a number.
- Sector thinning is a Yao graph; the keep rule for a hop objective is
  "most progress per cone", not nearest, which is consistent with the
  negative sector arms.
- Bidirectional search with fuel searches backward over "fuel needed to
  finish" (Righini & Salani 2006; Thomas et al. 2019): the framing that
  lets a backward seed start at an injection-only goal, which today's
  bidi cannot.

## Why the two reverse Jongou plots return nothing

Appendix C has the evidence from the traces and route files. The account:
the straight-line coarse frontier dies 5.8 kly from the island in a
northern lobe that does not connect to the arm the forward route follows
(5.6-9 kly south), never using an injection; with the field forced on the
chain does reach the goal, but through Gonguae GW-C d0, a neutron at the
bottom of a one-way funnel (its only plain neighbour was itself entered
by a 134 ly injection), and the exact planner cannot fly the 1,209 ly
edge into it -- a void needing injections, not a fuel problem -- and
proving that takes the budget because every injection costs 30 plain
jumps of search before A* will try it. The forward route is not simply
reversible either: 34-39 of its boosted hops land on non-neutrons and
cannot be flown back. Nothing in the portfolio asks the cheaper question
first. The R3 spine answers it: a feasibility route (state = star, jumps
since the last scoop, injections used; injections in lexicographic
buckets; bidirectional, the backward seed entering the island with one
injection), then refinement cut at witness neutrons so a dead leg cannot
exist, then LazySP repair of any false edge.

## The R3 plan this feeds

Tasks, each gated on the 28-route matrix on both thread counts plus the
rim pair and the reverse Jongou pair: (1) bounded, memoised leg checks
with a fuel-blind precheck and LazySP repair; (2) the feasibility-first
incumbent at t = 0, shared with every variant as a pruning bound, and the
fallback answer at the budget; (3) component labels per reach band;
(4) the widen's relax set thinned; (5) the bound-gap stop rule and
resumed waves; (6) the winner tabulation per route class (the "we got
lucky" question). R2 Tasks 4 and 5 follow; R4 (honest bridge pricing from
the aggregate, the highway adjacency file, per-band field costs) after.

The appendices follow verbatim.

# Appendix A. The planner against the search literature (fresh-context analyst, 2026-10-17, read-only)

## Two code facts the ledger did not state

1. A failed refinement leg kills its whole variant (`refine_waypoints`, long_range.rs): the coarse graph never learns the edge was false, nothing is re-searched around it.
2. A leg that fails on its time cap is never remembered: `LegMemo::put` stores routes and `NoRoute` but not `Budget` or `Cancelled`, so every variant that reaches the same dead leg pays the cap again (13 failed legs on the reverse Jongou trace).

## 1. Techniques that fit but are not used

**The coarse graph promises edges the exact planner cannot always fly** (gaps under boosted reach + 4 plain jumps mid-route, 12 at the goal, 60 at the start). Hierarchical planning calls the missing guarantee the downward refinement property (Bacchus & Yang, AIJ 1994); HPA* (Botea, Muller & Schaeffer, 2004) builds every abstract edge from a real low-level path. Robotics handles costly edge checks lazily: LazySP (Dellin & Srinivasa, ICAPS 2016) solves the optimistic graph, checks the solution's edges, reprices or removes the failures, solves again; Lazy Weighted A* (Cohen, Phillips & Likhachev, 2014) postpones each edge check until the edge is about to be used. The planner has neither: a failed edge is a failed plot.

**A fixed weight, a 30,000-expansion cap, then waves.** Weighted A* fails when the heuristic tracks the true cost-to-go badly (Wilt & Ruml, SoCS 2012): progress costs ~2.5 jumps per 433 ly in the core and another rate at the rim, so no single weight works everywhere. Restarting Weighted A* (Richter, Thayer & Ruml, ICAPS 2010) is what D2 proposes. Focal search (Pearl & Kim, 1982) and EES (Thayer & Ruml, IJCAI 2011) separate what guides the search from what bounds it; corrections learned during the search (Thayer, Dionne & Ruml, ICAPS 2011) would learn the jumps-per-ly rate per region and retire the weight sweep and the cap, which today stands in for "the heuristic is lying here". ANA* (van den Berg et al., AAAI 2011) and Potential Search (Stern, Puzis & Felner, ICAPS 2011; JAIR 2014) need no weight schedule: they expand by likelihood of beating the incumbent. For the first route fast, search on distance-to-go (Thayer & Ruml, ICAPS 2009).

**The 19-variant portfolio run to completion.** Run times are heavy-tailed (1.2 s for one variant, never for another on the same pair). Portfolios (Gomes & Selman, AIJ 2001), heavy tails (Gomes, Selman & Kautz, AAAI 1998), Luby's universal restart schedule (Luby, Sinclair & Zuckerman, 1993) and offline schedules fitted on training instances (Fast Downward Stone Soup, Helmert, Roger & Karpas, 2011) all say: slice time, and share the incumbent as a pruning bound. The matrix jsonl rows are the training data; the declared-order arm measured negative on three threads because the crossing family sat late.

**The grace clock and prize-aware waves** are hand-made stopping rules. The literature: performance profiles for anytime algorithms (Zilberstein, AI Magazine 1996; Hansen & Zilberstein, AIJ 2001) or ARA*'s bound (Likhachev, Gordon & Thrun, NIPS 2003; Hansen & Zhou, JAIR 2007): stop when incumbent <= (1+e) x lower bound. The reference-priced goal field is a proven lower bound for every ship; `ship_bound` already uses it, loosely, only to start another wave.

**Handing a stalled coarse search to the exact planner** has no literature behind it; LazySP would reprice the edge and search again.

**Landmarks, contraction, labels.** CH (Geisberger et al., 2008), hub labels (Abraham et al., SEA 2011), transit nodes (Bast et al., 2007) assume a fixed explicit graph with one metric; the fitting method for a ship-dependent metric is CRP (Delling, Goldberg, Pajor & Werneck, Transportation Science 2017): fixed partition, costs recomputed per ship (here per reach band). The goal field is a pattern-database-style perfect heuristic of an abstraction (Culberson & Schaeffer, 1998; Hierarchical A*, Holte et al., 1996) and, where built, makes 16-landmark ALT with 866 ly slack redundant. The field's measured fault (rim reverse 447 -> 460 j) is reference-ship pricing multiplied by weight 3: an abstraction too coarse for the ship, used as guidance without a bound; per-band customisation fixes the first part, a focal/EES split the second.

**Bidirectional search with fuel.** Bidirectional labelling for resource-constrained shortest paths (Righini & Salani, 2006) and bidirectional A* for the same (Thomas, Calogiuri & Hewitt, Networks 2019) search backward over "fuel needed to finish", not "fuel left" -- the framing that lets a backward seed start at an injection-only goal. MM and NBS (Holte et al., AAAI 2016; Chen et al., IJCAI 2017) bound each frontier's overreach.

**Fuel as a resource.** Electric-vehicle routing with charging stops is the analogue: Baum, Dibbelt, Gemsa, Wagner & Zundorf (Transportation Science 2019), Storandt & Funke (AAAI 2012), Khuller, Malekian & Mestre (ACM TALG 2011): settle feasibility on a graph of chargers whose edges are "reachable without charging", optimise charge in refinement -- the D3/R2 fuel-blind coarse with a dry-jump cap. The per-(cost, fuel) Pareto list is standard labelling (Desrochers & Soumis, 1988; Irnich & Desaulniers, 2005) with its known label explosion; the pulse algorithm (Lozano & Medaglia, 2013) prunes labels with a global bound, which the planner lacks.

**Sector thinning** is a Yao graph (a cone spanner); 2 per sector of 14 in 3D is a weak spanner and the keep rule for a hop objective is "most progress per cone" (hop spanners, Conroy & Toth, SoCG 2022; applicability to boosted unit-ball graphs uncertain). The negative sector arms agree.

## 2. Why the reverse Jongou plots find no route, and the existence-first shape

Two layers: the coarse graph's false promise (the 1,209 ly Jongua XV-E d11-0 -> Gonguae GW-C d0 bridge), then the exact leg planner proving a negative over an unbounded space (from a dense region at weight 1.3 the reachable set is the whole connected galaxy; "no route" can only arrive by timeout). Searching backward from the island side would be finite (the island's plain-jump component is small); solve the most constrained end first (Haralick & Elliott, 1980).

Recommended: (a) component labels per reach band (offline union-find over the star index for a few plain bands and boosted bands, per star or per cell): different components = NoRoute in microseconds; same component = a feasibility search is guaranteed to succeed; injections need their own band with a use limit, and an island goal is answered as "reachable only with injections" with its entry stars named. (b) Bounded-cost leg evaluation (Stern et al., 2011): U = coarse price x slack + c, prune g + h >= U, a failure proves "costs more than U" in seconds and is cached with its bound. (c) LazySP repair: a failed edge is repriced or removed and the coarse search re-runs (the thin single's coarse is ~0.5 s); LPA*/D* Lite (Koenig & Likhachev, 2002) if re-runs grow costly.

Shape: Stage 0 (ms) component check; Stage 1 (<= 5 s on three threads) greedy best-first on distance-to-go over certified or lazily checked edges, fuel relaxed to "refuel at any scoopable star, at most k dry arrivals" (k ~ 18 for the Caspian), with type-based exploration (Xie, Muller & Holte, AAAI 2014) or a novelty queue (Lipovetzky & Geffner, 2017) against plateaus, started from the sparser end, refined with bounded legs and LazySP repair -> the incumbent U and the fallback answer; Stage 2 (anytime) Potential Search or ANA* with the ship-scaled field as the bound and the learned rate as guidance, every frontier pruned by U, stop at U <= (1+e) x LB or at the budget. Open: whether a refinable coarse path into Jongou exists under today's bridge limits at all; if not, Stage 1 needs island-entry edges that permit injections, not a larger goal-bridge allowance (negative at 90).

## 3. Five changes, ranked

1. Bounded, cached leg checks + LazySP repair (U = 2 x coarse price + 10 jumps; failures incl. timeouts memoised with their bound; a failed edge repriced in the variant's coarse and re-searched). Confirm: the dead 1,209 ly leg fails < 3 s; no edge checked twice per plot; both reverse Jongou pairs plot or show a proven gap; 0 worse rows vs the current three-thread arm. Bury: a pinned route losing jumps because U cut a legitimate long leg (the 742 ly rim leg first).
2. Component labels per reach band + island entry points. Confirm: built < 1 h at a stated size; agrees with every matrix outcome; reverse Jongou classified < 1 ms. Bury: build cost/size the box cannot carry, or labels too coarse under relaxed fuel.
3. Feasibility-first incumbent (greedy on distance-to-go with exploration, sparser end first) feeding an anytime search pruned by it. Pre-register: first route < 10 s on all 28 rows on three threads; zero NoRoute on component-matched pairs; finals within +1 j / +1 stop of the current arm. Bury: no reduction in final-phase expansions with U shared.
4. Time-sliced schedule fitted on the matrix (Stone Soup / Luby) on three threads with the incumbent shared; leave-one-out fit. Bury: any route worse (the declared-order failure).
5. Learned rate + ship-scaled field with a focal/EES split, replacing the weight, the cap and the hand-off. Confirm: one weight column; rim reverse <= 447 j with the field on; Colonia -> Spase keeps 1.3 s; the run-time tail disappears. Bury: the 460-j effect returns. Least certain; 1 and 3 may already deliver it.

# Appendix B. Code audit: what the planner wastes and what it does not do (fresh-context auditor, 2026-10-17, read-only; C = read in the code, P = plausible, unmeasured)

The min-fuel portfolio is 19 variants (6 singles + 2 thin + bidi + 2 greedy + 2 off-ramp + 6 scan twins); the default shape is 13 (C).

## Three findings to read first

1. **The fallback that already exists cannot run on the plots that need it** (C). `plan_graph_first` runs only when `best.is_none()` after every wave, checks the deadline-wrapped `cancelled` closure, and its legs go through `plan()` which checks it too: on a plot whose waves run to the deadline (the reverse Jongou) it returns Cancelled at once.
2. **The thin bidi is not thin** (C). `THIN_FANOUT` is read only in `plan_long_with`; `plan_long_bidi` has no fanout cap and a fixed 0.75 bucket, so index 1 is the regular bidi at weight 3 (the doc comment on THIN_FANOUT overstates).
3. **One failed leg kills the whole variant, and its siblings run to completion first** (C). `refine_waypoints`' first pass is `planned[i] = Some(r?)` over a `par_iter().collect()` with no short-circuit: every other leg (each capped at 25 s) finishes before the variant returns Err. The dead edge is never fed back to any coarse search; the memo hands the next variant the same NoRoute and it dies on the same edge.

## 1. Inefficiencies by expected cost on a three-thread, 100 s plot

1. **The widen relaxes the whole annulus unthinned** (C): when an expansion pushes nothing, the rescan goes to direct + 12 x range (~1,400 ly vs 546, ~17x the volume) and relaxes every candidate, skipping the bucket/sector thinning and the fanout. R0 timers: 6.3 of 9 s a variant (70 %); the likely source of the 201 M beaten refusals (P). Measure: ED_COARSE_TIMERS + relax calls per widen.
2. **Escalation waves restart from zero** (C): each x4 wave re-runs every variant's coarse search from scratch (~25 % of wave 2 and 6 % of wave 3 is repetition, P). Measure: per-wave coarse expansions to the wave-1 frontier.
3. **Nineteen variants scan the same nodes** (C): each does its own Morton walk of 1,400-6,700 candidates per expansion; `highway_ok` decodes the full record when only the class code is needed (C). Measure: (node, radius band) scans vs distinct nodes = the payoff of a shared neighbour cache or hwgraph250.bin.
4. **No shared incumbent, no shared g across variants** (C): GraceClock holds the best score for timing only; nothing prunes with it. Measure: open states with jumps + floor >= incumbent at each finish.
5. **The coarse state carries 32 fuel levels and three hash maps** (C); `relax` recomputes `fuel_for` up to four times for the same arguments (C). The bidi is already fuel-blind and is what wins the rim reverse. Measure: best_g.len() vs distinct neutrons; relax ns per candidate.
6. **The exact planner quantises fuel to 1/255** (C) where D3 says 32: a no-route proof walks up to 256 states per system -- why a dead 1,209 ly leg could not say "no" in 40 s (P). Measure: states per system on the 742 and 1,209 ly rim legs at 255 vs 32.
7. **The exact planner is incomplete and its NoRoute is memoised as a fact** (C): non-thorough `toward = (goal, remaining + range)` drops any cell needing more than a one-range retreat, LEG_FANOUT is 512; a gap leg needing a sideways detour reports NoRoute, LegMemo stores it, every variant inherits it. Measure: re-run the failed reverse Jongou legs with `thorough` or `toward = None`.
8. **Nested par_iter inside variants** (C structurally): a variant blocked in its leg collect steals another variant onto its stack. Measure: coarse_done-to-route time vs the sum of leg times.
9. **Leg memo keys split by reserve** (C, correct): the 0.4 and 0.75 reserve variants share nothing (P ~half the portfolio gets no memo benefit). Measure: hit rate per reserve.

Smaller (each likely < 1 %, P): `through_stop` binary search per flagged candidate; `full_idx` maps waypoints back by name with two lowercase allocations; a thread-local EXACT_SCANS increment per candidate.

## 2. Techniques that fit and are absent

- **LazySP repair**: `refine_waypoints` returns the failing (from, to, fuel); a per-plot shared dead-edge set that `relax` in both coarse searches checks; the variant re-runs its coarse keeping the refined prefix; an abort flag short-circuits the first-pass collect. Replaces "variant dies, the next picks the same edge".
- **Feasibility-first incumbent** (the R3 seed): launch the fallback at t = 0 on its own worker under the plain user Stop, not the deadline closure -- a cell-graph path refined with a fuel-blind leg, or a fuel-blind BFS over the full index at full-tank range inside the corridor. Replaces the end-of-plot `plan_graph_first`, dead on deadline-bound plots.
- **Leg feasibility precheck**: a fuel-blind BFS over systems only, full-tank range, no `toward` cut, before `plan()` in `refine_leg_uncached`: the 25 s dead-leg timeout becomes milliseconds (P) and prices long bridges honestly without a sidecar (R4's cure).
- **Shared incumbent pruning**: an atomic best-jumps (+ judge score) owned by `run_variants`; coarse `relax` drops `jumps + ceil(remaining / (range x boost)) >= incumbent` with a separate integer jump counter (coarse g is a proxy).
- **Anytime restarts that resume**: keep each variant's open/best_g/parent across waves and raise the cap in place.
- **Shared-g multi-heuristic A*** (MHA*, Aine et al.) for singles with the same reserve and fuel model: one round-robin search with a shared best_g turns 6 singles into ~2 (P).
- **Precomputed highway adjacency** (hwgraph250.bin): CSR per neutron of (idx, d) within max boosted reach + 4 bridges, degree-bounded by sector, plus a long-bridge list for the widen; zero-build prototype = a per-plot shared map of full-tank scans.
- **Reach-band component ids**: `cgraph::build` already computes the union-find component per cell at R_REF = 600 and throws it away (C); persist it and per-band runs. Instant NoRoute and a bidi seed check; it will not rescue the reverse Jongou, which is connected.
- **Bound-gap stop rule**: incumbent <= LB x (1 + e), LB = max(euclid floor, field bound x ship ratio); replaces the arm ceiling + grace clock, which never arm on the rim (412 j vs a ceiling of ~247) so the plot burns 100 s. Needs a tight LB; measure LB/final on the matrix first.
- **Progress-rate stall**: field-jump or ly progress per 1,000 expansions over a window, replacing the flat caps.

## 3. The three changes first

1. **LazySP repair + leg precheck + the fallback started at t = 0.** Pre-register: both reverse Jongou plots return a route on three threads within 100 s; seconds in failed legs per plot fall from ~25 s x n to under 1 s; no matrix row worse on either thread count. Instrument: per-leg outcome, time, dead-edge hits.
2. **Thin the widen's relax set instead of skipping the widen** (bucket/sector thinning + fanout 64 over annulus candidates in both widens); fix the thin bidi's missing fanout as its own arm. Pre-register: widen time down 5x; Sol/Wongi -> Beagle 191 j; Colonia -> Spase unchanged; rim forward within +5 j.
3. **Shared incumbent pruning + resumed waves.** Pre-register: per-plot CPU on the three-thread arm down 30 %; no row worse; the x4 wave's time to the wave-1 frontier ~0.

# Appendix C. Why the two reverse Jongou plots find no route: the traces and route files (fresh-context analyst, 2026-10-17; a Python re-implementation of the fuel model reproduces the forward routes' fuel_after exactly; scripts in the session scratchpad `ana/`)

Reach figures: plain 71.9 ly full / 77.6 ly at reserve; neutron boost 432.8 / 467.1; premium injection (x2) 143.8 / 155. A maximum jump burns 6.7-6.8 t: a full tank lasts 18 jumps between scoops. The route files mark no injections, but hops beyond plain reach carry burns implying exactly x2: 5 on the AA-A forward route, 14 on XM-W (the external reference used 12 and 40 in that direction).

## 1. The forward route cannot be flown backwards

| | AA-A route (396 hops) | XM-W route (401 hops) |
|---|---|---|
| hops OK in reverse | 336 | 320 |
| OK with an injection | 5 | 12 |
| impossible in reverse | 55 | 69 |
| lowest fuel in reverse | 8.3 t | 35.1 t |

Every impossible hop is the reverse of a forward boosted hop. Neutron-to-neutron boosts (216 / 222) reverse fine if the tank is kept light (at reserve the boost reaches 467 ly; the forward flew up to 451). Boosts that LAND on a non-neutron (34 / 39) are one-way: the reverse would leave an F/K/M/A/G star with 77.6 ly plain (155 injected) and needs 394-454 ly. The forward routes' own 696 stars hold no reverse route either (a fuel-free, unlimited-injection search reaches 9 / 17 stars and dies within ~1 kly). Reversal gives no free answer; each one-way landing needs a fresh replacement from the full index.

## 2. The dead leg (Jongua XV-E d11-0 -> Gonguae GW-C d0, 1,209 ly) is a void needing injections, not fuel

Not fuel (120-128 t at the start; plain reach moves 71.9 -> 77.6 ly over the whole tank). XV-E sits near (11575, 15, -9025), inside the band the forward route crossed at hops 109-136. Within boosted reach of XV-E there is essentially one neutron (the isolated run scanned 2 neutrons in 2-3 expansions; the plot average is 73 per expansion). GW-C can only be left by boost: its neighbours are Eolls Eaec OK-C d14-0 at 70.8 ly (the only one inside plain reach, itself entered forward by a 133.6 ly injection), Eocs Aim PI-B d13-0 at 189.6, Jongua EF-R d4-0 at 419.1; the forward route left GW-C by a 419 ly boost and needed two more injections (108.8, 123.3 ly) to cross the 412 ly band at z -9611 to -10023. In reverse GW-C is the bottom of a one-way funnel; the coarse search chose it because it is a neutron and priced 1,209 ly as if 12 plain jumps bridge any gap. The exact planner burns the budget because an injection costs INJECTION_UNITS (~30 plain jumps): A* exhausts every plain state cheaper than path + 30k before trying the k-th injection. The external reference flies Byoi -> AA-A with 2 injections, most likely both on the island entry (AA-A's nearest corridor neighbour is 121.8 ly, beyond plain reach): the GW-C void is avoidable; choosing that waypoint is the error. Behind the first dead leg more may hide (GW-C -> Eolls Eaec NX-A 1,181 ly; Dryeae Aec SI-K -> Bloeh CY-F 1,850 ly; Bloeh CY-F -> Jongou 873 ly, which needs at least one injection) -- they showed Cancelled in 1-7 ms and never really ran.

## 3. Where the reverse coarse search dies

Without the field the thin single stops at [22850 2 -4818] in every run: 5,833 ly in a straight line from Jongou AA-A (the measure says 5,733), best_remaining stuck from expansion 5,000 to the cap with 21-26k states open, no injection ever used, refusals beaten 2.2 M / Pareto 344k / no fuel 95k / too far 0. The expanded nodes form a northern lobe at z ~ -4.5k for x = 17-23k, following the straight line; the forward chain at the same x runs at z ~ -10,400 (5.6 kly south), and at x = 13-20k at z -12k to -13.5k (7.5-9 kly south); the nearest forward star is 5,338 ly away. The weighted variants cap out 29.6 kly out near [-405 -344 5329]. A budget limit with a heuristic trap, not disconnection: the forward route proves a neutron chain exists. Fixes compared: longer priced bridges -- wrong direction (more optimistic edges); the goal field alone -- connects the coarse chain but through the GW-C funnel (measured); the smallest structural fix -- the field plus a repair loop (ban the failed edge or waypoint, re-run the coarse from the leg's start, keep the legs that plotted; today one dead leg cancels the variant); bidi seeding from the island -- must allow an injected last arrival (<= 144-155 ly) into an injection-only goal, the documented break in the thin bidi; the real cure -- an existence-first search.

## 4. Design: an existence-first search

State (star, jumps since last scoop 0..18, injections used <= K). The fuel counter is conservative: full-tank ranges everywhere (plain 71.9, boost from a neutron 432.8, injected 143.8); <= 18 jumps between scoopable stars (K/G/F/A/B/O/M or the companion flag) is always flyable, so any path replays clean in `minimize_refuels`; it gives up ~5.7 ly of light-tank reach. Ignored: fuel optimality, jump count, stops, the 1,800 s injection price. Kept: fuel feasibility, the injection budget (K from --inject), boost only from a neutron. Order: lexicographic (injections used, then greedy straight-line distance to goal) -- each injection count a separate bucket, so injections cost nothing until the plain/boost frontier is exhausted; a 200 M-bit visited bitset (25 MB) per bucket keeps it complete (the 5.7 kly lobe is drained once, never re-entered). Run from both ends: the backward seed from the island starts at every star within 144 ly of the goal entering with one injection; predecessor queries are neutrons within 433 ly plus any star within 72 ly (144 injected); stop when the closed sets meet. Seeding the optimisers: (a) the anytime incumbent the judge can always return; (b) an upper bound pruning the optimisers' A*; (c) corridor knowledge -- the witness's cells as the goal field; (d) legs with a witness -- cut refinement windows at witness neutrons every 2-5 kly so each leg has a proven fallback segment: a dead leg cannot exist, refinement can only improve on the witness or keep it. Estimated cost (unmeasured): ~400 jumps, greedy expansion at 10-100x route length = 40-400k expansions at 2-10 us each = 1-4 s single-threaded plus the same to drain a dead pocket; target under 5 s on three threads, measured on the 28 pins before believing it. A precomputed cell-connectivity graph (graph250) would bring the existence query to milliseconds at a build per range class.

# Appendix D. The maintainer's independent review (another model, 2026-10-17; the full text is `gemini_feedback_routing_.md` at the repo root): what it adds

It agrees with appendices A-C on focal search / EES with a calibrated progress estimate against pure weighted A*, on the coarse-to-exact cliff (never hand a gap over ~1,200 ly to the exact planner as one leg; subdivide along the cell-graph path or a fuel-blind plain-star stepper), on `highway_ok` decoding a full record per candidate, and on the nested rayon stall. It adds, ranked first in its own table:

- **Ship-calibrated query-time pricing of the goal field.** `graph250.bin` stores a static `cost10` per edge priced for an idealised 470 ly reference ship; the per-plot Dijkstra then sums fixed weights. Store `(dist_ly, min_pair_ly, flags)` instead and price each edge at query time with the caller's fuel model and boost (highway edge ~ ceil(d / actual boosted reach) with a scoop overhead; gap edge ~ d / actual plain range x a plain weight; hard-filter edges whose `min_pair_ly` exceeds the ship's boosted reach unless bridged). That turns the field from a loose reference bound into a tight, ship-accurate consistent potential -- the exact fault that cost the rim 13 jumps with the field on (447 -> 460) and the premise under which the field could be on by default. **Decision: R4's first task, ahead of the highway adjacency file.**
- **A 1D dominance array once the coarse state is (star, dry_hops):** a star popped at dry 0 is never improved by a later arrival; the per-node Pareto `Vec` collapses to one byte. Goes with R2 Task 4.
- **Bit-packed 64-bit edges for `hwgraph250.bin`** (target 22 bits, is-white-dwarf, has-local-scoop, distance in tenths, cell id) so every filter runs in registers from one sequential stream. Goes with R4's adjacency file.
- **A per-leg expansion budget inside the race and a `powf` lookup table in `FuelModel::fuel_for`** (eight exponents exist, the ratio is in [0, 1]); and porting the exact planner's half-sphere scan and quickselect to `carrier.rs`. Micro items for the record; the leg budget is already in (time-capped) as of R2.
