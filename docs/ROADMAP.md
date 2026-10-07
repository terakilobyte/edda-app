# Roadmap

What is next, in the order it currently matters, and what was tried and
buried. This page is the project's record from the public release on:
decisions carry their date, buried ideas carry the numbers that buried
them, and the reasoning behind a change is in its pull request. Bench
verdicts live in the CSV headers under `docs/benches/`.

## Server

- **The weekly syncs were retired in the repo on 2026-09-09 but ran on
  the box until 2026-10-07** (measured). The box kept the pre-0.3.0
  `edda-weekly.sh`, so every Sunday still pulled Spansh's galaxy_7days
  (39 min, 1.45M body rows, 0 systems — all already taught) and EDSM's
  bodies7days (11 s). What the EDSM weekly uniquely taught, from
  `stars.source` over the last 8 days: feed 230,623 star rows, daily
  dump 28,361, EDSM weekly 5,455 — about 2 % of the feed's, in systems
  nobody on the feed has scanned. The repo's script (no syncs, plus the
  market index reindex) was installed on 2026-10-07, so 2026-10-11 is
  the first Sunday the ruling actually holds. If those ~5k star classes
  a week are wanted back, it is one line in the weekly script; the
  maintainer decides.
- **One long plot starves every other plot** (2026-10-07, measured on
  the box). Not a regression: the planner is unchanged since #140 and
  crossings got faster since September on the PC (14.5 s vs 43-54 s);
  a Sol → Beagle Point crossing costs 31 s on the box because the
  portfolio runs 22 variants on a slower core, and the sidecars move
  nothing (Waldorf's CSV). The finding: all plots share ONE rayon pool
  of 6 threads; the lanes gate admission, not work, so an interactive
  plot's fan-out queues behind a crossing's 22 variants — Sol → Alioth,
  70 ms alone, took 70 s and 105 s beside a Beagle plot with two cores
  idle. The crossing itself came back 504 at the 120 s lane budget and
  kept planning after the client was gone (`Control::none()`). Shipped:
  the wire says `cached`/`lane`/`budget_ms`, the log carries planner
  ms/jumps/expansions/variants and sidecar availability per plot, the
  sub-index says what it carries when opened, and an hourly canary
  (`scripts/route_canary.sh`) plots three pinned routes against bounds
  and fails loudly — its first dry run caught the 70 s.
  `docs/benches/2026-10-07-route-latency-prod.csv`. Next, measured
  before deployed: a rayon pool per lane and a cancel flag dropped with
  the request; then the product ruling on an interactive crossing lane
  (first good route early, refine behind "Try harder").
- **The database's growth was one index** (2026-10-02, measured on
  the box and the mirror). `market_commodity_fresh_idx` was 38 GB
  against 9.7 GB freshly built from the same data: the board writer
  replaces a board by DELETE + INSERT and the observed_at-keyed B-tree
  never reuses freed pages. A concurrent reindex gave back 29 GB
  (database 85 → 56 GB) in 3 min without blocking ingest; the weekly
  timer now does it every Sunday with one retry, after the first
  attempt deadlocked with a deploy's post-restart ANALYZE
  (`docs/benches/2026-10-02-market-index-bloat.csv`). Open: the writer
  could update boards in place and stop the bloat at the source — build
  it only after measuring what share of a refreshed board's rows
  actually change; `market_pkey` and `outfitting_pkey` carry ~1 GB of
  the same bloat each, left for that measurement. Also open: the app
  artifact directory keeps every installer since 0.2.0 (~2 GB); prune
  to the last two.
- **Clients dashboard: four panels removed, three made honest at zero**
  (2026-09-20, live audit through the tunnel, 95 panels). "Coverage
  gaps /h by path/kind" read `edda_client_events_total{callsite=~
  "edda::coverage_gap::.*"}` and "Local-data share" / "Always check API"
  read feature flags `data_source_remote` / `api_fallback_always` — the
  client in this repo emits none of those (feature_flags sends
  auto_update, the voice engine and capi_linked), so the panels were
  pre-registered for telemetry that does not exist. Removed; re-add
  when the client sends them (the flags are one push each in
  `telemetry::feature_flags` once the settings exist here). "Failed
  operations", "Trade search failure rate" and "Trade failures" filter
  on `ok="false"` and had no series while nothing failed; they now fall
  back to 0. Host Health's 18 panels and the Server Postgres row were
  dead for want of exporters (#120), live since the 2026-09-20 setup
  re-run (node 1,781 series, postgres 1,124). Still quiet, not broken:
  station board requests, trade-search saturation and cache, EDSM
  proxy latency, knowledge errors (no events in 7 days), and
  `edda_artifact_bytes_total` (no artifact download served in 7 days —
  worth a look: clients on local data should be pulling nightly).
- **Nearest-service search: rare services fixed by an index, common
  ones still scan** (2026-09-20, measured on the box). A 300 ly
  material-trader search around Anana took 1.8–2.0 s server-side (mean
  2.0 s over 40 near requests): the sphere holds 548,501 stations and,
  with a service filter, the plan probed station_services once per
  station because the table is keyed (station_id, service). Index
  (service, station_id) (migration 0020; 491 MB, 14.7 s, built
  concurrently on the box first): the same query does 23 ms of work and
  the request from a Mac is 0.77 s, of which ~0.6 s is TLS and distance
  (a 1 ms systems-mode request costs 0.59 s from the same place).
  Still open: common services and the unfiltered search at 300 ly
  (2.5–3.1 s, 1.5 s) walk 105,784 systems and half a million stations to
  keep 100 — the fix is answering from the smallest radius ring that
  fills the limit (the 25 nearest at 50 ly are all within 8 ly of
  Anana), measured before built. Also seen: one first-after-restart
  request stalled for the client's full 15 s and the retry answered in
  2.6 s — unexplained; the plan's JIT is ~0.3 s, not 15.
  `docs/benches/2026-09-20-stations-near-service.csv`.
- **Kill counts are gone from mission tracking** (2026-09-19, ruled:
  "if there's no reliable way to read exact mission data at any given
  time, I think we have to abandon kill count tracking"). The journal
  has no per-kill mission counter, and every way of inferring one was
  measured wrong: (a) a target that dies before the ship's scan
  completes writes NO `Bounty`/`FactionKillBond` yet counts for the
  mission — live on the maintainer's ship the same afternoon, 3 kills in
  game against 1 in the journal, then 13 against 9 (the journal after
  that one Bounty shows two Imperial Eagles targeted at scan stage 0 and
  the lock dropping, nothing else); (b) same-giver missions credit one
  after another, different givers together (the 2026-09-16 ruling,
  CONFIRMED live: two Ahayan Defence Party massacres, one kill, the game
  credited one, EDDA showed one on each), which the earlier "13/13
  concurrent" bench had got backwards because a counter capped at
  KillCount cannot see over-credit — that verdict is RETRACTED in the
  CSV header; (c) 46 of 65 redirected massacres were 2–23 kills short
  at the redirect. Frontier's API was probed the same day for a mission
  or kill tally: none (`/profile`, `/journal`; the journal endpoint is
  the same file). What remains is what the game states outright:
  `KillCount` as the target, `MissionRedirected` as completion (status
  → ready to turn in), `MissionCompleted` at hand-in, the wing flag,
  expiry, reward and hand-in. Removed: `kills_done`, `kills_remaining`,
  the Bounty/FactionKillBond replay with its system gate and per-giver
  rule, assassination-by-pilot-name completion, the per-kill "Mission
  progress" callout and the HUD's fewest-kills sort key (HUD order is
  now status, expiry, acceptance). Statler's #103 (restore the
  consecutive rule) is moot. Kill counting comes back only if a source
  of exact mission state appears — none is known.
- **Mission hand-ins, kill credit and completions** (2026-09-19, tester
  feedback 5 + measured on the maintainer's store, PR pending). Three
  defects, one report ("EDDA keeps saying Yamazaki Port"): (1) the
  Missions tab's "Hand in" showed `DestinationStation` from
  `MissionAccepted`, which for a kill mission is a station in the TARGET
  system — 74 of 74 of the maintainer's massacres named a station he
  never docked at; the hand-in is the station docked at when accepting,
  64 of 64 redirects agree (`docs/benches/2026-09-19-mission-handins.csv`;
  maintainer: "for the kill/massacre missions it's pretty much always
  turn in at issuing location"). Now `giver_*`/`hand_in_*` on the
  mission, the hand-in moving only on `MissionRedirected`;
  `destination_*` stays the objective. (2) Kill credit ignored the
  system; now a kill counts only in the mission's `DestinationSystem`
  (position from `FSDJump`/`Location`/`Docked`) — inert on the
  maintainer's data (every credited kill was in Anana), kept as the
  game's rule. The 2026-09-16 ruling "consecutive within a giver" was
  built, then dropped for a day on a broken instrument, then restored:
  the redirect-instant check
  (`docs/benches/2026-09-19-mission-kill-credit-at-redirect.csv`) read
  `kills_done`, which is capped at the target, so an over-credited
  mission sat at the cap and read "exact" — it could not see over-credit
  at all, and its 13/13 for concurrent was that blindness. The instrument
  that can see it (time the store first reached the target vs the game's
  redirect, `knobs/mission_callout_replay.py`, 2026-09-16) had already
  shown concurrent crediting finishing 18 same-giver missions 20 min to
  14 h early, and on 2026-09-19 the maintainer watched the game credit
  one of a same-giver pair while concurrent EDDA showed both. Rule: one
  mission per giver at a time, earliest accepted first. Lesson for the
  record: a capped counter is not an instrument for over-credit; when
  two measurements disagree, find the broken one before overturning a
  ruling. Open: the store runs UNDER the game on most missions (median
  8.5 kills at the redirect on that journal) and nothing in the journal
  explains it (no murder of the target, no Bounty without a faction,
  wing kills in other windows, PVPKill 0). CLOSED the same day: the gap
  is kills the ship never finished scanning (no journal event, full
  mission credit), and kill counting was removed altogether — see the
  entry above. (3) Any
  `MissionRedirected` completed the mission and was spoken TWICE — a
  per-event "Objective complete" in callouts.rs and the per-pass
  "Mission complete" (18 seconds carried both in the maintainer's log,
  40 vs 49 lines). The per-event line is gone; a redirect completes
  do-then-return kinds only, and a delivery/courier/passenger redirect is
  spoken as "Mission redirected: …, now to …". Not measured yet: a
  callout replay on the maintainer's journal with the new rules
  (Waldorf's harness) — asked for.
- **Ship pastes: Coriolis JSON, EDSY SLEF or EDDA SLEF** (2026-09-19,
  maintainer). A Coriolis SLEF export carries only the ship and modules;
  the route page refused its physics and then plotted on whatever jump
  range was typed — no fuel model, boost or scoop stops — which gave a
  wildly different route from the same build pasted from EDSY or read in
  the app (58 jumps Sol → Colonia for both of those). Ruling: accept
  Coriolis's own JSON export, EDSY's SLEF and EDDA's SLEF (Ships tab); a
  refused paste blocks the plot until cleared; the message names the
  three. Coriolis's JSON is reshaped into the journal Loadout: `dryMass`
  (not `unladenMass`, which includes a full tank) → UnladenMass,
  `fuelCapacity` → FuelCapacity.Main, `maxRange` → MaxJumpRange, the FSD
  and Guardian booster synthesised as journal item names. Checked on the
  maintainer's Caspian Explorer export: our full-tank range 72.13 ly
  against Coriolis's 72.14 (fixture in `crates/ed-galaxy/tests/fixtures`).
- **Planner threads on the box** (2026-09-09). The route planner runs on
  cores − 2 threads at low priority, a rule sized when the API process
  also ran the EDDN feed. Now that the feed is its own unit, sweep 2/3/4
  threads (`EDDA_API_PLANNER_THREADS`, to be added) on a Beagle Point plot
  before changing the default. Baseline: 39 s on the box vs 20 s on a
  desktop for the same 193-jump route.
- ~~**`rows_applied` does not count most of what a hydrate applies**
  (2026-09-15, measured). The 2026-09-15 stations backfill recorded
  `rows_applied = 0` in `service_hydrations` while its own summary line
  reported 799,068 identities applied, 1,242,732 bodies, 294,666
  hotspots and 2,191 stars taught. So the column counts only some
  categories, and any week-over-week reading of it — including the
  feed-vs-dump question below — is measuring a fraction of the work and
  calling it the whole. Fix the counter before drawing the curve, and
  persist the per-kind counts the item below actually asks for rather
  than one aggregate.~~ Done 2026-09-16 (#58): thirteen per-kind counts persisted on
  `service_hydrations`, and `rows_applied` now means rows written.
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
  Documented on the endpoint in #45 (2026-09-16). Still open: whether
  the default should follow the app and send the cheap plot unless asked.
- **The stations outage** (2026-09-15, closed same night). `/v1/stations`
  returned 502 on every request for about twenty minutes after v0.3.2
  deployed. Chain, each link worth keeping: three columns added to a
  shared `COLS` string pushed the near search's appended `distance_ly`
  from index 14 to 17; index 14 was then a nullable economy, decoded as
  `f64`, and the worker panicked. Twenty panics, five of them our own
  probes, no commander-visible history — because the client fails closed
  and an unreachable API reads exactly like an empty result.
  Three deeper causes, all fixed: rows were mapped by position against a
  shared list (now by name, in `stations.rs` and `trade_report.rs`); the
  economy feature had shipped with only its parse half, so the columns
  could never fill (write half restored, 842k stations backfilled); and
  `stations_answers_the_three_lookups` reproduces the exact panic in
  0.12 s but CI had never run it — all twelve integration tests were
  `#[ignore]`d behind `EDDA_API_TEST_DATABASE_URL` and no runner ever
  set one. The ubuntu job now has a Postgres service. Verified by
  checking the buggy commit out and watching the suite fail.
  Near miss worth naming: opening the freshness gate for never-learned
  columns made an unconditional `is_carrier` write reachable, which
  would have un-carriered carriers. Caught in review, reproduced, fixed;
  live check found 55,029 carriers flagged and zero damaged rows.
- ~~**An unreachable API must not read as an empty result** (2026-09-15).
  `nearest_service` returns `None` on an API error and the callers fail
  closed, so the Engineering tab prints "none known within 300 ly"
  whether there are no traders, no economies, or no server. That is why
  a dead endpoint looked like the data gap we already knew about, and
  why 0.3.2 looked clean. The client should say it could not reach the
  community API. Owned by the second session.~~ Done 2026-09-15 (#59): `traderStatus` says which of the three
  facts it is; a missing position is blamed on the position, never on
  the API.
- ~~**The release path does not run `cargo deny`** (2026-09-15). It is in
  `ci.yml` only, so a tag never checks licences or advisories. v0.3.2
  shipped carrying RUSTSEC-2026-0285 (rustls 0.23.43) for that reason —
  the advisory landed in the database after the tag, and nothing on the
  release path would have caught it either way.~~ Done 2026-09-15 (#55): the release workflow runs the same
  licence and advisory check as CI, and rustls moved to 0.23.45.
- ~~**`redemption_office` returns nothing**~~ (reported 2026-09-13,
  closed 2026-09-15). Not a bug, and the answer is worth keeping so it
  is not re-opened. The key was never wrong: `station_services` holds
  15,875 rows under `voucherredemption`, which is what both the dump
  parser and the query parameter map to. The split is the whole story
  and it is absolute — galaxy-wide, all 15,875 are on fleet carriers and
  ZERO on static stations, against `materialtrader`'s 1,638 which are
  all static and none carrier. Within 200 ly of Sol there are 10,618,
  every one a carrier; `/v1/stations` excludes carriers unless asked, so
  the default query is right to answer nothing, and
  `include_carriers=true` returns them at once. Confirmed independently
  from the dump (first 20,000 systems of galaxy_populated: Redemption
  Office 1,786, all Drake-Class Carrier, 0 static; Material Trader 161,
  all static). The original report counted a service without its type
  split, which made a carrier-only service look like one the API was
  losing.
- ~~**A service search that finds only carriers should say so**
  (2026-09-15). Falls out of the above: a commander asking for
  redemption offices sees an empty list with no hint that every match
  was a carrier their default filter removed. The same "an empty result
  that is not empty" shape as the trader panel, one layer over. When a
  service search returns nothing with carriers excluded, ask again with
  them included and say "none at a station within N ly; M on fleet
  carriers". Client-side, second session.~~ Done 2026-09-16 (#66): an empty service search asks again with
  carriers included and says how many it found there.
- **The 43,252 stations with no economy are the dump being honest**
  (2026-09-15, a reading and not a proof). After the full backfill,
  799,028 of 842,280 stations carry an economy. The remainder break
  down as 39,691 with no known station type at all, then Outpost 438,
  Planetary Outpost 270, Planetary Construction Depot 172,
  SurfaceStation 115, Settlement 108 and a long tail. Dominated by
  stations the dump barely describes in any field, which reads as
  absence at the source rather than a gap in ingest. Recorded with the
  numbers so it is not re-investigated as a loss.
- **Alpha and beta journals, for dev builds only** (2026-09-16, ruled).
  The alpha and beta clients write `JournalAlpha.*` / `JournalBeta.*`
  into the same folder. A RELEASE build must not read them (maintainer:
  "release edda should not read alpha and beta journals") — they come
  from a test server, so one can carry a ship, a location or materials
  that do not exist in the live galaxy. The exclusion is now explicit
  and pinned (`is_live_journal_name`), because it looks exactly like the
  bug fixed the same day, where unrecognised names sorted after every
  dated file, and the obvious "fix" is to match them the way
  EDMarketConnector does. When we bring up a new game version, add
  support in dev builds behind its own switch.
- **Mission stacking: the mechanic, and three things that do not model
  it** (2026-09-16, maintainer). Domain knowledge first, because it is
  not derivable from the journal and it governs everything below.
  Kill credit is CONCURRENT across missions from DIFFERENT source
  factions that share one target faction — a single kill advances all of
  them — and CONSECUTIVE between missions from the SAME source faction,
  which queue. The maintainer: "ideally I accept as many as I can
  against the same target from multiple factions so I get simultaneous
  credit. If I accept multiple missions from the same faction against
  the same target, progress is consecutive and not concurrent." So the
  stack worth flying is many givers, one target, and the unit of
  estimation is the pair (source faction, target faction): within one
  source faction remainders ADD, across source factions they take the
  MAXIMUM.
  **Modelling this is explicitly NOT wanted** (maintainer, 2026-09-16:
  "I don't think we need strong modeling, the hud and mission tracker is
  working correctly as far as concurrent tracking"). The rule is written
  down because it is expensive to re-derive and easy to get backwards,
  not because anything is waiting on it. (Kill counting was removed on
  2026-09-19 — see the Server section — so the two observations below
  describe code that no longer exists; kept for the numbers.)
  - `kills_done` is an ESTIMATE, and now a measured one. Replaying the
    maintainer's journal from 2026-09-09 (262 credited kills, 42
    missions) against the game's own `MissionRedirected`: 18 were
    inferred complete EARLIER than the game said, by 20 minutes to 14
    hours, and every one of those was a same-giver duplicate — the
    consecutive rule above, showing up as an over-count exactly where
    predicted. Another 19 redirected without ever being inferred, an
    under-count of roughly 7% whose cause is not established (in one
    window the game credited 40 where the journal holds 37 Anana
    Brotherhood bounty events and nothing else kill-shaped). Exactly one
    of the 42 matched to the second. So `MissionRedirected` is the only
    trustworthy completion signal, and the count beside it is an
    approximation that overshoots on same-giver stacks and undershoots
    slightly otherwise. This does not disturb what the HUD does: status
    comes from the redirect, so ordering and completion are right; it is
    the displayed number that is soft. Recorded as fact, not as work —
    the maintainer has ruled the tracker correct for his use.
  - The kill callout (`mission_progress`, watcher.rs) emits for the
    FIRST matching mission in list order only, and the same replay
    measured what that costs. Of 18 kills that completed something (23
    completions, up to 3 on a single kill): the OLD acceptance order
    said "Mission complete" 88 times, the same already-finished mission
    repeating across some 80 consecutive kills, and named the mission
    that actually completed on 2 of 18. The NEW order says it twice,
    both for the wrong mission, and names the right one 0 times. Loud
    and wrong became quiet and silent; neither announces a completion.
    The remaining question for the maintainer is the shape, not the
    cause: a kill that finishes three missions at once probably wants
    one callout with a count.
  Not a plan, and nothing here blocks anything.
- **A carrier's real inventory needs CAPI** (2026-09-16, ruled). The
  Ships tab listed what the commander had personally moved aboard,
  summed from `CargoTransfer` over all time, which reads like current
  stock and is not: the journal never sees the carrier's own market
  sales, another commander's transfers, or services consuming cargo, so
  the figure drifts further from the truth the longer the carrier
  trades. Removed rather than relabelled (maintainer: "if we can't show
  a carrier's *current* inventory we shouldn't show the inventory at
  all"). `carrier_hold` is still derived and still returned by `status`
  — it costs nothing and is the basis for a diff when the real figures
  arrive. Frontier's CAPI `/fleetcarrier` returns actual cargo; the
  spike in `src-tauri/src/capi_spike.rs` already probes that endpoint
  (204 at the time, no carrier owned). Wiring it is the work, and the
  display should not come back before it.
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
- ~~**A route that cannot exist should be refused fast** (2026-09-10). On a
  145k-system fixture with no path, the planner spent 161 s (weight 1.3)
  and over 240 s (exact) before saying no; galos's router answered in
  about a second in all three of its modes. A commander whose range is
  too small for a gap sits through that. Measure the reachable-component
  size first, then pick: a bounded frontier, a connectivity pre-check on
  the cell graph, or a wall-clock cap that returns NoRoute.
  Numbers in `docs/benches/2026-09-09-galos-index-spike.csv`.~~ Done 2026-09-10 (#28): the search state carries the dry-jump
  count only when fuel makes it matter, so an unreachable goal is refused
  in one pass over the reachable systems instead of 7.25 M expansions.
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

- **The highway sub-index is built before a routing version is published**
  (2026-09-21, maintainer: "why the hell did a plot from the bubble to
  colonia in my explorer just recommend 310 jumps? replotting showed a
  correct route" and "wondering if we shouldn't hotswap the rebuilt
  indexes"). Measured from the log: two plots a minute apart, same ship
  and range, 311 hops in 655 ms then 59 in 1273 ms; the second was no
  cache hit. The cause was a documented choice in `galaxy_service.rs`: a
  plot arriving before the new version's neutron highway sub-index was
  built ran without it, served as a bare-range route and never cached. The
  daily reconcile had just republished. Now: `build_highway_blocking` is
  shared, and both publishers (reconcile, adopt) build `.highway/<version>`
  before the manifest points at the version; the route handler waits up
  to 45 s for a missing one (`edda_route_highway_wait_seconds`); a plot
  that still ran without it says `highway_pending` (never cached,
  `edda_route_highway_pending_total`), the client logs it and the Route
  tab says so. Pinned: the builder test in `galaxy_service.rs`; the
  plot test asserts the flag both ways.
- **Never again: the release binary is launched before it ships**
  (2026-09-21, maintainer: "sequence it, fix it, never again"). The
  sequence, cause, fix and gate are in
  `docs/2026-09-21-0.3.5-launch-crash.md`. Landed: a panic hook that
  makes a start-up panic the last line of `edda.log`; `EDDA_SMOKE_EXIT`
  (setup runs to the end, logs "smoke: setup complete", exits 0);
  `scripts/smoke-launch.sh` (fails on non-zero exit, timeout, or a log
  without the line); `release.yml` smokes the signed Windows and Xvfb
  Linux binaries before upload; `ci.yml` smokes the debug binary on every
  PR. Not covered, by design: crashes after setup and anything needing a
  journal — the flight's job.
- **0.3.5 crashed at launch in every release build; 0.3.6 is the fix**
  (2026-09-20, maintainer: "why does the prod app immediately crash?").
  The deep-link plugin was registered under `#[cfg(debug_assertions)]`
  while `setup()` called `app.deep_link()` in every build; Tauri's
  `state::<T>()` panics on an unmanaged type, so the released binary
  died after "voice discovered" and before "overlay ready", three times
  in the maintainer's log, with nothing written (a panic in a GUI
  process has no stderr). The flight gate did not catch it because the
  maintainer flies the debug build, which had the plugin. Lessons: (1)
  a `cfg(debug_assertions)` divergence at start-up is invisible to the
  flight; the audit found no other; (2) a crashed 0.3.5 cannot update
  itself (the check runs after setup), so those installs need the 0.3.6
  installer by hand — the notes say so. A release-build smoke launch in
  CI is the open item.
- **Empty slots and pre-engineered modules as swaps** (2026-09-20,
  maintainer: "need the option to remove an item, i.e. leave empty on
  every slot"; "missing the guardian fsd … The technology broker one is
  equivalent to grade 5 engineered in both fast boot and increased
  range"). `ed_ships::EMPTY` is a swap every slot but a core one takes
  (refit removes the module; the SLEF drops it; an imported build that
  leaves a fitted slot empty says so as a swap). The brokers'
  pre-engineered modules have no symbol of their own — the journal shows
  the plain item with an engineering block and no engineer — so they are
  `data/preengineered.json` presets: the SCO drive V1 in sizes 2–8, its
  fixed multipliers read from the maintainer's Kestrel the evening it was
  bought (optimal mass ×1.7, boot ×0.2, mass ×1.3, power ×1.15, integrity
  ×0.7, heat ×1.2) and pinned; offered next to the plain module, applied
  as fixed figures (not a roll), written into the SLEF as modifiers;
  imports recognise a bought one and plan only the experimental. Sizes
  other than 4 assume the same multipliers (the game's double-engineered
  items are defined that way); a journal Loadout of another size would
  confirm.
- **Module swaps and unowned hulls in the Build planner** (2026-09-20,
  maintainer: "on the build planner tab we should let players swap
  modules out too, which means we need to ensure we only allow selecting
  valid modules for a slot"; "what if someone imports a build for a ship
  they don't have yet?"; "for the ships we'll have to actually build in
  their exact slotting and what module types can go where"). Measured
  first: the journal numbers internals with gaps (the Anaconda has no
  Slot11/12; the Type-10's Slot11 follows its two military slots), so slot
  names are per-hull facts, not a rule
  (`docs/benches/2026-09-20-slot-order-pin.csv`). The facts live in
  `crates/ed-ships/data/ship_slots.json` (every hull's slots by journal
  name, size, and what each takes) and `module_kinds.json` (every
  module's kind, class, hull binding, one-per-ship limit), generated by
  `docs/benches/knobs/gen_ship_slots.py` with EDSY as the reference for
  the game's rules (no EDSY code or file vendored) and pinned against
  Coriolis sizes and the maintainer's twelve Loadouts
  (`docs/benches/2026-09-20-ship-slots-pin.csv`: 194 agree; every fitted
  slot named). `ed_ships::Slots` answers fits/candidates/over_limit and
  makes a stock Loadout for any hull; every Loadout reader takes a `hull`
  (ship_modules, import_build, build_plan_report, build_performance,
  ship_slef); `slot_options` lists every slot with what fits. The planner
  has one row per slot with a Swap to dropdown grouped by kind, an "Any
  ship" group in the ship dropdown, swaps in the saved plan, the figures,
  the report and the SLEF. Not vendored upstream: the Lynx Highliner's
  physics (Coriolis has no data yet; its slots are in the table).
- **A wingmate's shared-mission reward writes nothing** (2026-10-06,
  boss, turning in three shared missions with the journal watched live:
  tellurium, polonium 20, exquisite focus crystals 18 — "I didn't see
  either move"). Each turn-in left exactly one line, a `ShipLocker`
  event, which fires on every docking anyway; no `MissionCompleted`, no
  material line. His OWN wing missions (accepted, `Wing: true`) do write
  completions — all seven this session did, and their polonium landed.
  The companion API's `/profile` carries no materials, so there is no
  second source; the game's login `Materials` snapshot is the only
  resync. The Inventory tab now says when the game last verified the
  counts and that shared-mission rewards show after a relog.
- **The Route tab has shown no route since 0.4.2** (2026-10-07, boss,
  dev app against prod: "in about 2 seconds it seemed like it found a
  result, but it didn't show the actual route"; his console: `Uncaught
  ReferenceError: Place is not defined at RoutePanel.svelte:252`). The
  one-renderer change (0d749af, 0.4.2) put `<Place>` in the hop table
  without importing it; Svelte compiles an unknown component as a
  runtime reference, so the build was green and the tab blanked only
  when it had a route to draw — which is also last night's "failed in
  two seconds" and "nothing came back". Fixed by the import; pinned by
  a server-side render of the tab with the real production Colonia
  answer in the store (`routepanel-renders-route.test.js`, fixture
  `colonia-route-2026-10-07.json`); and every uncaught webview error now
  reaches the app log (`main.js` -> `frontend_log`), so a blank tab is
  never silent again. Shipped broken in 0.4.2 and 0.4.3: those days the
  boss flew trade routes and the game-route follower, not the plotter tab.
- **OPEN: suffix reuse for near-duplicate plots** (2026-10-07, boss,
  replotting Sol -> Colonia right after HIP 90112 -> Colonia: "a bit
  surprised it's using the full budget to calculate again. The route is
  going to have 99% overlap"). The server cache keys on the exact
  request (index version, from, to, range, fuel model, boost,
  supercharge, dry-jump cap, weights; 1 h), so a new origin is a search
  from scratch. Idea: from a cached route to the same destination, plot
  the short leg to its nearest reachable hop and splice the suffix, as
  an INSTANT first candidate for the early-answer lane (#198) with the
  full search refining behind it. Valid only when the ship and the fuel
  state at the join match the cached plan's; not guaranteed optimal
  (the best corridor out of a different origin can differ early).
  Measure before building: spliced vs fresh across the September matrix
  for a second origin per destination — jumps and time. After the 0.4.4
  deploy.
- **Routing night, 2026-10-07** (boss, taking screenshots for the Reddit
  post: HIP 90112 -> Beagle Point took 35 s then 50 s, Stop did nothing,
  the tab showed nothing; HIP 90112 -> Colonia "failed in two seconds"
  — the log says the API answered it in 2.0 s with 92 hops; "we can
  NEVER have this happen again"; the post is held, Statler enlisted for
  the server half). Measured: the API is healthy (/healthz 0.57 s,
  Sol->Alioth 0.6 s); a galaxy crossing costs the planner itself ~14 s
  on the boss's PC and 31-50 s on the box. The suspected cause — the
  server's highway sub-index carries no graph250.bin / alt250.bin — was
  pre-registered as a 3x win and measured NEGATIVE
  (`docs/benches/2026-10-07-highway-sidecars-crossing.csv`): 14.5 s
  without, 14.2 s with, the 2026-09 sidecar comment stands. The cost is
  the portfolio (22 variants to completion). Client fixes landed: Stop
  now cancels an API plot (the token raced against the request; until
  tonight only the local planner read it); the in-game plotter arm no
  longer writes its failure into the plot's error slot; every plot the
  tab rejects is written to the app log (`frontend_log`, target
  `frontend`) so the next "it just failed" has evidence. OPEN: why the
  tab showed the Colonia plot as failed — unreproduced; the new log line
  will say. OPEN (boss ruling needed): what crossing latency is
  acceptable, and whether the interactive lane should answer with the
  first good route in a few seconds and refine behind "Try harder".
- **Early answer budget, measured** (2026-10-07, for the interactive
  crossing lane the boss ruled "sounds fine" on, via Statler): across
  the 18-route September matrix the first found route exists within
  0.1-1.1 s on every route and the best route known by 5 s is at least
  as short as the final on every route; serving at first-found costs
  0-3 jumps, serving at first-found + 5 s grace costs none
  (`docs/benches/2026-10-07-early-answer-budget.csv`, knob
  `knobs/sweep_first_found.py`). Companion finding from the planner-pool
  A/B (`2026-10-07-planner-pool-per-lane.csv`, PR #196): the box's long
  lane runs 87 variants and always fills its 30 s budget, so the time
  from first-found to final is the budget, not the search.
- **Mission material rewards never reached the inventory** (2026-10-06,
  boss: "I don't know if our materials are updating in real time").
  Measured: a replay of the journal since the login snapshot against the
  store found 11 materials behind the game, all undercounts, all from
  the 15 hand-ins with a `MaterialsReward` since that snapshot — the
  materials replay (`ed_journal::inventory`) never read that field, nor
  `TechnologyBroker` and `ScientificResearch` spends. All three now
  apply; schema 11 re-derives existing stores. The same check cleared
  his other report (Peculiar Shield Frequency Data 48/100 refused a
  20-unit reward): the game's own snapshot says 48 and nothing since
  touches it, so the refusal is not that cap. The frontend's copy of the
  trade ratio moved into `materials.js` under a vitest pin of the same
  nine journal trades (boss: "you are adding tests for everything we do,
  right?" — the Rust side always was; the panels were only built).
- **Mission rewards: ready over in play** (2026-10-05, boss: "it'd be nice
  to see the total currently completed missions rewards — i.e.
  455,087,199/540,899,321"). The Missions tab's "Rewards pending" stat is
  now "Rewards · ready / in play": the stated rewards of missions that
  are completed and waiting at a counter, over every mission in play,
  full figures. The HUD's missions line carries the same pair.
- **Material trader: give the near-full, take the gaps** (2026-10-04,
  boss: "I need to go trade down materials maximally... it'd be awesome
  if edda could do that"; then "or a dedicated material trading tab, and
  maybe a callout that prompts commanders to trade down/across/up if
  something is getting close to full — smart enough not to prompt if
  there isn't a trade and toggleable in voice settings"). Measured first:
  the ratio model replays all 114 `MaterialTrade` events in the boss's
  journal exactly (`docs/benches/2026-10-04-material-trade-ratios.csv`,
  knob `knobs/material_trade_ratios.py`) — same group d down 1:3^d, u up
  6^u:1; across groups same grade 6:1, d down 2:3^(d-1), u up 6^(u+1):1;
  Guardian/Thargoid rows do not trade. Planner
  (`ed_journal::mat_trade::plan`): sources are materials at ≥ 90 % of cap
  (knob), spent to a floor of 50 % (knob), highest grade first; targets
  are the non-sources below cap, bottom grade first by default (the
  boss, following the first plan: "why wouldn't I just trade a top mat
  for a bottom mat instead of doing the intermediary?" -- one unit goes
  furthest at the bottom, 1:81; his 49 spare Pharmaceutical Isolators
  fill three Chemical grades and still make 48 Manipulators, where
  nearest-first put all 49 into Manipulators at 1:3), nearest grade
  first as the option that keeps value (one G5 is 1296 G1-equivalents:
  3 G4 keep 648 of them, 81 G1 keep 81); every line is a direct trade
  (a chain costs the same at the game's rates); own group before across
  (off by a knob), down before up (knob); whole trades
  only, never over a cap; a source never receives (that was churn: iron
  up to zinc so zinc could go up to tin so tin could refill the selenium
  that filled zinc). "Still short" means below the near-full threshold,
  not below cap: the first list showed Phosphorus 299/300 as short (boss
  screenshot). Audit of the boss's 29 manual raw trades the same evening
  (`knobs/material_trade_audit.py`, `2026-10-04-material-trade-audit.txt`):
  all same-group, all down, no overflow — rate-perfect; he cascaded
  G4→G3→G2→G1 in steps, which costs nothing at geometric rates (1:3 three
  times is 1:27), and went to a 33 % floor; the planner from the same
  start does it in 20 direct trades instead of 29. Not a knapsack: an
  integer transportation problem — sources with budgets, targets with
  capacities, geometric rates; within a group every routing is
  equivalent, so greedy is exact there, and the only real decisions are
  the floor, the order (units vs value) and whether to pay 6× to cross.
  The boss's actual aim, said after doing it by hand: "fill up on the
  G1-3 mats, then rebalance G4/G5 so I'd have room to accept mat rewards
  from missions since it's all G4/5" ("a mission where I pass up the
  material reward because I was full hurts my soul"). So the defaults are
  that workflow: only G4/G5 are spent, across and up are off, and a
  **room pass** follows the fill — anything still over 85 % of its cap
  (knob) moves into a G4/G5 that has room, own group first (1:3 down,
  6:1 up), then across at the same grade (6:1), then one down (2:1),
  never pushing a receiver over the ceiling. The first plan's cross
  trades "didn't make sense" to him for exactly this reason: they were
  filling other groups' G1s from his full G1s at 6:1. **Layout** (boss
  screenshot of Reiter City's manufactured trader): the Inventory tab is
  the game's grid — one row per trader group, G1 to G5 left to right,
  count over cap in each cell — and the Trader tab is gone; "Material
  trading mode" on the Inventory tab draws the plan on the cells (−given,
  +received) and, with a material selected, shows in every other cell
  what you would pay of it for one of the selected, as the game does
  (216 ⇄ 1 on a G1 for a G4). OPEN: the group order on the game's
  screen — Chemical, Thermic, Heat are from the screenshot; the rest of
  manufactured, and raw and encoded, are laid out from memory until the
  boss scrolls the screen.
  The planner plans for the docked station's type
  when a `MaterialTrade` was made there or its economy says (else the
  type with most to spend), shows sources, the give/receive list with the
  counts left, what stays short, and the nearest traders of that type;
  it re-plans on every journal change, so the list shrinks at the
  counter. Callout kind `trader` (voice settings): the pickup that takes
  a material over the threshold speaks once — "X is nearly full, 90 of
  100. A manufactured trader would trade the surplus down into Y and Z"
  — and only when the single-source plan has a trade.
- **HUD follows the selected tab** (2026-10-04, boss: "maybe dynamically
  changing the hud depending on the edda tab selected"). OPEN, not
  built: the overlay is its own window fed by status events; it would
  need the main window's tab as an input and a per-tab card set (Trader →
  the next trade and counts; Missions → the stacking board; Trade → the
  leg; Route → the follower). Shape it after the trader has flown.
- **"Target their engines" is a lap through the journal, not a blind
  press** (2026-10-04, boss, after the ship computer pressed Cycle Next
  Subsystem once and landed on the cargo hatch: "how do other apps target
  a specific subsystem?"). The game has no per-module binding and the
  cycle order is the target's own module list, so a fixed count lands
  somewhere different on every hull. It does write a `ShipTargeted`
  (ScanStage 3) line with the subsystem's symbol on every change — the
  boss's 2026-08-22 log has nine in eight seconds, one per press; today's
  single press at 14:31:23.339 produced the hatch line in the same
  second. `src-tauri/src/subsystem.rs` closes the loop: press, tail the
  journal at 150 ms (boss ruling; the watcher's 500 ms is for callouts),
  compare the symbol family, press again; stops on the match, a lost
  target, an unanswered press (an unscanned target cycles nothing — the
  reply names the scan stage), or a budget of 36 presses (duplicates such
  as two multi-cannons make a lap undetectable by symbol). Kinds are
  keyed by symbol family (`int_powerplant`, `ext_drive` is what the
  journal writes for thrusters, `modularcargobaydoor` for the hatch,
  `int_hyperdrive`, …); "weapon" and "utility" are EDCD's `hardpoint` /
  `utility` categories from the vendored outfitting table, so point
  defence is a utility, not a gun. Spoken order ("target / lock / go for
  the …") and ship-computer tool `target_subsystem`; "next subsystem" is
  still the blind step. Binds are read fresh from the newest
  `Custom.*.binds` at every press (yours: Ctrl+O). Every request traces
  presses, ms, the subsystems passed and the verdict (`subsystem
  targeting`); the first flights are the measurement of the lap time.
- **Scoop talk needs a scoop** (2026-10-04, maintainer: "edda should only
  say a star is scoopable if I have a fuel scoop installed. Otherwise it
  should say if there's fuel available if there's a station I could dock
  at — only if I need fuel or am getting low"). The Loadout sets
  `has_fuel_scoop`; without one the FSDTarget line drops ", not
  scoopable" and the two low-fuel star cautions stay quiet. The fuel-trap
  guard already spoke scoopless wording for the trap case; it now also
  speaks the positive case — tank under 35 %, no scoop, a dock that fits
  at the target: "Fuel at N percent and no scoop fitted. X has a station
  you can dock at for fuel." With a scoop, or with fuel, the dock is not
  mentioned. Second sighting the same morning ("Next Antliae Sector ...,
  class M, scoopable"): that line is the *game-route* reader
  (`ed_store::route::next_hop_text`), a different speaker from the
  FSDTarget callout. The brief now carries `scoop_fitted` from the latest
  Loadout; without a scoop the hop lines lose every scoop word and the
  plot-time "jump N is not scoopable; top up at X first" sentence is
  skipped. One flag, three speakers — the EDDA-plan follower already had
  `ship_has_scoop` on the route.
- **The idle app read 25 MB/s and wrote 6 MB/s** (2026-10-03, boss: "why is
  EDDA using 1.9% cpu and 6.0 MB/s of disk when the game isn't running?").
  Measured (`docs/benches/2026-10-03-quiet-sync-io.csv`): a spike every
  ~6 s of 153 MB read / 37 MB written. The quiet sync itself cost 17 ms;
  the damage was downstream: `ingest_companions` re-read and re-wrote all
  nine companion files every pass regardless of mtime and reported nine
  "updated" snapshots, so the watcher emitted JOURNAL_CHANGED every 5 s
  with nothing new, and the HUD plus the visible panel re-derived
  everything — one `missions::active` on a fresh read connection was
  19 MB read and 5 MB WRITTEN (its ORDER BY temp b-tree spilled to disk).
  Fixed: unchanged companions are skipped, the signal fires only on a real
  change, `temp_store = MEMORY` on every connection (writes per call 5.3 →
  0 MB, reads 19 → 13 MB). Harnesses `sync_cost` and `conn_churn` under
  `crates/ed-store/examples/`. Also: EDDA said the game was running
  because EliteDangerous64.exe was still alive with no window after the
  session ended — process detection told the truth.
- **Engineer table vs the wiki overview, 2026-10-04** (boss: "is our
  engineering in line with this?", pasting the wiki's Engineers table):
  25 engineers each side, one difference — Lori Jameson's Kill Warrant
  Scanner G3, which the overview omits and her own wiki article lists.
  Ours stands. Knob: `docs/benches/knobs/engineer_table_vs_wiki_overview.py`.
- **Engineer unlocks landed only at the next login** (2026-10-03, boss:
  "it says I don't have some engineers unlocked … are we confident in our
  knowledge of engineer abilities?"). The table was right (audited
  2026-09-19); the store was not: it derived only the roll-call form of
  `EngineerProgress` (the `Engineers` array at login) and ignored the
  single-engineer events the game writes on unlock and rank-up. His
  journal: Broo Tarquin unlocked 14:14 and ranked to 5 by 14:16, Juri
  Ishmaak unlocked 15:02, The Sarge and Bris Dekker invited 15:04 — all
  still "Invited"/absent in the store, so the planner said no engineer
  could do G5 lasers. Both forms now apply in event order (a rank-only
  event means unlocked); schema 10 forces the re-derive.
- **Speculative kill tracking is back as an opt-in estimate** (2026-10-03,
  boss: users asked again; "let's make it a checkbox"). The 2026-09-19
  burial stands on its measurement — the journal cannot see every kill
  the game credits (46 of 65 redirected massacres 2–23 short at the
  redirect) — so what returns is a FLOOR, labelled "speculative", off by
  default: `kills_seen` counts `Bounty`/`FactionKillBond` on the target
  faction, in the mission's system, one mission per giver at a time,
  capped at the target; it never moves a status, the redirect still does.
  A completed massacre is a measurement (boss: "one is 54 and one is 64,
  we're tracking 45 but then the 54 finishes — how many kills do we know
  we have?" — 54): its KillCount raises the floor of every other giver's
  mission on the same target and system that was active before its first
  kill; a same-giver mission waited its turn and one accepted after the
  kills began keeps its own count — unless the stream is calibrated:
  a completion fixes the stream's exact miss count (boss's journal,
  2026-10-03: 24 of 35 seen at Jet Central's redirect, 45 of 63 at the
  Ahayan Defence one — ~30 % unseen), a quiet gap (no kill, no
  UnderAttack/HullDamage/Died/FighterDestroyed) carries it to a later
  acceptance, and when a calibrated neighbour completes the newcomer's
  count is the game's own for that instant, shown "= n" (boss: "it should
  grant the delta, no?" / "we know when we're in combat, right?"). With
  combat in the gap, as on 2026-10-03 (UnderAttack 21:31 between the
  21:19 completion and the 21:43 acceptances), it stays a floor. Config
  `speculative_missions`; the Missions tab checkbox; "≥ n / N seen" or
  "= n / N exact" on the row and the HUD; knob
  `cargo run -p ed-store --example missions_dump -- <store>` (with
  `EDDA_MISSIONS_TRACE=1` for the per-kill trace). The kill-progress callouts of September
  were NOT restored. Premise change: user demand plus explicit opt-in and
  labelling, not any new journal data.
- **Shield callouts need a shield generator; every utterance passes the
  repeat gate** (2026-10-02, maintainer: "only make shield callouts when
  a shield generator is equipped"; "anything the voice says should go
  through the same debounce logic we have for 'under attack'"). The
  Loadout's modules set `has_shield_generator`; a shieldless hull's
  `ShieldState` (the game writes ShieldsUp:false at every launch) says
  nothing. The `RepeatGate` moved into `VoiceHandle::say` (kind "voice",
  10 s) so AI replies, route-following messages and the `say` command are
  debounced like callouts; `say_unthrottled` is for speech the commander
  asked for this instant — voice samples, greetings after picking an
  engine, barge-in, spoken-order replies including "repeat that", and the
  wake acknowledgement.
- **A route request plots right away** (2026-10-02, maintainer: "that
  opens the app router but I still have to click plot there, why not do
  it right away?"). `RoutePanel.plot()` ran only the in-game arm when the
  destination was inside `game_route_max_ly` with the map controls taught,
  and returned with no EDDA route — from a result-list arrow that looked
  like nothing happened. It now runs EDDA's own plot first, every time,
  then arms Elite's plotter as before.
- **One renderer for a place you can go** (2026-10-02, maintainer: mining
  results had no way to navigate to a system — "same as we do on every
  other system result page … wire this into a unified flow/renderer,
  these inconsistencies make us look unpolished"). `Place.svelte`: the
  system name, the route arrow (Route tab plots there) and a copy button
  for the galaxy map, with one shared clipboard state
  (`clipboard.svelte.js`). Used by Market, Galaxy services, Trade (legs
  and loops, both ends), Mining (all five tables), Engineers, Powerplay,
  the build planner's sellers, the shopping list's three lists, and the
  Route tab's hop list (arrow off there). Before: an arrow in two panels,
  a text "route" button in two, a copy button in two, nothing in three.
- **Faction completion from a precomputed list** (2026-10-01, boss: "no
  auto complete for factions … seems like something we could
  pre-compute"). Measured on the mirror: 38,911 distinct factions; a
  DISTINCT over `stations.controlling_faction` is ~140 ms per keystroke
  and a btree index makes it 1 s. 0025 creates `factions(name)`, backfills
  it from the station table (0.8 s), and the writer adds a name the first
  time a Docked event carries it; `/v1/names/complete?kind=faction`
  answers prefix matches first, then contains. The trade panel's box uses
  the shared Autocomplete, with the current report's factions ahead of
  the server's.
- **Local mirror of production** (2026-10-01, boss: "sync the server db
  to the wsl db too so that we're one for one"). `scripts/mirror-prod-db.sh`
  dumps the box (directory format, 2 jobs, zstd: 1.8 GB in 65 s), rsyncs
  it (3 min at the PC's ~10 MB/s), restores into `edda_mirror` with 6
  jobs (30 min, index builds; 46 GB), verifies (850,979 stations,
  803,949 with a faction, 102.6M market rows, 68.1M outfitting rows,
  schema at 0023), migrates with the local binary and repoints the WSL
  `ed-api serve` + `ed-api ingest` at it; the EDDN feed keeps it current.
  Dev builds fly against it (`dev_api_local`). Also found and fixed on
  the way: the decoder dropped `Docked.StationFaction`, so station
  factions only ever came from the Spansh dump.
- **Trade: controlling faction on both ends, selectable as the sell-side
  filter** (2026-09-30, maintainer grinding Alioth Independents for the
  Alioth permit — Allied with that minor faction, then its invitation
  mission within ~12 ly of Alioth, per the wiki). `StationRef.
  controlling_faction` from `stations.controlling_faction` (EDDN Docked);
  `Constraints.sell_faction` keeps legs whose sell station the faction
  controls, so a loop is two sales to it; the panel shows factions on
  both ends, click to filter, "any" = none. Measured: 78 Ursae Majoris is
  all Terran Colonial Forces (wrong faction); Sugiyama Territories ⇄
  Mahon Bell -2117 sells to Alioth Independents both ways at 13.9M/loop.
- **Merc-coin modules: the feed's v3 prices are kept, the commander's own
  boards are exact, merc-only listings hide from a commander with no
  coins** (2026-09-29, maintainer: "for outfitting we're counting items
  sold for merc coin … if a player doesn't have merc coin we shouldn't
  show the result"; "how does inara do it?"). Grounded: the game's
  Outfitting.json at Omega Prospect (Merope) lists 24 merc-coin items,
  every one a 2026 pre-engineered variant under the PLAIN module's
  symbol (Balanced Power Distributor = `int_powerdistributor_size5_class5`,
  500 MC, 0 Cr); 22 symbols in outfitting.csv carry such a twin. EDDN
  outfitting/2 is bare symbols and EDMC sends every module regardless of
  price, so a v2 board cannot tell them apart; outfitting/3
  (https://eddn.edcd.io/schemas/outfitting/3) carries `id`, `BuyPrice`,
  `BuyMercCoinsPrice` per entry — which our decoder accepted and
  flattened to names. Inara shows Omega Prospect with 98 credit-priced
  modules and no merc ones: it reads v3 prices. Measured on the relay
  (`docs/benches/2026-09-29-eddn-outfitting-v3-share.csv`): 10 of 98
  outfitting boards in 600 s were v3 (EDO Materials Helper, EDDI); EDMC
  (`develop` too, 6.1.x) and EDDiscovery send v2. Every v3 board carried
  the SAME merc-only set — the merc catalogue is the same at every
  outfitting station — so a twin symbol on a v2 board is always there
  and proves nothing about a credit sale; the client marks such a row
  "unconfirmed".
  Now: the decoder keeps v3 prices (`Snapshot.module_prices`), 0024 adds
  `credits_price` / `merc_price` / `merc_variant_ids` to outfitting
  (NULL = v2, unknown), the server drops `credits_price = 0` rows unless
  the request says `currency: any`, and the client (a) records the
  commander's own Outfitting.json per station in `outfitting_seen`
  (schema 9) and overrides the feed with it, (b) reads `MercCoins_Current`
  from Statistics and asks for `any` only with a balance, (c) names a
  merc variant by its FDev id ("Balanced Power Distributor", not "Power
  Distributor 5A"), notes a station that sells both, and counts what it
  hid. Honest limit: ~90 % of boards are v2, so an unvisited station can
  still list a merc-only variant as the plain module, and says
  "unconfirmed" rather than pretending. The only cure for
  the commons is EDDA uploading outfitting/3 itself — a boss decision,
  not taken here. The "engineer" station service is NOT a marker (33,760
  stations have it).
- **Module search: names resolve to exact symbols, the server matches
  them exactly, the symbol column is indexed** (2026-09-29, maintainer:
  "module search in the market is taking forever, returning no results";
  "why are we not aligned on common names?"). The commodity search got
  the symbol-is-the-key treatment on the 27th; the module search had
  not: it built a stem from the typed words by hand ("Bi-Weave Shield
  Generator" → `biweaveshieldgenerator`, which no symbol contains — the
  real ones are `int_shieldgenerator_sizeN_class3_fast`) and the server
  substring-matched it over the whole outfitting table. Measured on the
  local 66M-row copy (`docs/benches/2026-09-29-outfitting-search.csv`,
  knob `outfitting_search_bench.sql`): the substring match reads every
  row whatever the text, 17-19 s (6-7 s and an 8 s timeout on the box);
  exact symbols 4.3 s without an index, 0.75 s with one. Now
  `ed_journal::modules::resolve_search` turns the words into the
  outfitting table's symbols (EDCD name, a commander alias, or a symbol
  typed in; a leading size or size+rating narrows; an exact name wins
  over names containing it), the wire carries `symbols`, the server
  matches `= ANY` when they are present and keeps the substring match
  for an older client; for an older SERVER the text is the symbols as
  one LIKE pattern (`int_shieldgenerator_size%_class3_fast`), so the
  search answers correctly on production before the server ships, only
  slowly. 0022/0023 add `(symbol, station_id)` indexes
  — CONCURRENTLY, one statement per file (3 min 41 s for outfitting
  locally, plain or concurrent; the EDDN writer is not locked out). The
  completion box keeps a typed size: "5A bi" offers "5A Bi-Weave Shield
  Generator". Local server development is back: the WSL PostgreSQL copy
  (edda_dev, 127.0.0.1:55432) took every migration in 106 s — 0009 56 s,
  0015 47 s, 0021 10 ms here against 133 s on the box.
- **Missions: the login roll-call closes what the game no longer lists**
  (2026-09-29, maintainer: "we're tracking a permit acquisition
  opportunity as an active mission, but I already have the permit").
  `MISSION_genericPermit1` wrote `MissionAccepted` on 2026-09-27 and
  nothing else — the permit is granted on the spot — so the store held it
  Active for two days while nine logins' `Missions` events listed
  `Active: []`. The store now reads `Missions`: an open mission the game
  lists Active stays (its `Expires` fills a missing expiry), one listed
  Complete is ready to turn in, one listed Failed failed, and one in
  none of the three is closed as completed at that login. A roll-call
  before a mission's acceptance says nothing about it. Test:
  `the_login_roll_call_closes_what_the_game_no_longer_lists`.
- **Trade: a loaded leg is timed at the range for its own tons**
  (2026-09-29, maintainer: "nonsensical that a single leg is more
  profitable than a round trip when the round trip incorporates the
  single leg"). Every loaded leg was timed at the FULL-hold laden range
  whatever it carried, while a leg's repeat rate flies back empty at the
  unladen range; a 10 t return therefore cost one jump more than the
  empty return, and the loop's cr/h fell under the leg's. The `Leg` doc
  claimed the opposite. `Ship::range_at_tons` is the 1/mass curve
  through both Loadout figures; `cycle`, `make_leg`, `pair_rate` and
  `fill_hold` use it, so a light return costs no extra jump and the loop
  beats the leg whenever the return earns anything. A FULL hold back
  that earns less than its extra jump still loses to the leg repeated
  empty — honestly — and the Round trips tab now says so in a line.
  Test: `a_light_return_cargo_does_not_cost_a_full_holds_jumps`.
- **Migrations run before the restart; the readiness wait is 300 s**
  (2026-09-29, the follow-up from the 0.4.0 server deploy). `ed-api
  migrate` applies every migration not yet recorded and exits;
  `deploy/edda-apply api` runs it with the new binary while the old
  service keeps serving, then restarts, and waits up to 300 s for
  readiness instead of 120. The CI log records "migrations applied in
  N s" per deploy, so a slow migration is a number, not a failed run.
  Installed on the box from the repo copy the same day.
- **v0.4.0 first cut: the AppImage prune met a directory** (2026-09-29).
  The Linux release job built and signed its three bundles, then
  `scripts/appimage-fixup.sh` died: `find -name 'libgst*'` matched
  `usr/share/doc/libgstreamer-plugins-base1.0-0`, a directory on the
  runner's package layout, and `rm -f` refused it. The prune takes
  files and symlinks only now; the tag was moved to the fix (nothing
  had published) and the release re-run.
- **0.4.0 server deploy: the readiness wait is shorter than a market
  migration** (2026-09-29). `deploy-api` from main reported failure:
  "ed-api not ready after 120 s". The service was fine — migration
  0021 (the commodity fold, which repoints and deletes market rows)
  took 133 s on production and finished twelve seconds after the box's
  `edda-apply api` stopped waiting; readiness was 200 a moment later,
  the migration is recorded, the new binary serves. Verified after:
  "Micro Controllers" by name from Sol answers three sellers within
  9 ly; the commodities table holds 0 display-name symbols (426 rows,
  down from 728; the 14 nameless are the tissue-sample symbols no
  source names). The deploy was re-run for a green record. To do on the
  box: a readiness wait that outlasts a market-table migration (the
  wait lives in `edda-apply`, not in this repo), or a migration step
  that runs before the restart.
- **The Engineers tab; module names complete; the Sirius brokers**
  (2026-09-29). The directory had gone into the Engineering panel,
  hidden since 0.3.5 — "engineering tab is still hidden/missing" — so
  it is its own tab now, after Build planner, refreshed on every
  journal change. The Market tab's outfitting box completed nothing
  ("heat" offered no Heat Sink Launcher): module names now complete
  from the bundled outfitting table, and a box naming a pre-engineered
  module says such modules are not sold at outfitting and where they
  are. The Sirius pre-engineered modules come from the technology
  brokers on five Sirius Corporation megaships (the wiki's Heatsink
  Launcher page): "Technology Broker (Sirius)" in the services search
  names them and lists any in range, and a Sirius broker line in the
  plan says where it is bought.
- **The engineers directory, and a fitted variant is fitted** (2026-09-29;
  the maintainer: "one thing I do miss from the engineer tab is showing
  which engineers can do what — and who is unlocked, known, unknown",
  "which engineers can do which grades would be nice so I can
  prioritize who to unlock first"; and, with six modified shards now
  on the ship, "edda isn't realized I already have them equipped").
  The Engineering tab's row of pills (journal engineers only) is a
  directory of every engineer: the journal's word (Unlocked, Invited,
  Known, Not known), where they are and how they are met (the vendored
  guide), what they do to what grade (`Catalog::engineer_module_grades`,
  from the blueprint table the audited engineer table pins), and — for
  one not unlocked — what unlocking them adds over the grades the
  unlocked ones already reach ("Frame Shift Drive G3→G5", "Thrusters
  G5 (none now)"), most first: the order to unlock in. A filter takes a
  name, a system or a module type; a grade pill filters. And the
  fitted variant: the journal writes a bought module's block with the
  first blueprint in lowercase ("weapon_longrange", grade 1, the
  broker's EngineerID, no engineer), and a saved swap to that variant
  was still a swap. A slot carrying the module with that blueprint at
  that grade is fitted now — in the plan's rows (slot options carry
  the variant's blueprint symbol), in the report (no broker line), and
  in a fresh import (nothing to swap); a learned preset the table
  already knows by blueprint and grade is not a second entry. Pinned
  end to end with the six as the journal wrote them.
- **A broker purchase after the last storage snapshot counts as a unit
  owned** (2026-09-29; the maintainer, after buying six modified shards
  in three minutes and reading "1 in storage, × 5 to buy": "are we not
  summing correctly?"). The sum was right for what the journal said:
  the game writes StoredModules at the next dock or outfitting screen,
  not at the purchase, and the last snapshot (03:55:02) fell after the
  first purchase only. The plan now takes every TechnologyBroker event
  later than the storage snapshot whose payment matches the variant's
  recipe as one unit bought and not yet listed, satisfies one slot
  with it ("bought at the broker at 03:56 — in storage there once the
  game lists it"), and prices only the rest; once the snapshot catches
  up, storage takes over. Pinned end to end: one in storage, one bought
  after the snapshot, nothing left to buy.
- **Modules in storage are known, and a build that calls for one owned
  is told so** (2026-09-28; the maintainer: "do we know what ship
  modules someone has in storage?", "if a build calls for a ship module
  that the player already has, we should know from the journal then",
  "we probably need a storage (or extend) the storage crate and put
  that info in sqlite for fast access"). The journal writes a
  StoredModules event at every dock and every outfitting screen — the
  maintainer's latest listed 55 — with each module's system, storage
  slot, bought engineering (blueprint, grade, quality), hot flag,
  transfer cost and time, in transit; nothing read it. A derived
  `stored_modules` table now holds the latest snapshot, replayed like
  ships and materials (schema 8 rebuilds it from the kept events), and
  `query::stored_modules` reads it. The plan: a unit in storage — the
  plain module, or the variant with this very engineering — satisfies
  one slot and says where it is and what the transfer costs ("in
  storage at Mbooni · transfer 51,000 cr, 12 min") with nothing to buy
  or unlock for it; the rest is priced as before. The same module
  fitted on another owned ship is said ("also fitted on Murderface
  (Kestrel Mk II) — moving one strips that ship"), never spent. Pinned
  end to end: two modified 2A shards over plain ones, one bought and
  in storage at Mbooni — one line stored, one unit to buy, two Power
  Converters. CAPI was not made a source: `/profile` carries the current
  ship's modules, and nothing confirmed it carries storage.
- **The build plan follows the journal** (2026-09-28; the maintainer:
  "any reason we aren't using that same logic for the list in build
  planner? getting commodities, trading materials, etc?" — "seems we
  could update that in real time too"). The report was a snapshot from
  the click. It is derived from the inventory and the hold, so the
  planner now re-derives it on every journal change while it is
  showing — a trade made, a commodity bought, a material collected moves
  the materials table, the commodities table, the shopping list and the
  broker lines without a click; a pick survives by what the trade is,
  not where it sits; a hidden tab catches up once when shown. The
  tracked HUD list follows from it (the HUD keeps its own journal
  matching for the moments the planner is not showing). The answers
  the report asks the community API for — the nearest traders around a
  system, the sellers of a good — are kept ten minutes by key
  (`report_ask_cache`), hits logged with their age, so the live report
  costs the API nothing between asks.
- **A pinned trade vanishes from the HUD as it is made** (2026-09-28;
  the maintainer: "as the trades are being made why isn't it being
  tracked?", "when the trades are complete the things can just
  disappear from the hud"). The pinned list was a snapshot and nothing
  read the trades. The journal writes a MaterialTrade for each (paid
  and received by Frontier's names, the trader type):
  `query::material_trades_since` and the `material_trades_since`
  command return every trade since the pin's moment; the overlay
  refetches on every journal change and whenever the pin changes, and
  a pinned line whose give and get match a trade since the pin (each
  journal trade paying for one line) is gone; the header counts "N of
  M made"; with nothing left and nothing short the section disappears.
  Pinned in the store with a trade as the maintainer's journal wrote it.
- **The trade list on the HUD, tracked from the build planner**
  (2026-09-28; the maintainer: "we need to be able to selectively show
  what we're supposed to trade for in the hud. Like i'm at the material
  trader now and it'd be nice not to have to tab back and forth" —
  "something to check in the build planner page", "Track trade list in
  HUD"). A checkbox on the shopping list (the build planner's and the
  Engineering tab's alike) pins the picked trades and what stays short
  to the HUD over the storage bus (`KEYS.hudShopping`), and while it is
  ticked every change to the picks follows. A new HUD section,
  "shopping", lists them: the status now carries what the dock offers
  (`ShipStatus::dock`: the Docked event's economy, whether it has a
  material trader, and the trader's kind from the economy — the same
  rule the services search uses), so at a manufactured trader the
  manufactured trades come first with a "manufactured trader here"
  pill and the others dim; compact shows only that kind's. In the Trade
  preset, hidden in none; a layout of any shape is still made whole
  (`hudLayout.test.js`).
- **Commodities to buy have their own table; a search from the black
  asks from inside the bubble, not the nearest colony** (2026-09-27;
  the maintainer: "why didn't it give me power converters in a list and
  tell me where to go buy them?", "they each take 2 power convertors
  but we only list that in grey text that's easy to miss"). Two things
  in the log. The seller search for the modified shards' Power
  Converters ran from Synuefe GV-T b50-4, found nothing within 500 ly,
  re-asked from the nearest inhabited system — Synuefe QX-J c25-3, a
  lone colony 5 ly from the Guardian site — and found nothing again.
  `nearest_in_bubble` now takes the nearest inhabited system with at
  least 15 inhabited neighbours within 30 ly (the bubble's edge, or
  Colonia's cluster), and the answer says the count. The Titan Drive
  Component the SCO V1 drives ask for was searched twice a plan for
  nothing: salvage is never on a market board ("salvaged from destroyed
  Thargoid Titans", the wiki), so it is said instead. And each broker
  line listed its commodity in grey text with nothing summing them:
  the plan now pools commodities across every broker line into a
  "Commodities to buy" table beside the materials table — need, in
  hold, short, and the nearest markets with the shortfall in stock —
  and the header's "short" count includes them; the broker line says
  "plus, to buy: 4 Power Converter" in plain text. Pinned end to end:
  two modified 2A shards over plain ones pool to four Power
  Converters, none in hold, not fully met.
- **A bought variant is told from a roll by the second modification**
  (2026-09-27; the maintainer: "we can infer they are the sirius ones
  since it has two enhancements, no?"). An EDSY export never names the
  engineer, so on a module engineers work a Sirius launcher and a grade
  1 Ammo Capacity roll carried the same blueprint at the same grade.
  The block tells them apart: every pre-engineered module carries a
  second modification (the wiki on the V1 drives: other engineering
  "will remove the bonus second modification"), so its figures fall
  outside what the named blueprint at that grade can produce.
  `ed_engineering::Catalog::beyond_blueprint` judges a block against the
  blueprint data's own effects — a figure moved the other way (the
  Sirius halves the mass Ammo Capacity doubles), a figure beyond the
  grade's bound with the best experimental stacked on it (the
  maintainer's Kestrel drive: optimal mass 1.77 against Increased Range's
  1.55 and Mass Manager's 4%), or a figure neither the blueprint nor
  any experimental touches (its boot time) — and the import takes a
  preset for such a module only when the block is beyond its blueprint;
  a block inside the figures is a roll, planned as one. Pinned in
  ed-engineering with the Sirius, the roll, the Kestrel and a grade 5
  roll with Mass Manager, and end to end: four Sirius blocks import as
  four swaps to the preset, four grade 1 roll blocks import as four
  engineering rows and no swap.
- **Identical callouts inside a window are one callout** (2026-09-27;
  the maintainer: "no need to spam call out under attack, add a
  debounce or throttle on callouts"). Measured in the day's log: 182
  "Under attack." lines, several a second in a fight — the journal's
  UnderAttack event fires with every hit — and the voice queue dropping
  the overflow. `callouts::RepeatGate` in the announcer: the same kind
  and text inside a window (danger 30 s, fuel and heat 20 s, else 10 s)
  is not said again; a different text passes at once; the repeats are
  counted and logged with the next one that passes ("callout: repeats
  not said inside the window", with the count and the window), so the
  burst is measured rather than lost. Pinned with a fourteen-hit burst.
- **Module names carry class and rating; one broker line per variant
  with the hold against every unit** (2026-09-27; the maintainer: "I
  don't think we're summing this properly, and we aren't showing the
  size/grade next to the guardian modules still (i.e. 3c, 2a, 3d,
  etc...)", "not sure what this text is supposed to really tell me",
  and a window that scrolled sideways with no scrollbar). A weapon
  printed its mount and size as words with no class or rating, so a 3C
  and a 3D shard read alike: `item_name` now prints every module as
  the outfitting screen does ("Guardian Shard Cannon 3C (fixed)", "Heat
  Sink Launcher 0I"), and `recipe_name` keeps the broker's spelling for
  recipe lookups ("Guardian Shard Cannon (Fixed, Large)"; a recipe named
  without a size, "(Fixed)", now matches too). The technology broker
  section printed one line per slot, each ticking its own five
  components against the hold, so two modified shards needing ten read
  as covered twice: one line per variant now, with the unit count, the
  slots, materials and commodities summed across units, and the sellers
  asked for the summed shortfall. The import's note on a recognised
  variant says what it is and where its price is, instead of "nothing
  to plan for it"; a variant no table knows says it is planned as the
  plain module. The preset's name is short ("Modified Shard Cannon 2A
  (fixed) · pre-engineered Guardian Shard Cannon 2A (fixed)") with the
  engineering in words in `description`. The seller pills wrap.
- **Pre-engineered modules: the table is generated from sources, a
  variant is told from the plain module by its block, and each unit is
  paid for** (2026-09-27; the maintainer, on a build wanting two
  modified 2A shards beside two plain ones the ship had: "for the
  guardian modules we don't seem to differentiate by size, class, and
  modified ... the slef is different so.... why can't we know/support?",
  "the pre-engineered mods are one-time buy, not a permanent unlock",
  "we're also missing the pre-engineered heatsinks via a sirius
  technology broker", and the rule: "stop making assumptions. You need
  to ground everything with a search and confirmation from a reliable
  source"). `preengineered.json` held seven SCO drives typed by hand.
  It is now generated by `docs/benches/knobs/gen_preengineered.py` from
  the 29 `preEngineered` rows in the vendored EDCD/coriolis-data (their
  blueprints, grade, experimental, availability and stat ratios against
  the base row), with the broker's recipe from `blueprints.json`, the
  purchase rule from the wiki (every broker pre-engineered module is
  bought again with each unit: "one payment of the requested resources
  will immediately grant one pre-Engineered module ... More of these
  modules can be obtained only by paying more resources each time"),
  and stat ratios from the wiki's module pages only where Coriolis has
  none. `tech_broker_pin.py` pins every recipe in `blueprints.json`
  against the wiki's Technology Broker page
  (`docs/benches/2026-09-27-tech-broker-pin.csv`): every per-unit recipe
  agrees with its own module page; the broker page's Purchases table
  lists the plain unlock costs and is the wrong one; the plain unlocks
  differ on three counts (shard turret large 26 vs 28 technology
  components, shock cannon fixed medium 26 vs 22 tungsten, meta alloy
  hull reinforcement 25 vs 15 focus crystals) with no third source yet —
  OPEN. The six SCO V1 drive recipes were missing; added from the wiki's
  table, the class 4 row confirmed exactly by the maintainer's own
  TechnologyBroker event of 2026-08-23. `preengineered-pin.csv` records
  where Coriolis and the wiki disagree on a figure (the Sirius AX racks'
  mass and rate of fire, the modified mining laser's range): OPEN,
  Coriolis's ratio used because it is relative to the base table the
  physics multiplies. In code: a `Preset` carries every blueprint, its
  experimental and `per_unit`; the import matches a bought variant by
  any of its blueprints at its grade (or the one variant with that
  blueprint), and reads a variant the table lacks off the block itself
  (`Preset::from_engineering`, kept in `presets_seen`, no recipe
  guessed); the plan prices a variant by its own recipe once per unit,
  never as "already unlocked", and says when the data has no recipe;
  the unlock proof for the plain module ignores a fitted variant (its
  blueprint says so) and a broker payment that matches a variant's
  recipe. Found on the way: Coriolis lists a variant after the base row
  under the same symbol and the base table took the last row, so the
  plain heat sink launcher weighed the Sirius half and the plain 2A
  shard cost nothing — fixed and pinned.
- **An unlock the journal has already seen is not asked for again**
  (2026-09-27; the maintainer: "for guardian components, it doesn't
  appear that we actually take already unlocked components into
  account"). The plan priced the large shard cannon's Guardian unlock —
  28 technology components, a blueprint fragment, 20 wreckage
  components, 18 Micro Controllers — for a Python Mk II flying six of
  them, unlocked at a Guardian broker on 2026-08-17 per the journal's
  own `TechnologyBroker` event, which nothing read.
  `ed_store::query::unlocked_modules` now proves an unlock two ways:
  that event's `ItemsUnlocked`, or any ship's Loadout that carried the
  module (it cannot be bought before its unlock); the plan's broker
  line says which ("unlocked at a Guardian technology broker on
  2026-08-17" / "already fitted on the Python Mk II") and gathers
  nothing for it. Pinned in the store.
- **Services by kind, typed rather than picked** (2026-09-27; the
  maintainer: "when searching galaxy services it just says Technology
  Broker but there are different types of brokers. Also it might just
  be nicer to have it an autocomplete text box rather than a dropdown").
  The data lists a broker plainly; its type follows the station's
  economy (High Tech offers the Guardian modules, Industrial the Human
  ones), the same rule that already split material traders into raw,
  manufactured and encoded. `remote_lookup::kinded_service` names the
  kinds — `guardian_technology_broker`, `human_technology_broker`, the
  three trader kinds — and `nearest_service` answers with the kind
  applied and a note saying the rule, or, when the data carries no
  economies for the stations found, that the list is every broker in
  range rather than a promise of the kind. The ship computer's tool
  takes the same keys. The Galaxy tab's service is a text box that
  completes on any part of a label ("brok" → both brokers); an
  ambiguous or unknown entry is said, never searched as something else.
- **Names: the symbol is the key, Frontier's string prints, and an audit
  measures it** (2026-09-27; the maintainer: "we really need to audit
  every name and ensure we translate and align so we don't get
  mismatches like this. It's things like this that make us
  untrustworthy"). The day's two faults were both names: "Guardian
  Wreckage Components" (the game, the journal, the recipes) against
  FDevIDs' "Guardian Sentinel Wreckage Components", and a market
  search for "Micro Controllers" that hit a nameless server row whose
  SYMBOL was that display name and answered nothing 28 ly from sellers.
  The rule now, in code: the FDev symbol is the key everywhere
  (storage, wire, joins); a display name exists only to print; where
  Frontier's own string (the journal's `Name_Localised`) differs from
  EDCD's, Frontier's prints and EDCD's stays an alias
  (`ed_journal::catalog::FRONTIER_NAMES`, gated by `tests/edcd_exact.rs`
  against the fixture's measured `catalog` section); anything that
  arrives as a name resolves to a symbol at the boundary — the client
  sends a commodity's symbol (`remote_search::wire_text`), the server's
  apply folds display-name symbols onto the canonical row on the way in
  (`ed_store::postgres`, with Frontier's short strings such as "low
  temp. diamonds" in `frontier_commodity_symbol`), and the resolver
  prefers a named row. Migration 0021 folds the 298 display-name rows
  production still carried (0009 deleted only unreferenced ones, and one
  EDDN sender kept making them: five stations each of "festive gifts"
  and "low temp. diamonds" that morning). The instrument:
  `docs/benches/knobs/name_audit.py`, run to
  `docs/benches/2026-09-27-name-audit.csv`, cross-checks recipe
  ingredients, community sites, the commander's own journal strings,
  the server's commodities export and the EDSY tables against FDevIDs.
  Its first run found and this change fixed: Frontier prints "Guardian
  Weapon/Module Blueprint Fragment" where EDCD and the blueprint data
  say Segment; "Limpet" where EDCD says Limpets; two Thargoid materials
  FDevIDs lacks (Caustic Crystal, Caustic Shard: `EXTRA_MATERIALS`,
  pinned to vanish when FDevIDs gains them); three recipe spellings
  matching no table (Abnormal Compact Emission Data, Ballistic Data,
  Xihe Companions); the Caspian's bulkheads named as every other hull's
  by the slot generator instead of EDCD's "Mk II Ablative …"; and the
  recipe seam itself — `ed_engineering::Catalog::load` now resolves
  every ingredient to the catalog's printed name, so the recipe, the
  inventory and the plan meet on one spelling. Left OPEN in the CSV
  (no Frontier string measured for modules in this journal): EDSY's
  "Manifest Scanner" and "Anti-Corrosion Cargo Rack" group labels
  against EDCD's "Cargo Scanner" and "Corrosion Resistant Cargo Rack";
  and fourteen tissue-sample commodity symbols on the server that
  FDevIDs (equal to upstream that day) does not list, with no name from
  any source. Rerun the audit after 0021 deploys: the server section
  should then read zero display-name symbols.
- **The unlock's commodity is looked up, and the hold counts** (2026-09-27;
  the maintainer: "why aren't we offering to perform a market search,
  or just doing one?"). The technology-broker line said "commodities to
  buy: 18 Micro Controllers" and nothing more, and did not know the hold
  might carry them. Each commodity line now carries what the hold has
  and, when short, the three nearest markets with the shortfall in
  stock, from where the commander is, each with a route button
  (`fill_unlock_sellers`, one market search per line short; the report
  itself stays offline). And a search from the black — a Guardian site
  950 ly out — answered "nothing" because the server's reach is 500 ly:
  `remote_search::search` now asks again from the nearest inhabited
  system (the bundled bubble index, the origin's position from the
  journal) and says so in the answer, in the Market tab and on the
  unlock line alike.
- **A plot from the SRV planned the ship on the buggy's tank**
  (2026-09-27). "the community API did not answer ({"error":"no_route"})
  and the bundled bubble index cannot plot this on its own (unknown
  system "Synuefe NL-N c23-4"); try again in a moment" — for a 220 ly
  hop on a full Mandalay. The log had the cause in its own plot line:
  `start_fuel: 0.0`. In the SRV (and on foot) Status.json's Fuel block
  is the vehicle's, FuelMain 0.0 beside a 0.37 t reservoir, and
  `ship_fuel_for` took it for the ship's; the server's fuel model found
  no first jump and refused, honestly. Three things were wrong in what
  the commander read: the server *had* answered (a refusal is not a
  transport failure), "try again" could not help (nothing transient),
  and no retry of any kind had happened. Now: Status fuel counts only
  with InMainShip set, else the journal's last reading of the ship's
  tank (pinned in `start_fuel_fails_closed_to_the_journals_last_reading`
  with the measured SRV Status); `plot_via_api` types its failure —
  a refusal is said with the inputs that decided it ("no route from X
  to Y for the Mandalay departing with 0.0 t of 32 t; refuel first, or
  plot with the fuel model off"), only a transport failure says "did
  not answer … try again in a moment"; `send_api` retries once, after
  1.5 s, a connection that never opened or a 502/503/504 (a timeout is
  not retried: the long lane already waits 130 s; 429 kept its own
  retry); and the in-game plotter logs when it arms, because the
  "then it succeeded" was that path, not a second plot — the log showed
  one API plot all session. Logged with the source of the start fuel.
- **Shopping list: Frontier's material names win, one pickup row per
  site, one block per material** (2026-09-27, the maintainer's shard
  cannon unlock). Three faults in one report. "20 Guardian Wreckage
  Components — where to get it:" had nothing under it and "Have 0"
  beside 23 in the hold: FDevIDs names the material "Guardian Sentinel
  Wreckage Components", the game, the journal's `Name_Localised` and
  every recipe say "Guardian Wreckage Components", so the inventory,
  the farm plan and the sources all missed by name. The ships rule
  applies (Frontier's own string wins): `Catalog::FRONTIER_NAMES`
  overrides the printed name, EDCD's stays an alias, and the gate in
  `tests/edcd_exact.rs` reads the override from the fixture's new
  `materials` section. A recipe-vs-catalog cross-check found it the only
  ship-material mismatch. Pickup rows were keyed by (system, body), so a
  site's pickups before the first ApproachBody made a second row
  ("Synuefe NL-N c23-4" twice); now one row per system, named by the
  body most picked up on. The Site column printed the body's full name
  after the system it already contains, and "1 pickups"; the report
  showed each still-short material twice (farm table, then a prose
  block with the same pickups). Now one block per material: the table
  when there is one (the maintainer's preference), with the kind's
  methods and any system-less community site folded under it; the
  prose block only for a material with no table, and a plain "no source
  in EDDA's data yet" line when it is empty rather than a bare heading.
  The Guardian method text now names wreckage components.
- **Module limits are pools, not kinds** (2026-09-26). Importing an
  EDSY build onto the maintainer's Python Mk II failed: "2 × advanced
  docking computer: a ship carries at most 1; 6 × guardian shard cannon:
  a ship carries at most 4" — for the ship as flown, which the game had
  allowed. The swap logic was right (a swapped slot's old module left
  the count); the limit table was wrong three ways. `over_limit` counted
  per module kind, but the game's limit is a named pool: every AX and
  Guardian weapon shares one pool of four (so 4 AX multi-cannons + 1 AX
  missile rack is over, which the kind-wise count allowed), the docking
  computer and the supercruise assist are one kind but two pools, and the
  Experimental Weapon Stabiliser widens the weapon pool by one (class 3)
  or two (class 5) — the maintainer's six shards ride a class 5. The
  generator also fell back to the kind when a module had no pool, which
  capped flak launchers at four and the shutdown field neutraliser at one
  beside a xeno scanner; the game does neither. `module_kinds.json` now
  carries `limit_group`, `unlimit` and `unlimit_count` from EDSY's own
  pool table (data of 2026-09-23), `over_limit` counts by pool with the
  allowance and says how to widen it, and `kind()` reads a `_free`
  early-access variant as its paid module (the Type-11's hangar). Two
  pins added to `gen_ship_slots.py` and the CSV
  (`docs/benches/2026-09-26-ship-slots-pin.csv`, 218 agree, the two
  by-design rows differ): every Loadout in the store keeps within every
  pool, and every fitted module is known and on a hull it is sold for —
  the second found the Advanced Planetary Approach Suite, on every ship
  since the 2026 update, missing from the table; added. Refreshing from EDSY's current data also brought
  its corrections: the Large Planetary Vehicle Hangar and the Vessel
  Hangar are bound to the 13 fighter-capable hulls, the Mk II hangar to
  the Caspian, Type-11 and Panther Mk II, and the Type-11's FighterBay01
  takes a vehicle hangar — to be flown before release.
- **Stack economics and hand-ins at the dock, from ODEliteTracker's
  ideas** (2026-09-20; the maintainer: "let's make sure we aren't missing
  anything", and "since we aren't copying code, just ideas, make sure we
  say based on some ideas derived from"). Statler's gap review
  (`docs/2026-09-20-odelitetracker-mission-gap-review.md`) recommended
  three items; measured first on the maintainer's store
  (`docs/benches/2026-09-20-missions-reconcile-pin.csv`,
  `knobs/missions_reconcile_pin.py`): across 185 startup `Missions`
  events EDDA held ZERO missions the game no longer listed and missed
  ZERO of its Failed list, so startup reconciliation is a null result
  and stays in the ledger unbuilt; 43 of 481 docks had hand-ins ready
  (mean 4, max 12), so the dock line is worth a line. Built: `Stack`
  gains kills_needed (largest per-giver sum), kills_remaining (over
  open missions), kills_credited (every count added), value,
  value_ready, value_shareable, target_system — all stated fields, pinned
  on the twenty-mission fixture (120 / 72 / 824, pre-registered);
  `ready_here` and `hand_ins_here` (the derived `location` table for the
  dock); `MissionCompleted.Reward` replaces the offer, `Donated` kept;
  `missions_here` command; the watcher speaks one `mission_hand_in` line
  per dock; the tab and HUD show the figures and the dock pill. Idea
  credit to ODEliteTracker in the notes and help; no code copied (it has
  no license file).
- **Engineering tab hidden** (2026-09-20, maintainer: "let's get rid of
  the engineering tab now (just hide it). I think it's now replaced fully
  by the build planner"). The tab is out of the strip; `EngineeringPanel`
  and its store stay on disk (ShoppingReport still reads the store). The
  Ships tab's per-module Plan opens the planner on that ship; the old
  hand-over flag lands on the planner too. The help topic folded into
  Ships, and the help test refuses any "Engineering tab" wording.
- **Guardian and Thargoid materials are never traded** (2026-09-20,
  maintainer: "we seem to suggest trading for guardian technology
  components and whatnot is possible. It is not" — the list had offered
  5 Sensor Fragment → 45 Guardian Technology Component at 1:9). EDCD's
  material table gives those materials the trader group `None`; the
  solver now drops any such material at the door, as a thing to give or
  to get, so they fall to "still short" with their own ways (sentinels,
  obelisks and relic pylons at Guardian sites; Thargoid combat and
  wreckage) instead of a trade that does not exist. Pinned by a test
  with the maintainer's two lines.
- **Where to get what is short** (2026-09-20, maintainer: "if someone is
  genuinely short, why doesn't edda know where to get them? … we could
  look in the data we know and go 'oh you got this here before', even if
  it isn't the exact material but could be traded for the needed
  material … for the technology broker components we could refer to the
  recipe for the component and break it down"). Measured first: the
  vendored site list covered raw 6/28, manufactured 47/64, encoded 6/45
  materials (10 sites), so most shortfalls read "no known farm site".
  Now, for every material still short after trading: the commander's
  own pickups from the journal (system, body, units, pickups, distance —
  one pass over the event log for all materials, `witnessed_sources_all`),
  the community sites, and the ways the kind is obtained
  (`methods_for_kind`: surface prospecting and crystal shards; HGE by
  system state, salvage, missions; ship and wake scans, data points, the
  two crash sites; and the trader rates). Own pickups also feed the
  farm-then-trade solver, so "farm Iron where you found it before, trade
  6:1" appears. Swaps to technology-broker modules break down into their
  unlock recipe (`unlock_recipe`, Guardian and Human types of the
  blueprint data): materials pool into the plan, commodities are listed
  to buy.
- **Build planner is its own tab** (2026-09-20, maintainer, first flight of
  the plan: the Import build and include buttons were "a hell of a lot
  smaller than other buttons", and "it might be good to have a build
  planner page with a dropdown of ships to select, and the plan build
  button on the ships page can just link to that"). `BuildPlannerPanel`
  with a ship dropdown (the flown ship by default), the import box as a
  proper block with full-size buttons, the plan table, figures and report
  moved out of `ShipsPanel` unchanged; `planner.svelte.js` is the hand-off
  (Plan build → the planner on that ship), the same shape as the
  Engineering tab's. Help chips and the notes point at the tab.
- **Customisable HUD** (2026-09-20, maintainer: "I'd like to get the
  customizeable hud in too"). The overlay's nine lines are now named
  sections (`frontend/src/lib/hudLayout.js`: location, Powerplay, next,
  route, gauges, trade, missions, callouts) with a layout — order, hidden,
  compact — written by Settings → HUD → Layout and read by the overlay
  over the storage bus, live like opacity and scale. Presets Combat,
  Trade, Explore, Default; a layout can be remembered per ship (JSON map
  by ShipID) and wins over the shared one while that ship is flown.
  Compact drops the label, shrinks the pills and shows fewer items (three
  hops, two missions, two callouts). Still one window: separate movable
  widgets were offered and not asked for; the shaping is pure and tested
  (`hudLayout.test.js`), a stored layout of any shape is made whole.
- **Ship physics, phase 1: mass, jump, power** (2026-09-19, maintainer:
  "can we work out ship physics like weight and whatnot like edsy and
  coriolis do to see what the perf would be after engineering?" — and
  "it's fine if we take the physics stuff", the vendoring call). New crate
  `ed-ships` with Coriolis's figures vendored (`data/coriolis`, 1.4 MB,
  upstream 0db9234 of 2026-04-24; Frontier's data under Frontier's terms,
  see THIRD-PARTY-NOTICES) and the journal's rolled values winning over
  computed ones. A planned blueprint applies its Coriolis effect ranges
  at a full roll on the module's base figures, EDSY's convention. The
  Ships tab's plan shows "as flown → with this plan": unladen mass, jump
  (full tank / laden / max) and power retracted / deployed against the
  plant. Pinned against EDSY on the maintainer's Type-10 as flown
  (`docs/benches/2026-09-19-ship-physics-phase1-vs-edsy.csv`): mass to
  0.1 t, jump to 0.01 ly, power to 0.1 %. Two facts from the pin: the
  journal's cockpit and cargo hatch are fixed parts (0 t; the hatch 0.6
  MW), and EDSY weighs the fuel reserve but does not jump with it. Next
  phases, each with its own pin: speed and boost (thruster mass curve),
  shields and armour, weapons (DPS, thermal, distributor). Ships Coriolis
  lacks (none of the maintainer's; the data has the Caspian, the Panther
  Mk II, the Type-11, the Kestrel) fall back to the journal's own totals.
- **Printed names are EDCD's, and Frontier's** (2026-09-20, maintainer:
  "every ship name, every module name, every commodity, every material,
  every everything has a coded name and a printed name and we should be
  exact"; "no mismatch between us, EDDN, EDMC, EDCD"). Measured first
  (`docs/benches/2026-09-20-edcd-name-exactness.csv`): materials 137/137
  and commodities 270/270 exact; ships 39/48 and modules 1041/1236 from
  hand tables; rare commodities absent. Two facts decided the rule.
  The journal's `*_Localised` module strings are the ship panel's SHORT
  forms ("FSD (SCO)", "Frag Cannon", "K-Warrant Scanner"), none of them
  in EDCD's table — a first check that read "156 of 156 agree" had
  skipped exactly those rows and counted nothing (instrument error,
  retracted the same night) — so modules print EDCD's outfitting-screen
  name, the one EDMC and EDDN use, and the short forms are a second
  namespace not yet carried; and Frontier writes "Krait Mk II"
  where EDCD's shipyard table writes "Krait MkII" — so ships print
  EDCD's name with Frontier's mark spacing. FDevIDs `outfitting.csv`,
  `shipyard.csv`, `rare_commodity.csv`, `microresources.csv` and
  `engineers.csv` are vendored; `item_name` and `display_name` read the
  tables first and keep the hand tables only as the fallback for a
  symbol newer than the table; rare goods join the catalog; "Tod
  McQuinn" is spelled as the journal spells him (the nickname in), which
  is why he never showed as unlocked. Gates:
  `crates/ed-journal/tests/edcd_exact.rs` (every table row, and
  Frontier's strings from the journal as a fixture) and the
  engineer-spelling test. EDMC's own ship map is inconsistent on the
  same spacing ("Cobra MkIII", "Python Mk II"), so Frontier's strings
  are the tiebreak, not EDMC. Not yet in the catalog: Odyssey
  microresources as a kind of their own.
- **Import a build, plan the gap** (2026-09-19, maintainer: "import a
  build from either edsy or coriolis and we calculate what they're
  missing to get to it"). The Ships tab's plan takes a SLEF paste (EDSY
  and Coriolis both export the journal Loadout) and diffs it against the
  ship as flown: modules to swap (fitted → wanted, per slot), then every
  engineered module of the build as a plan row — same blueprint continues
  from the fitted grade, a swapped module or another blueprint starts at
  grade 0, a module the ship already has is left unticked. From there the
  whole-build report applies unchanged. Coriolis's JSON export is refused
  with the way out (it names modules Coriolis's way; its reshaping serves
  the route planner's drive only). Experimental effects arrive as journal
  symbols; `experimental_for_symbol` maps the 90 of them to blueprint-data
  names (from Coriolis's specials list; three matched by hand).
- **Frontier link in first-run setup** (2026-09-19, maintainer: "we also
  need to include authing to fdev in the setup flow"). Step 7 of 8,
  "Frontier account", between Galaxy-map interaction and Ready: the same
  card as Settings → Frontier account (`FrontierLink.svelte`), with the
  narration and the page saying what it is for (the carrier's real hold),
  where the login happens (Frontier, in the browser) and what never
  leaves the PC (nothing from Frontier reaches EDDA's servers). Optional,
  skippable, revisitable. The narrated-steps set is by index, so a
  commander who finished setup before this change has the old "Ready"
  index marked heard and hears the new step on a revisit — harmless.
- **Shopping list: the game's material names, one trader ask** (2026-09-20,
  maintainer flying the build plan: "Still short after trading: 5 Abnormal
  Compact Emission Data … these can be traded for at the encoded trader.
  Are we missing info?" and "what's our timeout?"). Two defects. (1) EDEngineer
  spells one ship material differently from the game ("Emission" for
  "Emissions"); the gap report keys the hold by the game's names, so the
  plan read short 5 while 84 sat aboard, and the shopping list — which
  only knows the game's names — could not even name the trader kind.
  Measured across all 258 ingredient names: exactly one ship material
  differs; the rest of the mismatches are suit/weapon materials and
  tech-broker commodities, another namespace. Canonicalised at load, held
  by a test against `material.csv`. (2) The trader stops asked the API
  once per kind for the same unsplit list; in his log the first ask
  stalled for the full 15 s timeout and the second answered in 2.6 s, so
  "could not reach the API" stood beside a stop that could. One ask,
  split client-side, tried once more after a stall. Server note for
  Statler: a 300 ly nearest-service query at 2.6 s, and one 15 s stall.
- **Plan the whole build at once** (2026-09-19, maintainer: "my type 10
  has 9 weapon hardpoints — I'd like to be able to plan out all 9 at once
  and get the list. It should generalize to planning all hardpoints").
  The Ships tab's "Plan build" turns the module table into a plan: a
  blueprint, target grade and experimental per engineerable module,
  "same for all N" copying one row onto every module of its type, the
  fitted engineering continued to the top grade by default, saved per
  ship. One report (`build_plan_report`): materials pooled by name
  against the inventory (nine Focused lasers need nine times the iron),
  one shopping list from the pooled shortfall (the per-module
  `shopping_for` was split into `shopping_from_need` so both paths share
  it), and the fewest engineers that cover the plan (greedy set cover
  over unlocked engineers offering the target grade; slots nobody
  unlocked can do are named, not dropped). The planned build exports as
  SLEF with every planned slot at its target grade. The shopping-list
  markup is one component (`ShoppingReport.svelte`) shared with the
  Engineering tab. Pure shaping in `frontend/src/lib/buildplan.js`,
  tested.
- **Engineer audit** (2026-09-19, maintainer: "we need to do a clean
  sweep of engineers" — The Dweller was missing from his Type-10's pulse
  lasers). The vendored blueprint data (EDEngineer, byte-identical to
  upstream, last changed 2024-03) was compared per (engineer, module
  type, top grade) against Inara and the wiki
  (`docs/benches/2026-09-19-engineer-grade-audit.csv`). The Dweller row
  was RIGHT (Pulse Laser G4); the panel was wrong: it listed the
  engineers of the target grade only, and the target defaults to G5, so
  a G4 engineer vanished. Now every engineer who works the blueprint is
  listed with their cap ("to G4"). Two data errors fixed: Lori Jameson
  does Life Support to G4 (was G5); Juri Ishmaak does the three scanners
  to G3 (was absent). The audited table is checked in
  (`crates/ed-engineering/data/engineer_grades.json`) and a test fails
  if the blueprint data drifts from it. Inara-only rows (Cargo Rack,
  MRP, mining tools, Guardian G1) are modules the game does not engineer
  and were not adopted; Ram Tah's limpet controllers stay G4 (wiki and
  EDEngineer against Inara's G5).
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

- ~~**A veteran's journal read as 2022** (2026-09-15, fixed). A tester with
  a years-long journal saw a stale ship and "still in flight": the game's
  file names changed format in late 2022, and as plain strings
  `Journal.22...` sorts after `Journal.2026-...`, so every "latest" and
  every replay keyed on `(file, offset)` put 2021–2022 after today. Now
  files list by `ed_journal::journal_file::sort_key` and every time-order
  query orders by `ts` first; the tailer's watermark carries `ts`. Still
  open from the same report, **first-sync cost on a large journal**: we
  store every event kind with its raw JSON forever (179 kinds here, 24 MB
  for 51k events; 100 kinds and 22% of those bytes are never named by any
  reader), and a first run parses every line of every file. Measure a
  real multi-year journal before choosing between an allowlist at ingest,
  a shorter retention for unread kinds, and a first-pass that skips what
  no derived table needs.~~ Done in two parts, and the first was not enough: #60 (0.3.3)
  fixed the order files are read in, and #67 sourced the two-format
  claim and pinned the same-day overlap; the symptom survived until #74
  (0.3.4) fixed the derive bookmark that replayed the oldest history over
  today on every launch. The 0.3.4 notes say so.

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
