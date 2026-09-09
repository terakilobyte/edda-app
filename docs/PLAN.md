# EDDA (Elite Dangerous Desktop Aid) — architecture and build plan

Draft. No code written yet. Written against `577a739`, with all 8 existing crate tests
passing.

Scope: fold the Python prototypes in `../` and the useful parts of
[EliteIntel](https://github.com/SudoKrondor/EliteIntel) into this app, and cover the
four Inara workflows named — trade/market routes, Powerplay control state, engineering
and unlocks, system/station lookup.

---

## Decisions

Settled 23 Aug 2026.

1. **Database scope — populated systems + stations.** Both Spansh dumps,
   `galaxy_populated.json.gz` (4.33 GB) and `galaxy_stations.json.gz` (4.30 GB),
   ~8.6 GB downloaded once. Covers every station, market, outfitting, shipyard and
   Powerplay system. Uninhabited systems fall back to EDSM. The full `galaxy.json.gz`
   is 115.97 GB compressed and is out of scope — roughly a terabyte of JSON to parse.
   *(Sizes read from the download headers, not estimated. On-disk SQLite size is
   unmeasured — establish it early in Phase 2.)*

2. **EDDN ingest — everything, latest state only.** Subscribe to all schemas
   (`commodity/3`, `outfitting/2`, `shipyard/2`, `journal/1`, signals). Upsert in
   place so each station and system holds only its current value; disk stays roughly
   flat at bootstrap size. `journal/1` is what keeps Powerplay control state current
   for systems you have not personally flown to. No price history — if the trade
   solver later needs "is this price normal?", that is an additive change.

3. **Overlay — one window, click-through by default.** Always visible, always passes
   clicks through so it can never steal input. A global hotkey
   (`tauri-plugin-global-shortcut`) flips it interactive via
   `set_ignore_cursor_events` and back.

4. **Voice — local TTS, speech out only.** No STT. Runs in-process in the Tauri
   backend via ONNX; no Python runtime, no sidecar. Default engine
   [Piper](https://crates.io/crates/piper-tts-rs) (~40 ms to first audio, RTF 0.03,
   synthetic timbre that suits a ship computer), with
   [Kokoro-82M](https://lib.rs/crates/kokoroxide) swappable behind the same trait —
   both are ONNX, so supporting both costs almost nothing and the choice is made by
   ear. New crate: `ed-voice`.

5. **LLM — cloud Anthropic, provider-agnostic tool layer.** Tools sit behind a trait
   so another backend drops in without a rewrite. Reasoning: this app is built on
   multi-step tool-calling, which is exactly where local models are weakest, and a
   dropped tool call means the model answers from training — the failure this codebase
   exists to prevent.

6. **The AI is the primary interface, not a panel.** Its job is to interpret a
   natural-language request and compose the tool library to answer it — not to
   summarise output. This inverts the usual build order: capabilities are designed as
   composable, well-described tools first, and UI panels are added only where a fixed
   layout genuinely beats asking. See §4.1.

Consequence of 4 and 6: every AI answer returns two fields — a short spoken line and
the full detail. TTS reads the first; the overlay and main window show the second. This
keeps prose that reads badly aloud out of the speech path.

Two smaller things being done alongside: `frontend/node_modules` needs an
`npm install`, and `ai.rs` pins `claude-sonnet-4-5`, which is a generation behind.

---

## 1. What exists today

| Component | State |
|---|---|
| `ed-journal` | Materials snapshot+delta replay, `Cargo.json`, `Status.json`, FDevIDs name resolution. 8 tests. No GUI deps. |
| `ed-engineering` | 1,172 vendored EDEngineer blueprints, 63 module types, 265 experimental effects, cumulative grade requirements, inventory gap diff. Tested. |
| `src-tauri` | 7 commands, debounced `notify` watcher emitting `journal-changed`, Anthropic tool-calling loop with 2 live tools and 2 `not_implemented` stubs. |
| `frontend` | Svelte 5. Status, Engineering Planner, Ship Computer panels. |

In `../`: `eddn_listener.py` (ZeroMQ subscriber → `market_cache.json`, 3 stations),
`aisling_trade_loop.py` (Powerplay merit cycle solver over hand-scouted constants),
`ed_inventory.py` / `fdevids.py` (superseded by the Rust crates), `requirements/*.json`
(tech-broker unlocks — a shape `ed-engineering` doesn't model).

---

## 2. The merit formula — resolved (mostly)

**Status: solved in Phase 1.** Implemented in `ed-store::merits`, calibrated from
78,271 backfilled events. The finding contradicts every published model.

Merits are **linear in credit profit**, not a square root of anything:

```
merits = floor(profit / K)
```

At Herzog Prospect that single relation reproduces all 8 sales exactly, across 8
commodities and three orders of magnitude of profit (12,882 to 969,355 credits), with
`K` pinned to ±0.137%. A sqrt law cannot do that.

| Station | System | State | Samples | K |
|---|---|---|---:|---|
| Herzog Prospect | Uterni | Unoccupied | 8 | **1330.62** ±0.137% |
| Hopper Horizons | Col 285 WU-G a40-5 | Exploited | 5 | **8372.70** ±0.045% |
| Ramon City | Paesia | Stronghold | 13 | *no single K* |
| Hickam Orbital | Bodia | Unoccupied | 1 | 9.04 — different mechanic |

Both prior models are refuted, and there are regression tests pinned to these numbers
so neither can quietly come back:

- `70·√tons` (this project's Python solver) fit only the sale its constant was derived
  from, missing the rest by 4–6×.
- `0.375219·√profit` (wiki, Frontier forums) misses **every** observed sale by 0.5×–3.6×.

**Also corrected:** the earlier caveat that "one loop appears five times across the
files" was wrong — an artifact of the ad-hoc grep pipeline, not the data.
`query::duplicate_sales` reports zero duplicates; those were five genuine repeat runs
of the same loop, minutes apart.

### Still unresolved

- **What sets `K`.** It varies by station across a 6× range for ordinary trade.
  Candidate drivers: control state, control progress, station type.
- **Ramon City contradicts itself.** Two Platinum sales at the same station imply
  K = 4781 and K = 4092. Either `K` drifts with control progress, or some awards split
  across several `PowerplayMerits` events that the 5-second attribution window
  mis-assigns. Needs more sales to separate those.
- **Powerplay delivery commodities are a separate mechanic.** `hr7221wheat` at Hickam
  Orbital yielded K ≈ 9 — roughly 150× more merits per credit than trade. Trade-merit
  reasoning must never be applied to those.

Because of this, `MeritModel::estimate` returns `None` for an unseen or inconsistent
station rather than falling back to an average. An average across a 1330–8373 spread
would be wrong everywhere, and a route planner that guesses `K` produces confident,
specific, wrong advice.

**Separate mechanic, same section:** merits are computed per transaction, not per ton.
Community testing puts single-unit sales at 4 merits/unit, with per-unit yield falling
as batch size grows. Every sale observed so far was a single transaction, so the split
curve is not yet measurable from this journal — it needs deliberate experiment.

## 3. Data sources

| Source | Gives | Access | Use |
|---|---|---|---|
| Your journal | Powerplay control state and progress, materials, cargo, loadout, market history, merits, scans, missions, nav route | Local files | Primary. Authoritative for anywhere you have been. |
| EDDN | Galaxy-wide live market, outfitting, shipyard, journal broadcasts | `tcp://eddn.edcd.io:9500`, no key | Primary. Keeps the DB current. |
| Spansh dumps | Galaxy + station snapshot for bootstrapping | HTTPS bulk download, nightly | Yes. Published bulk endpoint. |
| [Ardent](https://api.ardent-insight.com/v2/) | Importers/exporters, nearest station with service, trade orders | REST, no key, no enforced rate limit | Yes. Default HTTP source. |
| EDSM | System coordinates, bodies, stations | REST, no key | Gap-filling. |
| Spansh live API | Neutron routing, carrier routing | Undocumented job-submit + poll | Narrowly, routing only. Honest User-Agent, cached, degrades to unavailable. |
| Inara | — | Thin CMDR-profile API | No. The tooling you use the site for is not in its API, and I will not scrape it. |

Two notes on the last two rows. EliteIntel reaches Spansh with a spoofed Firefox
User-Agent plus `Origin`, `Referer` and `Sec-Fetch-*` headers; it works and much of the
ecosystem does it, but it is not a stable contract and is not how I want this app
identified. And EliteIntel has no Inara integration at all — its search layer is
`edsm/`, `spansh/`, and a massacre-stacking helper at `iniv.space`.

---

## 4. Architecture

```
SOURCES  journal (local, live)   EDDN (live)   Spansh dumps (bootstrap)   Ardent/EDSM (on demand)
                   |                  |                |                          |
                   +------------------+----------------+--------------------------+
                                      v
STORE    ed-store — embedded SQLite. systems, stations, markets, outfitting, powerplay.
                    Journal data wins on conflict: first-hand and timestamped.
                                      v
CRATES   ed-journal*   ed-engineering*   ed-market   ed-route   ed-intel   ed-voice
                                      v
SHELL    src-tauri — commands + AI tool registry (one registration, two consumers)
                     LLM behind a provider trait; answers carry {spoken, detail}
                                      v
SURFACE  main window        HUD overlay        ship computer        local TTS
                                                (click-through)      (Piper/Kokoro)

* exists, extend
```

New crates: `ed-store` (schema, bootstrap, ingest), `ed-eddn` (ZeroMQ subscriber and
schema decoding), `ed-market` (price and importer/exporter lookup), `ed-route` (cycle
solver), `ed-intel` (HTTP clients), `ed-voice` (ONNX TTS behind an engine trait).

### 4.1 The tool surface is the product

Per decision 6, the model is how you ask for things, so **the tool registry is the real
API and the UI is a convenience over it.** That changes how every phase ships.

Each phase registers its capability as tools at the same time it builds them, not as a
Phase 7 afterthought. Concretely:

- **Granularity is chosen for composition, not for panels.** `get_system_info` and
  `find_stations_selling` compose into a hundred questions; one `plan_my_evening` tool
  answers exactly one and can't be recombined.
- **Descriptions and schemas are load-bearing.** They are the only documentation the
  model gets. A vague description is a silent capability loss — the tool exists and
  never gets called. Treat them like public API docs.
- **Tools return structured data, not prose.** The model chains outputs into the next
  call. Prose terminates a chain.
- **Multi-step composition is the normal case.** "Is it worth running my loop tonight
  or should I mine?" needs ship status, market prices, Powerplay state and the route
  solver in one turn. The current `MAX_TOOL_ROUNDS = 5` cap and serial execution both
  need revisiting; parallel tool calls within a round matter once there are 20+ tools.
- **Every tool states its own staleness.** EDDN data can be days old (§7). A tool that
  returns a price without its age lets the model present stale data as current.

This is also the strongest argument for decision 5: composing many tools across several
rounds is precisely what local models do worst.

---

### 4.2 Worked example

> *"I'd like to engineer my power plant to grade 5. What's the consensus for best?
> Can you show me any reference builds?"*

This is the target interaction, and decomposing it is the best test of the tool design.
Four lookups, three already local:

| Step | Tool | Source | Status |
|---|---|---|---|
| Which power plant do I have? | `get_loadout` | `Loadout` event | Data on disk; tool not built |
| What blueprints exist for it? | `list_blueprints` | `ed-engineering` | **Works today** |
| Do I have the engineer? | `get_engineer_status` | `EngineerProgress` | Data on disk; tool not built |
| Do I have the materials? | `get_engineering_gap` | `ed-journal` + `ed-engineering` | **Works today** |
| What's the consensus? | `search_community` | Web | **Not in the plan — see §5.1** |
| Reference builds? | `fetch_build` | Coriolis / EDSY | **Not in the plan — see §5.1** |

Run against the current save, the local half already produces the answer that matters
most, and it is a negative one:

- Power Plant has three graded blueprints — Armoured, Low Emissions, Overcharged — plus
  four experimental effects (Double Braced, Monstered, Stripped Down, Thermal Spread).
- **All three require Etienne Dorn or Hera Tani at grade 5, and neither is unlocked —
  neither is even listed as Known.** Grade 5 is currently impossible.
- Marco Qwent (unlocked, rank 5) reaches **grade 4** on all three. Felicity Farseer
  reaches grade 1.

So the correct response leads with "you can't, and here's the nearest thing you can do",
before any discussion of which blueprint is best. That check costs one journal read and
prevents a wasted trip — and no external site knows your unlock state, so nothing but
this app can make it.

Note also that 838 of 1,172 blueprints carry a `CoriolisGuid`, which is a direct join
key from local blueprint data to Coriolis reference builds.

### 4.3 Journal ingest: backfill once, then tail

The current `read_all` re-parses six journal files on every command *and* every
file-change event. That is not a performance footnote — it caps what the app can be.
Nothing can ask a question that needs history, because history is never assembled.

Replace it with a one-time backfill into a schema, then an incremental tail:

1. **Backfill.** Parse all 51 journal files once into `ed-store`. Roughly 60k events.
2. **Checkpoint.** Persist `(file_name, byte_offset)` after each batch.
3. **Tail.** On the `notify` event, seek to the stored offset in the newest file and
   read only the appended bytes.
4. **Resume.** On startup, continue from the checkpoint. Steady-state cost is bytes
   since the last event, not six files.

Two file classes, handled differently:

| Class | Files | Behaviour | Handling |
|---|---|---|---|
| Append-only | `Journal.*.log` | Grows, then rolls to a new file per session | Byte-offset tail |
| Rewritten whole | `Status.json`, `Cargo.json`, `Market.json`, `NavRoute.json`, `Outfitting.json`, `Shipyard.json`, `ShipLocker.json`, `Backpack.json` | Replaced in full on change | Re-read and upsert |

**Schema — event log plus derived state.** Both, not either:

- `events` — append-only, one row per journal line, with `(file, offset)` as the natural
  key, plus indexed `ts`, `event`, `system_address`, `market_id` and the raw JSON.
- Derived tables materialised from it: `materials`, `cargo`, `engineers`, `loadout`,
  `powerplay_observations`, `market_observations`, `sales`, `merit_events`.

Keeping the raw event log matters for a specific reason: when derivation logic turns out
to be wrong — and §2 shows it will — the fix is a re-derive from local rows, not a
re-parse of files, and not a lost history. It also makes §2's calibration a SQL join
instead of the ad-hoc pipeline that produced its caveats.

**Correctness details that will bite if skipped:**

- **Idempotency.** Re-applying an event must be safe. `(file, offset)` as the key
  dedupes a re-read of the same file while preserving genuinely repeated events.
- **Duplicate detection.** §2 saw one Cryolite sale appear five times. Once ingested,
  whether that is five real sales or overlapping file content is a query, not a guess.
  Resolve it before the calibration depends on it.
- **Rollover.** A new session writes a new `Journal.*.log`. The tailer must notice the
  new file and switch, not sit on the old one.
- **Truncation.** If the checkpointed file is now smaller than its stored offset, the
  file was replaced. Re-read it rather than seeking past the end.
- **Partial lines.** A tail can catch a half-written line mid-flush. Only commit
  complete lines; leave the remainder for the next read.
- **Schema migrations.** Backfill is expensive enough to want a version number and a
  re-derive path from `events`, rather than asking the user to re-ingest.

## 5. Verifying game mechanics

No API returns the merit multiplier, which rings hold Painite, or what Selene Jean
wants before she will see you. That lives in wikis, subreddits, forum threads and other
people's tools — and as §2 shows, it is frequently wrong.

The rule `catalog.rs` already follows for names extends to mechanics: every game
constant carries a cited source and a regression test, and is validated against your
journal before anything depends on it.

| Constant | Source | Validated against |
|---|---|---|
| Powerplay merit rules | Wiki, Frontier forums, r/EliteDangerous | Your 73 sales |
| Mining hotspots, ring types | r/EliteMiners, edtools.cc, hotspot maps | `ProspectedAsteroid`, `MiningRefined` |
| Engineer unlocks and referrals | Wiki, EDEngineer, Inara engineer pages | `EngineerProgress` (131 events) |
| Tech broker unlocks | Xeno Strike Force, wiki, your `requirements/*.json` | Names resolved through `Catalog` |
| Material trader rates | Wiki, EDEngineer, community calculators | `MaterialTrade` (75 events) |
| Exploration and bio values | Canonn, r/EliteExplorers, EDSM | `Scan`, `SAAScanComplete`, payouts |
| EDDN and journal schemas | EDCD published schemas | Live events — the one authoritative source |

Constants that cannot be validated yet are labelled estimated or unknown in the UI
rather than presented as fact — the same reasoning as `(unknown: symbol)` appearing on
screen instead of silently reading as zero.

### 5.1 Live research tools

§5 covers constants vendored at build time. §4.2 exposes a second, different need:
questions like *"what's the consensus for best?"* whose answer is opinion, changes with
the meta, and lives on Reddit, forums and build sites. That cannot be vendored, and it
must not come from the model's training — an LLM asked about the best power plant
blueprint will answer fluently and unverifiably, which is the exact failure this
codebase is built to avoid.

Two tools:

- **`search_community(topic)`** — actually fetches sources and returns excerpts with
  URLs and dates. The model summarises what came back; it does not answer from memory.
  If the fetch returns nothing, the honest answer is "I couldn't find current
  discussion", not a confident recommendation.
- **`fetch_build(url)`** — parses a Coriolis or EDSY link into a structured module and
  blueprint list, so a referenced build can be diffed against your actual `Loadout` and
  costed through `gap_report`. This is where a reference build stops being a link and
  becomes a shopping list.

**Trust tiers.** Every answer marks which tier each claim came from, because mixing them
silently is how a grounded app starts producing confident nonsense:

| Tier | Source | Presentation |
|---|---|---|
| Fact | Your journal, vendored blueprint data | Stated plainly. "You have 27 Propulsion Elements." |
| Sourced opinion | Fetched community pages | Attributed and dated. "Two threads from June 2026 prefer Overcharged." |
| Unsupported | Model's own knowledge | Not offered. If tools return nothing, say so. |

The engineer-unlock check in §4.2 is the clearest illustration of why the tiers matter:
community consensus will say Overcharged G5, and for this commander that advice is
useless until an engineer is unlocked. Only tier 1 knows that.

Open sub-decision, not blocking: whether `search_community` hits a search API, a fixed
allowlist of known-good sources (wiki, Coriolis, EDSY, specific subreddits), or both.
Allowlist is more predictable and less likely to surface a five-year-old thread as
current; a search API covers questions nobody anticipated.

---

## 6. Phases

Each leaves the app working. The order is a dependency chain.

Phases 0 and 1 are local infrastructure with no UI. From Phase 2 on, each registers its
capability as tools in the same pass that builds it, per §4.1 — "Ships" lines name the
crates; assume tools alongside.

### Phase 0 — journal ingest (no UI)

The schema and pipeline from §4.3: `ed-store` with the `events` table and derived
state, a one-time backfill of all 51 files, `(file, offset)` checkpointing, and an
incremental tail wired to the existing `notify` watcher. Whole-file JSON companions
(`Status.json`, `Cargo.json`, and the rest) re-read and upsert.

Everything else in the plan reads from here, so it goes first. It is also entirely
local — no network, no external schema to track, and the existing `ed-journal` parsers
are reused rather than rewritten.

Ships: `ed-store` (journal half), `ed-journal` ingest adapters.

### Phase 1 — Powerplay parse and merit calibration (no UI)

Now a query problem rather than a scripting problem. Parse and test the Powerplay
fields on `FSDJump` / `Location` / `CarrierJump` into `powerplay_observations`. Then
resolve §2 against `sales` joined to `merit_events` and the selling system's control
state — including settling whether the five identical Cryolite rows are real repeats or
file overlap, which the ingest makes checkable.

Derive the merit model and the transaction-split curve, with an honest confidence
interval and its sources recorded. Also: fix the `claude-sonnet-4-5` pin in `ai.rs`.

Ships: `ed-journal::powerplay`, `ed-journal::merits`.

### Phase 2 — galaxy database

Bootstrap `ed-store` from `galaxy_populated.json.gz` and `galaxy_stations.json.gz`
(~8.6 GB, decision 1). A Rust EDDN subscriber keeps it live across all schemas,
latest-state upsert (decision 2). Retires `eddn_listener.py` and `market_cache.json`.
Establish real on-disk size here — it is currently unmeasured.

Journal data wins on conflict: it is first-hand and timestamped.

Ships: `ed-store` (galaxy half), `ed-eddn`.

### Phase 3 — system and station lookup

Search any system or station: controlling power, control state, security, pad size,
services, distance to star, outfitting and shipyard stock. Your own visits answer from
Phase 0's tables; everywhere else from the galaxy DB, then Ardent/EDSM.

Ships: `ed-intel`, commands, UI panel.

### Phase 4 — trade routes and merit solver

Port `aisling_trade_loop.py` to Rust against live data. Keep what was right: pad
filtering that fails closed, supply/demand caps, excluded-station reporting. Use Phase
1's merit model and split curve. Powerplay filtering comes from the journal. Spansh
finds best paths but will not close a loop; Inara filters by power but only does
two-station round trips — the loop solver is the gap.

Ships: `ed-route`, `ed-market`.

### Phase 5 — HUD overlay and voice callouts

Second Tauri window: transparent, undecorated, always-on-top, click-through via
`set_ignore_cursor_events`, global hotkey to make it interactive (decision 3). Shows
current system with controlling power and state, jumps remaining from
`FSDTarget.RemainingJumpsInRoute`, next star's `StarClass` flagged for scoopability,
fuel, cargo, nearest engineering gap. Requires Elite in borderless.

Voice lands here rather than with the ship computer, because the callouts worth hearing
are journal-driven and need no LLM: fuel below threshold, non-scoopable star next in
route, jump complete, cargo full. `ed-voice` wraps an ONNX engine behind a trait, Piper
by default (decision 4).

The overlay is also what makes Phase 0 pay off — a polling overlay against the old
six-file re-read would have been unworkable.

Ships: Tauri window, Svelte overlay, `ed-voice`, `tauri-plugin-global-shortcut`.

### Phase 6 — engineering, unlocks, research

The phase that makes §4.2 answerable end to end.

Local half: `get_loadout` and `get_engineer_status` (unlocked / invited / known / not
known, with rank) — both now simple reads against Phase 0's derived tables. Plus
tech-broker unlocks in the `requirements/*.json` shape, and, using the galaxy DB, where
to buy or trade for what you are short.

Research half: `search_community` and `fetch_build` per §5.1, with trust tiers enforced
in the answer format. `search_community` is generic infrastructure and could land
earlier if something wants it; `fetch_build` pairs with loadout work, since its value is
diffing a reference build against your ship and costing the difference.

Ordering within the phase: unlock checks before recommendations. A blueprint you cannot
access is not a recommendation, and that check is one table read.

Ships: `ed-engineering`, `ed-intel` (research clients).

### Phase 7 — orchestration

By now the tools exist, registered phase by phase. This is the work of making the model
good at using them together: raising `MAX_TOOL_ROUNDS`, running independent calls in
parallel within a round, and writing the system prompt and tool descriptions that make a
twenty-tool surface navigable rather than confusing.

Fills remaining gaps from EliteIntel's query surface — exploration and bio-scan value,
fleet carrier status and routing, mission stacking, loadout analysis — and retires the
two stubs. Same rule as now: the model calls local functions, never answers from
training.

Answers return `{spoken, detail}`; `ed-voice` reads the spoken line. The provider trait
lands here, so a second LLM backend is a new impl rather than a refactor.

Evaluation matters more here than anywhere else: a regression suite of real questions
with known-correct tool sequences, so a prompt or schema change that quietly stops a
tool being called gets caught. Without it, capability loss is invisible.

Ships: tool orchestration, provider trait, spoken-answer shape, eval suite.

## 7. Known risks

- **EDDN is opportunistic.** A station updates only when another player running EDMC
  docks there. Quiet systems go stale for days. The UI must show data age.
- **Community knowledge is often wrong.** §2 is the proof — a specific, widely-cited
  figure that misses actual sales by up to 3.6×. Frontier also changes mechanics without
  documenting them.
- **Spansh can change without notice.** Undocumented means no deprecation warning.
  Confined to routing, cached, every call site degrades rather than breaking a panel.
- **Backfill is a one-time cost that will feel slow.** Roughly 60k events across 51
  files on first run, and it grows with playtime. Needs progress reporting and a
  resumable batch loop, or it reads as a hang on first launch.
- **Overlay needs borderless.** Exclusive fullscreen will not composite an overlay.
  Setup note, not a code fix.
- **Dump size.** See open question 1.
