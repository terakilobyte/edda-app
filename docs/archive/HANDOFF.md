# Handoff — EDDA (Elite Dangerous Desktop Aid)

Updated 2026-08-26 (evening). Workspace builds clean; all tests green
(`cargo test --workspace`). The app is in daily use by the commander; this
document is what a new session needs to know, in the order it matters.

Read `docs/PLAN.md` for architecture and the settled decisions.

---

## Where to start

```powershell
$env:Path = "$PWD\.data\tools\node;$env:Path"   # portable Node, if none installed
cargo test --workspace
cargo tauri dev                                  # main window + HUD overlay
cargo run -p ed-store  --example sql    --release -- "SELECT COUNT(*) FROM events"
cargo run -p ed-route  --example profit --release -- Wongi 200 30 any 48 120 0
cargo run -p ed-galaxy --example route  --release -- .data/galaxy_populated Wongi Deciat 37.6
```

**`cargo tauri dev` restarts the app on every Rust change** (1–2 min
rebuild, then the old process is killed). A long search or plot in flight
dies with it. Batch backend edits when the commander is testing.

**Data** lives under `.data/` (gitignored): the 27 GB SQLite database,
Spansh dumps, Piper voices, logs, portable Node, and the galaxy star
index. Do not delete casually.

---

## What exists (crates)

| Crate | Purpose | Tests |
|---|---|---|
| `ed-journal` | Journal parsing, FDevIDs names | 8 |
| `ed-engineering` | Blueprints + gap diffing | 4 |
| `ed-store` | Event log, derived tables, galaxy tables, EDDN apply, lookup, merits, missions, route brief, session/timelines, observation dataset | 62 |
| `ed-eddn` | Live relay subscriber | 6 |
| `ed-route` | Time cost, hull pads, **profit finder** (legs, loops, rings) | 16 |
| `ed-voice` | Piper sidecar TTS, SAPI fallback | 4 |
| `ed-galaxy` | **Whole-galaxy star index + A\* router** | 10 |
| `src-tauri` | Commands, watcher, callouts, personas, overlay, AI loop, routing | 18 |

---

## Today's big pieces

### Profit finder (`ed-route::profit`)
- Ranks by **cr/h**. Single legs use the *repeating* rate (outbound + empty
  return); loops and rings time every leg loaded. A leg can never outrank
  the loop it belongs to (loops pair from the full leg set, not the
  diversified one — that was a real bug).
- Laden range for loaded legs (from Loadout masses), unladen for empty.
- Mixed cargo: the hold is topped up with the next-best commodities on the
  same pair (fractional knapsack, exact).
- **Rings**: beam search over the best-leg graph, 3–12 stops, rotated to
  start nearest the origin. Not exhaustive (BEAM 400, FANOUT 6).
- Pairing is spatially bucketed by `max_leg_ly` (default 150) so galaxy-wide
  searches follow local density. `idx_mkt_updated` (built in the background
  on first launch, ~6 min) lets wide searches scan only fresh rows.
- Filters: pad, carriers, arrival distance, price age (default 48 h),
  station cap, Powerplay power/state on each side with modes
  controls / present / undermining (`sys_systems.powers` lists every
  power present).
- Verified against a real sale: predicted gold 4,475 → 61,304 at Ferguson
  Enterprise → Hiroyuki Vision; the commander sold at exactly those prices.
  Travel-time results are explicitly labelled estimated. Docking and
  undocking constants are calibrated from the commander's journal;
  supercruise and market-screen time cannot be isolated from events yet.

### Route plotter (`ed-galaxy`)
- `import` streams a Spansh dump into `stars.bin` (32-byte records sorted
  by 50 ly cell), `cells.bin`, `names.bin`, `byname.bin`; memory-mapped.
  Populated dump: 145,579 systems in 85 s. Full dump: 116 GB compressed,
  streamed (never stored), ~18k systems/s → ~2–3 h, ~8 GB index, ~8 GB RAM
  during the sort.
- `router::plan`: weighted A* (default 1.3), neutron ×4 / white dwarf ×1.5
  supercharging, `max_dry_jumps`, cancel/progress. **No fuel model yet** —
  that is where the ant-colony planner belongs (jump range changes with
  fuel mass; refuel timing matters).
- App: Route tab (autocomplete, Stop, map: top-down + elevation, optional),
  `.data/galaxy` preferred over `.data/galaxy_populated`, Settings → Galaxy
  index builds either in-app with progress/cancel (reqwest blocking +
  flate2, scratch dir swapped in on success).
- The commander's Wongi → Colonia plot on the populated index correctly
  returns "no route" — the black is empty there.

### Ship computer
- Sonnet 5 by default (Fable/Mythos-only `fallbacks` param removed).
- API key in **Windows Credential Manager** (`keyring`), model/persona/
  research in `.data/config.json`.
- **Research**: Anthropic server-side `web_search_20260209` +
  `web_fetch_20260209`, citations rendered as a Sources row, toggle in
  Settings. Verified against docs, not a live call (no key in the agent's
  environment).
- Tools now: status, inventory, engineers, blueprint access/gap, system,
  stations, station search, nearest service, merit model, powerplay seen,
  find_profit (all filters), combat_stats, station_market, systems_near,
  missions, current_route, commander_ranks, say. **plot_route is not an AI
  tool yet** (needs RoutingState reachable from `ai.rs`).
- Personas (Standard/Butler/Robotic/Sassy/Sultry) restyle callouts and the
  chat tone from the same facts.
- Speech normalisation: markdown stripped, "Mk II" → "Mark 2", units and
  ISO dates spoken.

### Callouts & HUD
- Route briefing on plot (fuel, dry runs, hazards with supercharge
  required/not, docking, opposing powers, anarchy) and next-star line on
  every arrival. Missions with inferred kill progress. Scans of us,
  under attack, hostile-territory warning, session summary.
- HUD: route strip (next 5 hops), pinned loop/ring, missions, remembered
  position (saved on every move), Ctrl+Shift+O / Ctrl+Shift+H.

### Concurrency
- Read-only commands and AI tools use **private read-only SQLite
  connections** (`AppState::with_read`); only journal sync, EDDN apply and
  VACUUM take the shared lock. Nothing blocks the UI thread: long work runs
  on `spawn_blocking` with cancel flags.

---

## Merit model — a lead on K

The commander's reference (the commander's own Inara logbook) states,
without formulas: trade/mining merits correlate with profit **margin**
(≥ 40 % required); **−35 %** for reinforcement (own power's systems),
**+5 %** for acquisition/undermining, **+50 %** for the power's ethos;
bounty merits on the kill. Those stack into exactly a per-station
constant. Next fit over `observations.jsonl`: `implied_k` vs margin,
relation-to-pledge, and ethos. `merits.rs` already keeps K per station and
refuses to average — keep that until the fit holds.

## Known limitations and estimated values

- Absolute trade ETAs remain approximate because the journal cannot isolate
  supercruise and market-screen time; use the estimate to compare routes.
- Rare-material list and hull→pad table are curated by hand. The Kestrel
  Mk II (`smallcombat01_nx`) has a display name but no pad size.
- Faction (system) states are not in the galaxy dump; only witnessed on
  your own FSDJumps. A "states witnessed" table is owed.
- Powerplay hostility wording ("expect their security to be hostile") is
  the PP2 rule as understood, not journal-stated.
- Research availability depends on the configured provider and account.
- EDDN decode errors still need a captured-message audit.

---

## Next

1. Verify the full galaxy index once the import lands; plot Wongi → Colonia.
2. `plot_route` as an AI tool; route → NavRoute hand-off (copy first hop).
3. Fuel-aware planner (ACO) using Loadout masses and FSD curve.
4. Calibrate the leg time model from MarketBuy→MarketSell pairs.
5. States-witnessed table; Kokoro TTS sidecar as a second voice backend;
   provider-agnostic LLM layer.
