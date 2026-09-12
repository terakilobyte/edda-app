//! System and station lookup -- the queries you currently open Inara for.
//!
//! Everything here answers from the local database. Two properties matter
//! more than speed:
//!
//! * **Your own visits outrank the galaxy tables.** `powerplay_observations`
//!   is first-hand and timestamped; `sys_systems.controlling_power` is
//!   whatever the last uploading player saw. [`system`] prefers the former
//!   and says which one it used.
//! * **Every answer carries its age.** Galaxy data is only as fresh as the
//!   last commander to dock there, and quiet systems go stale for days.
//!   A price without its timestamp invites the caller to present old data as
//!   current, so `updated` is not optional in these shapes.

use anyhow::Result;
use ed_domain::freshness;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Current time in epoch seconds, the unit every `*updated` column uses.
pub fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A stored epoch as the journal-style ISO string callers display.
fn iso(updated: Option<i64>) -> Option<String> {
    updated.map(crate::session::iso_from_epoch)
}

/// Age of a stored epoch in hours, `None` when nothing is stored.
fn age(now: i64, updated: Option<i64>) -> Option<f64> {
    updated.map(|t| freshness::age_hours(now, Some(t)))
}

/// Where a fact came from. Surfaced to the caller so an answer can say
/// "you saw this yourself on the 23rd" rather than implying live truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    /// From the commander's own journal.
    FirstHand,
    /// From the Spansh dump or the EDDN feed.
    Community,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub id64: Option<i64>,
    pub name: String,
    pub coords: Option<(f64, f64, f64)>,
    pub allegiance: Option<String>,
    pub government: Option<String>,
    pub primary_economy: Option<String>,
    pub security: Option<String>,
    pub population: Option<i64>,
    pub controlling_power: Option<String>,
    pub power_state: Option<String>,
    /// Where `controlling_power` / `power_state` came from.
    pub power_provenance: Option<Provenance>,
    /// Timestamp of the Powerplay reading, so callers can show its age.
    pub power_observed: Option<String>,
    pub control_progress: Option<f64>,
    pub station_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationInfo {
    pub id: i64,
    pub name: Option<String>,
    pub system_name: Option<String>,
    pub kind: Option<String>,
    pub class: StationClass,
    pub distance_to_arrival: Option<f64>,
    pub primary_economy: Option<String>,
    pub government: Option<String>,
    pub controlling_faction: Option<String>,
    pub max_pad: Option<PadSize>,
    pub has_market: bool,
    pub has_outfitting: bool,
    pub has_shipyard: bool,
    pub is_carrier: bool,
    /// When the station record was last observed, as an ISO timestamp.
    pub updated: Option<String>,
    /// The same, as hours before the query ran.
    pub age_hours: Option<f64>,
}

pub use ed_domain::station::{PadSize, StationClass};

fn station_from_row(r: &rusqlite::Row) -> rusqlite::Result<StationInfo> {
    Ok(StationInfo {
        id: r.get("id")?,
        name: r.get("name")?,
        system_name: r.get("system_name").ok(),
        class: StationClass::of(r.get::<_, Option<String>>("type")?.as_deref()),
        kind: r.get("type")?,
        distance_to_arrival: r.get("distance_to_arrival")?,
        primary_economy: r.get("primary_economy")?,
        government: r.get("government")?,
        controlling_faction: r.get("controlling_faction")?,
        max_pad: PadSize::from_counts(
            r.get("pad_large")?,
            r.get("pad_medium")?,
            r.get("pad_small")?,
        ),
        has_market: r.get::<_, i64>("has_market")? != 0,
        has_outfitting: r.get::<_, i64>("has_outfitting")? != 0,
        has_shipyard: r.get::<_, i64>("has_shipyard")? != 0,
        is_carrier: r.get::<_, i64>("is_carrier")? != 0,
        updated: iso(r.get("updated")?),
        age_hours: age(now_epoch_secs(), r.get("updated")?),
    })
}

const STATION_COLS: &str = "s.id, s.name, s.type, s.distance_to_arrival, s.primary_economy,
     s.government, s.controlling_faction, s.pad_large, s.pad_medium, s.pad_small,
     s.has_market, s.has_outfitting, s.has_shipyard, s.is_carrier, s.updated,
     sy.name AS system_name";

/// Look up one system by name.
pub fn system(conn: &Connection, name: &str) -> Result<Option<SystemInfo>> {
    let mut info = conn
        .query_row(
            "SELECT id64, name, x, y, z, allegiance, government, primary_economy,
                    security, population, controlling_power, power_state,
                    (SELECT COUNT(*) FROM sys_stations st WHERE st.system_id64 = sys_systems.id64)
             FROM sys_systems WHERE name = ?1 COLLATE NOCASE
             ORDER BY population DESC LIMIT 1",
            [name],
            |r| {
                let x: Option<f64> = r.get(2)?;
                let y: Option<f64> = r.get(3)?;
                let z: Option<f64> = r.get(4)?;
                Ok(SystemInfo {
                    id64: r.get(0)?,
                    name: r
                        .get::<_, Option<String>>(1)?
                        .unwrap_or_else(|| name.to_string()),
                    coords: match (x, y, z) {
                        (Some(x), Some(y), Some(z)) => Some((x, y, z)),
                        _ => None,
                    },
                    allegiance: r.get(5)?,
                    government: r.get(6)?,
                    primary_economy: r.get(7)?,
                    security: r.get(8)?,
                    population: r.get(9)?,
                    controlling_power: r.get(10)?,
                    power_state: r.get(11)?,
                    power_provenance: None,
                    power_observed: None,
                    control_progress: None,
                    station_count: r.get(12)?,
                })
            },
        )
        .optional()?;

    // Fall back to a journal-only system: somewhere visited but absent from
    // the dump (or the dump not yet imported).
    if info.is_none() {
        if let Some(obs) = crate::query::powerplay_for_system(conn, name)? {
            info = Some(SystemInfo {
                id64: None,
                name: obs.system_name.clone(),
                coords: None,
                allegiance: None,
                government: None,
                primary_economy: None,
                security: None,
                population: None,
                controlling_power: None,
                power_state: None,
                power_provenance: None,
                power_observed: None,
                control_progress: None,
                station_count: 0,
            });
        }
    }

    let Some(mut info) = info else {
        return Ok(None);
    };

    // First-hand beats community, always.
    if let Some(obs) = crate::query::powerplay_for_system(conn, &info.name)? {
        if obs.controlling_power.is_some() || obs.powerplay_state.is_some() {
            info.controlling_power = obs.controlling_power;
            info.power_state = obs.powerplay_state;
            info.control_progress = obs.control_progress;
            info.power_observed = Some(obs.ts);
            info.power_provenance = Some(Provenance::FirstHand);
        }
    }
    if info.power_provenance.is_none() && info.controlling_power.is_some() {
        info.power_provenance = Some(Provenance::Community);
    }

    Ok(Some(info))
}

/// Stations in a system, most useful first.
///
/// Ordered by station class then arrival distance. Raw distance ordering is
/// actively misleading here: parked fleet carriers sit at 0 ls and would
/// otherwise dominate every populated system.
///
/// `include_minor` adds settlements, construction depots and untyped
/// installations -- hundreds of them in a developed system, and none of them
/// somewhere you take a ship.
pub fn stations_in_system_filtered(
    conn: &Connection,
    system_name: &str,
    include_carriers: bool,
    include_minor: bool,
) -> Result<Vec<StationInfo>> {
    let sql = format!(
        "SELECT {STATION_COLS} FROM sys_stations s
         JOIN sys_systems sy ON sy.id64 = s.system_id64
         WHERE sy.name = ?1 COLLATE NOCASE"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([system_name], station_from_row)?;

    let mut out: Vec<StationInfo> = rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|s| include_carriers || s.class != StationClass::Carrier)
        .filter(|s| {
            include_minor
                || !matches!(
                    s.class,
                    StationClass::Settlement
                        | StationClass::ConstructionDepot
                        | StationClass::Other
                )
        })
        .collect();

    out.sort_by(|a, b| {
        a.class.rank().cmp(&b.class.rank()).then_with(|| {
            a.distance_to_arrival
                .unwrap_or(f64::MAX)
                .partial_cmp(&b.distance_to_arrival.unwrap_or(f64::MAX))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    Ok(out)
}

/// Dockable, non-carrier stations -- the sensible default.
pub fn stations_in_system(conn: &Connection, system_name: &str) -> Result<Vec<StationInfo>> {
    stations_in_system_filtered(conn, system_name, false, false)
}

/// Find stations by name, anywhere.
pub fn find_stations(conn: &Connection, name: &str, limit: usize) -> Result<Vec<StationInfo>> {
    let sql = format!(
        "SELECT {STATION_COLS} FROM sys_stations s
         LEFT JOIN sys_systems sy ON sy.id64 = s.system_id64
         WHERE s.name = ?1 COLLATE NOCASE
         ORDER BY s.is_carrier, s.name LIMIT ?2"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![name, limit as i64], station_from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NearbySystem {
    pub name: String,
    pub id64: i64,
    pub distance_ly: f64,
    pub controlling_power: Option<String>,
    pub power_state: Option<String>,
    pub population: Option<i64>,
}

/// Systems within `radius_ly` of a point, nearest first.
///
/// Stock SQLite has no spatial index, so this is a bounding-box scan on the
/// indexed `x` column with an exact distance filter applied after. The box
/// is the cheap part; without it this would be a full table scan of every
/// system in the galaxy.
pub fn systems_within(
    conn: &Connection,
    (x, y, z): (f64, f64, f64),
    radius_ly: f64,
    limit: usize,
) -> Result<Vec<NearbySystem>> {
    let mut stmt = conn.prepare(
        "SELECT name, id64, x, y, z, controlling_power, power_state, population,
                ((x-?1)*(x-?1) + (y-?2)*(y-?2) + (z-?3)*(z-?3)) AS d2
         FROM sys_systems
         WHERE x BETWEEN ?1 - ?4 AND ?1 + ?4
           AND y BETWEEN ?2 - ?4 AND ?2 + ?4
           AND z BETWEEN ?3 - ?4 AND ?3 + ?4
           AND d2 <= ?4 * ?4
           AND name IS NOT NULL
         ORDER BY d2 LIMIT ?5",
    )?;
    let rows = stmt.query_map(params![x, y, z, radius_ly, limit as i64], |r| {
        Ok(NearbySystem {
            name: r.get(0)?,
            id64: r.get(1)?,
            distance_ly: r.get::<_, f64>(8)?.sqrt(),
            controlling_power: r.get(5)?,
            power_state: r.get(6)?,
            population: r.get(7)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// A station's market board. Deserialize too: the API's station-board
/// endpoint answers in exactly this shape and the client parses it back.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct MarketEntry {
    pub symbol: String,
    pub name: Option<String>,
    pub category: Option<String>,
    pub buy_price: Option<i64>,
    pub sell_price: Option<i64>,
    pub demand: Option<i64>,
    pub supply: Option<i64>,
    pub updated: Option<String>,
    pub age_hours: Option<f64>,
}

pub fn market_for_station(conn: &Connection, station_id: i64) -> Result<Vec<MarketEntry>> {
    let now = now_epoch_secs();
    let mut stmt = conn.prepare(
        "SELECT c.symbol, c.name, c.category, m.buy_price, m.sell_price, m.demand, m.supply,
                m.updated
         FROM sys_market m JOIN sys_commodities c ON c.id = m.commodity_id
         WHERE m.station_id = ?1 ORDER BY c.symbol",
    )?;
    let rows = stmt.query_map([station_id], |r| {
        Ok(MarketEntry {
            symbol: r.get(0)?,
            name: r.get(1)?,
            category: r.get(2)?,
            buy_price: r.get(3)?,
            sell_price: r.get(4)?,
            demand: r.get(5)?,
            supply: r.get(6)?,
            updated: iso(r.get(7)?),
            age_hours: age(now, r.get(7)?),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Debug, Clone, Serialize)]
pub struct CommoditySearchHit {
    pub station_id: i64,
    pub station: String,
    pub system: String,
    pub distance_ly: f64,
    pub distance_to_arrival: Option<f64>,
    pub max_pad: Option<PadSize>,
    pub is_carrier: bool,
    pub price: i64,
    pub quantity: i64,
    pub updated: Option<String>,
    pub age_hours: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutfittingSearchHit {
    pub station_id: i64,
    pub station: String,
    pub system: String,
    pub distance_ly: f64,
    pub distance_to_arrival: Option<f64>,
    pub max_pad: Option<PadSize>,
    pub is_carrier: bool,
    pub symbol: String,
    pub name: String,
    pub class: Option<i64>,
    pub rating: Option<String>,
    pub category: Option<String>,
    pub ship: Option<String>,
    pub updated: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShipyardSearchHit {
    pub station_id: i64,
    pub station: String,
    pub system: String,
    pub distance_ly: f64,
    pub distance_to_arrival: Option<f64>,
    pub max_pad: Option<PadSize>,
    pub is_carrier: bool,
    pub symbol: String,
    pub name: String,
    pub updated: Option<String>,
}

// ── Galaxy interface for callers that must not know table names ─────

/// A station with a market inside a sphere, with the raw dump fields
/// (`type`, pad counts, `powers` list) already interpreted. What the profit
/// finder ranks and filters.
#[derive(Debug, Clone, PartialEq)]
pub struct MarketStation {
    pub station_id: i64,
    pub station: String,
    pub system: String,
    pub system_id64: i64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub arrival_ls: Option<f64>,
    /// The dump's `type` string; see [`StationClass::of`].
    pub kind: Option<String>,
    pub max_pad: Option<PadSize>,
    pub is_carrier: bool,
    pub controlling_power: Option<String>,
    pub power_state: Option<String>,
    pub powers: Vec<String>,
}

/// Every named station with a market within `radius_ly` of `origin`.
/// Carriers and pad sizes are reported, not filtered: the caller decides
/// and counts what it excludes.
pub fn market_stations_within(
    conn: &Connection,
    origin: (f64, f64, f64),
    radius_ly: f64,
) -> Result<Vec<MarketStation>> {
    let (x, y, z) = origin;
    let mut stmt = conn.prepare(
        "SELECT st.id, st.name, sy.name, sy.id64, sy.x, sy.y, sy.z,
                st.distance_to_arrival, st.type, st.is_carrier,
                st.pad_large, st.pad_medium, st.pad_small,
                sy.controlling_power, sy.power_state, sy.powers
         FROM sys_systems sy
         JOIN sys_stations st ON st.system_id64 = sy.id64
         WHERE sy.x BETWEEN ?1 - ?4 AND ?1 + ?4
           AND sy.y BETWEEN ?2 - ?4 AND ?2 + ?4
           AND sy.z BETWEEN ?3 - ?4 AND ?3 + ?4
           AND ((sy.x-?1)*(sy.x-?1) + (sy.y-?2)*(sy.y-?2) + (sy.z-?3)*(sy.z-?3)) <= ?4 * ?4
           AND st.has_market = 1
           AND st.name IS NOT NULL AND sy.name IS NOT NULL",
    )?;
    let rows = stmt.query_map(params![x, y, z, radius_ly], |r| {
        let station: String = r.get(1)?;
        // Flag OR callsign: the flag lies on fresh installs (no identity
        // yet) and on 63 measured misflagged carriers — carrier policy
        // (opt-in, envelope) must hold either way.
        let is_carrier =
            r.get::<_, i64>(9)? != 0 || ed_domain::station::is_carrier_callsign(&station);
        Ok(MarketStation {
            station_id: r.get(0)?,
            station,
            system: r.get(2)?,
            system_id64: r.get(3)?,
            x: r.get(4)?,
            y: r.get(5)?,
            z: r.get(6)?,
            arrival_ls: r.get(7)?,
            kind: r.get(8)?,
            is_carrier,
            max_pad: PadSize::from_counts(r.get(10)?, r.get(11)?, r.get(12)?),
            controlling_power: r.get(13)?,
            power_state: r.get(14)?,
            powers: r
                .get::<_, Option<String>>(15)?
                .map(|p| {
                    p.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// `(name, detail)` pairs for a name-completion box.
pub type NameCompletion = (String, Option<String>);

fn like_prefix(prefix: &str) -> String {
    format!("{}%", prefix.replace('%', "").replace('_', "\\_"))
}

/// Non-carrier station names starting with `prefix`, with their system.
pub fn complete_station_names(
    conn: &Connection,
    prefix: &str,
    limit: usize,
) -> Result<Vec<NameCompletion>> {
    let mut stmt = conn.prepare(
        "SELECT st.name, sy.name FROM sys_stations st JOIN sys_systems sy ON sy.id64 = st.system_id64
         WHERE st.name LIKE ?1 ESCAPE '\\' COLLATE NOCASE AND st.is_carrier = 0
         ORDER BY st.name LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![like_prefix(prefix), limit as i64], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// System names starting with `prefix`, with their controlling power.
pub fn complete_system_names(
    conn: &Connection,
    prefix: &str,
    limit: usize,
) -> Result<Vec<NameCompletion>> {
    let mut stmt = conn.prepare(
        "SELECT name, controlling_power FROM sys_systems
         WHERE name LIKE ?1 ESCAPE '\\' COLLATE NOCASE ORDER BY name LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![like_prefix(prefix), limit as i64], |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Distinct `(powers, states)` present in the galaxy, for filter menus.
pub fn powerplay_choices(conn: &Connection) -> Result<(Vec<String>, Vec<String>)> {
    let list = |sql: &str| -> Result<Vec<String>> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };
    Ok((
        list("SELECT DISTINCT controlling_power FROM sys_systems WHERE controlling_power IS NOT NULL ORDER BY 1")?,
        list("SELECT DISTINCT power_state FROM sys_systems WHERE power_state IS NOT NULL ORDER BY 1")?,
    ))
}

/// Row counts of the galaxy tables a status panel reports. Market rows are
/// deliberately absent: counting ~100M rows is not a status query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct GalaxyCounts {
    pub systems: i64,
    pub stations: i64,
    pub factions: i64,
    pub bodies: i64,
    pub hotspots: i64,
    pub services: i64,
}

pub fn galaxy_counts(conn: &Connection) -> Result<GalaxyCounts> {
    let count = |table: &str| -> Result<i64> {
        Ok(conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))?)
    };
    Ok(GalaxyCounts {
        systems: count("sys_systems")?,
        stations: count("sys_stations")?,
        factions: count("sys_factions")?,
        bodies: count("sys_bodies")?,
        hotspots: count("sys_ring_hotspots")?,
        services: count("sys_station_services")?,
    })
}

/// When the freshest non-carrier station record was observed (ISO), as a
/// proxy for how current the galaxy snapshot is.
pub fn newest_station_update(conn: &Connection) -> Result<Option<String>> {
    let newest: Option<i64> = conn.query_row(
        "SELECT MAX(updated) FROM sys_stations WHERE is_carrier = 0",
        [],
        |r| r.get(0),
    )?;
    Ok(iso(newest))
}

/// SQL predicate: the station (alias `st`) is NOT a fleet carrier. The
/// flag is trusted AND the XXX-XXX callsign is recognized, because the
/// flag lies in two measured ways (2026-09-04): fresh installs carry no
/// station identity yet, and misflagged carriers hide in bootstrapped
/// data (63 measured) — while zero real stations match the pattern.
/// Rust-side callers use `ed_domain::station::is_carrier_callsign`.
pub(crate) const SQL_ST_NON_CARRIER: &str =
    "(st.is_carrier = 0 AND st.name NOT GLOB '[A-Z0-9][A-Z0-9][A-Z0-9]-[A-Z0-9][A-Z0-9][A-Z0-9]')";

/// The exact service string the ingest records for a black-market
/// contact (measured against the bootstrapped services table).
pub const BLACK_MARKET_SERVICE: &str = "Black Market";

fn within_pad(have: Option<PadSize>, need: Option<PadSize>) -> bool {
    match need {
        None => true,
        Some(need) => have.is_some_and(|have| have.fits(need)),
    }
}

/// How [`search_commodity`] orders — and therefore SELECTS — its page.
/// The order is the selection: with a LIMIT, "sorted by distance" must
/// mean "the NEAREST matches", not "the cheapest matches re-sorted by
/// distance" (the 2026-09-04 market-lane finding F3: the old
/// price-ordered fetch made the panel's distance sort silently lie).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommoditySort {
    /// Best price first (buy: cheapest; sell: dearest).
    #[default]
    Price,
    /// Nearest first, best price as the tiebreak.
    Distance,
}

impl CommoditySort {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "price" => Some(CommoditySort::Price),
            "distance" => Some(CommoditySort::Distance),
            _ => None,
        }
    }
}

/// Stations buying from or selling to the commander for one exact commodity
/// symbol. `side` follows the player action: `buy` uses the station's
/// `buy_price`/supply and `sell` uses `sell_price`/demand, matching the profit
/// finder and the journal dump fields already used throughout EDDA.
///
/// Every filter runs in SQL before the LIMIT (F3 fix): the old shape
/// ordered by price, took `limit*4`, then pad-filtered in Rust — so a
/// large-pad search could silently under-fill and near-but-pricier
/// stations never entered a distance-sorted view at all. `min_quantity`
/// (0 = any) bounds supply/demand depth; unknown pads still fail closed.
#[allow(clippy::too_many_arguments)]
pub fn search_commodity(
    conn: &Connection,
    origin: (f64, f64, f64),
    symbol: &str,
    side: &str,
    radius_ly: f64,
    min_pad: Option<PadSize>,
    include_carriers: bool,
    include_prohibited: bool,
    max_age_hours: f64,
    min_quantity: i64,
    sort: CommoditySort,
    limit: usize,
) -> Result<Vec<CommoditySearchHit>> {
    let (price, quantity, direction) = match side.trim().to_ascii_lowercase().as_str() {
        "buy" => ("m.buy_price", "m.supply", "ASC"),
        "sell" => ("m.sell_price", "m.demand", "DESC"),
        other => anyhow::bail!("unknown commodity side {other:?}; expected buy or sell"),
    };
    let pad_clause = match min_pad {
        None => "1",
        Some(PadSize::Small) => {
            "(COALESCE(st.pad_large,0) > 0 OR COALESCE(st.pad_medium,0) > 0 OR COALESCE(st.pad_small,0) > 0)"
        }
        Some(PadSize::Medium) => "(COALESCE(st.pad_large,0) > 0 OR COALESCE(st.pad_medium,0) > 0)",
        Some(PadSize::Large) => "COALESCE(st.pad_large,0) > 0",
    };
    let order = match sort {
        CommoditySort::Price => format!("{price} {direction}, st.distance_to_arrival"),
        CommoditySort::Distance => format!(
            "((sy.x-?2)*(sy.x-?2) + (sy.y-?3)*(sy.y-?3) + (sy.z-?4)*(sy.z-?4)) ASC, {price} {direction}"
        ),
    };
    // Sell searches only: silence poisoned boards (the 999999-demand
    // sentinel at >3x the commodity mean — finding F1, measured at 31
    // bad rows in 99.8M) and stations that confiscate the commodity
    // (sys_market_prohibited stores lowercased display names; the join
    // on LOWER(name) is the measured 19-of-19 mapping). Buy searches
    // read supply, where neither hazard applies.
    let selling = side.trim().eq_ignore_ascii_case("sell");
    // Prohibited goods are opt-IN (maintainer, 2026-09-04): "if a station has a
    // black market to sell those prohibited goods at, that's a choice the
    // player can make." Opting in reveals the sale only where a black
    // market exists (104,444 of 327,043 prohibiting stations, measured) —
    // everywhere else the sale is impossible and stays hidden.
    let prohibited_gate = if include_prohibited {
        format!(
            "OR EXISTS (SELECT 1 FROM sys_station_services sv
                        WHERE sv.station_id = st.id
                          AND sv.service IN ('{BLACK_MARKET_SERVICE}', 'blackmarket'))"
        )
    } else {
        String::new()
    };
    // Station prices are shown as reported, spikes included: the "999999
    // demand at >3x mean" silencer shipped here for a few hours on
    // 2026-09-04 until the maintainer's Inara cross-check proved the flagship
    // "poison" board was a real demand spike the reference tool also
    // shows (ledger retraction). Confiscation is the one sell-side rule.
    let mut sell_guards = if selling {
        format!(
            "AND (NOT EXISTS (
                 SELECT 1 FROM sys_market_prohibited p
                 JOIN sys_commodities pc ON LOWER(pc.name) = p.symbol OR pc.symbol = p.symbol
                 WHERE p.station_id = st.id AND pc.id = m.commodity_id) {prohibited_gate})"
        )
    } else {
        String::new()
    };
    // Carrier envelope (maintainer rule 2026-09-04), both sides: a carrier
    // price outside one standard deviation of the extreme STATION
    // prices for the commodity is never shown.
    let (env_min, env_max, env_std) = if selling {
        ("station_sell_min", "station_sell_max", "station_sell_std")
    } else {
        ("station_buy_min", "station_buy_max", "station_buy_std")
    };
    sell_guards.push_str(&format!(
        " AND ({SQL_ST_NON_CARRIER} OR NOT EXISTS (
             SELECT 1 FROM sys_commodity_stats ce
             WHERE ce.commodity_id = m.commodity_id
               AND COALESCE(ce.station_boards, 0) >= {floor}
               AND ({price} > ce.{env_max} + {k} * ce.{env_std}
                 OR {price} < ce.{env_min} - {k} * ce.{env_std})))",
        floor = crate::market::STATS_MIN_BOARDS,
        k = crate::market::CARRIER_ENVELOPE_STD,
    ));
    let (x, y, z) = origin;
    let sql = format!(
        "SELECT st.id, st.name, sy.name, st.distance_to_arrival,
                st.pad_large, st.pad_medium, st.pad_small, st.is_carrier,
                {price}, COALESCE({quantity},0), m.updated,
                sy.x, sy.y, sy.z
         FROM sys_market m
         JOIN sys_commodities c ON c.id = m.commodity_id
         JOIN sys_stations st ON st.id = m.station_id
         JOIN sys_systems sy ON sy.id64 = st.system_id64
         WHERE c.symbol = ?1 AND {price} > 0 AND {quantity} > 0
           AND sy.x BETWEEN ?2 - ?5 AND ?2 + ?5
           AND sy.y BETWEEN ?3 - ?5 AND ?3 + ?5
           AND sy.z BETWEEN ?4 - ?5 AND ?4 + ?5
           AND ((sy.x-?2)*(sy.x-?2) + (sy.y-?3)*(sy.y-?3) + (sy.z-?4)*(sy.z-?4)) <= ?5 * ?5
           AND (?6 OR {SQL_ST_NON_CARRIER})
           AND m.updated >= ?7
           AND {quantity} >= ?9
           AND {pad_clause}
           {sell_guards}
           AND st.name IS NOT NULL AND sy.name IS NOT NULL
         ORDER BY {order}
         LIMIT ?8"
    );
    let now = now_epoch_secs();
    let cutoff = now - (max_age_hours.max(1.0) * 3600.0) as i64;
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            symbol.to_ascii_lowercase(),
            x,
            y,
            z,
            radius_ly,
            include_carriers,
            cutoff,
            limit.max(1) as i64,
            min_quantity.max(0)
        ],
        |r| {
            let max_pad = PadSize::from_counts(r.get(4)?, r.get(5)?, r.get(6)?);
            let (sx, sy, sz): (f64, f64, f64) = (r.get(11)?, r.get(12)?, r.get(13)?);
            let updated: Option<i64> = r.get(10)?;
            let station: String = r.get(1)?;
            let is_carrier =
                r.get::<_, i64>(7)? != 0 || ed_domain::station::is_carrier_callsign(&station);
            Ok(CommoditySearchHit {
                station_id: r.get(0)?,
                station,
                system: r.get(2)?,
                distance_to_arrival: r.get(3)?,
                max_pad,
                is_carrier,
                price: r.get(8)?,
                quantity: r.get(9)?,
                updated: iso(updated),
                age_hours: age(now, updated),
                distance_ly: ((sx - x).powi(2) + (sy - y).powi(2) + (sz - z).powi(2)).sqrt(),
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Find module variants stocked near an origin. The text match is scoped by
/// the spatial station query first, so human display-name searches do not
/// scan the whole outfitting table.
pub fn search_outfitting(
    conn: &Connection,
    origin: (f64, f64, f64),
    text: &str,
    radius_ly: f64,
    min_pad: Option<PadSize>,
    include_carriers: bool,
    limit: usize,
) -> Result<Vec<OutfittingSearchHit>> {
    let needle = text.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let compact: String = needle
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let (x, y, z) = origin;
    let mut stmt = conn.prepare(&format!(
        "WITH nearby AS MATERIALIZED (
             SELECT st.id, st.name AS station, sy.name AS system, st.distance_to_arrival,
                    st.pad_large, st.pad_medium, st.pad_small, st.is_carrier, st.updated,
                    sy.x, sy.y, sy.z,
                    ((sy.x-?1)*(sy.x-?1) + (sy.y-?2)*(sy.y-?2) + (sy.z-?3)*(sy.z-?3)) AS dist2
             FROM sys_systems sy INDEXED BY idx_sys_x
             JOIN sys_stations st ON st.system_id64 = sy.id64
             WHERE sy.x BETWEEN ?1 - ?4 AND ?1 + ?4
               AND sy.y BETWEEN ?2 - ?4 AND ?2 + ?4
               AND sy.z BETWEEN ?3 - ?4 AND ?3 + ?4
               AND ((sy.x-?1)*(sy.x-?1) + (sy.y-?2)*(sy.y-?2) + (sy.z-?3)*(sy.z-?3)) <= ?4 * ?4
               AND (?5 OR {SQL_ST_NON_CARRIER})
               AND st.has_outfitting = 1 AND st.name IS NOT NULL AND sy.name IS NOT NULL
         )
         SELECT st.id, st.station, st.system, st.distance_to_arrival,
                st.pad_large, st.pad_medium, st.pad_small, st.is_carrier,
                mi.symbol, mi.name, mi.class, mi.rating, mi.category, mi.ship, st.updated, st.x, st.y, st.z
         FROM nearby st
         JOIN sys_outfitting o ON o.station_id = st.id
         JOIN sys_modules mi ON mi.id = o.module_id
         WHERE (lower(COALESCE(mi.name,'')) LIKE '%' || ?6 || '%'
                OR replace(replace(lower(mi.symbol),'_',''),'-','') LIKE '%' || ?7 || '%')
         ORDER BY st.dist2, st.distance_to_arrival, mi.symbol
         LIMIT ?8"
    ))?;
    let rows = stmt.query_map(
        params![
            x,
            y,
            z,
            radius_ly,
            include_carriers,
            needle,
            compact,
            (limit.max(1) * 4) as i64
        ],
        |r| {
            let symbol: String = r.get(8)?;
            let stored_name: Option<String> = r.get(9)?;
            let max_pad = PadSize::from_counts(r.get(4)?, r.get(5)?, r.get(6)?);
            let (sx, sy, sz): (f64, f64, f64) = (r.get(15)?, r.get(16)?, r.get(17)?);
            Ok(OutfittingSearchHit {
                station_id: r.get(0)?,
                station: r.get(1)?,
                system: r.get(2)?,
                distance_to_arrival: r.get(3)?,
                max_pad,
                is_carrier: r.get::<_, i64>(7)? != 0,
                name: stored_name.unwrap_or_else(|| ed_journal::modules::item_name(&symbol)),
                symbol,
                class: r.get(10)?,
                rating: r.get(11)?,
                category: r.get(12)?,
                ship: r.get(13)?,
                updated: iso(r.get(14)?),
                distance_ly: ((sx - x).powi(2) + (sy - y).powi(2) + (sz - z).powi(2)).sqrt(),
            })
        },
    )?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|h| within_pad(h.max_pad, min_pad))
        .take(limit.max(1))
        .collect())
}

pub fn search_shipyard(
    conn: &Connection,
    origin: (f64, f64, f64),
    text: &str,
    radius_ly: f64,
    min_pad: Option<PadSize>,
    include_carriers: bool,
    limit: usize,
) -> Result<Vec<ShipyardSearchHit>> {
    let needle = text.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let compact: String = needle
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let (x, y, z) = origin;
    let mut stmt = conn.prepare(&format!(
        "WITH nearby AS MATERIALIZED (
             SELECT st.id, st.name AS station, sy.name AS system, st.distance_to_arrival,
                    st.pad_large, st.pad_medium, st.pad_small, st.is_carrier, st.updated,
                    sy.x, sy.y, sy.z,
                    ((sy.x-?1)*(sy.x-?1) + (sy.y-?2)*(sy.y-?2) + (sy.z-?3)*(sy.z-?3)) AS dist2
             FROM sys_systems sy INDEXED BY idx_sys_x
             JOIN sys_stations st ON st.system_id64 = sy.id64
             WHERE sy.x BETWEEN ?1 - ?4 AND ?1 + ?4
               AND sy.y BETWEEN ?2 - ?4 AND ?2 + ?4
               AND sy.z BETWEEN ?3 - ?4 AND ?3 + ?4
               AND ((sy.x-?1)*(sy.x-?1) + (sy.y-?2)*(sy.y-?2) + (sy.z-?3)*(sy.z-?3)) <= ?4 * ?4
               AND (?5 OR {SQL_ST_NON_CARRIER})
               AND st.has_shipyard = 1 AND st.name IS NOT NULL AND sy.name IS NOT NULL
         )
         SELECT st.id, st.station, st.system, st.distance_to_arrival,
                st.pad_large, st.pad_medium, st.pad_small, st.is_carrier,
                si.symbol, si.name, st.updated, st.x, st.y, st.z
         FROM nearby st
         JOIN sys_shipyard sh ON sh.station_id = st.id
         JOIN sys_ships si ON si.id = sh.ship_id
         WHERE (lower(COALESCE(si.name,'')) LIKE '%' || ?6 || '%'
                OR replace(replace(lower(si.symbol),'_',''),'-','') LIKE '%' || ?7 || '%')
         ORDER BY st.dist2, st.distance_to_arrival, si.symbol
         LIMIT ?8"
    ))?;
    let rows = stmt.query_map(
        params![
            x,
            y,
            z,
            radius_ly,
            include_carriers,
            needle,
            compact,
            (limit.max(1) * 4) as i64
        ],
        |r| {
            let symbol: String = r.get(8)?;
            let stored_name: Option<String> = r.get(9)?;
            let max_pad = PadSize::from_counts(r.get(4)?, r.get(5)?, r.get(6)?);
            let (sx, sy, sz): (f64, f64, f64) = (r.get(11)?, r.get(12)?, r.get(13)?);
            Ok(ShipyardSearchHit {
                station_id: r.get(0)?,
                station: r.get(1)?,
                system: r.get(2)?,
                distance_to_arrival: r.get(3)?,
                max_pad,
                is_carrier: r.get::<_, i64>(7)? != 0,
                name: stored_name.unwrap_or_else(|| ed_journal::ships::display_name(&symbol)),
                symbol,
                updated: iso(r.get(10)?),
                distance_ly: ((sx - x).powi(2) + (sy - y).powi(2) + (sz - z).powi(2)).sqrt(),
            })
        },
    )?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|h| within_pad(h.max_pad, min_pad))
        .take(limit.max(1))
        .collect())
}

/// Nearest material traders of one kind. The dump only says "Material
/// Trader"; which kind is fixed by the station's economy (raw at
/// extraction/refinery, manufactured at industrial, encoded at high tech /
/// military), which is how the game assigns them.
pub fn nearest_material_traders(
    conn: &Connection,
    origin: (f64, f64, f64),
    kind: &str,
    radius_ly: f64,
    limit: usize,
) -> Result<Vec<StationWithService>> {
    let economies: &[&str] = match kind.trim().to_ascii_lowercase().as_str() {
        "raw" => &["Extraction", "Refinery"],
        "manufactured" => &["Industrial"],
        "encoded" => &["High Tech", "Military"],
        other => {
            anyhow::bail!("unknown trader kind {other:?}; expected raw, manufactured or encoded")
        }
    };
    let (x, y, z) = origin;
    let sql = format!(
        "SELECT {STATION_COLS},
                ((sy.x-?1)*(sy.x-?1) + (sy.y-?2)*(sy.y-?2) + (sy.z-?3)*(sy.z-?3)) AS d2
         FROM sys_stations s
         JOIN sys_systems sy ON sy.id64 = s.system_id64
         WHERE s.has_material_trader = 1
           AND s.is_carrier = 0
           AND s.primary_economy IN (?5, ?6)
           AND sy.x BETWEEN ?1 - ?4 AND ?1 + ?4
           AND sy.y BETWEEN ?2 - ?4 AND ?2 + ?4
           AND sy.z BETWEEN ?3 - ?4 AND ?3 + ?4
           AND d2 <= ?4 * ?4
         ORDER BY d2, s.distance_to_arrival IS NULL, s.distance_to_arrival
         LIMIT ?7"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            x,
            y,
            z,
            radius_ly,
            economies[0],
            economies.get(1).copied().unwrap_or(economies[0]),
            limit as i64
        ],
        |r| {
            let d2: f64 = r.get("d2")?;
            Ok(StationWithService {
                station: station_from_row(r)?,
                distance_ly: d2.sqrt(),
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StationWithService {
    pub station: StationInfo,
    pub distance_ly: f64,
}

/// The services the dump lists, by a friendly key (snake_case or spaced).
pub const SERVICES: &[(&str, &str)] = &[
    ("interstellar_factors", "Interstellar Factors Contact"),
    ("technology_broker", "Technology Broker"),
    ("universal_cartographics", "Universal Cartographics"),
    ("black_market", "Black Market"),
    ("search_and_rescue", "Search and Rescue"),
    ("refuel", "Refuel"),
    ("repair", "Repair"),
    ("restock", "Restock"),
    ("refinery", "Refinery Contact"),
    ("vista_genomics", "Vista Genomics"),
    ("crew_lounge", "Crew Lounge"),
    ("fleet_carrier_vendor", "Fleet Carrier Vendor"),
    ("material_trader", "Material Trader"),
    ("missions", "Missions"),
    ("redemption_office", "Redemption Office"),
    ("pioneer_supplies", "Pioneer Supplies"),
    ("powerplay", "Powerplay"),
    ("bartender", "Bartender"),
    ("frontline_solutions", "Frontline Solutions"),
    ("apex_interstellar", "Apex Interstellar"),
    ("workshop", "Workshop"),
    ("livery", "Livery"),
    ("shop", "Shop"),
    ("system_colonisation", "System Colonisation"),
    ("construction_services", "Construction Services"),
];

/// The dump's name for a friendly service key ("tech broker", "technology_broker"...).
pub fn service_name(key: &str) -> Option<&'static str> {
    let k = key.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    let k = k.trim_end_matches("_contact").to_string();
    SERVICES
        .iter()
        .find(|(f, n)| {
            *f == k
                || n.to_ascii_lowercase().replace(' ', "_") == k
                || (k == "tech_broker" && *f == "technology_broker")
                || (k == "cartographics" && *f == "universal_cartographics")
        })
        .map(|(_, n)| *n)
}

/// Nearest stations offering a service, with a landing-pad filter.
///
/// `service` is `market`, `outfitting`, `shipyard`, or any key in
/// [`SERVICES`] (matched against the station's listed services). Stations
/// whose pad size is unrecorded are excluded when `min_pad` is set --
/// failing closed, because "probably fits" is how a Cutter ends up at an
/// outpost.
pub fn nearest_with_service(
    conn: &Connection,
    origin: (f64, f64, f64),
    service: &str,
    min_pad: Option<PadSize>,
    radius_ly: f64,
    include_carriers: bool,
    limit: usize,
) -> Result<Vec<StationWithService>> {
    // ?7 is always bound (rusqlite counts placeholders), so the column cases
    // carry a no-op comparison against the empty string.
    let (predicate, svc): (String, String) = match service.trim().to_ascii_lowercase().as_str() {
        "market" => ("s.has_market = 1 AND ?7 = ''".into(), String::new()),
        "outfitting" => ("s.has_outfitting = 1 AND ?7 = ''".into(), String::new()),
        "shipyard" => ("s.has_shipyard = 1 AND ?7 = ''".into(), String::new()),
        other => match service_name(other) {
            Some(n) => ("EXISTS (SELECT 1 FROM sys_station_services ss WHERE ss.station_id = s.id AND ss.service = ?7)".into(), n.to_string()),
            None => anyhow::bail!("unknown service {other:?}; expected market, outfitting, shipyard or one of: {}", SERVICES.iter().map(|(f, _)| *f).collect::<Vec<_>>().join(", ")),
        },
    };
    let (x, y, z) = origin;

    let sql = format!(
        "SELECT {STATION_COLS},
                ((sy.x-?1)*(sy.x-?1) + (sy.y-?2)*(sy.y-?2) + (sy.z-?3)*(sy.z-?3)) AS d2
         FROM sys_stations s
         JOIN sys_systems sy ON sy.id64 = s.system_id64
         WHERE {predicate}
           AND sy.x BETWEEN ?1 - ?4 AND ?1 + ?4
           AND sy.y BETWEEN ?2 - ?4 AND ?2 + ?4
           AND sy.z BETWEEN ?3 - ?4 AND ?3 + ?4
           AND d2 <= ?4 * ?4
           AND (?5 = 1 OR s.is_carrier = 0)
         ORDER BY d2, s.distance_to_arrival IS NULL, s.distance_to_arrival
         LIMIT ?6"
    );

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![
            x,
            y,
            z,
            radius_ly,
            include_carriers as i64,
            (limit * 4) as i64,
            svc
        ],
        |r| {
            let d2: f64 = r.get("d2")?;
            Ok(StationWithService {
                station: station_from_row(r)?,
                distance_ly: d2.sqrt(),
            })
        },
    )?;

    let mut out = Vec::new();
    for row in rows {
        let row = row?;
        if let Some(required) = min_pad {
            match row.station.max_pad {
                Some(p) if p.fits(required) => {}
                // Unknown pad fails closed rather than being assumed adequate.
                _ => continue,
            }
        }
        out.push(row);
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO sys_systems (id64,name,x,y,z,population,controlling_power,power_state)
               VALUES (1,'Deciat',0,0,0,1000,'A. Lavigny-Duval','Stronghold'),
                      (2,'Sol',5,0,0,2000,'Zachary Hudson','Fortified'),
                      (3,'Faraway',100,0,0,10,NULL,NULL);
             INSERT INTO sys_stations
               (id,system_id64,name,type,distance_to_arrival,pad_large,pad_medium,pad_small,
                has_market,has_outfitting,has_shipyard,is_carrier)
               VALUES (10,1,'Garay Terminal','Coriolis Starport',300,4,4,4,1,1,1,0),
                      (11,1,'Tiny Outpost','Outpost',900,0,2,4,1,0,0,0),
                      (12,1,'Unknown Pads','Outpost',50,NULL,NULL,NULL,1,0,0,0),
                      (13,2,'Abraham Lincoln','Orbis Starport',500,4,4,4,1,1,1,0),
                      (14,2,'W1V-8BQ','Drake-Class Carrier',100,8,4,4,1,0,0,1);",
        )
        .unwrap();
        conn
    }

    /// `sys_stations.updated` is an epoch; the station shape reports it as
    /// an ISO string *and* a computed age, like the commodity hits do.
    #[test]
    fn station_info_reports_market_age_hours() {
        let conn = db();
        let two_hours_ago = now_epoch_secs() - 7200;
        conn.execute(
            "UPDATE sys_stations SET updated = ?1 WHERE id = 10",
            [two_hours_ago],
        )
        .unwrap();
        let st = find_stations(&conn, "Garay Terminal", 1).unwrap().remove(0);
        assert_eq!(
            st.updated.as_deref(),
            Some(crate::session::iso_from_epoch(two_hours_ago).as_str())
        );
        let age = st.age_hours.unwrap();
        assert!((age - 2.0).abs() < 0.01, "age_hours = {age}");

        let unknown = find_stations(&conn, "Tiny Outpost", 1).unwrap().remove(0);
        assert_eq!((unknown.updated, unknown.age_hours), (None, None));
    }

    #[test]
    fn market_stations_within_reports_pads_carriers_and_powers_interpreted() {
        let conn = db();
        let mut found = market_stations_within(&conn, (0.0, 0.0, 0.0), 10.0).unwrap();
        found.sort_by_key(|s| s.station_id);
        let ids: Vec<i64> = found.iter().map(|s| s.station_id).collect();
        assert_eq!(ids, vec![10, 11, 12, 13, 14], "Faraway is out of range");
        assert_eq!(found[0].max_pad, Some(PadSize::Large));
        assert_eq!(found[1].max_pad, Some(PadSize::Medium));
        assert_eq!(found[2].max_pad, None);
        assert!(found[4].is_carrier);
        assert_eq!(
            found[0].controlling_power.as_deref(),
            Some("A. Lavigny-Duval")
        );
    }

    #[test]
    fn system_lookup_returns_power_and_station_count() {
        let conn = db();
        let s = system(&conn, "deciat").unwrap().unwrap();
        assert_eq!(s.name, "Deciat");
        assert_eq!(s.controlling_power.as_deref(), Some("A. Lavigny-Duval"));
        assert_eq!(s.power_provenance, Some(Provenance::Community));
        assert_eq!(s.station_count, 3);
    }

    #[test]
    fn a_first_hand_observation_overrides_the_community_view() {
        let conn = db();
        // The commander jumped in and saw something different from the dump.
        conn.execute(
            "INSERT INTO powerplay_observations
                 (file,offset,ts,system_name,controlling_power,powerplay_state,control_progress)
             VALUES ('J.log',0,'2026-08-24T00:00:00Z','Deciat','Aisling Duval','Exploited',0.5)",
            [],
        )
        .unwrap();

        let s = system(&conn, "Deciat").unwrap().unwrap();
        assert_eq!(s.controlling_power.as_deref(), Some("Aisling Duval"));
        assert_eq!(s.power_state.as_deref(), Some("Exploited"));
        assert_eq!(s.power_provenance, Some(Provenance::FirstHand));
        assert_eq!(s.power_observed.as_deref(), Some("2026-08-24T00:00:00Z"));
    }

    #[test]
    fn max_pad_is_the_largest_offered_and_unknown_stays_unknown() {
        assert_eq!(
            PadSize::from_counts(Some(4), Some(4), Some(4)),
            Some(PadSize::Large)
        );
        assert_eq!(
            PadSize::from_counts(Some(0), Some(2), Some(4)),
            Some(PadSize::Medium)
        );
        assert_eq!(
            PadSize::from_counts(Some(0), Some(0), Some(4)),
            Some(PadSize::Small)
        );
        assert_eq!(PadSize::from_counts(None, None, None), None);
    }

    #[test]
    fn pad_filter_excludes_stations_whose_pads_are_unrecorded() {
        let conn = db();
        let all =
            nearest_with_service(&conn, (0.0, 0.0, 0.0), "market", None, 50.0, true, 10).unwrap();
        assert!(all
            .iter()
            .any(|r| r.station.name.as_deref() == Some("Unknown Pads")));

        // With a pad requirement, the unknown-pad station must drop out --
        // failing closed rather than assuming it fits.
        let large = nearest_with_service(
            &conn,
            (0.0, 0.0, 0.0),
            "market",
            Some(PadSize::Large),
            50.0,
            true,
            10,
        )
        .unwrap();
        let names: Vec<_> = large
            .iter()
            .filter_map(|r| r.station.name.as_deref())
            .collect();
        assert!(!names.contains(&"Unknown Pads"));
        assert!(
            !names.contains(&"Tiny Outpost"),
            "medium pad cannot take a large ship"
        );
        assert!(names.contains(&"Garay Terminal"));
    }

    #[test]
    fn carriers_can_be_excluded_because_they_move() {
        let conn = db();
        let with =
            nearest_with_service(&conn, (0.0, 0.0, 0.0), "market", None, 50.0, true, 10).unwrap();
        let without =
            nearest_with_service(&conn, (0.0, 0.0, 0.0), "market", None, 50.0, false, 10).unwrap();
        assert!(with.iter().any(|r| r.station.is_carrier));
        assert!(!without.iter().any(|r| r.station.is_carrier));
    }

    #[test]
    fn nearby_search_is_bounded_by_radius_and_sorted_by_distance() {
        let conn = db();
        let near = systems_within(&conn, (0.0, 0.0, 0.0), 10.0, 10).unwrap();
        let names: Vec<_> = near.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["Deciat", "Sol"], "Faraway is 100ly out");
        assert!((near[1].distance_ly - 5.0).abs() < 1e-9);
    }

    #[test]
    fn shipyard_search_only_returns_stations_that_have_one() {
        let conn = db();
        let found =
            nearest_with_service(&conn, (0.0, 0.0, 0.0), "shipyard", None, 50.0, true, 10).unwrap();
        assert!(found.iter().all(|r| r.station.has_shipyard));
        assert_eq!(found.len(), 2);
    }

    /// Sell searches enforce confiscation (opt-in via black markets) and
    /// the carrier envelope; STATION prices are shown as reported — the
    /// 999000/999999-demand board here stays visible on purpose (the
    /// 2026-09-04 retraction: real demand spikes are real prices, and
    /// Inara shows them too). Buy searches are untouched by all of it.
    #[test]
    fn sell_searches_enforce_confiscation_carriers_and_the_envelope() {
        let conn = db();
        conn.execute_batch(
            "INSERT INTO sys_commodities (symbol,name,category) VALUES ('gold','Gold','Metals');
             INSERT INTO sys_market (station_id,commodity_id,buy_price,sell_price,demand,supply,updated)
                 SELECT 10,id,9000,9100,200,500,unixepoch() FROM sys_commodities WHERE symbol='gold';
             INSERT INTO sys_market (station_id,commodity_id,buy_price,sell_price,demand,supply,updated)
                 SELECT 13,id,9000,999000,999999,500,unixepoch() FROM sys_commodities WHERE symbol='gold';
             INSERT INTO sys_market (station_id,commodity_id,buy_price,sell_price,demand,supply,updated)
                 SELECT 11,id,9000,9500,300,500,unixepoch() FROM sys_commodities WHERE symbol='gold';
             INSERT INTO sys_commodity_stats (commodity_id, mean_sell, boards)
                 SELECT id, 9000.0, 1000 FROM sys_commodities WHERE symbol='gold';
             INSERT INTO sys_market_prohibited (station_id, symbol) VALUES (11, 'gold');
             -- A carrier the flag lies about: callsign name, is_carrier 0.
             INSERT INTO sys_stations (id,system_id64,name,type,distance_to_arrival,pad_large,pad_medium,pad_small,has_market,is_carrier)
                 VALUES (15,1,'X9Z-99X','',100,8,4,4,1,0);
             INSERT INTO sys_market (station_id,commodity_id,buy_price,sell_price,demand,supply,updated)
                 SELECT 15,id,9000,9200,400,500,unixepoch() FROM sys_commodities WHERE symbol='gold';",
        )
        .unwrap();
        let sellers = search_commodity(
            &conn,
            (0.0, 0.0, 0.0),
            "gold",
            "sell",
            20.0,
            None,
            false,
            false,
            72.0,
            0,
            CommoditySort::Price,
            10,
        )
        .unwrap();
        assert_eq!(
            sellers.iter().map(|h| h.station.as_str()).collect::<Vec<_>>(),
            vec!["Abraham Lincoln", "Garay Terminal"],
            "the spike board stays visible; confiscating and misflagged-carrier boards are silenced"
        );
        let buyers = search_commodity(
            &conn,
            (0.0, 0.0, 0.0),
            "gold",
            "buy",
            20.0,
            None,
            false,
            false,
            72.0,
            0,
            CommoditySort::Price,
            10,
        )
        .unwrap();
        assert_eq!(
            buyers.len(),
            3,
            "buying is legal everywhere; supply data is untouched"
        );

        // Carrier envelope (maintainer rule): a carrier selling outside one
        // std dev of the extreme station prices vanishes; back inside,
        // it appears. Station results are never envelope-filtered.
        conn.execute_batch(
            "UPDATE sys_commodity_stats SET station_sell_min = 8000, station_sell_max = 9500,
                    station_sell_std = 200, station_boards = 1000;
             INSERT INTO sys_market (station_id,commodity_id,buy_price,sell_price,demand,supply,updated)
                 SELECT 14,id,0,15000,500,0,unixepoch() FROM sys_commodities WHERE symbol='gold';",
        )
        .unwrap();
        let sellers = |conn: &Connection, prohibited_in: bool| {
            search_commodity(
                conn,
                (0.0, 0.0, 0.0),
                "gold",
                "sell",
                20.0,
                None,
                true,
                prohibited_in,
                72.0,
                0,
                CommoditySort::Price,
                10,
            )
            .unwrap()
            .iter()
            .map(|h| h.station.clone())
            .collect::<Vec<_>>()
        };
        assert_eq!(
            sellers(&conn, false),
            vec!["Abraham Lincoln", "X9Z-99X", "Garay Terminal"],
            "15000 is past 9500 + 1x200 for a CARRIER; the station spike is untouched"
        );
        conn.execute(
            "UPDATE sys_market SET sell_price = 9600 WHERE station_id = 14",
            [],
        )
        .unwrap();
        assert_eq!(
            sellers(&conn, false),
            vec!["Abraham Lincoln", "W1V-8BQ", "X9Z-99X", "Garay Terminal"],
            "9600 sits inside the envelope and outbids the ordinary station"
        );

        // Prohibited opt-in (maintainer rule): the confiscating station appears
        // only when opted in AND it has a black-market contact.
        assert!(
            !sellers(&conn, true).contains(&"Tiny Outpost".to_string()),
            "opted in without a black market, the sale is still impossible"
        );
        conn.execute(
            "INSERT INTO sys_station_services (station_id, service) VALUES (11, 'Black Market')",
            [],
        )
        .unwrap();
        assert!(
            sellers(&conn, true).contains(&"Tiny Outpost".to_string()),
            "opted in with a black market, the sale is the commander's choice"
        );
        assert!(
            !sellers(&conn, false).contains(&"Tiny Outpost".to_string()),
            "the default still hides it"
        );
    }

    #[test]
    fn compact_catalogs_drive_market_outfitting_and_ship_searches() {
        let conn = db();
        conn.execute_batch(
            "INSERT INTO sys_commodities (symbol,name,category)
                 VALUES ('gold','Gold','Metals');
             INSERT INTO sys_market
                 (station_id,commodity_id,buy_price,sell_price,demand,supply,updated)
                 SELECT 10,id,10000,9000,200,500,unixepoch()
                 FROM sys_commodities WHERE symbol='gold';
             INSERT INTO sys_modules (symbol,name,class,rating,category)
                 VALUES ('int_hyperdrive_size5_class5','5A Frame Shift Drive',5,'A','core');
             INSERT INTO sys_outfitting (station_id,module_id)
                 SELECT 10,id FROM sys_modules
                 WHERE symbol='int_hyperdrive_size5_class5';
             INSERT INTO sys_ships (symbol,name)
                 VALUES ('cobramkiii','Cobra Mk III');
             INSERT INTO sys_shipyard (station_id,ship_id)
                 SELECT 10,id FROM sys_ships WHERE symbol='cobramkiii';",
        )
        .unwrap();

        let market = search_commodity(
            &conn,
            (0.0, 0.0, 0.0),
            "gold",
            "buy",
            20.0,
            Some(PadSize::Large),
            false,
            false,
            72.0,
            0,
            CommoditySort::Price,
            10,
        )
        .unwrap();
        assert_eq!(market.len(), 1);
        assert_eq!(market[0].station, "Garay Terminal");
        assert_eq!((market[0].price, market[0].quantity), (10000, 500));
        // F3 semantics under LIMIT 1: distance sort selects the NEAREST
        // match, min_quantity bounds depth in SQL, and a pad the station
        // lacks empties the result instead of under-filling it.
        let nearest = search_commodity(
            &conn,
            (0.0, 0.0, 0.0),
            "gold",
            "buy",
            20.0,
            None,
            false,
            false,
            72.0,
            0,
            CommoditySort::Distance,
            1,
        )
        .unwrap();
        assert_eq!(
            nearest.len(),
            1,
            "distance order fills from the nearest match"
        );
        let deep = search_commodity(
            &conn,
            (0.0, 0.0, 0.0),
            "gold",
            "buy",
            20.0,
            None,
            false,
            false,
            72.0,
            501,
            CommoditySort::Price,
            10,
        )
        .unwrap();
        assert!(deep.is_empty(), "supply 500 fails a min_quantity of 501");

        let modules = search_outfitting(
            &conn,
            (0.0, 0.0, 0.0),
            "5A Frame",
            20.0,
            Some(PadSize::Large),
            false,
            10,
        )
        .unwrap();
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].class, Some(5));
        assert_eq!(modules[0].rating.as_deref(), Some("A"));

        let ships = search_shipyard(
            &conn,
            (0.0, 0.0, 0.0),
            "Cobra",
            20.0,
            Some(PadSize::Large),
            false,
            10,
        )
        .unwrap();
        assert_eq!(ships.len(), 1);
        assert_eq!(ships[0].name, "Cobra Mk III");
    }

    #[test]
    fn an_unknown_service_is_an_error_not_an_empty_result() {
        let conn = db();
        // Silently returning nothing would read as "no such station nearby".
        assert!(nearest_with_service(
            &conn,
            (0.0, 0.0, 0.0),
            "quantum_repair",
            None,
            10.0,
            true,
            5
        )
        .is_err());
    }

    #[test]
    fn stations_are_ranked_by_class_before_distance() {
        let conn = db();
        let sts = stations_in_system(&conn, "Deciat").unwrap();
        let names: Vec<_> = sts.iter().filter_map(|s| s.name.as_deref()).collect();
        // The starport leads even though an outpost is physically closer:
        // sorting purely by distance is what buries the station you actually
        // want under parked carriers and 50-ls installations.
        assert_eq!(
            names,
            vec!["Garay Terminal", "Unknown Pads", "Tiny Outpost"]
        );
    }

    #[test]
    fn carriers_and_settlements_are_excluded_by_default() {
        let conn = db();
        conn.execute_batch(
            "INSERT INTO sys_stations
               (id,system_id64,name,type,distance_to_arrival,pad_large,pad_medium,pad_small,
                has_market,has_outfitting,has_shipyard,is_carrier)
             VALUES (20,1,'Parked Carrier','Drake-Class Carrier',0,8,4,4,1,0,0,1),
                    (21,1,'Some Hovel','Settlement',62,0,0,1,0,0,0,0),
                    (22,1,'Nameless Beacon',NULL,NULL,NULL,NULL,NULL,0,0,0,0);",
        )
        .unwrap();

        let default = stations_in_system(&conn, "Deciat").unwrap();
        let names: Vec<_> = default.iter().filter_map(|s| s.name.as_deref()).collect();
        assert!(
            !names.contains(&"Parked Carrier"),
            "a carrier at 0 ls must not lead"
        );
        assert!(!names.contains(&"Some Hovel"));
        assert!(!names.contains(&"Nameless Beacon"));

        // They are available on request -- filtered, not discarded.
        let all = stations_in_system_filtered(&conn, "Deciat", true, true).unwrap();
        let names: Vec<_> = all.iter().filter_map(|s| s.name.as_deref()).collect();
        assert!(names.contains(&"Parked Carrier"));
        assert!(names.contains(&"Some Hovel"));
    }

    #[test]
    fn station_classes_match_the_dump_type_strings() {
        use StationClass::*;
        for (kind, want) in [
            ("Coriolis Starport", Starport),
            ("Orbis Starport", Starport),
            ("Ocellus Starport", Starport),
            ("Dodec Starport", Starport),
            ("Asteroid base", Starport),
            ("Mega ship", Starport),
            ("Planetary Port", PlanetaryPort),
            ("Outpost", Outpost),
            ("Planetary Outpost", Outpost),
            ("Drake-Class Carrier", Carrier),
            ("Settlement", Settlement),
            ("Surface Settlement", Settlement),
            ("Space Construction Depot", ConstructionDepot),
            ("Planetary Construction Depot", ConstructionDepot),
        ] {
            assert_eq!(StationClass::of(Some(kind)), want, "{kind}");
        }
        // An untyped row is Other, not silently treated as dockable.
        assert_eq!(StationClass::of(None), Other);
        assert!(!Other.is_dockable());
        assert!(Starport.is_dockable());
    }
}
