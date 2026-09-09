# API-only client — server half (Phase A.2, A.3, A.5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `/v1/trade/search` returns the complete `ProfitReport` (legs, round trips, rings, exclusions, timings) computed on the server; `/v1/stations` gives the seven local-only lookup tools a wire half; the limiter accepts a per-install key.

**Architecture:** Postgres supplies candidate stations and fresh market rows for the sphere; the Rust pipeline in `ed-route::profit` (`best_legs → round_trips → rings → diversify`) runs on the server over that set through one public seam, `assemble`, so local and remote give the same answer from the same code. `/v1/trade/search` stays backward compatible: a request carrying `ship` gets the report, a request without it gets today's legs. `/v1/stations` is a new module mirroring `ed_store::lookup`'s three station queries with the same response shape.

**Tech Stack:** Rust (axum 0.8, sqlx 0.8 Postgres, serde), `ed-route`, `ed-domain`, `ed-galaxy`; WSL Postgres `edda_dev` (100 M market rows) for benches; `edda_test` for the ignored Postgres tests.

**Spec:** `docs/superpowers/specs/2026-09-07-api-only-client-design.md`

## Global Constraints

- Measure before building (CLAUDE.md doctrine 1): Task 1's bench gates Task 4's default `max_stations`.
- New things ship measurable (doctrine 2): every new endpoint emits `edda_<name>_requests_total{...,outcome}` and `edda_<name>_seconds`, naming per `crates/ed-api/src/metrics.rs`.
- Bench records are CSVs in `docs/benches/` with the verdict in the header; harnesses live in `docs/benches/knobs/` as files.
- **Do not touch `crates/ed-api/src/plot.rs` or `crates/ed-api/src/main.rs`** — the assistant session owns Phase A step 1 and the routing gate (message, 2026-09-07). Routing step 4 (the 422s) is CLOSED on the assistant's evidence: every 422 was `unknown_system` for the stale name `Eol Prou RS-T d3-94` in the harness's FAR list; not a planner defect.
- Rust edits happen in this worktree with `CARGO_TARGET_DIR` = `<a target dir outside the tree>` (PowerShell: `$env:CARGO_TARGET_DIR = ...`).
- Postgres-backed tests are `#[ignore]` and run in WSL: `EDDA_API_TEST_DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_test RUSTC_WRAPPER= cargo test -p ed-api --test postgres -- --ignored <name>` from `~/edda-linux` (fetch this branch there first).
- Nothing deploys. Binaries are staged as `ed-api.new` on the maintainer's word only.
- Address the player as "Commander" only (no user-facing text in this plan needs it, but every string is checked).

---

## File Structure

| File | Responsibility |
|---|---|
| `docs/benches/knobs/trade-rows-pull.sh` (create) | Bench: cost of pulling every fresh row for a trade sphere's candidate stations, three origins |
| `docs/benches/trade-rows-pull-2026-09-07.csv` (create) | The record, verdict in the header |
| `crates/ed-domain/src/station.rs` (modify) | `Deserialize` on `StationClass`, `PadSize` |
| `crates/ed-route/src/cost.rs` (modify) | `Serialize`/`Deserialize` on `Ship`, `Duration`, `Confidence` |
| `crates/ed-route/src/profit.rs` (modify) | `Deserialize` on the report types; `note: String`; `pub fn assemble`; `pub fn silence_prohibited`; `pub struct Prepared` |
| `crates/ed-api/Cargo.toml` (modify) | add `ed-route` |
| `crates/ed-api/src/geo.rs` (create) | `cells_covering` lifted from `market_search.rs` |
| `crates/ed-api/src/market_search.rs` (modify) | use `crate::geo::cells_covering` |
| `crates/ed-api/src/trade_report.rs` (create) | Postgres → `Prepared` (candidates, rows, prohibited, exclusions, timings) |
| `crates/ed-api/src/trade_search.rs` (modify) | `ReportRequest`; `TradeService::report`; clamps; cache key |
| `crates/ed-api/src/stations.rs` (create) | `/v1/stations`: query parsing, service vocabulary, three SQL modes, response shape |
| `crates/ed-api/src/http.rs` (modify) | trade handler dispatch on `ship`; `/v1/stations` route + handler; `source_of` honours `X-EDDA-Install` |
| `crates/ed-api/src/lib.rs` (modify) | `pub mod geo; pub mod trade_report; pub mod stations;` |
| `crates/ed-api/tests/postgres.rs` (modify) | ignored tests: report round trip; stations three modes |
| `crates/ed-api/README.md` (modify) | endpoint docs |
| `docs/ROUTING-NEXT.md` (modify) | ledger: bench verdict, carrier-envelope gap, Routing step 4 closed |

---

### Task 1: Bench — what does pulling the sphere's fresh rows cost?

**Files:**
- Create: `docs/benches/knobs/trade-rows-pull.sh`
- Create: `docs/benches/trade-rows-pull-2026-09-07.csv`

**Interfaces:**
- Produces: the number that sets `DEFAULT_MAX_STATIONS` in Task 4 (2,500 if Wyrd ≤ 1.5 s, else 1,000).

- [ ] **Step 1: Write the harness**

```bash
#!/usr/bin/env bash
# Trade-report row pull (API-only spec, Phase A.2). The server-side
# profit finder needs EVERY fresh row for the sphere's candidate
# stations, not the best-8 per commodity the legacy query keeps.
# PRE-REGISTERED (doctrine 4): candidates + rows for 100 ly / 48 h is
# <= 0.5 s at Deciat and <= 1.5 s at Wyrd (the densest origin we serve).
# If Wyrd is over, the server's default max_stations is 1,000 nearest
# (the local finder caps at 2,500).
#
#   docs/benches/knobs/trade-rows-pull.sh [DATABASE_URL] > docs/benches/trade-rows-pull-YYYY-MM-DD.csv
#
# Each origin runs the candidate query alone, then candidates+rows as one
# COPY to /dev/null (the serialisation the server pays), warm (second of
# two runs is recorded). Timing is wall-clock around psql on the same
# host; the local docker round trip is sub-millisecond.
set -uo pipefail
DB="${1:-postgres://edda:edda@127.0.0.1:55432/edda_dev}"
RADIUS="${RADIUS:-100}"
AGE_H="${AGE_H:-48}"

ms() { local s; s=$(date +%s%N); "$@" >/dev/null 2>&1; echo $(( ($(date +%s%N) - s) / 1000000 )); }

candidates_sql() {
  cat <<SQL
WITH o AS (SELECT x, y, z FROM systems WHERE lower(name) = lower('$1') LIMIT 1)
SELECT st.id FROM stations st JOIN systems sy ON sy.address = st.system_address, o
WHERE sy.x BETWEEN o.x-$RADIUS AND o.x+$RADIUS AND sy.y BETWEEN o.y-$RADIUS AND o.y+$RADIUS
  AND sy.z BETWEEN o.z-$RADIUS AND o.z+$RADIUS
  AND (sy.x-o.x)^2 + (sy.y-o.y)^2 + (sy.z-o.z)^2 <= $RADIUS*$RADIUS
  AND st.has_market AND NOT COALESCE(st.is_carrier, false)
SQL
}
rows_sql() {
  cat <<SQL
COPY (WITH cand AS ($(candidates_sql "$1"))
SELECT m.station_id, m.commodity_symbol, m.buy_price, m.sell_price, m.demand, m.supply,
       EXTRACT(EPOCH FROM now() - m.observed_at) / 3600.0
FROM market m JOIN cand ON cand.id = m.station_id
WHERE m.observed_at > now() - make_interval(hours => $AGE_H)) TO STDOUT
SQL
}

echo "origin,radius_ly,max_age_h,candidate_stations,fresh_rows,candidates_ms,candidates_plus_rows_ms"
for origin in Sol Deciat Wyrd; do
  n_cand=$(psql "$DB" -Atc "SELECT count(*) FROM ($(candidates_sql "$origin")) c")
  n_rows=$(psql "$DB" -Atc "$(rows_sql "$origin" | sed -e 's/^COPY (//' -e 's/) TO STDOUT$//' | sed '1s/^/SELECT count(*) FROM (/' ; echo ') r')")
  for _ in 1 2; do c_ms=$(ms psql "$DB" -Atc "$(candidates_sql "$origin")"); done
  for _ in 1 2; do r_ms=$(ms psql "$DB" -Atc "$(rows_sql "$origin")"); done
  echo "$origin,$RADIUS,$AGE_H,$n_cand,$n_rows,$c_ms,$r_ms"
done
```

- [ ] **Step 2: Run it against edda_dev in WSL**

Run: `wsl -e bash -lc 'cd ~/edda-linux && git fetch -q https://github.com/terakilobyte/edda.git <branch> && git checkout -q FETCH_HEAD && bash docs/benches/knobs/trade-rows-pull.sh'`
Expected: four lines; Wyrd's `candidates_plus_rows_ms` is the number. If `n_rows` prints an error, fix the count query (it is the COPY body wrapped in `SELECT count(*) FROM (...) r`).

- [ ] **Step 3: Record with the verdict**

Write `docs/benches/trade-rows-pull-2026-09-07.csv`: a `#` header naming the binary/DB, the pre-registered expectation, the measured numbers and the verdict (`DEFAULT_MAX_STATIONS = 2500` or `1000`), then the harness output verbatim.

- [ ] **Step 4: Commit**

```bash
git add docs/benches/knobs/trade-rows-pull.sh docs/benches/trade-rows-pull-2026-09-07.csv
git commit -m "Bench: the sphere row pull the server-side profit finder pays (pre-registered; sets the server station cap)"
```

---

### Task 2: The report types cross the wire

**Files:**
- Modify: `crates/ed-domain/src/station.rs:16-18`, `:87-89`
- Modify: `crates/ed-route/src/cost.rs:64-78`, `:88-97`
- Modify: `crates/ed-route/src/profit.rs` (every `#[derive(Debug, Clone, Serialize)]` on the report types; `note`)
- Modify: `src-tauri/src/remote_trade.rs:31` and `:139` (`NOTE` becomes a `String` at the construction site)

**Interfaces:**
- Produces: `ProfitReport: Serialize + Deserialize` (and every type inside it); `ProfitReport.note: String`.

- [ ] **Step 1: Write the failing test** (append to `mod tests` in `crates/ed-route/src/profit.rs`)

```rust
    /// The report goes over the wire whole (API-only spec): every type
    /// inside it must deserialize back to itself.
    #[test]
    fn report_round_trips_through_serde() {
        let station = |id: i64, x: f64| StationRef {
            station_id: id, station: format!("S{id}"), system: format!("Sys{id}"), system_id64: id * 10,
            x, y: 0.0, z: 0.0, arrival_ls: Some(100.0), max_pad: Some(PadSize::Large),
            class: StationClass::of(Some("Coriolis")), is_carrier: false,
            controlling_power: None, power_state: None, powers: Vec::new(),
        };
        let ship = Ship { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 };
        let buy = MarketRow { station_id: 1, symbol: "gold".into(), name: Some("Gold".into()), buy_price: 100, sell_price: 0, demand: 0, supply: 500, age_hours: 1.0 };
        let sell = MarketRow { station_id: 2, symbol: "gold".into(), name: Some("Gold".into()), buy_price: 0, sell_price: 200, demand: 500, supply: 0, age_hours: 1.0 };
        let leg = make_leg(&station(1, 0.0), &station(2, 10.0), &buy, &sell, &ship);
        let report = ProfitReport {
            origin: "Sys1".into(), timing: SearchTiming::default(), constraints: Constraints::default(),
            ship: ShipSummary { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 },
            stations_considered: 2, excluded: Excluded::default(), legs: vec![leg.clone()],
            round_trips: vec![RoundTrip { out: leg.clone(), back: leg.clone(), profit: 2 * leg.profit, profit_per_hour: 1.0, duration: leg.duration }],
            rings: vec![Ring { legs: vec![leg.clone()], stops: 3, profit: leg.profit, duration: leg.duration, profit_per_hour: 1.0 }],
            confidence: Confidence::Estimated, note: "n".into(), coverage: None, offer: None, fallback: None,
        };
        let text = serde_json::to_string(&report).unwrap();
        let back: ProfitReport = serde_json::from_str(&text).unwrap();
        assert_eq!(back.legs[0].profit, leg.profit);
        assert_eq!(back.round_trips.len(), 1);
        assert_eq!(back.rings[0].stops, 3);
        assert_eq!(back.legs[0].from.max_pad, Some(PadSize::Large));
        assert_eq!(back.note, "n");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p ed-route report_round_trips_through_serde`
Expected: compile error — `ProfitReport: Deserialize` not satisfied; `note` expects `&'static str`.

- [ ] **Step 3: Add the derives and change `note`**

In `crates/ed-domain/src/station.rs` change both derive lines to `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]` and make sure `use serde::{Deserialize, Serialize};` is imported.

In `crates/ed-route/src/cost.rs`: `Confidence` and `Duration` derives gain `Deserialize`; `Ship` becomes `#[derive(Debug, Clone, Copy, Serialize, Deserialize)]`. Import `serde::{Deserialize, Serialize}`.

In `crates/ed-route/src/profit.rs`: every `#[derive(Debug, Clone, Serialize)]` / `#[derive(Debug, Clone, Default, Serialize)]` / `#[derive(Debug, Clone, Copy, Default, Serialize)]` / `#[derive(Debug, Clone, Copy, Serialize)]` on `Constraints`, `StationRef`, `Leg`, `CargoLine`, `RoundTrip`, `Excluded`, `SearchTiming`, `ProfitReport`, `ShipSummary`, `Ring` gains `Deserialize`. Change `pub note: &'static str` to `pub note: String`. In `find_at` the `note:` literal gets `.into()`.

In `src-tauri/src/remote_trade.rs` change `note: NOTE,` to `note: NOTE.to_string(),`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ed-route` and `cargo test -p ed-domain`; then `cargo check -p edda` (the app builds against `note: String`).
Expected: all pass; the app compiles.

- [ ] **Step 5: Commit**

```bash
git add crates/ed-domain/src/station.rs crates/ed-route/src/cost.rs crates/ed-route/src/profit.rs src-tauri/src/remote_trade.rs
git commit -m "ed-route: the profit report deserializes — it crosses the wire whole under the API-only spec"
```

---

### Task 3: One seam for the pipeline — `assemble`

**Files:**
- Modify: `crates/ed-route/src/profit.rs:473-527` (`silence_untradeable_sells`), `:999-1078` (`find_at`)

**Interfaces:**
- Produces:
  ```rust
  pub struct Prepared { pub stations: Vec<StationRef>, pub rows: Vec<MarketRow>, pub excluded: Excluded, pub timing: SearchTiming }
  pub fn silence_prohibited(rows: &mut [MarketRow], prohibited: &HashMap<i64, HashSet<String>>) -> usize
  pub fn assemble(origin_name: &str, origin: (f64, f64, f64), from_station: Option<i64>, ship: &Ship, c: &Constraints, limit: usize, prepared: Prepared, ctl: &SearchControl) -> ProfitReport
  ```
  `assemble` is everything `find_at` does after the rows are in hand: `best_legs`, round trips from the FULL set, rings, diversify, truncate, the report.

- [ ] **Step 1: Write the failing test** (append to `mod tests` in `profit.rs`)

```rust
    /// Round trips pair from the FULL leg set (profit.rs comment at
    /// `find_at`): with `limit: 1` the legs list is one long but the
    /// loop is still found. This is the property the remote path lost.
    #[test]
    fn assemble_pairs_round_trips_from_the_full_set() {
        let station = |id: i64, x: f64| StationRef {
            station_id: id, station: format!("S{id}"), system: format!("Sys{id}"), system_id64: id * 10,
            x, y: 0.0, z: 0.0, arrival_ls: Some(100.0), max_pad: Some(PadSize::Large),
            class: StationClass::of(Some("Coriolis")), is_carrier: false,
            controlling_power: None, power_state: None, powers: Vec::new(),
        };
        let row = |st: i64, sym: &str, buy: i64, sell: i64| MarketRow {
            station_id: st, symbol: sym.into(), name: None, buy_price: buy, sell_price: sell,
            demand: if sell > 0 { 1000 } else { 0 }, supply: if buy > 0 { 1000 } else { 0 }, age_hours: 1.0,
        };
        let prepared = Prepared {
            stations: vec![station(1, 0.0), station(2, 10.0)],
            rows: vec![row(1, "gold", 100, 0), row(2, "gold", 0, 200), row(2, "silver", 50, 0), row(1, "silver", 0, 150)],
            excluded: Excluded::default(),
            timing: SearchTiming::default(),
        };
        let ship = Ship { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 };
        let report = assemble("Sys1", (0.0, 0.0, 0.0), None, &ship, &Constraints::default(), 1, prepared, &SearchControl::none());
        assert_eq!(report.legs.len(), 1, "limit applies to the legs list");
        assert_eq!(report.round_trips.len(), 1, "the loop is found from the full set");
        assert_eq!(report.stations_considered, 2);
        assert!(report.timing.pairing_ms < 1_000);
    }

    #[test]
    fn silence_prohibited_zeroes_the_sale_and_counts_it() {
        let mut rows = vec![
            MarketRow { station_id: 2, symbol: "gold".into(), name: None, buy_price: 0, sell_price: 200, demand: 10, supply: 0, age_hours: 0.0 },
            MarketRow { station_id: 2, symbol: "silver".into(), name: None, buy_price: 0, sell_price: 100, demand: 10, supply: 0, age_hours: 0.0 },
        ];
        let mut prohibited = HashMap::new();
        prohibited.insert(2, HashSet::from(["gold".to_string()]));
        assert_eq!(silence_prohibited(&mut rows, &prohibited), 1);
        assert_eq!((rows[0].sell_price, rows[0].demand), (0, 0));
        assert_eq!(rows[1].sell_price, 100);
    }
```

Add `use std::collections::{HashMap, HashSet};` at the top of the tests module if `HashSet` is not already imported there.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ed-route assemble_pairs silence_prohibited`
Expected: compile error — `Prepared`, `assemble`, `silence_prohibited` undefined.

- [ ] **Step 3: Implement**

Add after `SearchTiming`:

```rust
/// What the finder needs in hand before pairing: candidate stations,
/// their fresh rows, and what was excluded getting there. Local builds
/// it from SQLite; the server builds it from Postgres; [`assemble`]
/// does the rest identically for both.
#[derive(Debug, Default)]
pub struct Prepared {
    pub stations: Vec<StationRef>,
    pub rows: Vec<MarketRow>,
    pub excluded: Excluded,
    pub timing: SearchTiming,
}
```

Split `silence_untradeable_sells`: the prohibited loop becomes

```rust
/// Zero the sale side of every row a station confiscates; returns how
/// many. Pure, so the server applies the same rule.
pub fn silence_prohibited(
    rows: &mut [MarketRow],
    prohibited: &std::collections::HashMap<i64, std::collections::HashSet<String>>,
) -> usize {
    let mut confiscated = 0;
    for row in rows.iter_mut() {
        if row.sell_price <= 0 {
            continue;
        }
        if prohibited.get(&row.station_id).is_some_and(|p| p.contains(&row.symbol)) {
            row.sell_price = 0;
            row.demand = 0;
            confiscated += 1;
        }
    }
    confiscated
}
```

and `silence_untradeable_sells` keeps the carrier-envelope loop (lines 494–516 minus the prohibited branch) then returns `Ok((silence_prohibited(rows, &prohibited), enveloped))`.

Extract `assemble` from `find_at` — move everything from `let phase = std::time::Instant::now(); let sources ...` through the `Ok(ProfitReport { ... })` into:

```rust
/// The pipeline after the data is in hand: best legs, round trips from
/// the FULL set, rings, then the diversified, truncated list. One
/// implementation for the local finder and the server.
pub fn assemble(
    origin_name: &str,
    origin: (f64, f64, f64),
    from_station: Option<i64>,
    ship: &Ship,
    c: &Constraints,
    limit: usize,
    prepared: Prepared,
    ctl: &SearchControl,
) -> ProfitReport {
    let Prepared { stations, rows, excluded, mut timing } = prepared;
    (ctl.progress)(stations.len(), stations.len());
    let phase = std::time::Instant::now();
    let sources: Option<Vec<i64>> = from_station.map(|id| vec![id]);
    let all = best_legs(&stations, &rows, sources.as_deref(), ship, c, ctl);
    // Round trips pair from the FULL leg set. Pairing from the diversified
    // list dropped the return leg of the best pair whenever its source sold
    // that commodity to more than three buyers, so the top leg had no loop
    // at all -- which read as "a leg beats its own loop".
    let trips = if from_station.is_some() {
        let unrestricted = best_legs(&stations, &rows, None, ship, c, ctl);
        round_trips(&unrestricted, limit)
            .into_iter()
            .filter(|t| Some(t.out.from.station_id) == from_station)
            .collect()
    } else {
        round_trips(&all, limit)
    };
    timing.pairing_ms = phase.elapsed().as_millis() as u64;
    let phase = std::time::Instant::now();
    let ring_list = rings(&all, origin, c.max_stops, limit);
    timing.rings_ms = phase.elapsed().as_millis() as u64;
    let mut legs = diversify(all);
    legs.truncate(limit);
    ProfitReport {
        origin: origin_name.to_string(),
        timing,
        constraints: c.clone(),
        ship: ShipSummary { cargo_capacity: ship.cargo_capacity, jump_range_ly: ship.jump_range_ly, laden_range_ly: ship.laden_range_ly },
        stations_considered: stations.len(),
        excluded,
        legs,
        round_trips: trips,
        rings: ring_list,
        confidence: Confidence::Estimated,
        note: "Prices are community-reported and carry their age. Credits per hour uses an \
               estimated supercruise/market timing: compare legs with it; absolute ETA is approximate.".into(),
        coverage: None,
        offer: None,
        fallback: None,
    }
}
```

`find_at` ends with:

```rust
    let prepared = Prepared { stations, rows, excluded, timing };
    Ok(assemble(origin_name, origin, from_station, ship, c, limit, prepared, ctl))
```

(the `(ctl.progress)(stations.len(), stations.len())` call moves into `assemble`, so delete it from `find_at`).

- [ ] **Step 4: Run all ed-route tests**

Run: `cargo test -p ed-route`
Expected: the two new tests pass and every existing `find`/`find_at` test still passes unchanged (the refactor moved code, it did not change it).

- [ ] **Step 5: Commit**

```bash
git add crates/ed-route/src/profit.rs
git commit -m "ed-route: assemble() is the one seam after the rows are in hand; silence_prohibited is pure — the server runs the same pipeline"
```

---

### Task 4: `trade_report` — Postgres builds `Prepared`

**Files:**
- Modify: `crates/ed-api/Cargo.toml` (add `ed-route = { path = "../ed-route" }`)
- Create: `crates/ed-api/src/geo.rs`
- Modify: `crates/ed-api/src/market_search.rs:117-125` (move `cells_covering` and its `CELL_LY` to `geo.rs`, re-export or call `crate::geo::cells_covering`)
- Create: `crates/ed-api/src/trade_report.rs`
- Modify: `crates/ed-api/src/lib.rs` (`pub mod geo; pub mod trade_report;`)
- Modify: `crates/ed-api/tests/postgres.rs` (new ignored test)

**Interfaces:**
- Consumes: `ed_route::profit::{Prepared, StationRef, MarketRow, Excluded, SearchTiming, Constraints, silence_prohibited}`; `crate::market_search::{Refusal, origin_coords}`.
- Produces:
  ```rust
  pub const DEFAULT_MAX_STATIONS: usize = /* Task 1 verdict: 2_500 or 1_000 */;
  pub async fn prepare(pool: &PgPool, origin: (f64, f64, f64), c: &Constraints) -> Result<Prepared, Refusal>
  pub fn station_ref(row: &sqlx::postgres::PgRow) -> StationRef   // pub(crate) is enough
  ```

- [ ] **Step 1: Write the failing unit test** (in `trade_report.rs`, pure part — the exclusion rules)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use ed_domain::station::{PadSize, StationClass};

    fn station(id: i64, x: f64, pad: Option<PadSize>, carrier: bool, arrival: Option<f64>) -> StationRef {
        StationRef {
            station_id: id, station: format!("S{id}"), system: "Sys".into(), system_id64: 1,
            x, y: 0.0, z: 0.0, arrival_ls: arrival, max_pad: pad, class: StationClass::of(Some("Coriolis")),
            is_carrier: carrier, controlling_power: None, power_state: None, powers: Vec::new(),
        }
    }

    /// Same rules as the local finder's `candidate_stations`: carriers
    /// out unless asked, far-from-star out, pad too small / unknown out
    /// when a pad is required, then the nearest `max_stations` kept.
    #[test]
    fn exclusions_mirror_the_local_finder() {
        let c = Constraints { min_pad: Some(PadSize::Large), max_arrival_ls: 1_000.0, max_stations: 2, ..Constraints::default() };
        let all = vec![
            station(1, 1.0, Some(PadSize::Large), false, Some(10.0)),
            station(2, 2.0, Some(PadSize::Large), true, Some(10.0)),    // carrier
            station(3, 3.0, Some(PadSize::Medium), false, Some(10.0)),  // pad too small
            station(4, 4.0, None, false, Some(10.0)),                   // pad unknown
            station(5, 5.0, Some(PadSize::Large), false, Some(5_000.0)),// too far from star
            station(6, 6.0, Some(PadSize::Large), false, Some(10.0)),
            station(7, 7.0, Some(PadSize::Large), false, Some(10.0)),   // beyond the cap
        ];
        let mut excluded = Excluded::default();
        let kept = exclude(all, (0.0, 0.0, 0.0), &c, &mut excluded);
        assert_eq!(kept.iter().map(|s| s.station_id).collect::<Vec<_>>(), vec![1, 6]);
        assert_eq!(excluded.carriers, 1);
        assert_eq!(excluded.pad_too_small, 1);
        assert_eq!(excluded.pad_unknown, 1);
        assert_eq!(excluded.too_far_from_star, 1);
        assert_eq!(excluded.beyond_station_cap, 1);
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ed-api exclusions_mirror`
Expected: compile error — module/functions undefined.

- [ ] **Step 3: Implement `geo.rs`, `trade_report.rs`**

`crates/ed-api/src/geo.rs`: move `CELL_LY` (the 100 ly value used by `cells_covering`) and `cells_covering` verbatim from `market_search.rs`, both `pub`. In `market_search.rs` replace the definitions with `pub(crate) use crate::geo::cells_covering;` (keep the doc comment about migration 0015 with the function).

`crates/ed-api/src/trade_report.rs`:

```rust
//! The profit finder's data half on the server (API-only spec, Phase
//! A.2): Postgres supplies the sphere's candidate stations and every
//! fresh row at them; `ed_route::profit::assemble` does the pairing.
//! The exclusion rules here mirror `ed_route::profit::candidate_stations`
//! line for line so the two finders count the same things.
//!
//! Known gap (ledgered): the carrier price envelope
//! (`commodity_stats_by_symbol`) is not applied here — the server has no
//! per-commodity stats table yet. With `include_carriers: false` (the
//! default) nothing differs.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use ed_domain::station::{PadSize, StationClass};
use ed_route::profit::{silence_prohibited, Constraints, Excluded, MarketRow, Prepared, SearchTiming, StationRef};
use sqlx::{PgPool, Row};

use crate::market_search::Refusal;

/// Task 1's verdict (docs/benches/trade-rows-pull-2026-09-07.csv).
pub const DEFAULT_MAX_STATIONS: usize = 2_500;
pub const MAX_RADIUS_LY: f64 = 500.0;

pub(crate) fn station_ref(row: &sqlx::postgres::PgRow) -> StationRef {
    let (ps, pm, pl): (Option<i32>, Option<i32>, Option<i32>) = (row.get(8), row.get(9), row.get(10));
    StationRef {
        station_id: row.get::<i64, _>(0),
        station: row.get::<Option<String>, _>(1).unwrap_or_default(),
        system: row.get::<String, _>(2),
        system_id64: row.get::<i64, _>(3),
        x: row.get::<Option<f64>, _>(4).unwrap_or(0.0),
        y: row.get::<Option<f64>, _>(5).unwrap_or(0.0),
        z: row.get::<Option<f64>, _>(6).unwrap_or(0.0),
        arrival_ls: row.get::<Option<f64>, _>(7),
        max_pad: PadSize::from_counts(pl.map(i64::from), pm.map(i64::from), ps.map(i64::from)),
        class: StationClass::of(row.get::<Option<String>, _>(12).as_deref()),
        is_carrier: row.get::<bool, _>(11),
        controlling_power: row.get::<Option<String>, _>(13),
        power_state: row.get::<Option<String>, _>(14),
        powers: row
            .get::<Option<String>, _>(15)
            .map(|s| s.split(',').map(|p| p.trim().to_owned()).filter(|p| !p.is_empty()).collect())
            .unwrap_or_default(),
    }
}

/// The local finder's rules, in its order: carriers, arrival distance,
/// pad, then the nearest `max_stations`.
pub(crate) fn exclude(all: Vec<StationRef>, origin: (f64, f64, f64), c: &Constraints, excluded: &mut Excluded) -> Vec<StationRef> {
    let mut kept: Vec<StationRef> = Vec::with_capacity(all.len());
    for s in all {
        if s.is_carrier && !c.include_carriers {
            excluded.carriers += 1;
            continue;
        }
        if s.arrival_ls.unwrap_or(0.0) > c.max_arrival_ls {
            excluded.too_far_from_star += 1;
            continue;
        }
        if let Some(required) = c.min_pad {
            match s.max_pad {
                None => {
                    excluded.pad_unknown += 1;
                    continue;
                }
                Some(pad) if !pad.fits(required) => {
                    excluded.pad_too_small += 1;
                    continue;
                }
                Some(_) => {}
            }
        }
        kept.push(s);
    }
    let cap = c.max_stations.max(1);
    if kept.len() > cap {
        let (ox, oy, oz) = origin;
        let d2 = |s: &StationRef| (s.x - ox).powi(2) + (s.y - oy).powi(2) + (s.z - oz).powi(2);
        kept.sort_by(|a, b| d2(a).total_cmp(&d2(b)));
        excluded.beyond_station_cap = kept.len() - cap;
        kept.truncate(cap);
    }
    kept
}

/// Candidate stations, their fresh rows, confiscations applied.
pub async fn prepare(pool: &PgPool, origin: (f64, f64, f64), c: &Constraints) -> Result<Prepared, Refusal> {
    let (ox, oy, oz) = origin;
    let r = c.radius_ly.clamp(1.0, MAX_RADIUS_LY);
    let mut excluded = Excluded::default();
    let mut timing = SearchTiming::default();

    let phase = Instant::now();
    let rows = sqlx::query(
        "SELECT st.id, st.name, sy.name, sy.address, sy.x, sy.y, sy.z, st.arrival_ls, \
                st.pad_small, st.pad_medium, st.pad_large, COALESCE(st.is_carrier, false), st.station_type, \
                sy.controlling_power, sy.power_state, sy.powers \
         FROM stations st JOIN systems sy ON sy.address = st.system_address \
         WHERE sy.cell = ANY($5) \
           AND sy.x BETWEEN $1-$4 AND $1+$4 AND sy.y BETWEEN $2-$4 AND $2+$4 AND sy.z BETWEEN $3-$4 AND $3+$4 \
           AND (sy.x-$1)^2 + (sy.y-$2)^2 + (sy.z-$3)^2 <= $4*$4 \
           AND st.has_market",
    )
    .bind(ox).bind(oy).bind(oz).bind(r)
    .bind(crate::geo::cells_covering(ox, oy, oz, r))
    .persistent(false)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let stations = exclude(rows.iter().map(station_ref).collect(), origin, c, &mut excluded);
    timing.candidates_ms = phase.elapsed().as_millis() as u64;

    let phase = Instant::now();
    let ids: Vec<i64> = stations.iter().map(|s| s.station_id).collect();
    let rows = sqlx::query(
        "SELECT m.station_id, m.commodity_symbol, NULLIF(c.name, ''), m.buy_price, m.sell_price, m.demand, m.supply, \
                EXTRACT(EPOCH FROM now() - m.observed_at)::DOUBLE PRECISION / 3600.0 \
         FROM market m LEFT JOIN commodities c ON c.symbol = m.commodity_symbol \
         WHERE m.station_id = ANY($1) AND m.observed_at > now() - make_interval(secs => $2)",
    )
    .bind(&ids)
    .bind(c.max_age_hours.max(0.25) * 3600.0)
    .persistent(false)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let mut seen: HashSet<i64> = HashSet::new();
    let mut market: Vec<MarketRow> = rows
        .iter()
        .map(|row| {
            let station_id: i64 = row.get(0);
            seen.insert(station_id);
            MarketRow {
                station_id,
                symbol: row.get::<String, _>(1),
                name: row.get::<Option<String>, _>(2),
                buy_price: row.get::<i64, _>(3),
                sell_price: row.get::<i64, _>(4),
                demand: row.get::<i64, _>(5),
                supply: row.get::<i64, _>(6),
                age_hours: row.get::<f64, _>(7),
            }
        })
        .collect();
    excluded.no_market_data += ids.len() - seen.len();
    timing.market_ms = phase.elapsed().as_millis() as u64;

    let phase = Instant::now();
    let prohibited_rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT station_id, lower(symbol) FROM station_prohibited WHERE station_id = ANY($1)")
            .bind(&ids)
            .fetch_all(pool)
            .await
            .map_err(|e| Refusal::Invalid(e.to_string()))?;
    let black_markets: HashSet<i64> = if c.include_prohibited {
        sqlx::query_scalar::<_, i64>("SELECT station_id FROM station_services WHERE service = 'blackmarket' AND station_id = ANY($1)")
            .bind(&ids)
            .fetch_all(pool)
            .await
            .map_err(|e| Refusal::Invalid(e.to_string()))?
            .into_iter()
            .collect()
    } else {
        HashSet::new()
    };
    let mut prohibited: HashMap<i64, HashSet<String>> = HashMap::new();
    for (station_id, symbol) in prohibited_rows {
        // Opted in, the sale is allowed exactly where a black market
        // exists to take the goods; elsewhere it stays impossible.
        if black_markets.contains(&station_id) {
            continue;
        }
        prohibited.entry(station_id).or_default().insert(symbol);
    }
    // Symbols in `market` are the journal's lowercase names already;
    // the prohibited list is lowered in SQL to match.
    excluded.confiscated_sales = silence_prohibited(&mut market, &prohibited);
    timing.guards_ms = phase.elapsed().as_millis() as u64;

    Ok(Prepared { stations, rows: market, excluded, timing })
}
```

Set `DEFAULT_MAX_STATIONS` to Task 1's verdict. Add `pub mod geo; pub mod trade_report;` to `lib.rs`.

- [ ] **Step 4: Run the unit test and the crate's tests**

Run: `cargo test -p ed-api exclusions_mirror` then `cargo test -p ed-api`
Expected: pass; nothing else regresses (`market_search` still finds `cells_covering`).

- [ ] **Step 5: Write the ignored Postgres test** (append to `crates/ed-api/tests/postgres.rs`)

```rust
/// API-only spec, first defect: round trips must come from the server.
/// Two stations, gold one way and silver the other: one loop.
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn trade_report_finds_the_round_trip_the_legacy_query_could_not() {
    let _serial = DATABASE.lock().await;
    let database_url = std::env::var("EDDA_API_TEST_DATABASE_URL").unwrap();
    let pool = PgPool::connect(&database_url).await.unwrap();
    reset_database(&pool).await;
    sqlx::raw_sql(
        "INSERT INTO systems (address, name, x, y, z) VALUES (1, 'Alpha', 0, 0, 0), (2, 'Beta', 10, 0, 0);
         INSERT INTO stations (id, system_address, name, has_market, pad_large, is_carrier, station_type)
           VALUES (11, 1, 'A Dock', true, 4, false, 'Coriolis'), (22, 2, 'B Dock', true, 4, false, 'Coriolis');
         INSERT INTO commodities (symbol) VALUES ('gold'), ('silver') ON CONFLICT DO NOTHING;
         INSERT INTO market (station_id, commodity_symbol, buy_price, sell_price, demand, supply, observed_at) VALUES
           (11, 'gold',   100, 0,   0,    1000, now()),
           (22, 'gold',   0,   200, 1000, 0,    now()),
           (22, 'silver', 50,  0,   0,    1000, now()),
           (11, 'silver', 0,   150, 1000, 0,    now());",
    )
    .execute(&pool)
    .await
    .unwrap();
    let c = ed_route::profit::Constraints { radius_ly: 50.0, max_age_hours: 48.0, ..Default::default() };
    let prepared = ed_api::trade_report::prepare(&pool, (0.0, 0.0, 0.0), &c).await.unwrap();
    assert_eq!(prepared.stations.len(), 2);
    assert_eq!(prepared.rows.len(), 4);
    let ship = ed_route::cost::Ship { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 };
    let report = ed_route::profit::assemble("Alpha", (0.0, 0.0, 0.0), None, &ship, &c, 10, prepared, &ed_route::profit::SearchControl::none());
    assert_eq!(report.legs.len(), 2);
    assert_eq!(report.round_trips.len(), 1, "gold out, silver back");
    assert_eq!(report.stations_considered, 2);
}
```

If `INSERT INTO systems` fails on a NOT NULL column, look at the `INSERT INTO systems` in `trade_search_pairs_legs_and_caches` (same file, ~line 826) and add the same columns. `ed-route` and `ed-api` are both available to the integration test through `[dev-dependencies]` if `ed-route` is not already reachable — add `ed-route = { path = "../ed-route" }` under `[dev-dependencies]` too if the compiler asks.

- [ ] **Step 6: Run it in WSL**

Run: `wsl -e bash -lc 'cd ~/edda-linux && git fetch -q https://github.com/terakilobyte/edda.git <branch> && git checkout -q FETCH_HEAD && EDDA_API_TEST_DATABASE_URL=postgres://edda:edda@127.0.0.1:55432/edda_test RUSTC_WRAPPER= CARGO_TARGET_DIR=$HOME/edda-target cargo test -p ed-api --test postgres -- --ignored trade_report_finds'`
Expected: `test trade_report_finds_the_round_trip_the_legacy_query_could_not ... ok`. (Push the branch to GitHub before this step.)

- [ ] **Step 7: Commit**

```bash
git add crates/ed-api/Cargo.toml crates/ed-api/src/geo.rs crates/ed-api/src/market_search.rs crates/ed-api/src/trade_report.rs crates/ed-api/src/lib.rs crates/ed-api/tests/postgres.rs
git commit -m "ed-api: trade_report builds the finder's Prepared set from Postgres — the round trip the legacy query could not see"
```

---

### Task 5: `/v1/trade/search` serves the report when the request carries a ship

**Files:**
- Modify: `crates/ed-api/src/trade_search.rs` (new `ReportRequest`, `TradeService::report`)
- Modify: `crates/ed-api/src/http.rs:157-205` (`trade_search` handler dispatches on `ship`)
- Modify: `crates/ed-api/src/metrics.rs` (doc table row)
- Modify: `crates/ed-api/README.md`

**Interfaces:**
- Consumes: `trade_report::{prepare, DEFAULT_MAX_STATIONS, MAX_RADIUS_LY}`, `ed_route::profit::assemble`.
- Produces (the wire contract the client plan builds against):
  - Request v2: `{ "system": "Deciat", "ship": {"cargo_capacity": 720, "jump_range_ly": 30.5, "laden_range_ly": 22.1}, "constraints": <ed_route::profit::Constraints, every field optional via serde(default)>, "from_station_id": null, "limit": 20 }`. Presence of `ship` selects v2.
  - Response v2: the `ProfitReport` JSON (serde) plus `"provenance": "server"` and `"as_of": <ISO>`. Absent `ship`: today's `{origin, legs, provenance, as_of}` unchanged.
  - 429 when the gate is full (unchanged); 422 unknown system (unchanged).
  - Metrics: `edda_trade_search_requests_total{outcome="report"}`, `edda_trade_report_phase_seconds{phase=candidates|market|guards|pairing|rings}`, `edda_trade_report_stations` (histogram).

- [ ] **Step 1: Write the failing unit tests** (in `trade_search.rs` `mod tests`)

```rust
    #[test]
    fn a_body_with_a_ship_is_a_report_request() {
        let body: serde_json::Value = serde_json::json!({
            "system": "Deciat",
            "ship": {"cargo_capacity": 720, "jump_range_ly": 30.5, "laden_range_ly": 22.1},
            "constraints": {"radius_ly": 60.0, "max_stops": 3}
        });
        let req = ReportRequest::parse(&body).unwrap();
        assert_eq!(req.constraints.radius_ly, 60.0);
        assert_eq!(req.constraints.max_stops, 3);
        assert_eq!(req.constraints.max_age_hours, ed_route::profit::Constraints::default().max_age_hours);
        assert_eq!(req.limit(), ed_route::request::ProfitRequest::DEFAULT_LIMIT);
        assert!(ReportRequest::parse(&serde_json::json!({"system": "Sol"})).is_none(), "no ship: legacy");
    }

    #[test]
    fn report_request_clamps_to_the_server_budget() {
        let mut req = ReportRequest {
            system: "Sol".into(),
            ship: ed_route::cost::Ship { cargo_capacity: 1, jump_range_ly: 1.0, laden_range_ly: 1.0 },
            constraints: ed_route::profit::Constraints { radius_ly: 9_000.0, max_stations: 1_000_000, max_age_hours: 0.0, ..Default::default() },
            from_station_id: None,
            limit: Some(9_000),
        };
        req.clamp();
        assert_eq!(req.constraints.radius_ly, crate::trade_report::MAX_RADIUS_LY);
        assert_eq!(req.constraints.max_stations, crate::trade_report::DEFAULT_MAX_STATIONS);
        assert_eq!(req.constraints.max_age_hours, 0.25);
        assert_eq!(req.limit(), 100);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ed-api a_body_with_a_ship report_request_clamps`
Expected: compile error — `ReportRequest` undefined.

- [ ] **Step 3: Implement**

In `trade_search.rs` add:

```rust
/// The v2 request (API-only spec, Phase A.2): the client's resolved
/// plan — ship and constraints — verbatim, so the server computes the
/// same report the local finder would. `ship` present selects this
/// path; absent, the legacy legs answer (0.2.9 clients) is served.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct ReportRequest {
    pub system: String,
    pub ship: ed_route::cost::Ship,
    #[serde(default)]
    pub constraints: ed_route::profit::Constraints,
    pub from_station_id: Option<i64>,
    pub limit: Option<usize>,
}

impl ReportRequest {
    pub fn parse(body: &serde_json::Value) -> Option<Self> {
        body.get("ship")?;
        serde_json::from_value(body.clone()).ok()
    }
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(ed_route::request::ProfitRequest::DEFAULT_LIMIT).clamp(1, MAX_LEGS as usize)
    }
    /// A shared box gives everyone the same ceiling.
    pub fn clamp(&mut self) {
        let c = &mut self.constraints;
        c.radius_ly = c.radius_ly.clamp(1.0, crate::trade_report::MAX_RADIUS_LY);
        c.max_stations = c.max_stations.clamp(1, crate::trade_report::DEFAULT_MAX_STATIONS);
        c.max_age_hours = c.max_age_hours.clamp(0.25, 720.0);
        c.max_stops = c.max_stops.min(5);
        self.limit = Some(self.limit());
    }
    fn cache_key(&self, origin: (f64, f64, f64)) -> String {
        format!("report|{:.0},{:.0},{:.0}|{}|{:?}", origin.0, origin.1, origin.2,
            serde_json::to_string(&self.constraints).unwrap_or_default(),
            (self.ship.cargo_capacity, (self.ship.jump_range_ly * 10.0) as i64, (self.ship.laden_range_ly * 10.0) as i64, self.from_station_id, self.limit))
    }
}
```

Add to `impl TradeService`:

```rust
    pub async fn report(&self, pool: &PgPool, req: &ReportRequest) -> Result<TradeOutcome, Refusal> {
        let mut req = req.clone();
        req.clamp();
        let origin = crate::market_search::origin_coords(pool, req.system.trim()).await?;
        let key = req.cache_key(origin);
        if let Some(value) = self.cache_hit(&key) {
            return Ok(TradeOutcome::Legs(value, "hit"));
        }
        let Ok(_permit) = tokio::time::timeout(Duration::from_secs(15), self.gate.acquire()).await else {
            return Ok(TradeOutcome::Saturated);
        };
        if let Some(value) = self.cache_hit(&key) {
            return Ok(TradeOutcome::Legs(value, "hit"));
        }
        let prepared = crate::trade_report::prepare(pool, origin, &req.constraints).await?;
        metrics::histogram!("edda_trade_report_stations").record(prepared.stations.len() as f64);
        for (phase, ms) in [("candidates", prepared.timing.candidates_ms), ("market", prepared.timing.market_ms), ("guards", prepared.timing.guards_ms)] {
            metrics::histogram!("edda_trade_report_phase_seconds", "phase" => phase).record(ms as f64 / 1000.0);
        }
        let system = req.system.trim().to_owned();
        let (ship, constraints, from_station, limit) = (req.ship, req.constraints.clone(), req.from_station_id, req.limit());
        let report = tokio::task::spawn_blocking(move || {
            ed_route::profit::assemble(&system, origin, from_station, &ship, &constraints, limit, prepared, &ed_route::profit::SearchControl::none())
        })
        .await
        .map_err(|join| Refusal::Invalid(format!("report panicked: {join}")))?;
        for (phase, ms) in [("pairing", report.timing.pairing_ms), ("rings", report.timing.rings_ms)] {
            metrics::histogram!("edda_trade_report_phase_seconds", "phase" => phase).record(ms as f64 / 1000.0);
        }
        let mut value = serde_json::to_value(&report).map_err(|e| Refusal::Invalid(e.to_string()))?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("provenance".into(), json!("server"));
            obj.insert("as_of".into(), json!(crate::market_search::now_iso()));
        }
        self.remember(&key, value.clone());
        Ok(TradeOutcome::Legs(value, "miss"))
    }
```

(`cache_hit` and `remember` are the existing private cache helpers — if the storing helper has another name, use that name; do not add a second cache.)

In `http.rs`, change the `trade_search` handler's body extractor to `axum::Json(body): axum::Json<serde_json::Value>` and dispatch:

```rust
    let outcome = if let Some(report) = crate::trade_search::ReportRequest::parse(&body) {
        counter("report");
        state.trade_service.report(&state.pool, &report).await
    } else {
        let legacy: crate::trade_search::TradeSearchApiRequest = match serde_json::from_value(body) {
            Ok(r) => r,
            Err(e) => return (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        };
        state.trade_service.search(&state.pool, &legacy).await
    };
    match outcome { /* the existing arms, unchanged */ }
```

where `counter` is the handler's existing `outcome_counter` closure. Add the metrics row to the table in `metrics.rs` and a "Request v2 (report)" paragraph to the `/v1/trade/search` section of `crates/ed-api/README.md` with the request/response shape from **Interfaces** above.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p ed-api`
Expected: the two new tests pass; `trade_search_pairs_legs_and_caches` (ignored, Postgres) is unchanged. Run it too in WSL with the Task 4 command, substituting `trade_search_pairs_legs_and_caches` — the legacy path must still answer.

- [ ] **Step 5: Postgres test for the v2 path** (append to `tests/postgres.rs`, same seed as Task 4's test — copy its `raw_sql` block)

```rust
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn trade_search_with_a_ship_returns_the_report() {
    // seed exactly as trade_report_finds_the_round_trip_the_legacy_query_could_not
    // ... (the same reset + raw_sql block) ...
    let service = ed_api::trade_search::TradeService::default();
    let req = ed_api::trade_search::ReportRequest {
        system: "Alpha".into(),
        ship: ed_route::cost::Ship { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 },
        constraints: ed_route::profit::Constraints { radius_ly: 50.0, max_age_hours: 48.0, ..Default::default() },
        from_station_id: None,
        limit: Some(10),
    };
    let ed_api::trade_search::TradeOutcome::Legs(value, verdict) = service.report(&pool, &req).await.unwrap() else { panic!("saturated") };
    assert_eq!(verdict, "miss");
    assert_eq!(value["round_trips"].as_array().unwrap().len(), 1);
    assert_eq!(value["provenance"], "server");
    let ed_api::trade_search::TradeOutcome::Legs(_, verdict) = service.report(&pool, &req).await.unwrap() else { panic!("saturated") };
    assert_eq!(verdict, "hit");
}
```

Run in WSL as in Task 4 Step 6 with `trade_search_with_a_ship`. Expected: ok.

- [ ] **Step 6: Commit**

```bash
git add crates/ed-api/src/trade_search.rs crates/ed-api/src/http.rs crates/ed-api/src/metrics.rs crates/ed-api/README.md crates/ed-api/tests/postgres.rs
git commit -m "ed-api: /v1/trade/search serves the whole ProfitReport when the request carries a ship (legacy legs kept for 0.2.9)"
```

---

### Task 6: `/v1/stations`

**Files:**
- Create: `crates/ed-api/src/stations.rs`
- Modify: `crates/ed-api/src/http.rs` (route + handler)
- Modify: `crates/ed-api/src/lib.rs` (`pub mod stations;`)
- Modify: `crates/ed-api/src/metrics.rs`, `crates/ed-api/README.md`
- Modify: `crates/ed-api/tests/postgres.rs`

**Interfaces:**
- Consumes: `crate::geo::cells_covering`, `crate::names::complete_stations`, `GalaxyService::current()` → `handle.galaxy.find(name)` / `pos_of(idx)`, `knowledge_limiter`.
- Produces (wire contract for the client plan):
  - `GET /v1/stations?system=<name>[&include_carriers=false][&include_minor=true][&limit=50]`
  - `GET /v1/stations?near=<system>&service=<key>[&radius_ly=50][&min_pad=s|m|l][&include_carriers=false][&limit=20]`
  - `GET /v1/stations?name=<prefix>[&limit=20]`
  - Exactly one of `system` / `near` / `name` else 400. Unknown `service` → 400 with the accepted keys. Unknown system → 422 `{"error":"unknown_system"}`; an empty sphere is `[]`.
  - Response: JSON array of `{ id, name, system_name, kind, class, distance_to_arrival, primary_economy: null, government: null, controlling_faction: null, max_pad, has_market, has_outfitting, has_shipyard, is_carrier, updated, age_hours, distance_ly }` — `ed_store::lookup::StationInfo`'s fields plus `distance_ly` (null except for `near`).
  - Metrics: `edda_stations_requests_total{mode,outcome}`, `edda_stations_seconds{mode}`.
  - `pub fn service_key(text: &str) -> Option<&'static str>` — friendly key or raw journal key → stored value.

- [ ] **Step 1: Failing unit tests** (`stations.rs` `mod tests`)

```rust
    #[test]
    fn exactly_one_mode() {
        let q = |s: &str| serde_urlencoded::from_str::<StationsQuery>(s).unwrap();
        assert!(matches!(q("system=Sol").mode().unwrap(), Mode::InSystem(ref s) if s == "Sol"));
        assert!(matches!(q("near=Sol&service=material_trader").mode().unwrap(), Mode::Near { ref service, .. } if service.as_deref() == Some("materialtrader")));
        assert!(matches!(q("name=jame").mode().unwrap(), Mode::Name(ref p) if p == "jame"));
        assert!(q("").mode().is_err());
        assert!(q("system=Sol&name=x").mode().is_err());
        assert!(q("near=Sol&service=teleporter").mode().unwrap_err().contains("material_trader"));
    }

    #[test]
    fn service_keys_accept_friendly_and_journal_names() {
        assert_eq!(service_key("material_trader"), Some("materialtrader"));
        assert_eq!(service_key("Material Trader"), Some("materialtrader"));
        assert_eq!(service_key("materialtrader"), Some("materialtrader"));
        assert_eq!(service_key("interstellar_factors"), Some("facilitator"));
        assert_eq!(service_key("universal_cartographics"), Some("exploration"));
        assert_eq!(service_key("market"), Some("market"));
        assert_eq!(service_key("teleporter"), None);
    }
```

Add `serde_urlencoded = "0.7"` under `[dev-dependencies]` in `crates/ed-api/Cargo.toml` (axum already depends on it; the dev-dep makes it nameable in tests).

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ed-api exactly_one_mode service_keys_accept`
Expected: compile error.

- [ ] **Step 3: Implement `stations.rs`**

```rust
//! GET /v1/stations — the wire half of the three station lookups the
//! ship computer ran on the local sys_* tables (API-only spec, Phase
//! A.3): stations in a system, nearest with a service, by name prefix.
//! The response is `ed_store::lookup::StationInfo`'s shape so the client
//! deserializes into the type it already renders.

use ed_domain::station::{PadSize, StationClass};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{PgPool, Row};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
pub const DEFAULT_NEAR_RADIUS_LY: f64 = 50.0;
pub const MAX_NEAR_RADIUS_LY: f64 = 500.0;

/// Friendly key → the journal's StationServices key as EDDN writes it
/// into `station_services.service` (vocabulary read from edda_dev,
/// 2026-09-07: 45 distinct values, e.g. materialtrader 857, techbroker
/// 1,182, blackmarket 8,433). `market`/`outfitting`/`shipyard` are the
/// boolean columns, not services.
pub const SERVICES: &[(&str, &str)] = &[
    ("interstellar_factors", "facilitator"),
    ("technology_broker", "techbroker"),
    ("universal_cartographics", "exploration"),
    ("black_market", "blackmarket"),
    ("search_and_rescue", "searchrescue"),
    ("refuel", "refuel"),
    ("repair", "repair"),
    ("restock", "rearm"),
    ("refinery", "refinery"),
    ("vista_genomics", "vistagenomics"),
    ("crew_lounge", "crewlounge"),
    ("fleet_carrier_vendor", "carriervendor"),
    ("material_trader", "materialtrader"),
    ("missions", "missions"),
    ("redemption_office", "voucherredemption"),
    ("pioneer_supplies", "pioneersupplies"),
    ("powerplay", "powerplay"),
    ("bartender", "bartender"),
    ("frontline_solutions", "frontlinesolutions"),
    ("apex_interstellar", "apexinterstellar"),
    ("workshop", "engineer"),
    ("livery", "livery"),
    ("shop", "shop"),
    ("system_colonisation", "registeringcolonisation"),
    ("construction_services", "colonisationcontribution"),
];
const COLUMN_SERVICES: &[&str] = &["market", "outfitting", "shipyard"];

/// Accepts the friendly key, its spaced form, or the raw journal key.
pub fn service_key(text: &str) -> Option<&'static str> {
    let key = text.trim().to_ascii_lowercase().replace(' ', "_");
    if let Some(c) = COLUMN_SERVICES.iter().find(|c| **c == key) {
        return Some(c);
    }
    SERVICES
        .iter()
        .find(|(friendly, raw)| *friendly == key || *raw == key.replace('_', ""))
        .map(|(_, raw)| *raw)
}

pub fn accepted_services() -> String {
    let mut keys: Vec<&str> = COLUMN_SERVICES.to_vec();
    keys.extend(SERVICES.iter().map(|(f, _)| *f));
    keys.join(", ")
}

#[derive(Debug, Default, Deserialize)]
pub struct StationsQuery {
    pub system: Option<String>,
    pub near: Option<String>,
    pub service: Option<String>,
    pub radius_ly: Option<f64>,
    pub min_pad: Option<String>,
    pub name: Option<String>,
    #[serde(default)]
    pub include_carriers: bool,
    #[serde(default = "default_true")]
    pub include_minor: bool,
    pub limit: Option<usize>,
}
fn default_true() -> bool { true }

#[derive(Debug)]
pub enum Mode {
    InSystem(String),
    Near { system: String, service: Option<&'static str>, radius_ly: f64, min_pad: Option<PadSize> },
    Name(String),
}

impl StationsQuery {
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
    pub fn mode(&self) -> Result<Mode, String> {
        let given = [self.system.is_some(), self.near.is_some(), self.name.is_some()].iter().filter(|b| **b).count();
        if given != 1 {
            return Err("give exactly one of system=, near=, name=".into());
        }
        if let Some(system) = &self.system {
            return Ok(Mode::InSystem(system.trim().to_owned()));
        }
        if let Some(prefix) = &self.name {
            return Ok(Mode::Name(prefix.trim().to_owned()));
        }
        let system = self.near.as_deref().unwrap_or("").trim().to_owned();
        let service = match self.service.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(text) => Some(service_key(text).ok_or_else(|| format!("unknown service {text:?}; accepted: {}", accepted_services()))?),
        };
        let min_pad = match self.min_pad.as_deref().map(str::trim) {
            None | Some("") | Some("any") => None,
            Some(text) => Some(PadSize::parse(text).ok_or_else(|| format!("unknown pad size {text:?}"))?),
        };
        Ok(Mode::Near { system, service, radius_ly: self.radius_ly.unwrap_or(DEFAULT_NEAR_RADIUS_LY).clamp(1.0, MAX_NEAR_RADIUS_LY), min_pad })
    }
}

const COLS: &str = "st.id, st.name, sy.name, st.station_type, st.arrival_ls, st.pad_small, st.pad_medium, st.pad_large, \
                    st.has_market, st.has_outfitting, st.has_shipyard, COALESCE(st.is_carrier, false), st.identity_observed_at";

fn station_json(row: &sqlx::postgres::PgRow, distance_ly: Option<f64>) -> Value {
    let (ps, pm, pl): (Option<i32>, Option<i32>, Option<i32>) = (row.get(5), row.get(6), row.get(7));
    let kind: Option<String> = row.get(3);
    let observed: Option<chrono_free::Timestamp> = None; // placeholder-free: see below
    let _ = observed;
    let updated: Option<String> = row.get::<Option<sqlx::types::time::OffsetDateTime>, _>(12).map(|t| t.to_string());
    let age_hours: Option<f64> = row
        .get::<Option<sqlx::types::time::OffsetDateTime>, _>(12)
        .map(|t| (sqlx::types::time::OffsetDateTime::now_utc() - t).as_seconds_f64() / 3600.0);
    json!({
        "id": row.get::<i64, _>(0),
        "name": row.get::<Option<String>, _>(1),
        "system_name": row.get::<Option<String>, _>(2),
        "kind": kind,
        "class": StationClass::of(kind.as_deref()),
        "distance_to_arrival": row.get::<Option<f64>, _>(4),
        "primary_economy": Value::Null,
        "government": Value::Null,
        "controlling_faction": Value::Null,
        "max_pad": PadSize::from_counts(pl.map(i64::from), pm.map(i64::from), ps.map(i64::from)),
        "has_market": row.get::<bool, _>(8),
        "has_outfitting": row.get::<bool, _>(9),
        "has_shipyard": row.get::<bool, _>(10),
        "is_carrier": row.get::<bool, _>(11),
        "updated": updated,
        "age_hours": age_hours,
        "distance_ly": distance_ly,
    })
}
```

Remove the two `observed` lines (they exist only so this listing has no `TBD`; the real code reads the timestamp once into a local and derives both fields). Use whichever timestamp type the crate already reads `TIMESTAMPTZ` columns with (search `http.rs`/`knowledge.rs` for `OffsetDateTime` or `chrono`; sqlx's `time` feature may need enabling in `Cargo.toml` — `sqlx = { ..., features = ["postgres", "runtime-tokio-rustls", "time"] }`).

```rust
fn minor(class: StationClass) -> bool {
    matches!(class, StationClass::Settlement | StationClass::ConstructionDepot | StationClass::Other)
}

pub async fn in_system(pool: &PgPool, system: &str, q: &StationsQuery) -> anyhow::Result<Vec<Value>> {
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM stations st JOIN systems sy ON sy.address = st.system_address \
         WHERE lower(sy.name) = lower($1) ORDER BY st.name"
    ))
    .bind(system)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<(StationClass, Value)> = rows
        .iter()
        .map(|r| station_json(r, None))
        .filter(|v| q.include_carriers || v["is_carrier"] != true)
        .map(|v| (StationClass::of(v["kind"].as_str()), v))
        .filter(|(class, _)| q.include_minor || !minor(*class))
        .collect();
    out.sort_by(|a, b| a.0.rank().cmp(&b.0.rank()).then_with(|| a.1["name"].as_str().cmp(&b.1["name"].as_str())));
    Ok(out.into_iter().map(|(_, v)| v).take(q.limit()).collect())
}

pub async fn near(pool: &PgPool, origin: (f64, f64, f64), service: Option<&str>, radius_ly: f64, min_pad: Option<PadSize>, q: &StationsQuery) -> anyhow::Result<Vec<Value>> {
    let (ox, oy, oz) = origin;
    let predicate = match service {
        None => "TRUE".to_string(),
        Some("market") => "st.has_market".into(),
        Some("outfitting") => "st.has_outfitting".into(),
        Some("shipyard") => "st.has_shipyard".into(),
        Some(_) => "EXISTS (SELECT 1 FROM station_services ss WHERE ss.station_id = st.id AND ss.service = $6)".into(),
    };
    let rows = sqlx::query(&format!(
        "SELECT {COLS}, sqrt((sy.x-$1)^2 + (sy.y-$2)^2 + (sy.z-$3)^2) AS distance_ly \
         FROM stations st JOIN systems sy ON sy.address = st.system_address \
         WHERE sy.cell = ANY($5) \
           AND sy.x BETWEEN $1-$4 AND $1+$4 AND sy.y BETWEEN $2-$4 AND $2+$4 AND sy.z BETWEEN $3-$4 AND $3+$4 \
           AND (sy.x-$1)^2 + (sy.y-$2)^2 + (sy.z-$3)^2 <= $4*$4 \
           AND {predicate} \
           AND ($7 OR NOT COALESCE(st.is_carrier, false)) \
         ORDER BY distance_ly, st.name LIMIT $8"
    ))
    .bind(ox).bind(oy).bind(oz).bind(radius_ly)
    .bind(crate::geo::cells_covering(ox, oy, oz, radius_ly))
    .bind(service.unwrap_or(""))
    .bind(q.include_carriers)
    .bind((q.limit() * 4) as i64)
    .persistent(false)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| station_json(r, Some(r.get::<f64, _>(13))))
        .filter(|v| match min_pad {
            None => true,
            Some(required) => v["max_pad"].as_str().and_then(PadSize::parse).is_some_and(|p| p.fits(required)),
        })
        .take(q.limit())
        .collect())
}

pub async fn by_name(pool: &PgPool, prefix: &str, q: &StationsQuery) -> anyhow::Result<Vec<Value>> {
    if prefix.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let hits = crate::names::complete_stations(pool, prefix, q.limit()).await?;
    let names: Vec<String> = hits.iter().map(|h| h.name.clone()).collect();
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM stations st LEFT JOIN systems sy ON sy.address = st.system_address \
         WHERE st.name = ANY($1) ORDER BY COALESCE(st.is_carrier, false), st.name LIMIT $2"
    ))
    .bind(&names)
    .bind(q.limit() as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|r| station_json(r, None)).collect())
}
```

(`NameHit` in `names.rs` — use its actual field for the station name; the listing assumes `name`.) `PadSize::fits` and `StationClass::rank` exist (used in `ed-store/src/lookup.rs`); `PadSize` serializes as its variant name, so `max_pad` round-trips through `PadSize::parse` — if `parse` does not accept the variant spelling, compare through `serde_json::from_value::<PadSize>` instead.

In `http.rs`, register `.route("/v1/stations", get(stations))` and add the handler:

```rust
/// GET /v1/stations — see stations.rs. `near=` resolves the system
/// through the routing index first (the ruling: the server knows more
/// than every client), then Postgres for the station set.
async fn stations(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<crate::stations::StationsQuery>,
) -> impl IntoResponse {
    use crate::stations::Mode;
    let mode = match query.mode() {
        Ok(m) => m,
        Err(message) => {
            metrics::counter!("edda_stations_requests_total", "mode" => "invalid", "outcome" => "invalid").increment(1);
            return (StatusCode::BAD_REQUEST, message).into_response();
        }
    };
    let mode_label: &'static str = match mode { Mode::InSystem(_) => "system", Mode::Near { .. } => "near", Mode::Name(_) => "name" };
    let counter = |outcome: &'static str| metrics::counter!("edda_stations_requests_total", "mode" => mode_label, "outcome" => outcome).increment(1);
    if !state.knowledge_limiter.allow(&source_of(&headers), std::time::Instant::now()) {
        counter("rate_limited");
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let started = std::time::Instant::now();
    let result = match mode {
        Mode::InSystem(system) => crate::stations::in_system(&state.pool, &system, &query).await,
        Mode::Name(prefix) => crate::stations::by_name(&state.pool, &prefix, &query).await,
        Mode::Near { system, service, radius_ly, min_pad } => {
            let origin = match state.galaxy.current().await {
                Ok(Some(handle)) => handle.galaxy.find(&system).map(|idx| { let p = handle.galaxy.pos_of(idx); (p[0] as f64, p[1] as f64, p[2] as f64) }),
                _ => None,
            };
            let origin = match origin {
                Some(o) => o,
                None => match crate::market_search::origin_coords(&state.pool, &system).await {
                    Ok(o) => o,
                    Err(_) => {
                        counter("unknown_system");
                        return (StatusCode::UNPROCESSABLE_ENTITY, axum::Json(serde_json::json!({ "error": "unknown_system", "system": system }))).into_response();
                    }
                },
            };
            crate::stations::near(&state.pool, origin, service, radius_ly, min_pad, &query).await
        }
    };
    match result {
        Ok(list) => {
            metrics::histogram!("edda_stations_seconds", "mode" => mode_label).record(started.elapsed().as_secs_f64());
            counter("ok");
            axum::Json(list).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, mode = mode_label, "stations: query failed");
            counter("error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
```

Add `pub mod stations;` to `lib.rs`; the metrics table row; a README section listing the three modes, the 400/422 rules and the response shape.

- [ ] **Step 4: Run unit tests**

Run: `cargo test -p ed-api stations`
Expected: both pass.

- [ ] **Step 5: Postgres test** (append to `tests/postgres.rs`; seed as Task 4, plus `INSERT INTO station_services (station_id, service) VALUES (22, 'materialtrader')`)

```rust
#[tokio::test]
#[ignore = "requires EDDA_API_TEST_DATABASE_URL"]
async fn stations_answers_the_three_lookups() {
    // reset + the Task 4 seed + the station_services row
    let q = ed_api::stations::StationsQuery::default();
    let in_alpha = ed_api::stations::in_system(&pool, "alpha", &q).await.unwrap();
    assert_eq!(in_alpha.len(), 1);
    assert_eq!(in_alpha[0]["name"], "A Dock");
    assert_eq!(in_alpha[0]["max_pad"], "Large");
    let traders = ed_api::stations::near(&pool, (0.0, 0.0, 0.0), Some("materialtrader"), 50.0, None, &q).await.unwrap();
    assert_eq!(traders.len(), 1);
    assert_eq!(traders[0]["id"], 22);
    assert!((traders[0]["distance_ly"].as_f64().unwrap() - 10.0).abs() < 1e-6);
    let none = ed_api::stations::near(&pool, (0.0, 0.0, 0.0), Some("techbroker"), 50.0, None, &q).await.unwrap();
    assert!(none.is_empty(), "an empty sphere is a valid empty answer");
    let by_name = ed_api::stations::by_name(&pool, "b d", &q).await.unwrap();
    assert_eq!(by_name.len(), 1);
    assert_eq!(by_name[0]["system_name"], "Beta");
}
```

Run in WSL as in Task 4 Step 6 with `stations_answers`. Expected: ok. If `max_pad` serializes differently from `"Large"`, assert against `serde_json::to_value(PadSize::Large).unwrap()`.

- [ ] **Step 6: Commit**

```bash
git add crates/ed-api/Cargo.toml crates/ed-api/src/stations.rs crates/ed-api/src/http.rs crates/ed-api/src/lib.rs crates/ed-api/src/metrics.rs crates/ed-api/README.md crates/ed-api/tests/postgres.rs
git commit -m "ed-api: GET /v1/stations — the wire half of stations_in_system, nearest_service and find_station"
```

---

### Task 7: The limiter accepts a per-install key

**Files:**
- Modify: `crates/ed-api/src/http.rs:102-111` (`source_of`)

**Interfaces:**
- Produces: `X-EDDA-Install: <32 lowercase hex>` keys the limiters as `install:<id>`; anything else falls back to the first `X-Forwarded-For` hop as today.

- [ ] **Step 1: Failing test** (in `http.rs` `mod tests`)

```rust
    #[test]
    fn an_install_id_keys_the_limiter_and_a_bad_one_is_ignored() {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.9, 10.0.0.1".parse().unwrap());
        assert_eq!(source_of(&h), "203.0.113.9");
        h.insert("x-edda-install", "0123456789abcdef0123456789abcdef".parse().unwrap());
        assert_eq!(source_of(&h), "install:0123456789abcdef0123456789abcdef");
        h.insert("x-edda-install", "not-hex".parse().unwrap());
        assert_eq!(source_of(&h), "203.0.113.9");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p ed-api an_install_id_keys`
Expected: FAIL — second assertion (still the IP).

- [ ] **Step 3: Implement**

```rust
/// The limiter key: the install ID when the client sends one (API-only
/// spec, "Per-install keys" — a random 32-hex string that names an
/// install to the limiter and nothing else), else the first forwarded
/// hop. Per-IP budgets are shared by everyone behind one NAT.
fn source_of(headers: &HeaderMap) -> String {
    if let Some(id) = headers
        .get("x-edda-install")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| v.len() == 32 && v.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
    {
        return format!("install:{id}");
    }
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}
```

- [ ] **Step 4: Run** `cargo test -p ed-api an_install_id_keys` — PASS; `cargo test -p ed-api` — all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/ed-api/src/http.rs
git commit -m "ed-api: X-EDDA-Install keys the limiters when present — per-install budgets ahead of the client that sends them"
```

---

### Task 8: Ledger, and the branch on GitHub

**Files:**
- Modify: `docs/ROUTING-NEXT.md`

- [ ] **Step 1: Ledger entry** (append)

```markdown
## API-only Phase A.2/A.3/A.5 BUILT (review, 2026-09-07; not deployed)

- `/v1/trade/search` v2: a request carrying `ship` gets the full
  `ProfitReport` — `ed_route::profit::assemble` runs on the server over
  Postgres's candidate stations and fresh rows (`trade_report.rs`).
  Round trips pair from the FULL set: the remote-mode defect is fixed
  where the data is. Legacy legs answer kept for 0.2.9 clients.
  Measured first: docs/benches/trade-rows-pull-2026-09-07.csv (verdict:
  DEFAULT_MAX_STATIONS = <value>).
- **Gap, deliberate:** the carrier price envelope
  (`commodity_stats_by_symbol`) is not applied server-side; the server
  has no per-commodity stats. `include_carriers: false` (default) is
  unaffected. Returns when the server computes stats.
- `GET /v1/stations`: system / near+service / name; the assistant's five
  notes applied (index-first resolution for near=, cell predicate,
  vocabulary validated → 400, names::complete_stations reused,
  knowledge limiter). Service vocabulary from edda_dev's
  `station_services` (45 keys).
- `X-EDDA-Install` accepted as the limiter key (32 hex).
- Routing step 4 CLOSED on the assistant's evidence: the c=50 route_long 422s
  were `unknown_system` for the stale `Eol Prou RS-T d3-94` in the
  harness's FAR list, not a planner defect.
- Server-side acceptance before the client switches: the pinned-origin
  parity test (local `find_at` vs `/v1/trade/search` v2, maintainer's ship,
  Deciat/Wyrd/Sol) is the client plan's first task.
```

- [ ] **Step 2: Push the branch**

```bash
git push origin HEAD:refs/heads/api-only-server
```

- [ ] **Step 3: Commit the ledger**

```bash
git add docs/ROUTING-NEXT.md
git commit -m "Ledger: API-only Phase A.2/A.3/A.5 built, the carrier-envelope gap, routing step 4 closed"
```

---

## Self-review

**Spec coverage.** Trade report (ship in request, `ed-route::profit` server-side, full report, measured first) → Tasks 1, 3, 4, 5. Lookups on the wire (`/v1/stations` three modes; knowledge/system, sphere, market/station already exist) → Task 6. Metrics on every new endpoint → Tasks 5, 6. Routing steps 1–3 → the assistant session (excluded by the global constraint); step 4 → closed in Task 8. Per-install keys, server side → Task 7. Phase A.5 "accepted, ignored until sent" → Task 7 (it is used when sent, which the spec allows: "limiters key on it").

**Placeholder scan.** Task 6's `station_json` listing carries two throwaway lines marked for deletion; the step text says so and gives the real reading. The timestamp type is named as a lookup ("whichever the crate already reads TIMESTAMPTZ with") with the two candidates — acceptable because it is a one-grep fact, not a design decision. `DEFAULT_MAX_STATIONS` is set by Task 1's verdict, stated as the rule.

**Type consistency.** `Prepared { stations, rows, excluded, timing }` (Task 3) is what `trade_report::prepare` returns (Task 4) and `assemble` consumes (Tasks 3, 5). `ReportRequest.limit()` returns `usize`; `assemble` takes `usize`. `TradeOutcome::Legs(Value, &'static str)` is reused for the report path. `service_key` returns the stored value used in the SQL predicate. `Refusal` from `market_search` throughout.
