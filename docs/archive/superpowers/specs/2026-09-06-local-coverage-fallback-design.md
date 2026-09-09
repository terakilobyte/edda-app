# Local data, honest gaps, consented fallback — design (0.2.6 gate)

Status: maintainer-assigned 2026-09-06 ("figure out how to do graceful api
fallback for if a user wants to use local data but they don't have info
on something. That way this doesn't happen again."), approved to
implement in the same breath. review. Builds in the docs worktree.

## 0. The problem, precisely

Local mode today cannot tell **"I have no data about this"** from
**"there is nothing there."** Both render as an empty list, and the
second is a claim EDDA is not entitled to make. Two real reports
prompted it: a squadron carrier with no market shown (its board was
never broadcast on EDDN, and EDDA's own docked board was never applied
— Item 57), and a daily-anchored install carrying only the seven-day
window. The distinction is the feature; fallback is one response to it.

Measured first (rule 1), from the code map:

| Path | Honors the local/remote choice today? | Empty local answer means |
|---|---|---|
| Route (`routing.rs:1103`) | yes; missing local index forces the API | n/a |
| Trade (`remote_trade.rs:37`) | yes | "no profitable legs"; `Excluded.no_market_data` is computed and **not rendered** (`TradePanel.svelte:156`) |
| Market search (`commands.rs:1137`) | **no — server-first unconditionally** | byte-identical payload for "no coverage" and "nothing sells it" |
| Station board (`commands.rs:1322`) | **no — local-first, silent server call on any empty board** | the one place that already treats empty as unknown, silently |
| Ship computer tools (`tools.rs:848/874/944`) | never see the choice or the server | the model's only honest option is "nothing", the maintainer's exact complaint |

So the fallback design is also a consistency fix: one rule for all four
paths, and the same rule for the model as for the panels.

**Coverage signal that already exists:** `galaxy.sys_market_watermarks`
(`schema.rs:381`) is written for every station whose board was ever
observed, including empty boards ("I looked, and there was nothing").
A station with `has_market = 1` and **no watermark row** has never been
observed. That is the gap signal, and it is free.

## 1. Rulings folded in

- **Item 57 (maintainer):** the commander's own `Market.json` is applied to
  `sys_market` at startup and on every dock. First-hand before fallback.
- **Consent — BOSS-RULED 2026-09-06, overruling the first draft:** "it
  should just work delightfully no matter what. Fallback should not be
  opt-in." Default policy is **always**: a local-mode empty answer over
  a gap goes to the community API automatically and the answer says
  so; the empty state names the gap either way (and, when the API could
  not answer, says that too, with a retry). `ask` and `never` remain
  as settings under Data source. The privacy page states it. Remote-mode
  users are unaffected.
- **Measure the miss before the fix (rule 1):** a coverage-gap counter
  ships first and independently, as `tracing::warn!` targets — counted
  by the existing callsite pipeline with zero wire change.

## 2. Coverage: one type, computed locally, carried on every answer

`ed_store::coverage` (new module):

```rust
#[derive(Serialize)] #[serde(tag = "status", rename_all = "snake_case")]
pub enum Coverage {
    /// Enough observed boards to trust an empty answer.
    Covered { observed: u32, stations: u32 },
    /// Empty cannot be trusted; `kind` is closed and telemetry-safe.
    Gap { kind: GapKind, observed: u32, stations: u32, detail: String },
}
pub enum GapKind {
    StationNeverObserved,   // has_market row, no watermark
    NoObservedInRadius,     // stations in radius, none with a watermark
    StationUnknown,         // no sys_stations row for this id
    NoLocalIndex,           // route/galaxy: bundled index absent
}
```

- `station(conn, station_id)`: unknown row → `StationUnknown`; row with
  `has_market = 1` and no watermark → `StationNeverObserved`; otherwise
  `Covered{1,1}`.
- `radius(conn, coords, r_ly, include_carriers)`: count `has_market`
  stations within `r` (the same bounding-box scan `market_stations_within`
  uses) and how many carry a watermark. Zero observed of N>0 →
  `NoObservedInRadius`; N=0 → `NoObservedInRadius` with `stations: 0`
  (a radius with no market stations at all is unknown, not empty — the
  station table itself may be thin); else `Covered`.
- Coverage is computed **only when the local answer is empty**. A
  non-empty answer is covered by construction. Cost: one indexed count.
- `detail` is a human sentence for the empty state ("none of the 14
  market stations within 40 ly of Deciat has a board on record") and
  never leaves the machine.

## 3. Instrument (ships first)

- On an empty local answer whose coverage is `Gap`:
  `tracing::warn!(target: "edda::coverage_gap::<path>::<kind>", ...)`
  with `path ∈ {market_search, station_board, trade, route}` and
  `kind` the snake-case `GapKind`. Callsites ship by target
  (`observe.rs:373`), so each pair is its own bucket in
  `edda_client_events_total{callsite}`. The message text never travels.
- Feature flag `data_source_remote` joins `telemetry::feature_flags`
  so the fleet's local/remote split is finally measured
  (`edda_client_feature_total{name="data_source_remote"}`).
- Dashboard: an "Honest gaps" row on EDDA Clients — gaps/h by path and
  kind, share of local users, fallback acceptances (see §5). Same day.
- Pre-registered reading: if gaps/h is near zero after a week, the
  fallback UI is a rare-papercut feature and stays; if it is a constant
  drumbeat, the next task is coverage itself (Item 57's reach, the
  daily window), not more UI.

## 4. Item 57: apply the commander's own board

`ed_store::market::apply_own_board(conn, raw_market_json, observed_at)`:

1. Parse `{MarketID, StationName, StarSystem, Items[{Name, Name_Localised,
   Category_Localised, BuyPrice, SellPrice, Stock, Demand}]}`.
2. Ensure the station row: if `sys_stations` lacks `MarketID`, insert it
   from the newest `Docked` event with that `MarketID` in `events`
   (`SystemAddress`, `StationType` → `is_carrier`), falling back to
   `sys_systems` by `StarSystem` name; `has_market = 1`. A squadron
   carrier nobody ever broadcast becomes a station EDDA knows.
3. `intern_commodity(Name, Name_Localised, Category_Localised)` per item
   (canonical symbol strips the `$…_name;` wrapper — the 0.2.4 rule).
4. `write_snapshot(station_id, observed_at, rows)` — newer-wins by the
   station watermark, so a replay never regresses an EDDN-fresher board.

Triggers: at startup (the one board `Market.json` holds), and in the
watcher whenever `snapshots_updated > 0` and the `Market.json` snapshot
`ts` is newer than `sys_meta.own_board_applied_ts`. `observed_at` is
the file's `timestamp`. Boards from before EDDA was installed do not
exist on disk (the `Market` event has no items; `snapshots` keeps one
row per file) — stated in the ledger, not papered over.

## 5. One rule for every path

Config: `AppConfig.api_fallback: Option<String>` ∈ `ask` (default when
`None`) | `always` | `never`. Requests gain `source: Option<String>`
(`auto` default | `local` | `community`) so a UI button or the model
can force one call to the API with the commander's consent.

| Choice | Local answer | Behaviour |
|---|---|---|
| remote | — | server first, local fallback on any non-200 (unchanged) |
| local | non-empty | return it, `coverage: covered` |
| local | empty, covered | return it, `coverage: covered` — "nothing matched" is now a claim EDDA can make |
| local | empty, gap, `api_fallback = always` | call the API for the same query; answer carries `provenance: server`, `fallback: "auto"` |
| local | empty, gap, `ask`/`never` | return empty + `coverage: gap` (+ `offer: true` when `ask`) |
| any | `source = community` | call the API for this one call (consent given in the UI or by voice) |

Applies to market search (fixing the unconditional server-first),
station board (replacing the silent call), trade (`no_market_data ==
candidates` ⇒ gap; `source = community` routes through `remote_trade`
even in local mode), and the ship-computer tools (`station_market`,
`market_search`, `find_profit` gain `source`; results carry `coverage`;
tool descriptions say: "when coverage.status is gap, tell the Commander
EDDA has no local data for this rather than that nothing exists, and
ask whether to check the community API; if yes, call again with
source community"). Route keeps its existing rule (missing index ⇒
API), now labelled `NoLocalIndex` in the answer.

## 6. UI

- Empty states get two shapes. Covered: today's copy. Gap: "EDDA has no
  local data for this — <detail>. That is different from 'nothing
  here'." with **Check the community API** (one call, `source:
  community`) and a checkbox **Always check the community API when
  local data has a gap** (sets `api_fallback = always`; a second
  checkbox state "never" lives in Settings → Data source).
- Answers served by fallback keep the existing `live` pill and add
  "(community API — local data had no coverage)".
- Settings → Data source gains the three-way "When local data has a
  gap: ask / always check / never" under the local card.
- Trade panel renders `no_market_data` in its excluded line.

## 7. Tests (one failing-then-passing per finding)

- `coverage::station`: unknown id → `StationUnknown`; has_market + no
  watermark → `StationNeverObserved`; watermark present, zero rows →
  `Covered` (an empty board is a real answer).
- `coverage::radius`: N stations, 0 watermarks → gap; 1 watermark →
  covered; carriers excluded when `include_carriers = false`.
- `apply_own_board`: fixture Market.json → station row created from a
  Docked event (carrier flagged from `StationType`), commodities
  interned canonically, rows written, watermark stamped; replay with an
  older timestamp is `Skipped`; a later EDDN snapshot still wins.
- `market_search_of`: local choice ⇒ no server call (a contract test
  asserts the local branch never touches `http`), coverage attached on
  empty; `source: community` ⇒ server.
- `station_market`: gap + `ask` ⇒ empty with coverage and **no** HTTP;
  gap + `always` ⇒ API; covered-empty ⇒ empty, no HTTP.
- Telemetry: the anonymity test extended for the new flag; a test that
  a gap emits exactly one warn with the expected target and no message
  content beyond fixed words.
- Frontend: empty-state component renders the two shapes from
  `coverage`; the offer button re-issues with `source: "community"`.
- Eval (`eval.rs`): "what does <never-observed station> sell" expects
  `station_market`, forbids a "nothing" answer wording, expects the
  community offer.

## 8. Out of scope, named

- Applying boards from before EDDA was installed (none exist on disk).
- Uploading to EDDN (a separate feature-shape question under the law).
- Route/galaxy coverage beyond the existing missing-index rule.
