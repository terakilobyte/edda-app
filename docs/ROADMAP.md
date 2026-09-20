# Roadmap

What is next, in the order it currently matters, and what was tried and
buried. This page is the project's record from the public release on:
decisions carry their date, buried ideas carry the numbers that buried
them, and the reasoning behind a change is in its pull request. Bench
verdicts live in the CSV headers under `docs/benches/`.

## Server

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
- **`rows_applied` does not count most of what a hydrate applies**
  (2026-09-15, measured). The 2026-09-15 stations backfill recorded
  `rows_applied = 0` in `service_hydrations` while its own summary line
  reported 799,068 identities applied, 1,242,732 bodies, 294,666
  hotspots and 2,191 stars taught. So the column counts only some
  categories, and any week-over-week reading of it — including the
  feed-vs-dump question below — is measuring a fraction of the work and
  calling it the whole. Fix the counter before drawing the curve, and
  persist the per-kind counts the item below actually asks for rather
  than one aggregate.
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
- **An unreachable API must not read as an empty result** (2026-09-15).
  `nearest_service` returns `None` on an API error and the callers fail
  closed, so the Engineering tab prints "none known within 300 ly"
  whether there are no traders, no economies, or no server. That is why
  a dead endpoint looked like the data gap we already knew about, and
  why 0.3.2 looked clean. The client should say it could not reach the
  community API. Owned by the second session.
- **The release path does not run `cargo deny`** (2026-09-15). It is in
  `ci.yml` only, so a tag never checks licences or advisories. v0.3.2
  shipped carrying RUSTSEC-2026-0285 (rustls 0.23.43) for that reason —
  the advisory landed in the database after the tag, and nothing on the
  release path would have caught it either way.
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
- **A service search that finds only carriers should say so**
  (2026-09-15). Falls out of the above: a commander asking for
  redemption offices sees an empty list with no hint that every match
  was a carrier their default filter removed. The same "an empty result
  that is not empty" shape as the trader panel, one layer over. When a
  service search returns nothing with carriers excluded, ask again with
  them included and say "none at a station within N ly; M on fleet
  carriers". Client-side, second session.
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

- **A veteran's journal read as 2022** (2026-09-15, fixed). A tester with
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
  no derived table needs.

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
