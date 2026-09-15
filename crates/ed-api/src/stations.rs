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
    Near {
        system: String,
        service: Option<&'static str>,
        radius_ly: f64,
        min_pad: Option<PadSize>,
    },
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
        self.limit
            .unwrap_or(MAX_SYSTEMS_ROWS)
            .clamp(1, MAX_SYSTEMS_ROWS)
    }

    /// Exactly one of `system` / `systems` / `near` / `name`; the error
    /// text is the 400 body.
    pub fn mode(&self) -> Result<Mode, String> {
        let given = [
            self.system.is_some(),
            self.systems.is_some(),
            self.near.is_some(),
            self.name.is_some(),
        ]
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
                return Err(format!(
                    "systems= takes at most {MAX_SYSTEMS} names, got {}",
                    names.len()
                ));
            }
            return Ok(Mode::InSystems(names));
        }
        if let Some(prefix) = &self.name {
            return Ok(Mode::Name(prefix.trim().to_owned()));
        }
        let system = self.near.as_deref().unwrap_or("").trim().to_owned();
        let service = match self
            .service
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            None => None,
            Some(text) => Some(service_key(text).ok_or_else(|| {
                format!(
                    "unknown service {text:?}; accepted: {}",
                    accepted_services()
                )
            })?),
        };
        let min_pad = match self.min_pad.as_deref().map(str::trim) {
            None | Some("") | Some("any") | Some("Any") => None,
            Some(text) => {
                Some(PadSize::parse(text).ok_or_else(|| format!("unknown pad size {text:?}"))?)
            }
        };
        Ok(Mode::Near {
            system,
            service,
            radius_ly: self
                .radius_ly
                .unwrap_or(DEFAULT_NEAR_RADIUS_LY)
                .clamp(1.0, MAX_NEAR_RADIUS_LY),
            min_pad,
        })
    }
}

/// The station columns, in order; timestamps as text and hours so no
/// sqlx time feature is needed.
/// Every column the station queries select, aliased to the field it
/// fills on [`StationRow`]. Names, not positions: a query may append
/// its own columns (the near search appends `distance_ly`) and adding
/// one here can no longer shift another out from under a reader.
/// Position-indexed reads against this list caused a production outage
/// on 2026-09-15 — `/v1/stations` 502'd on every request after three
/// columns were added and the appended distance moved from 14 to 17.
const COLS: &str = "st.id AS id, st.name AS name, sy.name AS system_name, \
                    st.station_type AS station_type, st.arrival_ls AS arrival_ls, \
                    st.pad_small AS pad_small, st.pad_medium AS pad_medium, st.pad_large AS pad_large, \
                    st.has_market AS has_market, st.has_outfitting AS has_outfitting, \
                    st.has_shipyard AS has_shipyard, COALESCE(st.is_carrier, false) AS is_carrier, \
                    st.identity_observed_at::text AS identity_observed_at, \
                    EXTRACT(EPOCH FROM now() - st.identity_observed_at)::DOUBLE PRECISION / 3600.0 AS age_hours, \
                    st.primary_economy AS primary_economy, st.government AS government, \
                    st.controlling_faction AS controlling_faction";

/// One station row, mapped by column name. `distance_ly` is only
/// selected by the near search, so it defaults to `None` everywhere
/// else rather than forcing every query to select it.
struct StationRow {
    id: i64,
    name: Option<String>,
    system_name: Option<String>,
    station_type: Option<String>,
    arrival_ls: Option<f64>,
    pad_small: Option<i32>,
    pad_medium: Option<i32>,
    pad_large: Option<i32>,
    has_market: bool,
    has_outfitting: bool,
    has_shipyard: bool,
    is_carrier: bool,
    identity_observed_at: Option<String>,
    age_hours: Option<f64>,
    primary_economy: Option<String>,
    government: Option<String>,
    controlling_faction: Option<String>,
    distance_ly: Option<f64>,
}

/// Mapped by hand rather than by `#[derive(FromRow)]`: the derive needs
/// sqlx's `macros` feature, and this crate builds sqlx with default
/// features off on purpose. The guarantee is the same — every column is
/// fetched by its own name, so no reader can be shifted by a column
/// added elsewhere.
impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for StationRow {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(StationRow {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            system_name: row.try_get("system_name")?,
            station_type: row.try_get("station_type")?,
            arrival_ls: row.try_get("arrival_ls")?,
            pad_small: row.try_get("pad_small")?,
            pad_medium: row.try_get("pad_medium")?,
            pad_large: row.try_get("pad_large")?,
            has_market: row.try_get("has_market")?,
            has_outfitting: row.try_get("has_outfitting")?,
            has_shipyard: row.try_get("has_shipyard")?,
            is_carrier: row.try_get("is_carrier")?,
            identity_observed_at: row.try_get("identity_observed_at")?,
            age_hours: row.try_get("age_hours")?,
            primary_economy: row.try_get("primary_economy")?,
            government: row.try_get("government")?,
            controlling_faction: row.try_get("controlling_faction")?,
            // Only the near search selects it; absent elsewhere.
            distance_ly: row.try_get("distance_ly").unwrap_or(None),
        })
    }
}

fn station_json(row: &StationRow) -> Value {
    let kind = row.station_type.as_deref();
    json!({
        "id": row.id,
        "name": row.name,
        "system_name": row.system_name,
        "kind": kind,
        "class": StationClass::of(kind),
        "distance_to_arrival": row.arrival_ls,
        "primary_economy": row.primary_economy,
        "government": row.government,
        "controlling_faction": row.controlling_faction,
        "max_pad": PadSize::from_counts(
            row.pad_large.map(i64::from),
            row.pad_medium.map(i64::from),
            row.pad_small.map(i64::from),
        ),
        "has_market": row.has_market,
        "has_outfitting": row.has_outfitting,
        "has_shipyard": row.has_shipyard,
        "is_carrier": row.is_carrier,
        "updated": row.identity_observed_at,
        "age_hours": row.age_hours,
        "distance_ly": row.distance_ly,
    })
}

fn minor(class: StationClass) -> bool {
    matches!(
        class,
        StationClass::Settlement | StationClass::ConstructionDepot | StationClass::Other
    )
}

/// Stations in one system, the local `stations_in_system_filtered`
/// order: class rank, then name.
pub async fn in_system(
    pool: &PgPool,
    system: &str,
    q: &StationsQuery,
) -> anyhow::Result<Vec<Value>> {
    let rows = sqlx::query_as::<_, StationRow>(&format!(
        "SELECT {COLS} FROM stations st JOIN systems sy ON sy.address = st.system_address \
         WHERE lower(sy.name) = lower($1) ORDER BY st.name"
    ))
    .bind(system)
    .fetch_all(pool)
    .await?;
    let mut out: Vec<(StationClass, Value)> = rows
        .iter()
        .map(station_json)
        .filter(|v| q.include_carriers || v["is_carrier"] != true)
        .map(|v| (StationClass::of(v["kind"].as_str()), v))
        .filter(|(class, _)| q.include_minor || !minor(*class))
        .collect();
    out.sort_by(|a, b| {
        a.0.rank()
            .cmp(&b.0.rank())
            .then_with(|| a.1["name"].as_str().cmp(&b.1["name"].as_str()))
    });
    Ok(out.into_iter().map(|(_, v)| v).take(q.limit()).collect())
}

/// Stations in each of several systems, one call: rows keep the order
/// the names were given (a route's hop order), then class rank, then
/// name. A name the server does not know simply contributes no rows —
/// the caller counts docks per `system_name` and treats absence as
/// "no dock known", which is what the fuel marks want.
pub async fn in_systems(
    pool: &PgPool,
    systems: &[String],
    q: &StationsQuery,
) -> anyhow::Result<Vec<Value>> {
    let keys: Vec<String> = systems.iter().map(|s| s.to_lowercase()).collect();
    let rows = sqlx::query_as::<_, StationRow>(&format!(
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
        .map(station_json)
        .filter(|v| q.include_carriers || v["is_carrier"] != true)
        .map(|v| (position(&v), StationClass::of(v["kind"].as_str()), v))
        .filter(|(_, class, _)| q.include_minor || !minor(*class))
        .collect();
    out.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.rank().cmp(&b.1.rank()))
            .then_with(|| a.2["name"].as_str().cmp(&b.2["name"].as_str()))
    });
    Ok(out
        .into_iter()
        .map(|(_, _, v)| v)
        .take(q.systems_limit())
        .collect())
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
    let rows = sqlx::query_as::<_, StationRow>(&format!(
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
        // distance_ly is appended AFTER `COLS`, so its index is the column
        // count -- not a literal that silently means something else the
        // next time a column is added. (2026-09-15: adding economy,
        // government and faction shifted 14 from the distance to a text
        // column, and every /v1/stations request panicked in production.)
        .map(station_json)
        .filter(|v| match min_pad {
            None => true,
            Some(required) => serde_json::from_value::<Option<PadSize>>(v["max_pad"].clone())
                .ok()
                .flatten()
                .is_some_and(|p| p.fits(required)),
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
    let rows = sqlx::query_as::<_, StationRow>(&format!(
        "SELECT {COLS} FROM stations st LEFT JOIN systems sy ON sy.address = st.system_address \
         WHERE st.name = ANY($1) ORDER BY COALESCE(st.is_carrier, false), st.name LIMIT $2"
    ))
    .bind(&names)
    .bind(q.limit() as i64)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(station_json).collect())
}

#[cfg(test)]
mod tests {
    /// Every field `StationRow` maps by name must actually be selected
    /// under that name. This replaces the column-count guard that stood
    /// while the reads were positional: a count cannot catch a rename,
    /// and the names are what the mapping now depends on. `distance_ly`
    /// is the one field a query appends rather than `COLS` selecting it.
    #[test]
    fn every_mapped_field_is_selected_under_its_own_name() {
        let cols = super::COLS;
        for field in [
            "id",
            "name",
            "system_name",
            "station_type",
            "arrival_ls",
            "pad_small",
            "pad_medium",
            "pad_large",
            "has_market",
            "has_outfitting",
            "has_shipyard",
            "is_carrier",
            "identity_observed_at",
            "age_hours",
            "primary_economy",
            "government",
            "controlling_faction",
        ] {
            assert!(
                cols.contains(&format!("AS {field}")),
                "COLS never aliases {field}"
            );
        }
        assert!(
            !cols.contains("AS distance_ly"),
            "the near search appends distance_ly itself"
        );
    }

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
        assert!(q("near=Sol&service=teleporter")
            .mode()
            .unwrap_err()
            .contains("material_trader"));
        assert!(matches!(
            q("near=Sol&min_pad=l").mode().unwrap(),
            Mode::Near {
                min_pad: Some(PadSize::Large),
                ..
            }
        ));
        assert!(q("near=Sol&min_pad=huge").mode().is_err());
    }

    /// `systems=` is a comma list: trimmed, empties dropped, duplicates
    /// folded case-blind with the first spelling and order kept (a
    /// route's hop order); bounded; exclusive with the other modes.
    #[test]
    fn a_system_list_is_one_mode() {
        let q = |s: &str| serde_urlencoded::from_str::<StationsQuery>(s).unwrap();
        let Mode::InSystems(names) = q("systems=Sol,%20Deciat%20,,sol,Wongi").mode().unwrap()
        else {
            panic!("systems= is the list mode");
        };
        assert_eq!(names, vec!["Sol", "Deciat", "Wongi"]);
        assert!(q("systems=,,").mode().is_err());
        assert!(q("systems=Sol&system=Sol").mode().is_err());
        let many: Vec<String> = (0..=MAX_SYSTEMS).map(|i| format!("S{i}")).collect();
        assert!(q(&format!("systems={}", many.join(",")))
            .mode()
            .unwrap_err()
            .contains("at most"));
        assert_eq!(
            q("systems=Sol").systems_limit(),
            MAX_SYSTEMS_ROWS,
            "the whole list by default"
        );
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
        assert_eq!(
            StationsQuery {
                limit: Some(9_000),
                ..Default::default()
            }
            .limit(),
            MAX_LIMIT
        );
        assert_eq!(
            StationsQuery {
                limit: Some(0),
                ..Default::default()
            }
            .limit(),
            1
        );
        assert_eq!(StationsQuery::default().limit(), DEFAULT_LIMIT);
    }
}
