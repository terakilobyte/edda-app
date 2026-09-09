//! GET /v1/stations — the wire half of the three station lookups the
//! ship computer ran on the local sys_* tables (API-only spec, Phase
//! A.3): stations in a system, nearest with a service, by name prefix.
//! The response is `ed_store::lookup::StationInfo`'s shape (plus
//! `distance_ly`) so the client deserializes into the type it already
//! renders. the assistant's notes (2026-09-07) are all here: `near=` resolves
//! the name through the routing index first (in the handler), the
//! sphere uses the cell predicate, an unknown service is a 400 not an
//! empty list, `name=` reuses the completion index, carriers are opt-in,
//! the limit is clamped, and the knowledge limiter gates it.

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
    if let Some(column) = COLUMN_SERVICES.iter().find(|c| **c == key) {
        return Some(column);
    }
    let raw_form = key.replace('_', "");
    SERVICES
        .iter()
        .find(|(friendly, raw)| *friendly == key || *raw == raw_form)
        .map(|(_, raw)| *raw)
}

pub fn accepted_services() -> String {
    let mut keys: Vec<&str> = COLUMN_SERVICES.to_vec();
    keys.extend(SERVICES.iter().map(|(friendly, _)| *friendly));
    keys.join(", ")
}

#[derive(Debug, Deserialize)]
pub struct StationsQuery {
    pub system: Option<String>,
    /// Comma-separated system names (B.4 gap 3: the game-route fuel
    /// marks count docks along a route in one call, not one per hop).
    pub systems: Option<String>,
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

fn default_true() -> bool {
    true
}

/// The programmatic default is the wire default: minor stations in,
/// carriers out.
impl Default for StationsQuery {
    fn default() -> Self {
        StationsQuery {
            system: None,
            systems: None,
            near: None,
            service: None,
            radius_ly: None,
            min_pad: None,
            name: None,
            include_carriers: false,
            include_minor: true,
            limit: None,
        }
    }
}

#[derive(Debug)]
pub enum Mode {
    InSystem(String),
    /// Up to `MAX_SYSTEMS` names, deduplicated case-blind, order kept.
    InSystems(Vec<String>),
    Near { system: String, service: Option<&'static str>, radius_ly: f64, min_pad: Option<PadSize> },
    Name(String),
}

/// Names one `systems=` call may carry, and rows it may return: a route
/// is a hundred hops at most and a hop has a handful of docks.
pub const MAX_SYSTEMS: usize = 200;
pub const MAX_SYSTEMS_ROWS: usize = 1_000;

impl StationsQuery {
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }

    /// The row cap for `systems=`: the whole list by default, so a
    /// route's dock count is never silently short.
    pub fn systems_limit(&self) -> usize {
        self.limit.unwrap_or(MAX_SYSTEMS_ROWS).clamp(1, MAX_SYSTEMS_ROWS)
    }

    /// Exactly one of `system` / `systems` / `near` / `name`; the error
    /// text is the 400 body.
    pub fn mode(&self) -> Result<Mode, String> {
        let given = [self.system.is_some(), self.systems.is_some(), self.near.is_some(), self.name.is_some()]
            .iter()
            .filter(|given| **given)
            .count();
        if given != 1 {
            return Err("give exactly one of system=, systems=, near=, name=".into());
        }
        if let Some(system) = &self.system {
            return Ok(Mode::InSystem(system.trim().to_owned()));
        }
        if let Some(list) = &self.systems {
            let mut seen = std::collections::HashSet::new();
            let names: Vec<String> = list
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter(|s| seen.insert(s.to_ascii_lowercase()))
                .map(str::to_owned)
                .collect();
            if names.is_empty() {
                return Err("systems= needs at least one name".into());
            }
            if names.len() > MAX_SYSTEMS {
                return Err(format!("systems= takes at most {MAX_SYSTEMS} names, got {}", names.len()));
            }
            return Ok(Mode::InSystems(names));
        }
        if let Some(prefix) = &self.name {
            return Ok(Mode::Name(prefix.trim().to_owned()));
        }
        let system = self.near.as_deref().unwrap_or("").trim().to_owned();
        let service = match self.service.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            None => None,
            Some(text) => Some(
                service_key(text)
                    .ok_or_else(|| format!("unknown service {text:?}; accepted: {}", accepted_services()))?,
            ),
        };
        let min_pad = match self.min_pad.as_deref().map(str::trim) {
            None | Some("") | Some("any") | Some("Any") => None,
            Some(text) => Some(PadSize::parse(text).ok_or_else(|| format!("unknown pad size {text:?}"))?),
        };
        Ok(Mode::Near {
            system,
            service,
            radius_ly: self.radius_ly.unwrap_or(DEFAULT_NEAR_RADIUS_LY).clamp(1.0, MAX_NEAR_RADIUS_LY),
            min_pad,
        })
    }
}

/// The 15 station columns, in order; timestamps as text and hours so no
/// sqlx time feature is needed.
const COLS: &str = "st.id, st.name, sy.name, st.station_type, st.arrival_ls, st.pad_small, st.pad_medium, st.pad_large, \
                    st.has_market, st.has_outfitting, st.has_shipyard, COALESCE(st.is_carrier, false), \
                    st.identity_observed_at::text, \
                    EXTRACT(EPOCH FROM now() - st.identity_observed_at)::DOUBLE PRECISION / 3600.0";

fn station_json(row: &sqlx::postgres::PgRow, distance_ly: Option<f64>) -> Value {
    let (ps, pm, pl): (Option<i32>, Option<i32>, Option<i32>) = (row.get(5), row.get(6), row.get(7));
    let kind: Option<String> = row.get(3);
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
        "updated": row.get::<Option<String>, _>(12),
        "age_hours": row.get::<Option<f64>, _>(13),
        "distance_ly": distance_ly,
    })
}

fn minor(class: StationClass) -> bool {
    matches!(class, StationClass::Settlement | StationClass::ConstructionDepot | StationClass::Other)
}

/// Stations in one system, the local `stations_in_system_filtered`
/// order: class rank, then name.
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
        .map(|row| station_json(row, None))
        .filter(|v| q.include_carriers || v["is_carrier"] != true)
        .map(|v| (StationClass::of(v["kind"].as_str()), v))
        .filter(|(class, _)| q.include_minor || !minor(*class))
        .collect();
    out.sort_by(|a, b| {
        a.0.rank().cmp(&b.0.rank()).then_with(|| a.1["name"].as_str().cmp(&b.1["name"].as_str()))
    });
    Ok(out.into_iter().map(|(_, v)| v).take(q.limit()).collect())
}

/// Stations in each of several systems, one call: rows keep the order
/// the names were given (a route's hop order), then class rank, then
/// name. A name the server does not know simply contributes no rows —
/// the caller counts docks per `system_name` and treats absence as
/// "no dock known", which is what the fuel marks want.
pub async fn in_systems(pool: &PgPool, systems: &[String], q: &StationsQuery) -> anyhow::Result<Vec<Value>> {
    let keys: Vec<String> = systems.iter().map(|s| s.to_lowercase()).collect();
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM stations st JOIN systems sy ON sy.address = st.system_address \
         WHERE lower(sy.name) = ANY($1) ORDER BY st.name"
    ))
    .bind(&keys)
    .fetch_all(pool)
    .await?;
    let position = |v: &Value| -> usize {
        v["system_name"]
            .as_str()
            .and_then(|name| keys.iter().position(|k| k == &name.to_lowercase()))
            .unwrap_or(usize::MAX)
    };
    let mut out: Vec<(usize, StationClass, Value)> = rows
        .iter()
        .map(|row| station_json(row, None))
        .filter(|v| q.include_carriers || v["is_carrier"] != true)
        .map(|v| (position(&v), StationClass::of(v["kind"].as_str()), v))
        .filter(|(_, class, _)| q.include_minor || !minor(*class))
        .collect();
    out.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.rank().cmp(&b.1.rank()))
            .then_with(|| a.2["name"].as_str().cmp(&b.2["name"].as_str()))
    });
    Ok(out.into_iter().map(|(_, _, v)| v).take(q.systems_limit()).collect())
}

/// Nearest stations with a service inside a sphere, nearest first. The
/// pad floor is applied after the query (four times the limit is
/// fetched so a Large-only ask still fills).
pub async fn near(
    pool: &PgPool,
    origin: (f64, f64, f64),
    service: Option<&str>,
    radius_ly: f64,
    min_pad: Option<PadSize>,
    q: &StationsQuery,
) -> anyhow::Result<Vec<Value>> {
    let (ox, oy, oz) = origin;
    let predicate = match service {
        None => "TRUE",
        Some("market") => "st.has_market",
        Some("outfitting") => "st.has_outfitting",
        Some("shipyard") => "st.has_shipyard",
        Some(_) => "EXISTS (SELECT 1 FROM station_services ss WHERE ss.station_id = st.id AND ss.service = $6)",
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
    .bind(ox)
    .bind(oy)
    .bind(oz)
    .bind(radius_ly)
    .bind(crate::geo::cells_covering(ox, oy, oz, radius_ly))
    .bind(service.unwrap_or(""))
    .bind(q.include_carriers)
    .bind((q.limit() * 4) as i64)
    .persistent(false)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| station_json(row, Some(row.get::<f64, _>(14))))
        .filter(|v| match min_pad {
            None => true,
            Some(required) => {
                serde_json::from_value::<Option<PadSize>>(v["max_pad"].clone()).ok().flatten().is_some_and(|p| p.fits(required))
            }
        })
        .take(q.limit())
        .collect())
}

/// Stations whose name starts with `prefix`: the completion index picks
/// the names, then the rows come back whole. Carriers sort last.
pub async fn by_name(pool: &PgPool, prefix: &str, q: &StationsQuery) -> anyhow::Result<Vec<Value>> {
    if prefix.chars().count() < 2 {
        return Ok(Vec::new());
    }
    let hits = crate::names::complete_stations(pool, prefix, q.limit()).await?;
    let names: Vec<String> = hits.iter().map(|h| h.name.clone()).collect();
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM stations st LEFT JOIN systems sy ON sy.address = st.system_address \
         WHERE st.name = ANY($1) ORDER BY COALESCE(st.is_carrier, false), st.name LIMIT $2"
    ))
    .bind(&names)
    .bind(q.limit() as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(|row| station_json(row, None)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_mode() {
        let q = |s: &str| serde_urlencoded::from_str::<StationsQuery>(s).unwrap();
        assert!(matches!(q("system=Sol").mode().unwrap(), Mode::InSystem(ref s) if s == "Sol"));
        assert!(matches!(
            q("near=Sol&service=material_trader").mode().unwrap(),
            Mode::Near { ref service, .. } if service.as_deref() == Some("materialtrader")
        ));
        assert!(matches!(q("name=jame").mode().unwrap(), Mode::Name(ref p) if p == "jame"));
        assert!(q("").mode().is_err());
        assert!(q("system=Sol&name=x").mode().is_err());
        assert!(q("near=Sol&service=teleporter").mode().unwrap_err().contains("material_trader"));
        assert!(matches!(q("near=Sol&min_pad=l").mode().unwrap(), Mode::Near { min_pad: Some(PadSize::Large), .. }));
        assert!(q("near=Sol&min_pad=huge").mode().is_err());
    }

    /// `systems=` is a comma list: trimmed, empties dropped, duplicates
    /// folded case-blind with the first spelling and order kept (a
    /// route's hop order); bounded; exclusive with the other modes.
    #[test]
    fn a_system_list_is_one_mode() {
        let q = |s: &str| serde_urlencoded::from_str::<StationsQuery>(s).unwrap();
        let Mode::InSystems(names) = q("systems=Sol,%20Deciat%20,,sol,Wongi").mode().unwrap() else {
            panic!("systems= is the list mode");
        };
        assert_eq!(names, vec!["Sol", "Deciat", "Wongi"]);
        assert!(q("systems=,,").mode().is_err());
        assert!(q("systems=Sol&system=Sol").mode().is_err());
        let many: Vec<String> = (0..=MAX_SYSTEMS).map(|i| format!("S{i}")).collect();
        assert!(q(&format!("systems={}", many.join(","))).mode().unwrap_err().contains("at most"));
        assert_eq!(q("systems=Sol").systems_limit(), MAX_SYSTEMS_ROWS, "the whole list by default");
        assert_eq!(q("systems=Sol&limit=5").systems_limit(), 5);
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

    #[test]
    fn the_limit_is_clamped() {
        assert_eq!(StationsQuery { limit: Some(9_000), ..Default::default() }.limit(), MAX_LIMIT);
        assert_eq!(StationsQuery { limit: Some(0), ..Default::default() }.limit(), 1);
        assert_eq!(StationsQuery::default().limit(), DEFAULT_LIMIT);
    }
}
