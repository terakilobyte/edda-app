# Planner unification: one currency, one search, measured

Design, 2026-10-17. Status: DRAFT for the maintainer's review. Nothing
below is built unless marked done; every rung names the measurement that
confirms or buries it before the next rung starts.

## The objective (maintainer, 2026-10-17)

EDDA is the fastest router for any route in the galaxy, period:

1. it produces the fastest route for the pilot;
2. it produces it in an absurdly small amount of compute time.

Constraints: no special-casing of particular routes; indexes may be
rebuilt or added; the production box is 8 vCPU / 15 GB (read as root on
2026-10-17; the "2 vCPU" in earlier bench headers was a sizing target);
the desktop client is journal-only and plots through `POST /v1/route`.

## What today's measurements say

Sources: `docs/benches/2026-10-17-rim-to-rim-stress.csv` (the Ood Fleau
ZJ-I d9-0 <-> Sphiesi HX-L d7-0 pair, 84,599 ly, and the Jongou XM-W d1-0
-> Byoi Fraae CQ-G d10-0 pair, 87,012 ly), the coarse-weight sweep on the
nine pinned routes, and five read-only audits of the code (search
internals, precomputation, competitors and literature, objective and fuel,
server economics). Numbers are the PC's (24 threads) unless marked box.

1. **The coarse heuristic under-prices progress, so weighted A* floods
   dense regions instead of crossing them.** `h = remaining / (range x
   boost) x w` prices one full boosted hop per 433 ly for the Caspian; the
   real chain through the core costs about 2.5 jumps per 433 ly (bridges
   1.25, scoop edges 2.0). Weighted A* dives only where `w x h-rate` meets
   the real rate: at the shipped 1.5 every variant floods the core and caps
   out at 30,000 expansions 43-45k ly short; at 3.0 the same variants
   cross, then flood at the rim where plain jumps make the real rate higher
   still. No scalar weight serves both regions. The density-aware bound
   that exists (goal field, ALT) was gated off on this route because the
   straight line is uniformly dense (`alt_line_is_nonuniform`), and the
   cell graph prices highway edges at the best case anyway.
2. **The neutron coarse graph cannot represent the corridor the external reference flies.**
   `MAX_BRIDGE_JUMPS = 4` mid-route; the external reference's route has 16 plain runs
   longer than 4 jumps (max 24). Along sparse corridors the coarse graph has
   no edge, so the only connected coarse path is through the core.
3. **A stalled coarse search hands the remainder to the exact planner as
   one leg, and the exact planner is not a long-range planner.** 45,000 ly:
   never returns. 5,303 ly across the west rim: 1.13M expansions, 68 s,
   computed six times by six variants. The "73 s neutron-ended crossing" was
   the exact planner flying 44,000 ly in 570-613 plain jumps. Standalone,
   the 1,138 ly final approach is 0.8 s forward and 0.1 s reversed.
4. **Three judges, and production judges by jump count alone.** Bench and
   tests: 60 s/jump + 120 s/stop, no tonnage. App local planner: the same
   at `stop_weight` 1.0. Server: `plot.rs` defaults `stop_weight` to 0.0
   and the app sends none, so every served route is ranked by jumps; the
   lean-stop judging never reached a served route. The journal (130 files,
   re-cut today) says a plain jump is 53 s, a boosted jump 73 s, a passing
   scoop +6 s; the neutron-cone approach (~+20 s per boosted hop) is priced
   nowhere; a scoop stop is free inside the exact search.
5. **The portfolio is 17 parallel variants and the box runs two at a
   time.** Long lane: 4 planner threads over 2 slots = 2 threads per plot,
   on 8 cores. Rayon's order, not speed, decides which two run first; the
   fastest variant on the Jongou pair finishes in 1.2 s on the PC, and the
   box found no first route in 100 s (2026-10-12). Nested `par_iter`
   (legs inside variants) lets a blocked collect steal an unrelated 68 s
   leg. The early answer picks by (jumps, boosts) while the planner judges
   seconds.
6. **The per-expansion millisecond is relax-bound, and the fanout cap is
   off.** Per candidate: 3-4 `fuel_for` powf, three hash maps including a
   Vec-allocating Pareto list, a heap push into 150-800k entries;
   `COARSE_FANOUT = usize::MAX`; `highway_ok` decodes the full record;
   bucket thinning keeps 2 per 36-54 ly bucket, 500-1,200 buckets in the
   core. Measured degree within 470 ly: mean 1,409, p99 17,136, core band
   mean 2,117 vs rim 55. `ED_COARSE_TIMERS` exists and has never been run.
7. **Fuel in the state costs more than it buys.** Coarse: (neutron x 32
   fuel levels) + Pareto, two priced edges per candidate; 30k expansions ->
   156-216k states. Settle rounds re-plan 1-6 of ~150 legs, never past
   round 1. `FLAG_SCOOP_NEARBY` marks 9.4% of highway stars, while 92% have
   a scoopable within 80 ly (p50 30 ly); the exact planner treats the 62%
   of index records with Unknown class as unscoopable.
8. **Injections are priced as a last resort but searched everywhere.**
   The exact planner runs a second, 8x-volume scan on every expansion while
   injections remain. Bidi dies at injection-locked ends because its
   sentinel scan is 360 ly and the widen skipped sentinels (fixed in this
   branch); bidi also ignored the weight override (fixed).
9. **The external reference's method is public** (its author's forum posts, 2022): a
   weighted heuristic run several times, lowering the weight each pass,
   stopping at the time limit or when "x nodes" pass without getting closer;
   a heuristic that penalises neutron gaps; ~40 B/system in an R-tree
   memory map; 200-500 closures per second. His 61 s answer is a time-limit
   incumbent, not a converged search. EDDA at weight 3 beats it: 415 jumps /
   104,765 ly / 18 stops vs 453 / 114,668 / 19, and wins under every
   pilot-seconds judge (27,060 s vs 29,400 s flat; 29,899 vs 32,998 with the
   cone cut). Our problem is direction and budget, not route quality.

### The weight sweep (PC, budget 100 s, same ship, premium injections 22)

| route | 1.5 | 2.0 | 2.5 | 3.0 |
|---|---|---|---|---|
| Sol -> Colonia | 58 j / 1.0 s | 58 / 0.7 | 58 / 0.6 | 58 / 0.6 |
| Sol -> Beagle Point | 191 / 17.9 | 193 / 10.0 | 202 / 0.7 | 198 / 0.4 |
| Wongi -> Beagle Point | 191 / 18.2 | 193 / 9.8 | 202 / 0.7 | 198 / 0.5 |
| Sol -> Sagittarius A* | 70 / 0.9 | 70 / 0.5 | 71 / 0.5 | 70 / 0.6 |
| Colonia -> Spase | 91 / 54.0 | 91 / 47.2 | 91 / 43.2 | 91 / 22.6 |
| Wongi -> Spase | 101 / 0.5 | 101 / 0.5 | 101 / 0.5 | 104 / 0.5 |
| Jongou XM-W -> Byoi CQ-G | 401 / 101 (budget) | 399 / 13.1 | none | 434 / 48.4 |
| Jongou AA-A -> Byoi AT-U | none | none | none | 406 / 42.1 |
| Ood Fleau -> Sphiesi | none | none | none | none (415 at 110 s with budget 300) |

Reading: weight is a speed knob with a 1-5% jump tax on the 20-65k ly
pins and no monotone behaviour on crossings (2.5 loses Jongou XM-W that 2.0
finds in 13 s). It is not the fix; it is evidence for finding 1.

## The design

One currency, one anytime search over a graph that can express the real
corridors, with the work precomputed where it is static, and a server that
runs the cheapest search first.

### D1. One currency: pilot seconds, everywhere

Judge, coarse edges and exact edges all price in seconds: plain hop 53,
boosted hop 73 (cone included), passing scoop +6, full stop = overhead +
tonnes / scoop rate (+ companion run where the scoop is a companion),
injection fixed at the boss's 1,800 s, white dwarf and secondary boost as
boosted hops. One judge in bench, app and server (`stop_weight` default 1.0
on the wire; the server's early-answer picker calls the judge). Per-
commander fits from the journal remain an overlay on the public defaults.

### D2. One anytime search replaces the fixed-weight portfolio and the waves

A restarting weighted A* on the coarse graph: the first pass near-greedy
(first route in seconds on any connected pair), the incumbent becomes the
upper bound, later passes lower the weight and prune by the incumbent until
the budget or the grace clock. The heuristic is the goal field priced at
realistic traversal cost (per-cell neutron spacing, bridges at plain
jumps), built for every long plot; the stall rule is progress per
expansion, not a flat 30,000 cap; a stall restarts the coarse search from
the stall point and never hands more than `LONG_ROUTE_LY` to the exact
planner. Two or three complementary families stay (lean/eager scan, bidi),
run in order of historical time-to-first, not seventeen at once.

### D3. A coarse graph that can express the corridors

A degree-bounded, reach-banded sector adjacency per highway star as a
side file (`hwgraph250.bin`, ~0.4-0.9 GB at 14 sectors x 4 boosted-reach
bands, built in minutes), so an expansion is a short list read, not a
Morton walk over 1,400 candidates; bridges longer than 4 jumps priced in
seconds instead of forbidden, which is what connects the external reference's corridor;
nearest-scoopable distance per highway star (`scoop250.bin`, 3.9 MB) so the
coarse search can be fuel-blind with a dry-run cap and the leg planner
places the stops (it already finds 12 of 18); highway component ids per
reach band so an unreachable pair or a bidi seed failure is answered at
once. The exact leg planner keeps fuel at 32 levels, treats Unknown-class
records by the scoop byte, scans the injected sphere only when the plain
pass pushed nothing closer, plans approach legs from the sparser end, and
memoises identical legs within a plot.

### D4. The server runs the cheapest thing first

Long lane threads sized for 8 cores; variants launched in time-to-first
order and staged (the rest only on failure or idle workers); leg memo per
plot; refinement behind the early answer capped by improvement, not by the
budget; per-plot CPU seconds and per-variant outcomes as metrics; progress
before the first answer on the wire.

## The rungs, each gated by its measurement

Pre-registered gates for every rung: the 18-route crossing matrix and the
nine pinned routes within +1 jump and +1 stop of today under ONE judge;
`plot_fuzz` same-seed with 0 unflyable routes; `fuel_replay` on every
final; the two rim pairs plotted on the box inside the budget; wall time
reported per phase. A rung that misses its gate is recorded in the CSV and
the ledger and does not ship.

- **R0 (instrument, before any change).** Run `ED_COARSE_TIMERS=1` on the
  stress pair and the matrix (relax vs scan share). Emit per-variant
  outcome, per-phase seconds, first-found time and per-plot CPU on the
  server. Fix the server judge (`stop_weight` 1.0 default; early-answer
  picker = judge). Land this branch's bidi fixes and debug traces.
  Gate: the numbers exist; the matrix re-run under one judge is the new
  baseline CSV.
- **R1 (days): remove the waste.** Leg memo per plot; lazy injected scan;
  `COARSE_FANOUT` and sector k-best at plot time (the zero-build prototype
  of D3); variant ordering and long-lane threads for 8 cores; approach legs
  from the sparser end. Expected: hand-off routes lose 2 x 68 s; Jongou
  first route on the box in single-digit seconds; stress pair under the
  budget on the box at weight 3 (the knob, not the default).
- **R2 (week): one currency, fuel-blind coarse.** `scoop250.bin`; seconds
  in coarse and exact edges and in the judge; coarse state = neutron (dry-
  run cap), scoop stops by the leg planner plus a generalised crossing
  replan. Expected: coarse states 5-7x fewer, relax halved, the 68 s leg ~8x
  cheaper; pins hold within the gate.
- **R3 (week): the anytime search.** Goal field always on with realistic
  pricing; restarting weighted A* with incumbent pruning; progress-rate
  stall; restart-from-stall instead of exact hand-off; 2-3 families staged.
  Expected: first route in seconds on every connected pair, final within
  the budget on the box, weight no longer a user-visible trade.
- **R4 (week+): the highway graph.** `hwgraph250.bin`, priced long bridges,
  component ids. Expected: expansions 10-30x cheaper, crossings in seconds,
  galaxy-scale plots on the box well under the interactive budget. CRP /
  customisable contraction on the cell graph only if coarse expansions still
  dominate after R4.

Targets at the end of R4, on the box: both rim pairs in under 10 s with no
more jumps than the external reference; every pinned route first-found under 2 s and final
within +1 jump / +1 stop of today's best; per-plot CPU under 30 s for any
pair.

## Measured today and already decided

- Fail-fast on a far stall (20 reaches): negative as a default (cost
  Jongou AA-A its route; the waves it freed did not cross). Knob only.
- Gateway inserted on a stalled chain: negative (the long hand-off arrives
  dry, the approach leg fails from 0 t). Reverted.
- Bidi sentinel widen and bidi reading the weight override: kept (correct
  behaviour; the A/B re-run with bidi at the same weight is part of R0).
- Coarse weight 3 as the default: not taken (1-5% jump tax on pins; non-
  monotone on crossings). Remains the harness knob until D2 replaces it.

## Buried ideas respected, premise changes named

Highway sidecars as a crossing speed-up (2026-10-07, null 14.5 -> 14.2 s)
were benched as a heuristic under a 22-variant portfolio with best-case
edge pricing and the nonuniformity gate; D2 uses the field as the always-on,
realistically priced heuristic and corridor -- a different premise, to be
re-benched, not re-litigated. Galos octree and router stay buried. The
cell-graph-first seed stays a fallback. Full star-to-star adjacency is
buried by today's census before being built (5.4 G edges). Secondary
boosts stay buried unless D1's run pricing lands, which is the premise its
entry names.

## Open questions

1. The relax/scan split in the core (R0's first number) decides whether
   `hwgraph250.bin` or SoA position arrays come first in R4.
2. Why the forward coarse search crosses from a neutron start and not from
   an injection-opened start -- expected to dissolve under D2/D3; if it
   does not, instrument the stall position per variant.
3. The reach-band set for the highway graph: needs the boosted-reach
   distribution of real requests (timings are in telemetry; ships are not;
   take it from the matrix ships until a count exists).
4. Whether a maintainer-flown route confirms 53/73 s per hop.
5. The acceptable crossing latency ruling (ledger, OPEN since 2026-10-07):
   the targets above are a proposal for it.
