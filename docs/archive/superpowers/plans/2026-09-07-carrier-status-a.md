# Fleet Carrier Status from the Journal (Item 52 A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** EDDA knows the commander's own (or squadron) fleet carrier from the journal alone — location, tank, capacity, pending jump, what the commander moved aboard — every figure carrying its age, and the ship computer answers "how much Tritium is on my carrier?" honestly.

**Architecture:** One derivation module in `ed-store` (`carrier.rs`) replays every `Carrier*` / `CargoTransfer` row already in `events` into two tables (`carriers`, `carrier_hold`), rebuilt on every derive pass (hundreds of rows, trivial). One read function turns them into the JSON the tool, the Tauri command, and the Ships card all share. Callouts get four `Carrier*` arms. The spec is `docs/superpowers/specs/2026-09-06-fleet-carrier-design.md` §3; the fixture shapes are the census in the ledger (2026-09-06 evening).

**Tech Stack:** Rust (rusqlite, serde_json, chrono), Tauri v2 commands, Svelte 5 runes, vitest.

**Rules that came from the fixtures:** never read `CarrierType` from `CarrierNameChange` (empty-string key bug); the stable key is `CarrierID` (== the `Docked`/`CarrierJump` `MarketID`); ownership = `CarrierBuy` seen OR `CarrierStats.CarrierType == "FleetCarrier"`; `CargoTransfer` carries no carrier id — attribute it to the single owned carrier, skip when there is none or more than one.

---

### Task 1: Tables

**Files:**
- Modify: `crates/ed-store/src/schema.rs` (main DDL, after `edsm_sweeps`; bump `SCHEMA_VERSION`)

- [ ] **Step 1: Add the DDL** (inside the main `DDL` string, before the galaxy section)

```sql
-- Item 52 A: the commander's own or squadron carrier, replayed from the
-- Carrier* events. Every value carries the timestamp it was true at.
CREATE TABLE IF NOT EXISTS carriers (
    carrier_id          INTEGER PRIMARY KEY,
    carrier_type        TEXT,
    callsign            TEXT,
    name                TEXT,
    owned               INTEGER NOT NULL DEFAULT 0,
    decommissioned      INTEGER NOT NULL DEFAULT 0,
    system_name         TEXT,
    system_address      INTEGER,
    body                TEXT,
    location_ts         TEXT,
    fuel_t              INTEGER,
    fuel_ts             TEXT,
    capacity_total      INTEGER,
    capacity_used       INTEGER,
    free_space          INTEGER,
    stats_ts            TEXT,
    jump_range_curr     REAL,
    jump_range_max      REAL,
    docking_access      TEXT,
    balance_cr          INTEGER,
    services            TEXT,
    pending_jump_system TEXT,
    pending_jump_body   TEXT,
    pending_departure   TEXT,
    pending_jump_ts     TEXT
);

-- What the COMMANDER moved aboard (CargoTransfer tocarrier minus
-- toship/tosrv, floored at zero): "what you moved", never "what is aboard".
CREATE TABLE IF NOT EXISTS carrier_hold (
    carrier_id INTEGER NOT NULL,
    commodity  TEXT NOT NULL,
    count      INTEGER NOT NULL,
    ts         TEXT,
    PRIMARY KEY (carrier_id, commodity)
);
```

- [ ] **Step 2: Bump `SCHEMA_VERSION`** by one so existing installs re-derive.
- [ ] **Step 3:** `cargo test -p ed-store schema` passes. Commit: `ed-store: carriers + carrier_hold tables (Item 52 A)`.

### Task 2: Derivation module with fixture tests

**Files:**
- Create: `crates/ed-store/src/carrier.rs`
- Modify: `crates/ed-store/src/lib.rs` (`pub mod carrier;`), `crates/ed-store/src/derive.rs` (call `carrier::rebuild(&tx)?` before the watermark is set; add the Carrier events to `DERIVED_FROM` so an incremental pass sees them)

- [ ] **Step 1: Write the failing tests** at the bottom of `carrier.rs` — fixtures verbatim from the census (identifiers replaced), one test per finding:
  - `a_bought_carrier_is_owned_and_named` (CarrierBuy → owned, callsign; CarrierStats → name, fuel 500, capacity; CarrierNameChange with the `""` key → name changes, type unchanged).
  - `a_squadron_carrier_is_known_but_not_owned` (CarrierStats CarrierType=SquadronCarrier → owned 0).
  - `jump_request_then_jump_moves_it_and_clears_the_pending_jump`.
  - `jump_request_then_cancel_clears_the_pending_jump_without_moving`.
  - `carrier_location_is_the_heartbeat` (moves the system, keeps everything else).
  - `deposit_fuel_total_is_newer_than_stats` (fuel 484 after stats 500 when the deposit is later; an older deposit does not regress).
  - `cargo_transfer_sums_per_commodity_and_floors_at_zero` (tocarrier 10, toship 3 → 7; toship 100 → 0).
  - `cargo_transfer_is_skipped_without_exactly_one_owned_carrier`.
  - `status_reports_ages_and_owned_first`.
- [ ] **Step 2: Run** `cargo test -p ed-store carrier::` — fails (module missing).
- [ ] **Step 3: Implement** `rebuild(conn)` (DELETE both tables; SELECT `ts, event, raw FROM events WHERE event IN (...) ORDER BY file, offset`; fold into a `HashMap<i64, Row>` + `HashMap<(i64,String), (i64,String)>`; INSERT) and `status(conn, now) -> Vec<CarrierStatus>` (serde `Serialize`, ages in hours from `now`, owned first).
- [ ] **Step 4: Run** the tests — pass. Commit: `ed-store: carrier derivation from the journal, fixtures from the census`.

### Task 3: Ship-computer tool + Tauri command

**Files:**
- Create: `src-tauri/src/capabilities/carrier.rs` (`pub fn status(state) -> CapResult<Value>`)
- Modify: `src-tauri/src/capabilities/mod.rs` (`pub mod carrier;`), `src-tauri/src/capabilities/tools.rs` (one `ToolSpec` row `get_carrier_status`), `src-tauri/src/commands.rs` (`#[tauri::command] carrier_status`), `src-tauri/src/lib.rs` (register), `frontend/src/lib/api.js` (`carrierStatus`), `src-tauri/src/eval.rs` (case `carrier-tritium`)

- [ ] **Step 1:** tool description: "The commander's own or squadron fleet carrier as the journal last saw it: location, Tritium in the tank, capacity used and free, pending jump with departure time, what the commander moved into the hold. Every figure carries its age in hours — say it. Does not know other players' deposits or sales. If `carriers` is empty, say EDDA has not seen a carrier in the journal yet and suggest opening Carrier Management once."
- [ ] **Step 2:** eval case: `Case { id: "carrier-tritium", question: "How much tritium is on my carrier?", expect_tools: &["get_carrier_status"], forbid_tools: &["get_inventory", "get_ship_status"], must_contain: &["tritium"], .. max_chars: 500 }`.
- [ ] **Step 3:** the tool-contract test (`capabilities/mod.rs`) already asserts definitions and runners agree. `cargo test -p edda capabilities` passes. Commit: `get_carrier_status tool + carrier_status command`.

### Task 4: Callouts

**Files:**
- Modify: `src-tauri/src/callouts.rs` (four arms in `from_event`; `CalloutState.carrier_system: Option<String>`), tests in the same file.

- [ ] **Step 1: Failing tests:** `carrier_jump_request_counts_down` (text contains "minutes"), `carrier_jump_cancelled_says_so`, `carrier_arrival_speaks_once_per_move` (first CarrierLocation silent, a changed system speaks, the same system again silent).
- [ ] **Step 2: Implement** with kind `"carrier"`, priority 1, `speak: true`; `CarrierJumpRequest`: minutes = (DepartureTime − timestamp) via chrono, "Carrier jump to {system} scheduled, departure in {n} minutes."; `CarrierJumpCancelled`: "Carrier jump cancelled."; `CarrierJump` (aboard): "Carrier arrived at {StarSystem}."; `CarrierLocation`: as described. System names pass through `phonetics::speak_system_names` at speech time like every other callout (no change needed here).
- [ ] **Step 3:** tests pass; `callouts_off` already mutes by kind. Commit: `Carrier callouts: jump scheduled, cancelled, arrived`.

### Task 5: Ships tab card

**Files:**
- Modify: `frontend/src/lib/ShipsPanel.svelte` (load `carrierStatus()` on mount and on `ship.currentId`; render a "Carrier" card above the ship list when non-empty), `frontend/src/lib/format.js` if an `fmtAge(hours)` helper is missing (check first).

- [ ] **Step 1:** card shows callsign + name, owned/squadron pill, location "as of N h", tank "N t as of …", capacity used/free, pending jump with departure time, and "moved aboard" lines.
- [ ] **Step 2:** `npx vite build` clean; `npx vitest run` green. Commit: `Ships tab: carrier card`.

### Task 6: Ledger + spec + telemetry note

- [ ] Ledger entry "Item 52 A BUILT"; spec §3 marked built; the pooled `carrier_stats_age_hours` number is HELD client-side until the server maps the kind (a rejected batch blacks out telemetry) — noted in both.

## Self-review
- Spec §3.1–3.6 all map to Tasks 1–6; §3.5's shipped number is deliberately deferred and said so.
- Names used consistently: `carrier::rebuild`, `carrier::status`, tool `get_carrier_status`, command `carrier_status`, api `carrierStatus`, callout kind `carrier`.
