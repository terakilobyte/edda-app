# Roadmap

What is next, in the order it currently matters. Decisions and their
measurements are recorded as they happen in the engineering ledger,
which is kept privately by the maintainer; this page is the public view
of what is open. Dates are when an item was opened.

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
- **Deploy user** (2026-09-09). CI deploys as root over SSH; move to a
  dedicated deploy user once the repository is public and CI minutes are
  free.
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
- **Windows and Linux flights before each release**: the maintainer flies
  every release candidate; Linux is alpha until a second tester has.

## Data and licensing

- **cargo-deny in CI** (2026-09-09): the job is in `ci.yml`; the first
  run on a pull request is the online check of the Windows and Linux
  dependency graphs.
- **Community data with no licence file** (FDevIDs, two engineering
  guides): reproduced with attribution; replaced on request.
