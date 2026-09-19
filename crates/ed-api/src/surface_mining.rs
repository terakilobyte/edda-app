//! Where to surface-mine a Rhino good.
//!
//! Two facts, from two places. The COUNT of mining locations on a body
//! comes from the game: a DSS scan writes `SAASignalsFound` with
//! `{"Type":"$PlanetaryMiningLocation_Name;","Count":N}` and no
//! commodity, and EDDN carries it; the feed (#91) and the Spansh dump
//! write it to `bodies.mining_locations`. WHAT a location yields is not in
//! any event a server receives (`MiningRefined` is not on EDDN's journal
//! allowlist), so it is a property of the body's GROUND — class, and for
//! Rocky bodies the volcanism — surveyed by the community from their own
//! refinery logs: the EDIntel prospecting guide, republished as a sheet by
//! Fumlop/EDRhinoSpotter (`mining_sheet.json`), bundled here with its
//! generation date and shown as what it is: the share of that ground's
//! surveyed locations that carried the good.
//!
//! Measured before building (docs/benches/2026-09-19-eddn-body-signals-rate.csv):
//! ~39k body-signal rows a day reach the feed; the column fills from scans
//! arriving after 2026-09-19 plus the dump backfill.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::market_search::Refusal;

/// The bundled survey, verbatim from RhinoSpotter's `mining_sheet.json`.
const SURVEY_JSON: &str = include_str!("../data/surface_mining_grounds.json");

#[derive(Debug, Deserialize)]
struct SurveyFile {
    generated: String,
    source: String,
    /// Surveyed locations per ground.
    locations: BTreeMap<String, u32>,
    /// Per ground: the goods its locations carried, with the share.
    grounds: BTreeMap<String, Vec<SurveyRow>>,
}

#[derive(Debug, Clone, Deserialize)]
struct SurveyRow {
    material: String,
    pct: f64,
    #[allow(dead_code)]
    median: Option<i64>,
    #[allow(dead_code)]
    best: Option<i64>,
}

/// What the page shows about the survey itself.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SurveyAbout {
    pub source: String,
    pub generated: String,
    pub locations_surveyed: u32,
}

fn survey() -> &'static SurveyFile {
    static SURVEY: OnceLock<SurveyFile> = OnceLock::new();
    SURVEY.get_or_init(|| serde_json::from_str(SURVEY_JSON).expect("bundled surface_mining_grounds.json parses"))
}

pub fn survey_about() -> SurveyAbout {
    let s = survey();
    SurveyAbout {
        source: s.source.clone(),
        generated: s.generated.clone(),
        locations_surveyed: s.locations.values().sum(),
    }
}

/// The survey's ground for a body: its class, and for Rocky bodies the
/// volcanism bucket (the sheet splits Rocky on it; magma on a metal-rich
/// or high-metal body counts under that class). None when the survey has
/// no column for this ground -- the body is still listed, with no share.
pub fn ground_of(sub_type: Option<&str>, volcanism: Option<&str>) -> Option<&'static str> {
    let class = sub_type.unwrap_or("").to_ascii_lowercase();
    let volc = volcanism.unwrap_or("").to_ascii_lowercase();
    if class.contains("rocky ice") {
        return Some("rocky-ice");
    }
    if class.contains("high metal content") {
        return Some("high-metal-content");
    }
    if class.contains("metal rich") || class.contains("metal-rich") {
        return Some("metal-rich");
    }
    if class.contains("icy") {
        return Some("icy");
    }
    if class.contains("rocky") {
        return Some(if volc.contains("metallic magma") {
            "rock 80%+ [metallic magma]"
        } else if volc.contains("rocky magma") {
            "rock 80%+ [rocky magma]"
        } else if volc.contains("silicate vapour geysers") {
            "rock 80%+ [silicate vapour geysers]"
        } else if volc.is_empty() || volc.contains("no volcanism") {
            "rock 80%+ [none]"
        } else {
            return None; // a volcanism the survey has no column for
        });
    }
    None
}

/// The survey's share (0-100) of `ground`'s locations that carried `good`,
/// or None when the ground was never surveyed for it.
pub fn share_of(ground: &str, good: &str) -> Option<f64> {
    let key = ed_store::mining::material_key(good);
    survey()
        .grounds
        .get(ground)?
        .iter()
        .find(|r| ed_store::mining::material_key(&r.material) == key)
        .map(|r| r.pct)
}

/// One body with mining locations, as the page lists it.
#[derive(Debug, Clone, Serialize)]
pub struct SurfaceSite {
    pub system: String,
    pub body: Option<String>,
    pub sub_type: Option<String>,
    pub volcanism: Option<String>,
    /// The survey's ground this body falls under, if it has one.
    pub ground: Option<String>,
    /// Share of the ground's surveyed locations that carried the good.
    pub share_pct: Option<f64>,
    pub mining_locations: i32,
    pub is_landable: bool,
    pub gravity: Option<f64>,
    pub distance_ly: f64,
    pub distance_to_arrival: Option<f64>,
}

/// Bodies with mining locations within the sphere, nearest first, each
/// with the survey's share for `good`. Bodies whose ground the survey
/// says never carried the good are dropped; bodies whose ground is not in
/// the survey at all are kept with no share -- the count is real, the
/// yield unknown.
pub async fn sites_near(
    pool: &PgPool,
    (ox, oy, oz): (f64, f64, f64),
    good: &str,
    radius: f64,
    limit: usize,
) -> Result<Vec<SurfaceSite>, Refusal> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, Option<String>, Option<String>, Option<String>, i32, bool, Option<f64>, f64, Option<f64>)> = sqlx::query_as(&format!(
        "SELECT sy.name AS system, b.name AS body, b.sub_type, b.volcanism, b.mining_locations, b.is_landable, b.gravity, \
                sqrt((sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$6)^2) AS distance_ly, b.distance_to_arrival \
         FROM bodies b \
         JOIN systems sy ON sy.address = b.system_address \
         WHERE b.mining_locations > 0 AND $1::text IS NOT NULL AND {sphere} \
         ORDER BY distance_ly ASC, b.mining_locations DESC \
         LIMIT $7",
        sphere = crate::mining::SPHERE
    ))
    // $1 is the good: not a filter (the survey filter runs in Rust) but referenced,
    // so the bind numbering matches SPHERE and Postgres can type the parameter.
    .bind(good)
    .bind(ox)
    .bind(oy)
    .bind(radius)
    .bind(crate::geo::cells_covering(ox, oy, oz, radius))
    .bind(oz)
    .bind((limit * 4) as i64) // over-fetch: the survey filter below drops some
    .persistent(false)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;

    let mut out: Vec<SurfaceSite> = rows
        .into_iter()
        .filter_map(|(system, body, sub_type, volcanism, mining_locations, is_landable, gravity, distance_ly, distance_to_arrival)| {
            let ground = ground_of(sub_type.as_deref(), volcanism.as_deref());
            let share_pct = ground.and_then(|g| share_of(g, good));
            // Surveyed ground that never carried the good: not a place to go.
            if ground.is_some() && share_pct.is_none() {
                return None;
            }
            Some(SurfaceSite {
                system,
                body,
                sub_type,
                volcanism,
                ground: ground.map(str::to_string),
                share_pct,
                mining_locations,
                is_landable,
                gravity,
                distance_ly,
                distance_to_arrival,
            })
        })
        .collect();
    out.truncate(limit);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_survey_parses_and_says_where_it_came_from() {
        let about = survey_about();
        assert!(about.source.contains("EDIntel"), "{}", about.source);
        assert!(about.generated.starts_with("2026-"), "{}", about.generated);
        assert!(about.locations_surveyed >= 900, "{}", about.locations_surveyed);
        assert_eq!(survey().grounds.len(), 8, "eight grounds");
    }

    /// The maintainer's ruling (2026-09-19): uranium is surface. The survey
    /// puts it on high-metal-content and metal-rich ground, nowhere icy.
    #[test]
    fn uranium_is_on_metal_grounds_and_not_on_ice() {
        assert_eq!(share_of("high-metal-content", "Uranium").map(|p| (p * 10.0).round() / 10.0), Some(38.9));
        assert_eq!(share_of("metal-rich", "uranium").map(|p| (p * 10.0).round() / 10.0), Some(22.8));
        assert_eq!(share_of("icy", "Uranium"), None);
        // The body the maintainer scanned (HIP 100449 6 a, Icy, 7 locations) carries water, not uranium.
        assert!(share_of("icy", "Water").unwrap() > 99.0);
    }

    /// Ground follows the sheet's own rule: class first; Rocky splits on
    /// volcanism; magma on a metal body counts under the metal class.
    #[test]
    fn ground_follows_class_then_volcanism() {
        assert_eq!(ground_of(Some("Icy body"), None), Some("icy"));
        assert_eq!(ground_of(Some("Rocky Ice world"), None), Some("rocky-ice"));
        assert_eq!(ground_of(Some("High metal content world"), Some("major metallic magma volcanism")), Some("high-metal-content"));
        assert_eq!(ground_of(Some("Metal rich body"), Some("minor rocky magma volcanism")), Some("metal-rich"));
        assert_eq!(ground_of(Some("Rocky body"), None), Some("rock 80%+ [none]"));
        assert_eq!(ground_of(Some("Rocky body"), Some("No volcanism")), Some("rock 80%+ [none]"));
        assert_eq!(ground_of(Some("Rocky body"), Some("minor metallic magma volcanism")), Some("rock 80%+ [metallic magma]"));
        assert_eq!(ground_of(Some("Rocky body"), Some("major silicate vapour geysers volcanism")), Some("rock 80%+ [silicate vapour geysers]"));
        assert_eq!(ground_of(Some("Rocky body"), Some("minor water magma volcanism")), None, "a volcanism the survey has no column for");
        assert_eq!(ground_of(Some("Gas giant with water based life"), None), None);
        assert_eq!(ground_of(None, None), None);
    }
}
