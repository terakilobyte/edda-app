# Roadmap

What is next, in the order it currently matters, and what was tried and
buried. This page is the project's record from the public release on:
decisions carry their date, buried ideas carry the numbers that buried
them, and the reasoning behind a change is in its pull request. Bench
verdicts live in the CSV headers under `docs/benches/`.

## Server

- **Planner threads on the box** (2026-09-09). The route planner runs on
  cores − 2 threads at low priority, a rule sized when the API process
  also ran the EDDN feed. Now that the feed is its own unit, sweep 2/3/4
  threads (`EDDA_API_PLANNER_THREADS`, to be added) on a Beagle Point plot
  before changing the default. Baseline: 39 s on the box vs 20 s on a
  desktop for the same 193-jump route.
- **The feed-vs-dump delta** (2026-09-09). The listener now parses scans,
  plotted routes and ring/body signals. Watch the nightly hydrate's
  `stars_taught`, `bodies_applied`, `hotspots_applied`, `systems_applied`
  week over week; pre-registered: stars and hotspots from the dump fall
  by an order of magnitude, systems less. When the curve flattens, that
  is the true residual the daily dump provides.
- **Plots cached before the highway sub-index lands** (2026-09-11,
  measured). After the first `apply routing` (version 3cff2b54, 79 s),
  a Sol→Colonia plot at range 50 answered 453 jumps / 0 boosted in
  1,051 ms and stayed byte-identical for 40 min while the same request
  at 49.9 or 50.1 answered 146 / 127 in 428 ms: the plot ran while the
  highway sub-index for the new version was still building, fell back to
  bare range, and was cached under the new version (in-memory, keyed on
  version + request, TTL 3,600 s, cap 256, `crates/ed-api/src/plot.rs`).
  Fix: do not cache (or do not serve) a plot while the sub-index for the
  current version is pending; a version change should also drop the
  cache. Small.
- **`boost.bin` is adopted but not published** (2026-09-11). The side
  file lands in `routing/<version>/` but the manifest's file list still
  names only the four EDGX files and `chunks.json`, so clients on local
  data never fetch it. Harmless while the product passes
  `secondary_boost_ls: 0`; needed the day the planner charges secondary
  boosts.
- **`apply api` must ship with its own script** (2026-09-11, measured).
  The 16:25 Deploy API ran the box's *old* `edda-apply`, which restarted
  only the API; `edda-eddn` kept the Sep 9 binary until a manual restart
  ~4 h later, and 180k more navroute star rows landed in between. The
  deploy path does not carry `deploy/edda-apply`; the installer rerun is a
  separate manual step. Either the workflow refuses when the box's script
  hash differs from the ref's, or the box script is shipped and
  reinstalled by the deploy itself.
- **Ingest unit restart gap** (2026-09-09). First time `edda-eddn.service`
  restarts alone, measure the gap in `edda_eddn_last_apply_unix_seconds`;
  pre-registered under 5 s.
- **Mining tables, first-fill numbers** (2026-09-09). Row counts and bytes
  of `bodies`, `body_materials`, `rings`, `ring_hotspots` after the
  weekly and after one daily; hydrate wall-time delta; P50 of the three
  mining lists at 100 and 500 ly.
- **Not parsed from EDDN yet, no table wants them**: fsssignaldiscovered,
  fssdiscoveryscan, scanbarycentre, fssallbodiesfound, codexentry,
  approachsettlement, docking events, scanorganic, navbeaconscan,
  fcmaterials.
- ~~**Deploy user** (2026-09-09). CI deploys as root over SSH; move to a
  dedicated deploy user once the repository is public and CI minutes are
  free.~~ Done 2026-09-09: CI reaches the box as user `deploy` with a
  fresh key (born in 1Password, public half in `deploy/deploy-key.pub`)
  that can only rsync into an inbox and run four `apply` verbs
  (`deploy/edda-deploy`, `deploy/edda-apply`); the host key is pinned in
  CI. Watch the per-verb `edda-apply <verb>: done in N s` lines in the
  release log; pre-registered: `apply api` under 30 s, the other three
  under 5 s.
- **A route that cannot exist should be refused fast** (2026-09-10). On a
  145k-system fixture with no path, the planner spent 161 s (weight 1.3)
  and over 240 s (exact) before saying no; galos's router answered in
  about a second in all three of its modes. A commander whose range is
  too small for a gap sits through that. Measure the reachable-component
  size first, then pick: a bounded frontier, a connectivity pre-check on
  the cell graph, or a wall-clock cap that returns NoRoute.
  Numbers in `docs/benches/2026-09-09-galos-index-spike.csv`.
- **The bare exact mode does not finish at full scale** (2026-09-10).
  `thorough` (weight 1.0, admissible boost heuristic) over 199.6 M
  systems, Sol to Colonia at 50 ly, was killed after 2 h 08 min on a
  shared PC; galos's proven-shortest search took 261 s for the same
  question with the galaxy resident. The product's long plot answers in
  1.1 s with 141 jumps against the proven 136, so this is about the exact
  mode only. Rerun on a quiet machine before deciding anything; then
  either a better admissible bound or an honest cap.
- **Full routing rebuild on demand** (2026-09-09). The monthly rebuild is
  retired; build the instrument that diffs the applied index against a
  fresh dump (systems missing, records mismatched, bucket skew) and
  rebuild only when it says so.

## Trade finder

- **Pairing cost** (2026-09-09). Pairing is quadratic in stations that
  hold a board; 1,000 stations pair in ~5 s. Measure its scaling at
  250/500/1,000 before touching the per-symbol seller × buyer loop.
- **Diversity rule** (2026-09-09). Three legs per (source, commodity) let
  one anomalous board hold nine of twenty slots. Options measured on
  real answers: two per source-commodity, or a cap per source station.
- **Per-ship measured timing** (2026-09-09). The client samples leg
  phases only while following a trade route, per hull, and sends a
  phase's constant once it has 20 samples. The defaults (fitted
  supercruise, undock by pad, 35 s market) stand until then; re-measure
  against the next loops.

## App

- **Browser route planner** (2026-09-09). Live at `/route/`; watch plots
  per hour from the page vs the app on the shared route budget, and the
  physics endpoint's P50 (pre-registered under 5 ms).
- **Galos's map over our data** (2026-09-10). Their bevy map runs
  unchanged over a directory written from our star index
  (`experiments/galos-index`, spike branch, `build --for-map`): 8.3 M
  systems, 2 GB resident, uncoloured. To make it a real offer: a
  streaming builder for their format (theirs holds the galaxy in memory,
  ~185 B/system, and cannot raise 199.6 M on a workstation), a dense
  sidecar of magnitude and temperature from the dump (~3 B/system), and
  a sparse populated/political export from Postgres. Only if a drawn
  full galaxy is wanted; it buys nothing for routing.
- **Windows and Linux flights before each release**: the maintainer flies
  every release candidate; Linux is alpha until a second tester has.

## Data and licensing

- **cargo-deny in CI** (2026-09-09): the job is in `ci.yml`; the first
  run on a pull request is the online check of the Windows and Linux
  dependency graphs.
- **Community data with no licence file** (FDevIDs, two engineering
  guides): reproduced with attribution; replaced on request.

## Buried

Ideas measured and set aside stay here with their numbers; they come
back only when a premise changes, and then they are re-measured, not
re-argued.

- **Galos's octree as the routing index** (2026-09-10). Built from our
  records and walked by our planner: identical plans and expansion
  counts, and 1.7–2.6× the time of our 50 ly grid per plot at bubble
  scale. Their levels serve drawing, not the neighbour question.
- **Galos's router for server-side plotting** (2026-09-10). Sol to
  Colonia at 50 ly over all 199.6 M systems: 136–137 jumps in 130–261 s
  with a 7.98 GB resident graph, against the product's 141 jumps in
  1.14 s from a memory-mapped index on the 16 GB box. Five jumps for two
  orders of magnitude and a galaxy in RAM; and no fuel, scoop or
  injection model. Their published 0.08 s described their 2.4 M-system
  database, not the galaxy.
- **Secondary boost stars in the planner** (2026-09-11). Measured
  twice: the first run's switch never reached the pins' path and was
  withdrawn; the second, with every departure priced by one helper,
  gives the same answer on the pins (no route takes one) and shows the
  feature working where it can: departing a black-hole system with a
  neutron 3 ls out, 7 jumps become 6. Under the flat public model that
  saves 60 s and costs a 150 s supercruise run, a net loss, and the judge
  does not yet charge the run. Buried for the product on those numbers
  (`docs/benches/2026-09-11-secondary-boost-pins.csv`); the flag and
  `boost.bin` stay in the index; the request switch stays off. Back
  only with a judge that charges the run and a route class that gains.
- **Doubling the supercruise estimate** (2026-09-09). Proposed from a
  comparison against the 45 s base alone; the full curve already priced
  the measured loop long (174/234 s vs 98/139 s flown). Replaced by the
  fitted shape, 150 s + 0.5 s per 1,000 ls.
- **The routing index not resident as the cause of slow Beagle plots**
  (2026-09-09). Refuted: 13 major faults over a two-minute plot; the
  cost is the long-range search's own allocation and the box's two
  low-priority planner threads.
- **A 10,000-station cap after the freshness gate** (2026-09-09). Took
  the box to 474 MB free on a Sol / 100 ly / 48 h search (2,653 fresh
  stations, ~7M materialised legs). Now 1,000 with legs bounded at 100k.
- **Weekly and monthly syncs** (2026-09-09). The dumps' station boards
  were 99.9 % already known from EDDN; the weekly was the daily's
  superset. Retired; break-glass is a documented manual line.
