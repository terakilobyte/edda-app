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
  The build is lazy and quick — the serve process re-reads the manifest
  per plot request and builds the highway on the first plot after a
  version change (`galaxy_service.rs`; 6 s for 164fcbd0, journal
  21:20:29Z) — so the window is the first few seconds of plotting after
  every publish, nightly included; it happened again at 21:20Z on
  164fcbd0 (453 / 0 on four fresh keys). Journal confirms the trigger:
  each version was mapped and its highway built within 10 s of the
  first plot after the flip, and both first plots were ours — no
  commander plotted in either window. Fixed 2026-09-11: a plot made
  while the highway for the current version is pending is served but
  never cached. Still open: a warm-up plot from adopt/reconcile so the
  build runs before a commander's request.
- **`boost.bin` is carried but not published** (2026-09-11). The side
  file lands in `routing/<version>/` but the manifest's file list still
  names only the four EDGX files and `chunks.json`, so clients on local
  data never fetch it. Harmless while the product passes
  `secondary_boost_ls: 0`; needed the day the planner charges secondary
  boosts. Half of it is closed: the reconcile's carry (PR #40) is
  confirmed in production on the first nightly after it shipped —
  `routing/a12fce97/boost.bin` and `routing/164fcbd0/boost.bin` are one
  inode (4932154, link count 2), so the file rides the chain by hard
  link rather than being copied or dropped. What remains is listing it
  in the manifest.
- **`apply api` must ship with its own script** (2026-09-11, measured).
  The 16:25 Deploy API ran the box's *old* `edda-apply`, which restarted
  only the API; `edda-eddn` kept the Sep 9 binary for another four
  hours, and ~60k more navroute star rows landed in between (hourly
  counts 8.7k, 19.7k, 21.5k, 8.6k, 3.4k up to 20:20 UTC). The
  deploy path does not carry `deploy/edda-apply`; the installer rerun is a
  separate manual step. Either the workflow refuses when the box's script
  hash differs from the ref's, or the box script is shipped and
  reinstalled by the deploy itself.
- **`thorough` defaults to true on the public route endpoint**
  (2026-09-11). `POST /v1/route` with the field absent plans the
  expensive path (`plot.rs`, `api.thorough.unwrap_or(true)`), while the
  app's Route tab sends `thorough: false` for its quick plot and `true`
  only for "Try harder". Defensible for a one-shot web caller, but
  undocumented, and it cost two sessions an evening of comparing rows
  that were not the same request. Document it on the endpoint; decide
  whether the default should follow the app.
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
  that can only rsync into an inbox and run five `apply` verbs
  (`deploy/edda-deploy`, `deploy/edda-apply`; `routing` joined them
  2026-09-11); the host key is pinned in CI. Watch the per-verb
  `edda-apply <verb>: done in N s` lines in the release log;
  pre-registered: `apply api` under 30 s, the others under 5 s. Measured
  on the two v0.3.1 deploys (2026-09-09, runs 34415889827 and
  34418299899), identical both times: `api` 14 s (readyz after 4 s),
  `app` 0 s, `site` 0 s, `dashboards` 0 s — all inside the
  pre-registration. `routing` is the exception by design and was not
  pre-registered: 79 s on 2026-09-11 for an 11 GB adopt (validate,
  rename into the artifact tree, chunk, rewrite the manifest). Closed.
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
- **Ship and module discounts** (2026-09-12, landed). No prices exist to
  compare -- the feed publishes bare stock lists -- so the Market tab
  applies the game's published rules instead, from
  `crates/ed-domain/src/discount.rs`: five Powerplay rules keyed on the
  controlling Power and its state, nine fixed stations, and the 2.5%
  Elite rank discount that stacks. Open: the wiki says "controlled by"
  for some Powers and "exploited by" for others, read here as
  Stronghold+Fortified against Exploited -- unverified against the game.
  The instrument for that is free and unbuilt: the journal writes the
  docked station's whole price list (630 modules with buy prices at
  Schmitt Enterprise, measured 2026-09-12), so EDDA can check its own
  table against a real list on every dock and say when they disagree.
  Sparsity measured in `docs/benches/2026-09-12-discount-coverage.csv`.

## Data and licensing

- **cargo-deny in CI** (2026-09-09): the job is in `ci.yml`; the first
  run on a pull request is the online check of the Windows and Linux
  dependency graphs.
- **Community data with no licence file** (FDevIDs, two engineering
  guides): reproduced with attribution; replaced on request.
- **Discount rules from the Elite Dangerous wiki** (2026-09-12): the
  Active Discounts table, read as facts -- percentages, Powers, station
  names -- and cited in `crates/ed-domain/src/discount.rs`. No prose
  reproduced, as with the Fandom text stripped from `synthesis.json` in
  the open-sourcing pass.

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
