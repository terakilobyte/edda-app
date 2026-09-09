# API-only client — client half (Phase B.1, B.2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remote trade search reads the server's `ProfitReport` whole (round trips and rings included) and the panel opens on round trips when the best loop beats the best leg; the station lookups and the fuel-trap guard get their API half.

**Architecture:** `remote_trade.rs` sends the client's resolved plan (ship + constraints) as the v2 request and deserializes the report; nothing is computed client-side any more. A pure `pickView(report)` decides the opening tab. `remote_lookup.rs` gives `stations_in_system` / `find_station` / `nearest_service` a `/v1/stations` path with the same `None → local` fallback shape as `remote_trade` (until Phase B.3 deletes the choice). The trap guard reads star classes through a `StarField` trait — the local index when it holds the target, `/v1/knowledge/sphere` otherwise.

**Tech Stack:** Rust (tauri v2, reqwest), Svelte 5 + vitest, `ed-route`, `ed-store`.

**Spec:** `docs/superpowers/specs/2026-09-07-api-only-client-design.md`. Server half: `docs/superpowers/plans/2026-09-07-api-only-server.md` (branch `api-only-server`; the client's v2 request needs that server or it falls back to local).

## Global Constraints

- Tests must not speak (`test_state` mutes the voice).
- "Commander" only in any user-facing string.
- Rust edits in this worktree, `CARGO_TARGET_DIR` = `<a target dir outside the tree>`.
- The maintainer flies before anything ships: the deliverable of B.1 is a release build he can install, not a release.
- Frontend tests: `.data/tools/node` (portable Node, no system Node) — `cd frontend && ../.data/tools/node/npm test` (or the `npx vitest run` equivalent through that Node).

---

### Task 1: `remote_trade` reads the report

**Files:**
- Modify: `src-tauri/src/remote_trade.rs` (rewrite: request v2, deserialize, drop `compose_leg`/`station_ref`/client pairing)

**Interfaces:**
- Consumes: `POST /v1/trade/search` v2 (server plan Task 5): `{system, ship, constraints, from_station_id, limit}` → `ProfitReport` JSON + `provenance`.
- Produces: `pub async fn try_server(state, req) -> Option<Result<ProfitReport, CapError>>` (unchanged signature); `fn parse_report(value: &serde_json::Value) -> Option<ProfitReport>` (None for the legacy shape → caller falls back to local); `stays_local` keeps only docked-station sourcing local.

- [ ] Write the failing tests (`stays_local` now lets rings and powerplay go remote; `parse_report` round-trips a report and rejects the legacy legs shape).
- [ ] Run `cargo test -p edda remote_trade` → fails (tests reference `parse_report`; `stays_local` expectations flipped).
- [ ] Rewrite the module as in the file (see the committed source).
- [ ] Run `cargo test -p edda remote_trade` → passes; `cargo check -p edda` clean.
- [ ] Commit: `edda: remote trade search reads the server's report — round trips and rings come from the API, nothing is paired here`.

### Task 2: Open on round trips when the best loop beats the best leg

**Files:**
- Create: `frontend/src/lib/tradeView.js` — `export function pickView(report)`.
- Create: `frontend/src/test/tradeView.test.js`.
- Modify: `frontend/src/lib/trade.svelte.js:118` and `frontend/src/lib/AiChat.svelte:48` — `trade.view = pickView(trade.report)` after a report lands.

**Interfaces:**
- `pickView(report)` → `"trips"` when `report.round_trips[0].profit_per_hour > report.legs[0].profit_per_hour_repeat` (legs are ranked by the repeating rate the panel headlines), `"rings"` when the best ring beats both, else `"legs"`. Null/empty → `"legs"`.

- [ ] Write the vitest: legs only → legs; loop 1.2× the leg → trips; ring above both → rings; equal → legs.
- [ ] Run `npm test` → fails (module missing).
- [ ] Implement `pickView`; wire both call sites.
- [ ] Run `npm test` → passes.
- [ ] Commit: `Trade: open on round trips when the best loop beats the best leg (maintainer, 2026-09-07)`.

### Task 3: Station lookups get their API half

**Files:**
- Modify: `crates/ed-store/src/lookup.rs` — `Deserialize` on `StationInfo`, `StationWithService`, `Provenance`, `SystemInfo`, `NearbySystem`.
- Create: `src-tauri/src/remote_lookup.rs` — `pub async fn stations_in_system(state, req) -> Option<Vec<StationInfo>>`, `find_station`, `nearest_service` → `Option<(String, Vec<StationWithService>)>`; each `None` when the choice is not remote, the server declines, or the shape does not parse.
- Modify: `src-tauri/src/capabilities/galaxy.rs` — the three functions try the remote path first when `state.remote_data()` says so (same gate as `remote_trade`), else local.

**Interfaces:**
- Consumes: `GET /v1/stations` (server plan Task 6). Rows deserialize straight into `StationInfo` (extra `distance_ly` ignored / used for `StationWithService`).

- [ ] Tests: a canned `/v1/stations` row deserializes to `StationInfo` with `class` and `max_pad`; a `near` row becomes `StationWithService { distance_ly }`.
- [ ] Implement; `cargo test -p edda remote_lookup`.
- [ ] Commit: `edda: stations_in_system, find_station and nearest_service ask /v1/stations on the remote choice`.

### Task 4: The trap guard reads the sphere

**Files:**
- Modify: `src-tauri/src/trap.rs` — `trait StarField { fn find(&self, name) -> Option<u32>; fn pos_of(&self, idx) -> [f32; 3]; fn class_of(&self, idx) -> StarClass; fn scoopable(&self, idx) -> bool; fn within(&self, pos, radius) -> Vec<(u32, f32)>; fn name_of(&self, idx) -> String }`, implemented for `ed_galaxy::Galaxy` and for `SphereField` (built from one `/v1/knowledge/sphere` answer around the target: `name`, `id64`, `primaryStar.type` → `StarClass::from_subtype`).
- `on_target`: bundle first (`g.find(name)` is `Some`), else `SphereField::fetch(state, target_coords_from /v1/knowledge/system, CELL_LY)` with a 1.5 s timeout through `tauri::async_runtime::block_on` (precedent: `tools.rs:1024`); the radius the guard asks for is capped at 100 ly on the API path and the callout says "within 100 ly" when capped.

- [ ] Tests: `SphereField` from a canned sphere answers `class_of` / `scoopable` / `within` / nearest scoopable; `assess` unchanged.
- [ ] Implement; `cargo test -p edda trap`.
- [ ] Commit: `edda: the fuel-trap guard reads star classes from the API when the bundle does not hold the target`.

### Task 5: A build the maintainer can fly

- [ ] `cargo test` for the workspace (ed-route, ed-domain, ed-store, ed-api lib, edda) green.
- [ ] Windows release build from this worktree (no publish): `scripts/release-app.ps1 -SkipUpload -AllowUntagged` — the installer lands under `.data/api/artifacts/app` locally only.
- [ ] Ledger: Phase B.1/B.2 built; what the flight must show (round trips on remote; the panel opening on trips; a low-tank jump past the bubble edge gets the callout).

## Self-review

Spec coverage: B.1 (Tasks 1–2), B.2 lookups (Task 3), B.2 trap (Task 4), the flight gate (Task 5). B.3/B.4/C are out of scope by design (each needs its own flight). `find_system` / `systems_near` stay local until B.3: `/v1/knowledge/sphere` lacks power/population and enriching it is a server task filed in the ledger.
