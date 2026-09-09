//! The profit finder.
//!
//! Given where the commander is and what they fly, find the trades that pay
//! best **per hour** -- not per ton, not per loop. A run that earns 30M in
//! twenty minutes beats one that earns 40M in forty-five, and every route
//! site that sorts by profit-per-ton quietly gets that wrong.
//!
//! # How it works
//!
//! 1. Collect candidate stations within `radius_ly` of the origin that have
//!    a market, fit the ship's pad, and (unless asked) are not fleet
//!    carriers. Stations whose pad size is unrecorded are **excluded** when a
//!    pad filter applies -- failing closed, because "probably fits" is how
//!    a Cutter ends up at an outpost.
//! 2. Pull every market row for those stations into memory (a few hundred
//!    stations × ~150 commodities) and pair sellers with buyers per
//!    commodity in Rust. That is far cheaper than a self-join over the
//!    99.8M-row market table, and keeps the whole scoring rule in one
//!    testable place.
//! 3. Score each leg: tons = min(cargo, supply, demand), profit = tons ×
//!    (sell − buy), time from [`crate::cost`], and rank by profit / hour.
//! 4. Round trips combine the best A→B leg with the best B→A leg.
//!
//! # What it refuses to hide
//!
//! * Every leg carries the age of the price data on both ends. Prices older
//!    than `max_age_hours` are dropped and counted in `excluded`, so a route
//!    that looks thin says *why* rather than showing stale gold.
//! * Time estimates inherit `Confidence::Estimated` from the cost model,
//!   because the supercruise term is not verified against the journal.
//!   Credits per hour is for *comparing* legs, never a promise.

use crate::cost::{self, Confidence, Ship};
use anyhow::Result;
use ed_domain::freshness;
use ed_store::lookup::{self, PadSize, StationClass};
use ed_store::market;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Search constraints. Everything optional has a defensible default.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Constraints {
    pub radius_ly: f64,
    /// Ship's pad requirement. `None` disables the filter *and* is reported
    /// so the caller can warn that outposts may be included.
    pub min_pad: Option<PadSize>,
    pub include_carriers: bool,
    /// Opt-in (maintainer, 2026-09-04): selling prohibited goods where a black
    /// market exists is the commander's choice. Off, such sales are
    /// silenced; on, they are allowed ONLY at black-market stations.
    pub include_prohibited: bool,
    /// Drop price data older than this.
    pub max_age_hours: f64,
    /// Skip stations further than this from the arrival star. Infinity
    /// (the default) is `null` on the wire: serde_json writes every
    /// non-finite float as null and would refuse to read one back.
    #[serde(with = "infinity_as_null")]
    pub max_arrival_ls: f64,
    /// Ignore legs whose demand is below this many tons.
    pub min_demand: i64,
    /// Ignore buy boards whose supply is below this many tons.
    pub min_supply: i64,
    /// Load markets for at most this many stations, nearest first. The
    /// rest are counted in `excluded.beyond_station_cap`.
    pub max_stations: usize,
    /// Longest multi-stop ring to search for (3 = triangle). 0 disables.
    pub max_stops: usize,
    /// The time constants legs are priced with: the commander's own when
    /// measured from the journal, the documented defaults otherwise.
    /// Optional on the wire, per field.
    #[serde(default)]
    pub timing: cost::Timing,
    /// Powerplay filters on the buy side and the sell side, matched
    /// case-insensitively against the system's controlling power and state.
    /// `None` = any. State `"none"` matches systems with no control.
    pub buy_power: Option<String>,
    pub buy_state: Option<String>,
    pub sell_power: Option<String>,
    pub sell_state: Option<String>,
    /// How the power filter matches: `controls` (default), `present`
    /// (controls or contesting), or `undermining` (present, not controlling).
    pub buy_power_mode: String,
    pub sell_power_mode: String,
    /// Longest single leg to consider. Bounds the pairing work; a leg
    /// beyond this never wins on cr/h anyway. 0 = unlimited.
    pub max_leg_ly: f64,
}

/// `f64::INFINITY` <-> JSON `null`, for the "no cap" constraint values.
mod infinity_as_null {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
        if v.is_finite() { s.serialize_some(v) } else { s.serialize_none() }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::INFINITY))
    }
}

impl Default for Constraints {
    fn default() -> Self {
        Constraints {
            radius_ly: 40.0,
            min_pad: None,
            include_carriers: false,
            include_prohibited: false,
            // Seven days. Measured against the real database from Wongi: with
            // 72 h only 117 of 6,795 local rows survived and no leg existed,
            // because the bulk dump is refreshed by EDDN only while the app
            // runs. A week keeps the bubble usable; the age is shown anyway.
            max_age_hours: 168.0,
            // No cap by default: the time model already charges for a long
            // supercruise, and a hard cutoff hid an 8,374 ls Coriolis that was
            // the best sale in 120 ly.
            max_arrival_ls: f64::INFINITY,
            min_demand: 1,
            min_supply: 1,
            max_stations: MAX_STATIONS,
            // Rings are opt-in (maintainer, 2026-09-05: "default trade search
            // to rings off"): the ring pass roughly doubled a bubble
            // search (6.7 s -> 3.1 s rings-off, measured on the live-DB
            // copy) and most searches want the best out-and-back, not a
            // tour.
            max_stops: 0,
            timing: cost::Timing::default(),
            buy_power: None,
            buy_state: None,
            sell_power: None,
            sell_state: None,
            buy_power_mode: "controls".into(),
            sell_power_mode: "controls".into(),
            max_leg_ly: 150.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationRef {
    pub station_id: i64,
    pub station: String,
    pub system: String,
    pub system_id64: i64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub arrival_ls: Option<f64>,
    pub max_pad: Option<PadSize>,
    pub class: StationClass,
    pub is_carrier: bool,
    pub controlling_power: Option<String>,
    pub power_state: Option<String>,
    /// Every power present in the system, controlling or contesting.
    pub powers: Vec<String>,
}

impl StationRef {
    pub fn distance_to(&self, other: &StationRef) -> f64 {
        let (dx, dy, dz) = (self.x - other.x, self.y - other.y, self.z - other.z);
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// One buy-here-sell-there trade.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Leg {
    pub from: StationRef,
    pub to: StationRef,
    pub symbol: String,
    pub commodity: String,
    pub buy_price: i64,
    pub sell_price: i64,
    pub profit_per_ton: i64,
    pub supply: i64,
    pub demand: i64,
    /// Tons actually movable: min(cargo, supply, demand).
    pub tons: i64,
    pub profit: i64,
    pub distance_ly: f64,
    pub jumps: i64,
    /// Outbound only: the rate if you stop counting on arrival.
    pub duration: cost::Duration,
    pub profit_per_hour: f64,
    /// Flying back empty to do it again. This is the rate legs are ranked
    /// by, so a one-way number can never outshine the loop it belongs to.
    pub return_duration: cost::Duration,
    pub profit_per_hour_repeat: f64,
    pub buy_age_hours: f64,
    pub sell_age_hours: f64,
    /// Further commodities filling the hold after the primary one ran out
    /// of supply or demand. `tons` and `profit` on the leg include them.
    pub extra: Vec<CargoLine>,
}

/// One more commodity sharing the hold on a leg.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CargoLine {
    pub symbol: String,
    pub commodity: String,
    pub tons: i64,
    pub profit_per_ton: i64,
    pub profit: i64,
}

/// A→B→A with the best commodity each way.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundTrip {
    pub out: Leg,
    pub back: Leg,
    pub profit: i64,
    pub duration: cost::Duration,
    pub profit_per_hour: f64,
}

/// Why candidate stations were dropped, so a thin result explains itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Excluded {
    pub carriers: usize,
    pub pad_too_small: usize,
    pub pad_unknown: usize,
    pub too_far_from_star: usize,
    pub no_market_data: usize,
    pub stale_price_rows: usize,
    /// Stations inside the radius but past the nearest-N cap.
    pub beyond_station_cap: usize,
    /// Sell rows silenced because the station confiscates the commodity
    /// (market finding F2 — the store's prohibition rule, enforced).
    pub confiscated_sales: usize,
    /// Carrier prices outside the station envelope (maintainer rule: within
    /// one standard deviation of the extreme station prices, or ignored).
    pub carrier_price_outliers: usize,
}

/// Default cap on stations a single search will load markets for.
///
/// A 120 ly radius in the bubble covers tens of thousands of stations and
/// froze the UI for the duration. The nearest few thousand are where any
/// leg worth flying starts anyway; the rest are counted, not searched.
pub const MAX_STATIONS: usize = 2_500;

/// Where a search's wall time went, in the report itself so a slow
/// search arrives with its own diagnosis (doctrine rule 2; field case
/// 2026-09-05: 58 s from Ega with nothing saying which phase).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SearchTiming {
    pub candidates_ms: u64,
    pub market_ms: u64,
    pub guards_ms: u64,
    pub pairing_ms: u64,
    pub rings_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfitReport {
    pub origin: String,
    pub timing: SearchTiming,
    pub constraints: Constraints,
    pub ship: ShipSummary,
    pub stations_considered: usize,
    pub excluded: Excluded,
    pub legs: Vec<Leg>,
    pub round_trips: Vec<RoundTrip>,
    pub rings: Vec<Ring>,
    pub confidence: Confidence,
    pub note: String,
    /// Set by the app on an EMPTY report: whether the sphere had any
    /// observed boards at all (`{"status": "covered"|"gap", ...}`). An
    /// empty report over a gap is not "no profitable legs".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<serde_json::Value>,
    /// With a gap: whether the UI should offer the community API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offer: Option<bool>,
    /// Set when the community API answered because local data had a gap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// How far from the origin the farthest CONSIDERED station sits, in
    /// ly. Equal to the radius when the station cap did not bind; smaller
    /// when it did — the UI can say "searched 34 of 60 ly" instead of
    /// letting a capped search read as a full one (2026-09-09: a 60 ly
    /// Wongi search that silently stopped at 34 ly hid Inara's top route).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach_ly: Option<f64>,
    /// What the server did with the commander's own docked board when
    /// the request carried one (B.4 gap 2); absent when it did not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub board: Option<BoardVerdict>,
}

/// The server's verdict on a request's docked board: `used` with reason
/// `newer` / `absent`, or not, with `older` / `mismatch` /
/// `invalid_timestamp` / `unknown_station`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BoardVerdict {
    pub used: bool,
    pub reason: String,
    /// Rows the board contributed.
    #[serde(default)]
    pub rows: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ShipSummary {
    pub cargo_capacity: i64,
    pub jump_range_ly: f64,
    pub laden_range_ly: f64,
}

/// Public so the API-leg composer (remote trade, 2026-09-06) can feed
/// [`make_leg`] the same rows the local finder does — one owner for
/// the leg math.
#[derive(Debug, Clone)]
pub struct MarketRow {
    pub station_id: i64,
    pub symbol: String,
    pub name: Option<String>,
    pub buy_price: i64,
    pub sell_price: i64,
    pub demand: i64,
    pub supply: i64,
    pub age_hours: f64,
}

/// Cooperative control for a long search: a cancel check and a progress
/// sink, polled between station batches. Both are optional no-ops.
pub struct SearchControl<'a> {
    pub cancelled: &'a dyn Fn() -> bool,
    /// `(stations_done, stations_total)`.
    pub progress: &'a dyn Fn(usize, usize),
    /// `(commodities_paired, commodities_total)`.
    pub pairing: &'a dyn Fn(usize, usize),
}

impl SearchControl<'_> {
    pub fn none() -> SearchControl<'static> {
        SearchControl {
            cancelled: &|| false,
            progress: &|_, _| {},
            pairing: &|_, _| {},
        }
    }
}

/// The error a cancelled search returns, so callers can tell it from a
/// real failure.
pub const CANCELLED: &str = "search cancelled";

/// Current time as journal-style seconds, injectable for tests.
pub fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Candidate stations around a point.
fn candidate_stations(
    conn: &Connection,
    origin: (f64, f64, f64),
    c: &Constraints,
    excluded: &mut Excluded,
) -> Result<Vec<StationRef>> {
    let mut out = Vec::new();
    for st in lookup::market_stations_within(conn, origin, c.radius_ly)? {
        let s = StationRef {
            station_id: st.station_id,
            station: st.station,
            system: st.system,
            system_id64: st.system_id64,
            x: st.x,
            y: st.y,
            z: st.z,
            arrival_ls: st.arrival_ls,
            max_pad: st.max_pad,
            class: StationClass::of(st.kind.as_deref()),
            is_carrier: st.is_carrier,
            controlling_power: st.controlling_power,
            power_state: st.power_state,
            powers: st.powers,
        };
        if s.is_carrier && !c.include_carriers {
            excluded.carriers += 1;
            continue;
        }
        if s.arrival_ls.unwrap_or(0.0) > c.max_arrival_ls {
            excluded.too_far_from_star += 1;
            continue;
        }
        if let Some(need) = c.min_pad {
            match s.max_pad {
                Some(have) if have >= need => {}
                Some(_) => {
                    excluded.pad_too_small += 1;
                    continue;
                }
                None => {
                    excluded.pad_unknown += 1;
                    continue;
                }
            }
        }
        out.push(s);
    }
    Ok(out)
}

/// Above this many candidate stations, one indexed scan of every row
/// fresher than the price window beats hundreds of thousands of per-station
/// lookups. Only used when the store can scan by freshness.
const WIDE_SCAN_STATIONS: usize = 20_000;

fn market_rows(
    conn: &Connection,
    stations: &[StationRef],
    c: &Constraints,
    now: i64,
    excluded: &mut Excluded,
    ctl: &SearchControl,
) -> Result<Vec<MarketRow>> {
    if stations.len() >= WIDE_SCAN_STATIONS && market::wide_scan_available(conn) {
        return market_rows_wide(conn, stations, c, now, excluded, ctl);
    }
    market_rows_narrow(conn, stations, c, now, excluded, ctl)
}

/// Galaxy-wide: every row inside the price window, then keep the ones at
/// candidate stations. Stale rows are never read, so `stale_price_rows`
/// stays zero on this path and `no_market_data` counts stations with no
/// fresh row at all.
fn market_rows_wide(
    conn: &Connection,
    stations: &[StationRef],
    c: &Constraints,
    now: i64,
    excluded: &mut Excluded,
    ctl: &SearchControl,
) -> Result<Vec<MarketRow>> {
    let mut scanned: usize = 0;
    let cutoff = now - (c.max_age_hours * 3600.0) as i64;
    let wanted: std::collections::HashSet<i64> = stations.iter().map(|s| s.station_id).collect();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let mut out = Vec::new();
    market::scan_rows_since(conn, cutoff, |row| {
        scanned += 1;
        if scanned.is_multiple_of(100_000) {
            if (ctl.cancelled)() {
                anyhow::bail!(CANCELLED);
            }
            (ctl.progress)(seen.len(), stations.len());
        }
        if !wanted.contains(&row.station_id) {
            return Ok(true);
        }
        seen.insert(row.station_id);
        out.push(market_row(row, now));
        Ok(true)
    })?;
    excluded.no_market_data += wanted.len() - seen.len();
    Ok(out)
}

fn market_row(row: market::PricedRow, now: i64) -> MarketRow {
    MarketRow {
        station_id: row.station_id,
        symbol: row.symbol,
        name: row.name,
        buy_price: row.buy_price,
        sell_price: row.sell_price,
        demand: row.demand,
        supply: row.supply,
        age_hours: freshness::age_hours(now, row.updated),
    }
}

fn market_rows_narrow(
    conn: &Connection,
    stations: &[StationRef],
    c: &Constraints,
    now: i64,
    excluded: &mut Excluded,
    ctl: &SearchControl,
) -> Result<Vec<MarketRow>> {
    let mut out = Vec::new();
    for (i, st) in stations.iter().enumerate() {
        if i % 250 == 0 {
            if (ctl.cancelled)() {
                anyhow::bail!(CANCELLED);
            }
            (ctl.progress)(i, stations.len());
        }
        let rows = market::rows_for_station(conn, st.station_id)?;
        if rows.is_empty() {
            excluded.no_market_data += 1;
            continue;
        }
        for row in rows {
            let row = market_row(row, now);
            if row.age_hours > c.max_age_hours {
                excluded.stale_price_rows += 1;
                continue;
            }
            out.push(row);
        }
    }
    Ok(out)
}

/// Zero the SELL side of rows the finder must never sell into: stations
/// the store says confiscate the commodity (unless the commander opted
/// into black-market sales and one exists there), and carrier prices
/// outside the station envelope. The BUY side of a confiscating
/// station's row stays: buying there is legal and its supply data is
/// unaffected. Zeroed demand means the pairing can never pick the
/// station as a destination for that commodity.
fn silence_untradeable_sells(
    conn: &Connection,
    stations: &[StationRef],
    rows: &mut [MarketRow],
    include_prohibited: bool,
) -> Result<(usize, usize)> {
    let stats = market::commodity_stats_by_symbol(conn).unwrap_or_default();
    let mut prohibited: std::collections::HashMap<i64, std::collections::HashSet<String>> =
        std::collections::HashMap::new();
    for st in stations {
        let confiscates = market::prohibited_symbols(conn, st.station_id)?;
        if confiscates.is_empty() {
            continue;
        }
        // Opted in, the sale is allowed exactly where a black market
        // exists to take the goods; elsewhere it stays impossible.
        if include_prohibited && market::has_black_market(conn, st.station_id)? {
            continue;
        }
        prohibited.insert(st.station_id, confiscates);
    }
    let carriers: std::collections::HashSet<i64> =
        stations.iter().filter(|s| s.is_carrier).map(|s| s.station_id).collect();
    let mut enveloped = 0usize;
    for row in rows.iter_mut() {
        let stat = stats.get(&row.symbol);
        // Carrier prices live inside the station envelope or not at all
        // (maintainer rule 2026-09-04) — both sides, since carriers set both
        // freely. STATION prices are never envelope- or spike-filtered:
        // the 302k palladium board this pass once silenced as "poison"
        // turned out to be a real demand spike Inara also shows (maintainer
        // cross-check, same day) — see the ledger retraction.
        if carriers.contains(&row.station_id) {
            if row.sell_price > 0 && stat.is_some_and(|s| s.carrier_sell_outside(row.sell_price)) {
                row.sell_price = 0;
                row.demand = 0;
                enveloped += 1;
            }
            if row.buy_price > 0 && stat.is_some_and(|s| s.carrier_buy_outside(row.buy_price)) {
                row.buy_price = 0;
                row.supply = 0;
                enveloped += 1;
            }
        }
    }
    Ok((silence_prohibited(rows, &prohibited), enveloped))
}

/// Zero the sale side of every row a station confiscates; returns how
/// many. Pure, so the server applies the same rule from Postgres's
/// `station_prohibited` that the local finder applies from SQLite.
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

/// Public with `MarketRow` and [`round_trips`]: the remote-trade
/// composer builds legs from API rows through exactly this math.
pub fn make_leg(from: &StationRef, to: &StationRef, a: &MarketRow, b: &MarketRow, ship: &Ship, timing: &cost::Timing) -> Leg {
    let tons = ship.cargo_capacity.min(a.supply).min(b.demand).max(0);
    let per_ton = b.sell_price - a.buy_price;
    let profit = per_ton * tons;
    let distance = from.distance_to(to);
    let (duration, return_duration, cycle_hours) = cycle(from, to, distance, ship, timing);
    Leg {
        from: from.clone(),
        to: to.clone(),
        symbol: a.symbol.clone(),
        commodity: a
            .name
            .clone()
            .or_else(|| b.name.clone())
            .unwrap_or_else(|| a.symbol.clone()),
        buy_price: a.buy_price,
        sell_price: b.sell_price,
        profit_per_ton: per_ton,
        supply: a.supply,
        demand: b.demand,
        tons,
        profit,
        distance_ly: distance,
        jumps: cost::jump_count(distance, ship.laden_range_ly),
        profit_per_hour: profit as f64 / duration.hours().max(1e-6),
        profit_per_hour_repeat: profit as f64 / cycle_hours.max(1e-6),
        duration,
        return_duration,
        buy_age_hours: a.age_hours,
        sell_age_hours: b.age_hours,
        extra: Vec::new(),
    }
}

/// Loaded out, empty back: different ranges, different jump counts. The
/// one place the leg's timing arithmetic lives — `make_leg` and the
/// pre-materialisation score both use it.
fn cycle(from: &StationRef, to: &StationRef, distance: f64, ship: &Ship, timing: &cost::Timing) -> (cost::Duration, cost::Duration, f64) {
    let duration = timing.leg_seconds_at_range(distance, to.arrival_ls.unwrap_or(0.0), ship.laden_range_ly);
    let return_duration = timing.leg_seconds_at_range(distance, from.arrival_ls.unwrap_or(0.0), ship.jump_range_ly);
    let cycle_hours = (duration.seconds + return_duration.seconds) / 3600.0;
    (duration, return_duration, cycle_hours)
}

/// `profit_per_hour_repeat` of a pair's primary commodity before the hold
/// is filled: `make_leg`'s number without the allocations.
fn pair_rate(from: &StationRef, to: &StationRef, per_ton: i64, avail: i64, ship: &Ship, timing: &cost::Timing) -> f64 {
    let tons = ship.cargo_capacity.min(avail).max(0);
    let (_, _, cycle_hours) = cycle(from, to, from.distance_to(to), ship, timing);
    (per_ton * tons) as f64 / cycle_hours.max(1e-6)
}

/// How many (from, to) pairs become `Leg`s. A Leg clones two StationRefs
/// and three strings; every profitable pair among 2,653 fresh stations
/// (Sol / 100 ly / 48 h, 2026-09-09) is ~7M legs, which is how ed-api
/// reached 13.8 GB with 474 MB left on the box. Past this bound pairs
/// are ranked by `pair_rate` — the primary commodity's repeat rate,
/// the hold fill not yet counted — and only the best become Legs. Under
/// it the pipeline is exact; the bubble cases (≤ 211 stations, ≤ 44k
/// pairs) never reach it.
const MATERIALISED_LEGS: usize = 100_000;

/// Fill whatever hold the primary commodity left with the next-best ones.
/// Every ton weighs the same, so greedy by profit per ton is exact.
fn fill_hold(leg: &mut Leg, cands: &[(i64, i64, &MarketRow, &MarketRow)], ship: &Ship) {
    let mut room = ship.cargo_capacity - leg.tons;
    for (per_ton, avail, a, b) in cands.iter().skip(1) {
        if room <= 0 {
            break;
        }
        let t = (*avail).min(room);
        if t <= 0 || *per_ton <= 0 {
            continue;
        }
        leg.extra.push(CargoLine {
            symbol: a.symbol.clone(),
            commodity: a
                .name
                .clone()
                .or_else(|| b.name.clone())
                .unwrap_or_else(|| a.symbol.clone()),
            tons: t,
            profit_per_ton: *per_ton,
            profit: per_ton * t,
        });
        leg.tons += t;
        leg.profit += per_ton * t;
        room -= t;
    }
    if !leg.extra.is_empty() {
        leg.profit_per_hour = leg.profit as f64 / leg.duration.hours().max(1e-6);
        let cycle = (leg.duration.seconds + leg.return_duration.seconds) / 3600.0;
        leg.profit_per_hour_repeat = leg.profit as f64 / cycle.max(1e-6);
    }
}

/// Best legs among a set of stations. `sources` restricts where cargo may
/// be bought; `None` means anywhere in the set.
fn best_legs(
    stations: &[StationRef],
    rows: &[MarketRow],
    sources: Option<&[i64]>,
    ship: &Ship,
    c: &Constraints,
    ctl: &SearchControl,
) -> Vec<Leg> {
    let by_id: HashMap<i64, &StationRef> = stations.iter().map(|s| (s.station_id, s)).collect();
    let pp_ok =
        |s: &StationRef, power: &Option<String>, mode: &str, state: &Option<String>| -> bool {
            let power_ok = match power {
                None => true,
                Some(p) => {
                    let controls = s
                        .controlling_power
                        .as_deref()
                        .is_some_and(|c| c.eq_ignore_ascii_case(p));
                    let present = controls || s.powers.iter().any(|x| x.eq_ignore_ascii_case(p));
                    match mode {
                        "present" => present,
                        "undermining" => present && !controls,
                        _ => controls,
                    }
                }
            };
            let state_ok = match state {
                None => true,
                Some(st) if st.eq_ignore_ascii_case("none") => {
                    s.power_state.is_none() && s.controlling_power.is_none()
                }
                Some(st) => s
                    .power_state
                    .as_deref()
                    .is_some_and(|x| x.eq_ignore_ascii_case(st)),
            };
            power_ok && state_ok
        };
    let buy_ok: std::collections::HashSet<i64> = stations
        .iter()
        .filter(|s| pp_ok(s, &c.buy_power, &c.buy_power_mode, &c.buy_state))
        .map(|s| s.station_id)
        .collect();
    let sell_ok: std::collections::HashSet<i64> = stations
        .iter()
        .filter(|s| pp_ok(s, &c.sell_power, &c.sell_power_mode, &c.sell_state))
        .map(|s| s.station_id)
        .collect();

    // Group by commodity.
    let mut sellers: HashMap<&str, Vec<&MarketRow>> = HashMap::new();
    let mut buyers: HashMap<&str, Vec<&MarketRow>> = HashMap::new();
    for r in rows {
        if r.buy_price > 0
            && r.supply >= c.min_supply.max(1)
            && buy_ok.contains(&r.station_id)
            && sources.is_none_or(|s| s.contains(&r.station_id))
        {
            sellers.entry(&r.symbol).or_default().push(r);
        }
        if r.sell_price > 0 && r.demand >= c.min_demand && sell_ok.contains(&r.station_id) {
            buyers.entry(&r.symbol).or_default().push(r);
        }
    }

    // Best leg per (from, to) pair -- one commodity per hop.
    //
    // Galaxy-wide, a commodity can have 50k sellers and 100k buyers; pairing
    // them all is billions of legs, almost all of them hundreds of light
    // years long and hopeless on cr/h. So buyers are bucketed on a grid of
    // `max_leg_ly` cells and each seller only meets buyers in the 27 cells
    // around it -- pairing cost follows local density, not galaxy size.
    let cell = if c.max_leg_ly > 0.0 {
        c.max_leg_ly
    } else {
        f64::INFINITY
    };
    let cell_of = |s: &StationRef| -> (i64, i64, i64) {
        if cell.is_finite() {
            (
                (s.x / cell).floor() as i64,
                (s.y / cell).floor() as i64,
                (s.z / cell).floor() as i64,
            )
        } else {
            (0, 0, 0)
        }
    };
    let neighbours: Vec<(i64, i64, i64)> = if cell.is_finite() {
        (-1..=1)
            .flat_map(|dx| (-1..=1).flat_map(move |dy| (-1..=1).map(move |dz| (dx, dy, dz))))
            .collect()
    } else {
        vec![(0, 0, 0)]
    };

    // Up to this many commodities per pair are remembered for the hold fill.
    const PER_PAIR: usize = 4;
    let mut cands: HashMap<(i64, i64), Vec<(i64, i64, &MarketRow, &MarketRow)>> = HashMap::new();
    let total_symbols = sellers.len();
    for (i, (symbol, sells)) in sellers.iter().enumerate() {
        if i % 25 == 0 {
            if (ctl.cancelled)() {
                return Vec::new();
            }
            (ctl.pairing)(i, total_symbols);
        }
        let Some(buys) = buyers.get(symbol) else {
            continue;
        };
        let mut grid: HashMap<(i64, i64, i64), Vec<&MarketRow>> = HashMap::new();
        for b in buys {
            if let Some(st) = by_id.get(&b.station_id) {
                grid.entry(cell_of(st)).or_default().push(b);
            }
        }
        for a in sells {
            let Some(from) = by_id.get(&a.station_id) else {
                continue;
            };
            let (cx, cy, cz) = cell_of(from);
            for (dx, dy, dz) in &neighbours {
                let Some(bucket) = grid.get(&(cx + dx, cy + dy, cz + dz)) else {
                    continue;
                };
                for b in bucket {
                    if a.station_id == b.station_id || b.sell_price <= a.buy_price {
                        continue;
                    }
                    let Some(to) = by_id.get(&b.station_id) else {
                        continue;
                    };
                    if cell.is_finite() && from.distance_to(to) > c.max_leg_ly {
                        continue;
                    }
                    let avail = a.supply.min(b.demand);
                    let per_ton = b.sell_price - a.buy_price;
                    if avail <= 0 || per_ton <= 0 {
                        continue;
                    }
                    let _ = to;
                    let v = cands.entry((a.station_id, b.station_id)).or_default();
                    v.push((per_ton, avail, *a, *b));
                    if v.len() > PER_PAIR {
                        v.sort_by(|x, y| y.0.cmp(&x.0));
                        v.truncate(PER_PAIR);
                    }
                }
            }
        }
    }
    (ctl.pairing)(total_symbols, total_symbols);

    let keep: Option<std::collections::HashSet<(i64, i64)>> = if cands.len() > MATERIALISED_LEGS {
        let mut scored: Vec<(f64, (i64, i64))> = cands
            .iter()
            .filter_map(|(&key, v)| {
                let (from, to) = (by_id.get(&key.0)?, by_id.get(&key.1)?);
                let &(per_ton, avail, _, _) = v.iter().max_by_key(|x| x.0)?;
                Some((pair_rate(from, to, per_ton, avail, ship, &c.timing), key))
            })
            .collect();
        scored.select_nth_unstable_by(MATERIALISED_LEGS - 1, |a, b| b.0.total_cmp(&a.0));
        scored.truncate(MATERIALISED_LEGS);
        Some(scored.into_iter().map(|(_, key)| key).collect())
    } else {
        None
    };
    let mut legs: Vec<Leg> = Vec::with_capacity(cands.len().min(MATERIALISED_LEGS));
    for ((from_id, to_id), mut v) in cands {
        if keep.as_ref().is_some_and(|k| !k.contains(&(from_id, to_id))) {
            continue;
        }
        v.sort_by(|x, y| y.0.cmp(&x.0));
        let (Some(from), Some(to)) = (by_id.get(&from_id), by_id.get(&to_id)) else {
            continue;
        };
        let (_, _, a, b) = v[0];
        let mut leg = make_leg(from, to, a, b, ship, &c.timing);
        if leg.tons <= 0 {
            continue;
        }
        fill_hold(&mut leg, &v, ship);
        legs.push(leg);
    }
    legs.sort_by(|a, b| {
        b.profit_per_hour_repeat
            .total_cmp(&a.profit_per_hour_repeat)
    });
    legs
}

/// How many legs may share one (source station, commodity).
///
/// Against real data a single mineral platform filled every top slot with
/// the same silver run to a dozen different buyers. Three keeps the best
/// alternatives visible without hiding the winner.
const PER_SOURCE_COMMODITY: usize = 3;

fn diversify(legs: Vec<Leg>) -> Vec<Leg> {
    let mut counts: HashMap<(i64, String), usize> = HashMap::new();
    legs.into_iter()
        .filter(|l| {
            let n = counts
                .entry((l.from.station_id, l.symbol.clone()))
                .or_insert(0);
            *n += 1;
            *n <= PER_SOURCE_COMMODITY
        })
        .collect()
}

/// A multi-stop ring: three or more stations, every leg loaded, closed
/// back to the start. Beats an A⇄B loop whenever the return commodity is
/// weak -- a third station that buys what B sells usually is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ring {
    pub legs: Vec<Leg>,
    pub stops: usize,
    pub profit: i64,
    pub duration: cost::Duration,
    pub profit_per_hour: f64,
}

/// Beam search over the best-leg graph.
///
/// Each station keeps its `FANOUT` best outgoing legs; paths extend from
/// the `BEAM` most profitable partial rings per depth; a path closes when
/// a leg back to its start exists (looked up in the full leg index, not
/// just the fan-out). Exact optimisation over hundreds of thousands of
/// stations is out of reach; this finds strong rings in well under a
/// second and is the seat an ant-colony search would take over later.
fn rings(legs: &[Leg], origin: (f64, f64, f64), max_stops: usize, limit: usize) -> Vec<Ring> {
    const FANOUT: usize = 6;
    const BEAM: usize = 400;
    if max_stops < 3 || legs.is_empty() {
        return Vec::new();
    }
    let index: HashMap<(i64, i64), &Leg> = legs
        .iter()
        .map(|l| ((l.from.station_id, l.to.station_id), l))
        .collect();
    let mut out: HashMap<i64, Vec<&Leg>> = HashMap::new();
    for l in legs {
        out.entry(l.from.station_id).or_default().push(l);
    }
    for v in out.values_mut() {
        v.sort_by(|a, b| b.profit_per_hour.total_cmp(&a.profit_per_hour));
        v.truncate(FANOUT);
    }

    struct Path<'a> {
        legs: Vec<&'a Leg>,
        profit: i64,
        secs: f64,
    }
    let rate = |p: i64, s: f64| p as f64 / (s / 3600.0).max(1e-6);

    let mut seeds: Vec<&Leg> = legs.iter().collect();
    seeds.sort_by(|a, b| b.profit_per_hour.total_cmp(&a.profit_per_hour));
    seeds.truncate(BEAM);
    let mut beam: Vec<Path> = seeds
        .into_iter()
        .map(|l| Path {
            legs: vec![l],
            profit: l.profit,
            secs: l.duration.seconds,
        })
        .collect();

    let mut found: Vec<Ring> = Vec::new();
    for _depth in 2..=max_stops {
        let mut next: Vec<Path> = Vec::new();
        for p in &beam {
            let last = p.legs[p.legs.len() - 1];
            let start = p.legs[0].from.station_id;
            let visited: std::collections::HashSet<i64> =
                p.legs.iter().map(|l| l.from.station_id).collect();
            // Close the ring if a leg home exists and it is at least a triangle.
            if p.legs.len() >= 2 {
                if let Some(home) = index.get(&(last.to.station_id, start)) {
                    let profit = p.profit + home.profit;
                    let secs = p.secs + home.duration.seconds;
                    let mut ring_legs: Vec<Leg> = p.legs.iter().map(|l| (*l).clone()).collect();
                    ring_legs.push((*home).clone());
                    found.push(Ring {
                        stops: ring_legs.len(),
                        legs: ring_legs,
                        profit,
                        duration: cost::Duration {
                            seconds: secs,
                            confidence: Confidence::Estimated,
                        },
                        profit_per_hour: rate(profit, secs),
                    });
                }
            }
            if let Some(outs) = out.get(&last.to.station_id) {
                for l in outs {
                    if visited.contains(&l.to.station_id) || l.to.station_id == start {
                        continue;
                    }
                    let mut legs2 = p.legs.clone();
                    legs2.push(l);
                    next.push(Path {
                        legs: legs2,
                        profit: p.profit + l.profit,
                        secs: p.secs + l.duration.seconds,
                    });
                }
            }
        }
        next.sort_by(|a, b| rate(b.profit, b.secs).total_cmp(&rate(a.profit, a.secs)));
        next.truncate(BEAM);
        beam = next;
        if beam.is_empty() {
            break;
        }
    }

    found.sort_by(|a, b| b.profit_per_hour.total_cmp(&a.profit_per_hour));
    // The same station set found via different entry points is one ring.
    let mut seen: std::collections::HashSet<Vec<i64>> = std::collections::HashSet::new();
    found.retain(|r| {
        let mut key: Vec<i64> = r.legs.iter().map(|l| l.from.station_id).collect();
        key.sort_unstable();
        seen.insert(key)
    });
    found.truncate(limit);
    // Start each ring at the station nearest the search origin -- the
    // order the commander would actually fly it from where they are.
    for r in found.iter_mut() {
        let d2 = |s: &StationRef| {
            (s.x - origin.0).powi(2) + (s.y - origin.1).powi(2) + (s.z - origin.2).powi(2)
        };
        if let Some((start, _)) = r
            .legs
            .iter()
            .enumerate()
            .map(|(i, l)| (i, d2(&l.from)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
        {
            r.legs.rotate_left(start);
        }
    }
    found
}

pub fn round_trips(legs: &[Leg], limit: usize) -> Vec<RoundTrip> {
    let index: HashMap<(i64, i64), &Leg> = legs
        .iter()
        .map(|l| ((l.from.station_id, l.to.station_id), l))
        .collect();
    let mut trips = Vec::new();
    for out in legs {
        let key = (out.to.station_id, out.from.station_id);
        let Some(back) = index.get(&key) else {
            continue;
        };
        // Each pair once, by the direction whose outbound is more lucrative;
        // a dead heat (same profit both ways, same distance) is broken by
        // station id, or the loop would be listed twice.
        if out.profit_per_hour < back.profit_per_hour
            || (out.profit_per_hour == back.profit_per_hour && out.from.station_id > back.from.station_id)
        {
            continue;
        }
        let seconds = out.duration.seconds + back.duration.seconds;
        let profit = out.profit + back.profit;
        let duration = cost::Duration {
            seconds,
            confidence: Confidence::Estimated,
        };
        trips.push(RoundTrip {
            out: out.clone(),
            back: (*back).clone(),
            profit,
            profit_per_hour: profit as f64 / duration.hours().max(1e-6),
            duration,
        });
    }
    trips.sort_by(|a, b| b.profit_per_hour.total_cmp(&a.profit_per_hour));
    trips.truncate(limit);
    trips
}

/// Best trades starting from `origin`.
///
/// `from_station` limits purchases to that one station (the "I'm docked,
/// what do I fill up with" question); `None` searches every station in
/// range as a source (the "where should I go trade tonight" question).
pub fn find(
    conn: &Connection,
    origin_name: &str,
    origin: (f64, f64, f64),
    from_station: Option<i64>,
    ship: &Ship,
    c: &Constraints,
    limit: usize,
    ctl: &SearchControl,
) -> Result<ProfitReport> {
    find_at(
        conn,
        origin_name,
        origin,
        from_station,
        ship,
        c,
        limit,
        now_epoch_secs(),
        ctl,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn find_at(
    conn: &Connection,
    origin_name: &str,
    origin: (f64, f64, f64),
    from_station: Option<i64>,
    ship: &Ship,
    c: &Constraints,
    limit: usize,
    now: i64,
    ctl: &SearchControl,
) -> Result<ProfitReport> {
    let mut excluded = Excluded::default();
    let mut timing = SearchTiming::default();
    let phase = std::time::Instant::now();
    let mut stations = candidate_stations(conn, origin, c, &mut excluded)?;
    let cap = c.max_stations.max(1);
    if cap != usize::MAX && stations.len() > cap {
        let (ox, oy, oz) = origin;
        let d2 = |s: &StationRef| (s.x - ox).powi(2) + (s.y - oy).powi(2) + (s.z - oz).powi(2);
        stations.sort_by(|a, b| d2(a).total_cmp(&d2(b)));
        excluded.beyond_station_cap = stations.len() - cap;
        stations.truncate(cap);
    }
    timing.candidates_ms = phase.elapsed().as_millis() as u64;
    let phase = std::time::Instant::now();
    let mut rows = market_rows(conn, &stations, c, now, &mut excluded, ctl)?;
    timing.market_ms = phase.elapsed().as_millis() as u64;
    let phase = std::time::Instant::now();
    let (confiscated, enveloped) =
        silence_untradeable_sells(conn, &stations, &mut rows, c.include_prohibited)?;
    excluded.confiscated_sales = confiscated;
    excluded.carrier_price_outliers = enveloped;
    timing.guards_ms = phase.elapsed().as_millis() as u64;
    let prepared = Prepared { stations, rows, excluded, timing };
    Ok(assemble(origin_name, origin, from_station, ship, c, limit, prepared, ctl))
}

/// The pipeline after the data is in hand: best legs, round trips from
/// the FULL set, rings, then the diversified, truncated list. One
/// implementation for the local finder and the server (API-only spec).
#[allow(clippy::too_many_arguments)]
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
    // The undock term follows the ship's pad unless the commander
    // measured it (maintainer, 2026-09-09) — resolved once here, for the
    // server and the client alike, whatever the wire carried.
    let resolved = Constraints { timing: c.timing.resolve(c.min_pad), ..c.clone() };
    let c = &resolved;
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
    let (ox, oy, oz) = origin;
    let reach_ly = stations
        .iter()
        .map(|s| ((s.x - ox).powi(2) + (s.y - oy).powi(2) + (s.z - oz).powi(2)).sqrt())
        .fold(None, |m: Option<f64>, d| Some(m.map_or(d, |m| m.max(d))));

    ProfitReport {
        origin: origin_name.to_string(),
        timing,
        constraints: c.clone(),
        ship: ShipSummary {
            cargo_capacity: ship.cargo_capacity,
            jump_range_ly: ship.jump_range_ly,
            laden_range_ly: ship.laden_range_ly,
        },
        stations_considered: stations.len(),
        excluded,
        reach_ly,
        board: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Round trips pair from the FULL leg set: with `limit: 1` the legs
    /// list is one long but the loop is still found. This is the property
    /// the remote path lost (API-only spec, first defect).
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

    /// Past MATERIALISED_LEGS profitable pairs, only the best-rated become
    /// Legs (2026-09-09: 2,653 fresh stations → ~7M Legs → 13.8 GB). Under
    /// it every pair still does. 400 stations that all trade gold both
    /// ways are 159,600 profitable pairs; the winner is the shortest hop.
    #[test]
    fn pairs_past_the_materialisation_bound_are_ranked_before_they_become_legs() {
        let station = |id: i64| StationRef {
            station_id: id, station: format!("S{id}"), system: format!("Sys{id}"), system_id64: id * 10,
            x: (id % 20) as f64 * 3.0, y: (id / 20) as f64 * 3.0, z: 0.0,
            arrival_ls: Some(100.0), max_pad: Some(PadSize::Large),
            class: StationClass::of(Some("Coriolis")), is_carrier: false,
            controlling_power: None, power_state: None, powers: Vec::new(),
        };
        let row = |st: i64| MarketRow {
            station_id: st, symbol: "gold".into(), name: None, buy_price: 100, sell_price: 200,
            demand: 1000, supply: 1000, age_hours: 1.0,
        };
        let ship = Ship { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 };
        let c = Constraints { max_leg_ly: 0.0, ..Constraints::default() };
        let stations: Vec<StationRef> = (1..=400).map(station).collect();
        let rows: Vec<MarketRow> = (1..=400).map(row).collect();
        let legs = best_legs(&stations, &rows, None, &ship, &c, &SearchControl::none());
        assert_eq!(legs.len(), MATERIALISED_LEGS, "bounded, not 159,600");
        // Every hop inside laden range is one jump and ties on rate; the
        // winner must be one of those, never a multi-jump haul.
        assert_eq!(legs[0].jumps, 1, "the best leg is still a one-jump hop ({} ly)", legs[0].distance_ly);
        let longest = legs.iter().map(|l| l.distance_ly).fold(0.0, f64::max);
        assert!(longest < 60.0, "the long hops are what the bound dropped, longest kept {longest}");
        let few: Vec<StationRef> = (1..=100).map(station).collect();
        let few_rows: Vec<MarketRow> = (1..=100).map(row).collect();
        let legs = best_legs(&few, &few_rows, None, &ship, &c, &SearchControl::none());
        assert_eq!(legs.len(), 100 * 99, "under the bound every pair is a leg");
    }

    /// Equal cr/h both ways used to list the same loop twice (found by
    /// the server-side Postgres test, where the fixture is symmetric).
    #[test]
    fn a_tied_pair_is_one_round_trip() {
        let station = |id: i64, x: f64| StationRef {
            station_id: id, station: format!("S{id}"), system: format!("Sys{id}"), system_id64: id * 10,
            x, y: 0.0, z: 0.0, arrival_ls: None, max_pad: Some(PadSize::Large),
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
        let report = assemble("Sys1", (0.0, 0.0, 0.0), None, &ship, &Constraints::default(), 10, prepared, &SearchControl::none());
        assert_eq!(report.legs.len(), 2);
        assert_eq!(report.round_trips.len(), 1, "one loop, not one per direction");
        assert_eq!(report.round_trips[0].out.from.station_id, 1);
    }

    #[test]
    fn silence_prohibited_zeroes_the_sale_and_counts_it() {
        let mut rows = vec![
            MarketRow { station_id: 2, symbol: "gold".into(), name: None, buy_price: 0, sell_price: 200, demand: 10, supply: 0, age_hours: 0.0 },
            MarketRow { station_id: 2, symbol: "silver".into(), name: None, buy_price: 0, sell_price: 100, demand: 10, supply: 0, age_hours: 0.0 },
        ];
        let mut prohibited = std::collections::HashMap::new();
        prohibited.insert(2, std::collections::HashSet::from(["gold".to_string()]));
        assert_eq!(silence_prohibited(&mut rows, &prohibited), 1);
        assert_eq!((rows[0].sell_price, rows[0].demand), (0, 0));
        assert_eq!(rows[1].sell_price, 100);
    }

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
        let leg = make_leg(&station(1, 0.0), &station(2, 10.0), &buy, &sell, &ship, &cost::Timing::default());
        let report = ProfitReport {
            origin: "Sys1".into(), timing: SearchTiming::default(), constraints: Constraints::default(),
            ship: ShipSummary { cargo_capacity: 100, jump_range_ly: 30.0, laden_range_ly: 25.0 },
            stations_considered: 2, excluded: Excluded::default(), legs: vec![leg.clone()],
            round_trips: vec![RoundTrip { out: leg.clone(), back: leg.clone(), profit: 2 * leg.profit, profit_per_hour: 1.0, duration: leg.duration }],
            rings: vec![Ring { legs: vec![leg.clone()], stops: 3, profit: leg.profit, duration: leg.duration, profit_per_hour: 1.0 }],
            confidence: Confidence::Estimated, note: "n".into(), coverage: None, offer: None, fallback: None, reach_ly: None, board: None,
        };
        let text = serde_json::to_string(&report).unwrap();
        let back: ProfitReport = serde_json::from_str(&text).unwrap();
        assert_eq!(back.legs[0].profit, leg.profit);
        assert_eq!(back.round_trips.len(), 1);
        assert_eq!(back.rings[0].stops, 3);
        assert_eq!(back.legs[0].from.max_pad, Some(PadSize::Large));
        assert_eq!(back.note, "n");
    }

    /// `sys_market.updated` is an integer epoch. Reading it back through a
    /// `datetime()` string and a strict RFC-3339 parser made every row
    /// infinitely old, so the finder returned nothing.
    #[test]
    fn profit_rows_keep_their_age_when_updated_is_an_epoch() {
        let conn = db();
        let c = Constraints::default();
        let mut excluded = Excluded::default();
        let stations = candidate_stations(&conn, (0.0, 0.0, 0.0), &c, &mut excluded).unwrap();
        let rows = market_rows(
            &conn,
            &stations,
            &c,
            NOW,
            &mut excluded,
            &SearchControl::none(),
        )
        .unwrap();
        assert!(!rows.is_empty());
        assert!(
            rows.iter()
                .all(|r| r.age_hours.is_finite() && r.age_hours >= 0.0),
            "ages: {:?}",
            rows.iter().map(|r| r.age_hours).collect::<Vec<_>>()
        );
    }

    /// ed-route may depend on ed-store, but it must not know table names:
    /// the galaxy schema is ed-store's to change.
    #[test]
    fn profit_finder_does_not_name_galaxy_tables() {
        let source = include_str!("profit.rs");
        let (code, _tests) = source.split_once("#[cfg(test)]").unwrap();
        for table in ["sys_market", "sys_stations", "sys_systems", "sys_commodities"] {
            assert!(!code.contains(table), "profit.rs names {table}");
        }
        assert!(!code.contains("INDEXED BY"));
    }

    /// The default's comment argues from measurement for a week; the value
    /// must be the one the measurement supports.
    #[test]
    fn default_max_age_matches_the_measured_week() {
        assert_eq!(Constraints::default().max_age_hours, 168.0);
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        ed_store::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO sys_systems (id64,name,x,y,z) VALUES
               (1,'Origin',0,0,0),(2,'Near',10,0,0),(3,'Far',200,0,0);
             INSERT INTO sys_stations (id,system_id64,name,type,distance_to_arrival,pad_large,pad_medium,pad_small,has_market,is_carrier) VALUES
               (10,1,'Home Port','Coriolis Starport',100,8,10,10,1,0),
               (20,2,'Outpost A','Outpost',300,0,2,4,1,0),
               (21,2,'Carrier X','Drake-Class Carrier',50,8,4,4,1,1),
               (22,2,'Mystery','Unknown',50,NULL,NULL,NULL,1,0),
               (30,3,'Far Port','Orbis Starport',10,8,8,8,1,0);
             INSERT INTO sys_commodities(symbol,name) VALUES ('gold','Gold'),('silver','Silver'),('tea','Tea');
             WITH v(station_id,symbol,buy,sell,demand,supply,updated) AS (VALUES
               (10,'gold',9000,8500,0,5000,'2026-08-25T00:00:00Z'),
               (10,'silver',4000,3800,0,100,'2026-08-25T00:00:00Z'),
               (20,'gold',0,12000,3000,0,'2026-08-25T00:00:00Z'),
               (20,'tea',1500,1400,0,2000,'2026-08-25T00:00:00Z'),
               (10,'tea',0,1900,10000,0,'2026-08-25T00:00:00Z'),
               (21,'gold',0,20000,3000,0,'2026-08-25T00:00:00Z'),
               (22,'gold',0,15000,3000,0,'2026-08-25T00:00:00Z'),
               (20,'silver',0,9000,50,0,'2020-01-01T00:00:00Z'))
             INSERT INTO sys_market (station_id,commodity_id,buy_price,sell_price,demand,supply,updated)
             SELECT v.station_id,c.id,v.buy,v.sell,v.demand,v.supply,unixepoch(v.updated)
             FROM v
             JOIN sys_commodities c ON c.symbol=v.symbol;",
        )
        .unwrap();
        conn
    }

    const NOW: i64 = 1_787_616_000; // 2026-08-25T08:26:40Z, a few hours after the rows

    fn ship() -> Ship {
        Ship {
            cargo_capacity: 200,
            jump_range_ly: 20.0,
            laden_range_ly: 20.0,
        }
    }

    #[test]
    fn best_leg_is_ranked_by_credits_per_hour_and_capped_by_supply_and_demand() {
        let conn = db();
        let c = Constraints {
            min_pad: Some(PadSize::Medium),
            ..Default::default()
        };
        let r = find_at(
            &conn,
            "Origin",
            (0.0, 0.0, 0.0),
            Some(10),
            &ship(),
            &c,
            10,
            NOW,
            &SearchControl::none(),
        )
        .unwrap();
        let top = &r.legs[0];
        assert_eq!(top.symbol, "gold");
        assert_eq!(top.to.station, "Outpost A");
        assert_eq!(top.tons, 200, "cargo-limited");
        assert_eq!(top.profit, 200 * (12000 - 9000));
        assert!(top.profit_per_hour > 0.0);
        assert_eq!(top.jumps, 1);
    }

    #[test]
    fn carriers_and_unknown_pads_are_excluded_and_counted() {
        let conn = db();
        let c = Constraints {
            min_pad: Some(PadSize::Large),
            ..Default::default()
        };
        let r = find_at(
            &conn,
            "Origin",
            (0.0, 0.0, 0.0),
            Some(10),
            &ship(),
            &c,
            10,
            NOW,
            &SearchControl::none(),
        )
        .unwrap();
        // Carrier X pays 20k for gold but is a carrier; Mystery pays 15k but
        // has no recorded pads; Outpost A has no large pad.
        assert!(
            r.legs.is_empty(),
            "nothing in range fits a large pad: {:?}",
            r.legs
        );
        assert_eq!(r.excluded.carriers, 1);
        assert_eq!(r.excluded.pad_unknown, 1);
        assert_eq!(r.excluded.pad_too_small, 1);
    }

    #[test]
    fn stale_prices_are_dropped_not_trusted() {
        let conn = db();
        let c = Constraints {
            min_pad: None,
            ..Default::default()
        };
        let r = find_at(
            &conn,
            "Origin",
            (0.0, 0.0, 0.0),
            Some(10),
            &ship(),
            &c,
            10,
            NOW,
            &SearchControl::none(),
        )
        .unwrap();
        // Silver at Outpost A pays 9000 on a 2020 price. Must not appear.
        assert!(r.legs.iter().all(|l| l.symbol != "silver"));
        assert!(r.excluded.stale_price_rows >= 1);
    }

    #[test]
    fn round_trip_pairs_the_return_leg() {
        let conn = db();
        let c = Constraints {
            min_pad: Some(PadSize::Medium),
            ..Default::default()
        };
        let r = find_at(
            &conn,
            "Origin",
            (0.0, 0.0, 0.0),
            Some(10),
            &ship(),
            &c,
            10,
            NOW,
            &SearchControl::none(),
        )
        .unwrap();
        let trip = r.round_trips.first().expect("gold out, tea back");
        assert_eq!(trip.out.symbol, "gold");
        assert_eq!(trip.back.symbol, "tea");
        assert_eq!(trip.profit, trip.out.profit + trip.back.profit);
        assert_eq!(trip.duration.confidence, Confidence::Estimated);
    }

    #[test]
    fn a_triangle_ring_beats_the_round_trip_when_the_return_is_weak() {
        let conn = db();
        // A third station: it buys the tea Outpost A sells, and sells silver
        // that Home Port buys -- so gold -> tea -> silver closes a ring where
        // every leg is loaded, against a loop whose return is thin.
        conn.execute_batch(
            "INSERT INTO sys_systems (id64,name,x,y,z) VALUES (4,'Third',0,10,0);
             INSERT INTO sys_stations (id,system_id64,name,type,distance_to_arrival,pad_large,pad_medium,pad_small,has_market,is_carrier)
               VALUES (40,4,'Third Port','Orbis Starport',100,8,8,8,1,0);
             WITH v(station_id,symbol,buy,sell,demand,supply,updated) AS (VALUES
               (40,'tea',0,9000,10000,0,'2026-08-25T00:00:00Z'),
               (40,'silver',1000,900,0,5000,'2026-08-25T00:00:00Z'),
               (10,'silver',0,12000,10000,0,'2026-08-25T00:00:00Z'))
             INSERT OR REPLACE INTO sys_market
             SELECT v.station_id,c.id,v.buy,v.sell,v.demand,v.supply,unixepoch(v.updated)
             FROM v JOIN sys_commodities c ON c.symbol=v.symbol;",
        )
        .unwrap();
        let c = Constraints {
            min_pad: Some(PadSize::Medium),
            // Rings are opt-in since 2026-09-05; this test is ABOUT them.
            max_stops: 3,
            ..Default::default()
        };
        let r = find_at(
            &conn,
            "Origin",
            (0.0, 0.0, 0.0),
            None,
            &ship(),
            &c,
            10,
            NOW,
            &SearchControl::none(),
        )
        .unwrap();
        let ring = r
            .rings
            .iter()
            .find(|x| x.stops == 3)
            .expect("Home -> Outpost A -> Third -> Home");
        let symbols: Vec<&str> = ring.legs.iter().map(|l| l.symbol.as_str()).collect();
        assert_eq!(symbols, ["gold", "tea", "silver"]);
        assert!(
            ring.profit_per_hour > r.round_trips[0].profit_per_hour,
            "ring {} should beat loop {}",
            ring.profit_per_hour,
            r.round_trips[0].profit_per_hour
        );
    }

    #[test]
    fn the_hold_is_topped_up_with_the_next_best_commodity() {
        let conn = db();
        // Gold at Home Port runs out at 150 t; beer fills the last 50 t.
        conn.execute_batch(
            "INSERT OR IGNORE INTO sys_commodities(symbol,name) VALUES ('beer','Beer');
             WITH v(station_id,symbol,buy,sell,demand,supply,updated) AS (VALUES
               (10,'gold',9000,8500,0,150,'2026-08-25T00:00:00Z'),
               (10,'beer',100,90,0,500,'2026-08-25T00:00:00Z'),
               (20,'beer',0,3000,500,0,'2026-08-25T00:00:00Z'))
             INSERT OR REPLACE INTO sys_market
             SELECT v.station_id,c.id,v.buy,v.sell,v.demand,v.supply,unixepoch(v.updated)
             FROM v JOIN sys_commodities c ON c.symbol=v.symbol;",
        )
        .unwrap();
        let c = Constraints {
            min_pad: Some(PadSize::Medium),
            ..Default::default()
        };
        let r = find_at(
            &conn,
            "Origin",
            (0.0, 0.0, 0.0),
            Some(10),
            &ship(),
            &c,
            10,
            NOW,
            &SearchControl::none(),
        )
        .unwrap();
        let leg = r.legs.iter().find(|l| l.to.station == "Outpost A").unwrap();
        assert_eq!(leg.symbol, "gold");
        assert_eq!(leg.tons, 200, "150 t gold + 50 t beer fills the 200 t hold");
        assert_eq!(leg.extra.len(), 1);
        assert_eq!(
            (leg.extra[0].commodity.as_str(), leg.extra[0].tons),
            ("Beer", 50)
        );
        assert_eq!(leg.profit, 150 * 3000 + 50 * 2900);
    }

    /// The supply floor (maintainer, 2026-09-05: a 3-ton seller headlining a
    /// 1,008-ton route is noise) hides thin buy boards — primaries and
    /// hold-fillers alike, one meaning.
    #[test]
    fn a_supply_floor_hides_thin_buy_boards() {
        let conn = db();
        conn.execute_batch(
            "INSERT OR IGNORE INTO sys_commodities(symbol,name) VALUES ('beer','Beer');
             WITH v(station_id,symbol,buy,sell,demand,supply,updated) AS (VALUES
               (10,'gold',9000,8500,0,150,'2026-08-25T00:00:00Z'),
               (10,'beer',100,90,0,500,'2026-08-25T00:00:00Z'),
               (20,'beer',0,3000,500,0,'2026-08-25T00:00:00Z'))
             INSERT OR REPLACE INTO sys_market
             SELECT v.station_id,c.id,v.buy,v.sell,v.demand,v.supply,unixepoch(v.updated)
             FROM v JOIN sys_commodities c ON c.symbol=v.symbol;",
        )
        .unwrap();
        let c = Constraints {
            min_pad: Some(PadSize::Medium),
            min_supply: 200,
            ..Default::default()
        };
        let r = find_at(
            &conn,
            "Origin",
            (0.0, 0.0, 0.0),
            Some(10),
            &ship(),
            &c,
            10,
            NOW,
            &SearchControl::none(),
        )
        .unwrap();
        let leg = r.legs.iter().find(|l| l.to.station == "Outpost A").unwrap();
        assert_eq!(leg.symbol, "beer", "the 150 t gold board is under the 200 t floor");
        assert!(leg.extra.is_empty(), "no thin board sneaks back in as a hold-filler");
    }

    #[test]
    fn systems_outside_the_radius_are_not_considered() {
        let conn = db();
        let c = Constraints {
            radius_ly: 40.0,
            ..Default::default()
        };
        let r = find_at(
            &conn,
            "Origin",
            (0.0, 0.0, 0.0),
            None,
            &ship(),
            &c,
            10,
            NOW,
            &SearchControl::none(),
        )
        .unwrap();
        assert!(r
            .legs
            .iter()
            .all(|l| l.to.system != "Far" && l.from.system != "Far"));
    }
}
