//! The mining search, server side (B.4, maintainer 2026-09-09: "Local search
//! should be limited to journal data"). Three honesty levels, the same
//! shape the Mining page has always rendered: ring HOTSPOTS for a
//! material (exact, from the dumps), nearest RINGS of the type that
//! carries a laser-mined good with no hotspot mechanic, and could-have
//! BODIES — landable, ranked by surface concentration. The commander's
//! own marks stay on the client; this module never sees them.
//!
//! Rows come from the dump hydration (`hydration::spansh`), which walks
//! every body and hands the prospecting-relevant ones to [`body_row`];
//! [`apply_bodies`] writes them newer-wins inside the batch transaction.
//! Astronomy only: nothing here names or places a commander.

use ed_store::galaxy::spansh;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::market_search::Refusal;

/// The Mining page's radius clamp.
const MAX_RADIUS_LY: f64 = 500.0;
const DEFAULT_RADIUS_LY: f64 = 100.0;
const DEFAULT_LIMIT: usize = 40;
const MAX_LIMIT: usize = 200;

// ---------------------------------------------------------------- rows

/// A body worth a row, with its children, as the dump describes it —
/// `ed_domain::BodyTeaching`, the same record the EDDN feed's `Scan`
/// arm produces (2026-09-09), so one writer (`ed_store::postgres::
/// apply_bodies`) serves both.
pub type BodyRow = ed_domain::BodyTeaching;
pub use ed_store::postgres::{apply_bodies, BodiesWritten};

/// Bodies worth a row: landable planets (surface materials, signals) and
/// anything with rings (hotspots). Stars and bare gas giants are `None`
/// — the same filter the local importer applied, so the tables track
/// prospecting, not the body census.
pub fn body_row(
    system_id64: i64,
    body: &spansh::Body,
    system_updated: Option<i64>,
    provenance: &str,
) -> Option<BodyRow> {
    let id64 = body.id64?;
    if system_id64 <= 0 {
        return None;
    }
    let has_materials = body.materials.as_ref().is_some_and(|m| !m.is_empty());
    let has_rings = body.rings.iter().any(|r| r.name.is_some());
    let signal = |key: &str| -> Option<i32> {
        body.signals
            .as_ref()
            .and_then(|s| s.signals.get(key).copied())
            .and_then(|n| i32::try_from(n).ok())
    };
    let bio = signal("$SAA_SignalType_Biological;");
    let geo = signal("$SAA_SignalType_Geological;");
    if !(body.is_landable || has_materials || has_rings || bio.is_some() || geo.is_some()) {
        return None;
    }
    let observed_epoch = body
        .update_time
        .as_deref()
        .and_then(ed_domain::freshness::parse_timestamp)
        .or(system_updated)
        .unwrap_or(0);
    let mut materials: Vec<(String, f64)> = body
        .materials
        .iter()
        .flatten()
        .map(|(m, pct)| (m.clone(), *pct))
        .collect();
    materials.sort_by(|a, b| a.0.cmp(&b.0));
    let mut rings = Vec::new();
    let mut hotspots = Vec::new();
    for ring in &body.rings {
        let Some(name) = ring.name.clone() else {
            continue;
        };
        if let Some(signals) = &ring.signals {
            for (material, count) in &signals.signals {
                hotspots.push((
                    name.clone(),
                    material.clone(),
                    i32::try_from(*count).unwrap_or(i32::MAX),
                ));
            }
        }
        rings.push(ed_domain::RingTeaching {
            name,
            kind: ring.kind.clone(),
            mass: ring.mass,
            inner_radius: ring.inner_radius,
            outer_radius: ring.outer_radius,
        });
    }
    hotspots.sort();
    Some(BodyRow {
        id64,
        system_address: system_id64,
        body_id: body.body_id.and_then(|b| i32::try_from(b).ok()),
        name: body.name.clone(),
        kind: body.kind.clone(),
        sub_type: body.sub_type.clone(),
        is_landable: body.is_landable,
        distance_to_arrival: body.distance_to_arrival,
        gravity: body.gravity,
        atmosphere: body.atmosphere.clone(),
        volcanism: body.volcanism.clone(),
        bio_signals: bio,
        geo_signals: geo,
        observed_at: ed_domain::ObservedAt::new(
            ed_store::session::iso_from_epoch(observed_epoch),
            observed_epoch,
        ),
        provenance: provenance.to_owned(),
        materials,
        rings,
        hotspots,
    })
}

// -------------------------------------------------------------- search

#[derive(Debug, Clone, Deserialize)]
pub struct MiningSearchRequest {
    /// The material or good as typed; resolved against the stored
    /// vocabulary case- and space-blind (`Low Temperature Diamonds` finds
    /// `LowTemperatureDiamond`), then against the laser-mined table.
    pub text: String,
    /// Origin by name, or by the commander's own journal coordinates —
    /// coordinates win when both are given (the fresh-install case:
    /// the server may not know the system by name yet).
    pub system: Option<String>,
    pub coords: Option<[f64; 3]>,
    pub radius_ly: Option<f64>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HotspotHit {
    pub system: String,
    pub body: Option<String>,
    pub ring: String,
    pub ring_type: Option<String>,
    pub material: String,
    pub count: i32,
    pub distance_ly: f64,
    pub distance_to_arrival: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RingSite {
    pub system: String,
    pub body: Option<String>,
    pub ring: String,
    pub ring_type: String,
    pub distance_ly: f64,
    pub distance_to_arrival: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BodyCandidate {
    pub system: String,
    pub body: Option<String>,
    pub sub_type: Option<String>,
    pub material: String,
    pub percent: f64,
    pub distance_ly: f64,
    pub distance_to_arrival: Option<f64>,
    pub gravity: Option<f64>,
    pub bio_signals: Option<i32>,
    pub geo_signals: Option<i32>,
}

/// One stored spelling of a material and which table holds it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MaterialEntry {
    pub stored: String,
    /// `"hotspot"` or `"surface"`.
    pub kind: String,
}

/// The laser-mined goods the ring-type hint knows, for the client's
/// autocomplete; kept in step with `ed_store::mining::ring_type_for`.
pub const LASER_GOODS: [&str; 15] = [
    "Gold",
    "Silver",
    "Palladium",
    "Osmium",
    "Bertrandite",
    "Indite",
    "Gallite",
    "Praseodymium",
    "Samarium",
    "Bauxite",
    "Cobalt",
    "Rutile",
    "Water",
    "Liquid Oxygen",
    "Lithium Hydroxide",
];

/// The stored vocabulary: distinct hotspot materials and surface
/// materials. A loose index scan (recursive CTE) walks each material
/// index once per distinct value — tens of probes, not a scan of every
/// row — so this is cheap enough to answer live.
pub async fn vocabulary(pool: &PgPool) -> Result<Vec<MaterialEntry>, Refusal> {
    let mut out = Vec::new();
    for (table, kind) in [("ring_hotspots", "hotspot"), ("body_materials", "surface")] {
        let names: Vec<(String,)> = sqlx::query_as(&format!(
            "WITH RECURSIVE walk AS ( \
                 (SELECT material FROM {table} ORDER BY material LIMIT 1) \
                 UNION ALL \
                 SELECT (SELECT material FROM {table} WHERE material > walk.material \
                         ORDER BY material LIMIT 1) \
                 FROM walk WHERE walk.material IS NOT NULL) \
             SELECT material FROM walk WHERE material IS NOT NULL ORDER BY material"
        ))
        .fetch_all(pool)
        .await
        .map_err(|e| Refusal::Invalid(e.to_string()))?;
        out.extend(names.into_iter().map(|(stored,)| MaterialEntry {
            stored,
            kind: kind.into(),
        }));
    }
    Ok(out)
}

/// The stored spellings a typed name resolves to: `(hotspot, surface)`.
pub fn resolve_material<'v>(
    vocab: &'v [MaterialEntry],
    text: &str,
) -> (Option<&'v str>, Option<&'v str>) {
    let key = ed_store::mining::material_key(text);
    let (mut hotspot, mut surface) = (None, None);
    for entry in vocab {
        if ed_store::mining::material_key(&entry.stored) == key {
            match entry.kind.as_str() {
                "hotspot" => hotspot = Some(entry.stored.as_str()),
                _ => surface = Some(entry.stored.as_str()),
            }
        }
    }
    (hotspot, surface)
}

/// The search: resolves the origin and the material, then runs the three
/// lists. `serde_json::Value` because the answer is exactly the object
/// the Mining page renders (its marks are added client-side).
pub async fn search(
    pool: &PgPool,
    req: &MiningSearchRequest,
) -> Result<serde_json::Value, Refusal> {
    let started = std::time::Instant::now();
    let text = req.text.trim();
    if text.is_empty() {
        return Err(Refusal::Invalid("text is required".into()));
    }
    let (origin, origin_name) = match (req.coords, req.system.as_deref().map(str::trim)) {
        (Some([x, y, z]), name) => ((x, y, z), name.unwrap_or("").to_owned()),
        (None, Some(name)) if !name.is_empty() => (
            crate::market_search::origin_coords(pool, name).await?,
            name.to_owned(),
        ),
        _ => return Err(Refusal::Invalid("system or coords is required".into())),
    };
    let radius = req
        .radius_ly
        .unwrap_or(DEFAULT_RADIUS_LY)
        .clamp(1.0, MAX_RADIUS_LY);
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let vocab = vocabulary(pool).await?;
    let (hotspot_name, surface_name) = resolve_material(&vocab, text);
    let ring_hint = ed_store::mining::ring_type_for(text);

    let hotspots = match hotspot_name {
        Some(material) => hotspots_near(pool, origin, material, radius, limit).await?,
        None => Vec::new(),
    };
    let rings = match ring_hint {
        Some((ring_type, _)) if hotspot_name.is_none() => {
            rings_near(pool, origin, ring_type, radius, limit).await?
        }
        _ => Vec::new(),
    };
    let bodies = match surface_name {
        Some(material) => body_candidates(pool, origin, material, radius, limit).await?,
        None => Vec::new(),
    };
    let ms = started.elapsed().as_millis() as u64;
    tracing::info!(
        radius,
        limit,
        hotspots = hotspots.len(),
        rings = rings.len(),
        bodies = bodies.len(),
        known_hotspot = hotspot_name.is_some(),
        known_surface = surface_name.is_some(),
        ms,
        "mining search"
    );
    Ok(serde_json::json!({
        "origin": origin_name,
        "hotspots": hotspots,
        "rings": rings,
        "bodies": bodies,
        "known_hotspot": hotspot_name.is_some(),
        "known_surface": surface_name.is_some(),
        "ring_hint": ring_hint.map(|(t, why)| serde_json::json!({"type": t, "why": why})),
        "data_installed": true,
        "radius_ly": radius,
        "ms": ms,
    }))
}

/// The sphere predicate every list shares: the system's grid cell (so
/// the planner counts, migration 0015), the box, then the exact sphere.
const SPHERE: &str = "sy.cell = ANY($5) \
    AND sy.x BETWEEN $2-$4 AND $2+$4 AND sy.y BETWEEN $3-$4 AND $3+$4 \
    AND sy.z BETWEEN $6-$4 AND $6+$4 \
    AND (sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$6)^2 <= $4*$4";

/// Ring hotspots for a material inside the sphere, nearest first, then
/// densest. Exact material match: the vocabulary supplied the spelling.
async fn hotspots_near(
    pool: &PgPool,
    (ox, oy, oz): (f64, f64, f64),
    material: &str,
    radius: f64,
    limit: usize,
) -> Result<Vec<HotspotHit>, Refusal> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        Option<String>,
        String,
        Option<String>,
        String,
        i32,
        f64,
        Option<f64>,
    )> = sqlx::query_as(&format!(
        "SELECT sy.name AS system, b.name AS body, h.ring_name AS ring, r.type AS ring_type, \
                h.material, h.count, \
                sqrt((sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$6)^2) AS distance_ly, \
                b.distance_to_arrival \
         FROM ring_hotspots h \
         JOIN bodies b ON b.id64 = h.body_id64 \
         JOIN systems sy ON sy.address = b.system_address \
         LEFT JOIN rings r ON r.body_id64 = h.body_id64 AND r.name = h.ring_name \
         WHERE h.material = $1 AND {SPHERE} \
         ORDER BY distance_ly ASC, h.count DESC \
         LIMIT $7"
    ))
    .bind(material)
    .bind(ox)
    .bind(oy)
    .bind(radius)
    .bind(crate::geo::cells_covering(ox, oy, oz, radius))
    .bind(oz)
    .bind(limit as i64)
    .persistent(false)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(
            |(system, body, ring, ring_type, material, count, distance_ly, distance_to_arrival)| {
                HotspotHit {
                    system,
                    body,
                    ring,
                    ring_type,
                    material,
                    count,
                    distance_ly,
                    distance_to_arrival,
                }
            },
        )
        .collect())
}

/// Nearest rings of a TYPE — the honest answer for laser-mined goods
/// with no hotspot mechanic (field case 2026-09-06: "Gold within
/// 500 ly, nothing" — gold comes from any Metallic ring).
async fn rings_near(
    pool: &PgPool,
    (ox, oy, oz): (f64, f64, f64),
    ring_type: &str,
    radius: f64,
    limit: usize,
) -> Result<Vec<RingSite>, Refusal> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, Option<String>, String, String, f64, Option<f64>)> =
        sqlx::query_as(&format!(
            "SELECT sy.name AS system, b.name AS body, r.name AS ring, r.type AS ring_type, \
                sqrt((sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$6)^2) AS distance_ly, \
                b.distance_to_arrival \
         FROM rings r \
         JOIN bodies b ON b.id64 = r.body_id64 \
         JOIN systems sy ON sy.address = b.system_address \
         WHERE r.type = $1 AND {SPHERE} \
         ORDER BY distance_ly ASC \
         LIMIT $7"
        ))
        .bind(ring_type)
        .bind(ox)
        .bind(oy)
        .bind(radius)
        .bind(crate::geo::cells_covering(ox, oy, oz, radius))
        .bind(oz)
        .bind(limit as i64)
        .persistent(false)
        .fetch_all(pool)
        .await
        .map_err(|e| Refusal::Invalid(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(
            |(system, body, ring, ring_type, distance_ly, distance_to_arrival)| RingSite {
                system,
                body,
                ring,
                ring_type,
                distance_ly,
                distance_to_arrival,
            },
        )
        .collect())
}

/// Landable bodies carrying the material, richest concentration first
/// with distance as the tiebreak — the "could have it" list for surface
/// prospecting (raw-materials namespace; commodity yields are not
/// derivable from composition, per the 2026-09-06 field session).
async fn body_candidates(
    pool: &PgPool,
    (ox, oy, oz): (f64, f64, f64),
    material: &str,
    radius: f64,
    limit: usize,
) -> Result<Vec<BodyCandidate>, Refusal> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        String,
        f64,
        f64,
        Option<f64>,
        Option<f64>,
        Option<i32>,
        Option<i32>,
    )> = sqlx::query_as(&format!(
        "SELECT sy.name AS system, b.name AS body, b.sub_type, m.material, m.percent, \
                sqrt((sy.x-$2)^2 + (sy.y-$3)^2 + (sy.z-$6)^2) AS distance_ly, \
                b.distance_to_arrival, b.gravity, b.bio_signals, b.geo_signals \
         FROM body_materials m \
         JOIN bodies b ON b.id64 = m.body_id64 \
         JOIN systems sy ON sy.address = b.system_address \
         WHERE m.material = $1 AND b.is_landable AND {SPHERE} \
         ORDER BY m.percent DESC, distance_ly ASC \
         LIMIT $7"
    ))
    .bind(material)
    .bind(ox)
    .bind(oy)
    .bind(radius)
    .bind(crate::geo::cells_covering(ox, oy, oz, radius))
    .bind(oz)
    .bind(limit as i64)
    .persistent(false)
    .fetch_all(pool)
    .await
    .map_err(|e| Refusal::Invalid(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(
            |(
                system,
                body,
                sub_type,
                material,
                percent,
                distance_ly,
                distance_to_arrival,
                gravity,
                bio_signals,
                geo_signals,
            )| {
                BodyCandidate {
                    system,
                    body,
                    sub_type,
                    material,
                    percent,
                    distance_ly,
                    distance_to_arrival,
                    gravity,
                    bio_signals,
                    geo_signals,
                }
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(json: serde_json::Value) -> spansh::Body {
        serde_json::from_value(json).unwrap()
    }

    /// The old importer's filter, kept: a ringed gas giant with hotspots
    /// and a landable rock with materials get rows; a bare star does not.
    /// Hotspots keep the wire spelling; bio/geo come from the SAA keys.
    #[test]
    fn prospecting_bodies_become_rows_and_bare_stars_do_not() {
        let giant = body(serde_json::json!({
            "id64": 100, "bodyId": 3, "name": "Deciat 6", "type": "Planet", "subType": "Class II gas giant",
            "updateTime": "2026-09-01 10:00:00",
            "rings": [{"name": "Deciat 6 A Ring", "type": "Metallic", "mass": 1.5e12,
                       "innerRadius": 1.0e8, "outerRadius": 2.0e8,
                       "signals": {"signals": {"Platinum": 2, "Painite": 1}, "updateTime": "2026-09-01 10:00:00"}}]
        }));
        let row = body_row(1, &giant, Some(5), "spansh:test").expect("ringed giant is a row");
        assert_eq!(row.rings.len(), 1);
        assert_eq!(row.rings[0].kind.as_deref(), Some("Metallic"));
        let mut hotspots: Vec<(&str, i32)> = row
            .hotspots
            .iter()
            .map(|(_, m, c)| (m.as_str(), *c))
            .collect();
        hotspots.sort();
        assert_eq!(hotspots, vec![("Painite", 1), ("Platinum", 2)]);
        assert_eq!(
            row.observed_at.epoch_seconds,
            ed_domain::freshness::parse_timestamp("2026-09-01 10:00:00").unwrap(),
            "the body's own updateTime wins over the system date"
        );
        assert!(!row.is_landable);

        let rock = body(serde_json::json!({
            "id64": 101, "bodyId": 4, "name": "Deciat 6 a", "type": "Planet", "subType": "Rocky body",
            "isLandable": true, "gravity": 0.12, "distanceToArrival": 812.5,
            "materials": {"Iron": 21.3, "Nickel": 16.1},
            "signals": {"signals": {"$SAA_SignalType_Biological;": 2, "$SAA_SignalType_Geological;": 5}}
        }));
        let row = body_row(1, &rock, Some(5), "spansh:test").expect("landable rock is a row");
        assert!(row.is_landable);
        assert_eq!(
            row.materials,
            vec![("Iron".to_string(), 21.3), ("Nickel".to_string(), 16.1)]
        );
        assert_eq!((row.bio_signals, row.geo_signals), (Some(2), Some(5)));
        assert_eq!(
            row.observed_at.epoch_seconds, 5,
            "no updateTime: the system's date"
        );

        let star = body(serde_json::json!({
            "id64": 102, "bodyId": 0, "name": "Deciat", "type": "Star", "subType": "K (Yellow-Orange) Star",
            "mainStar": true
        }));
        assert!(
            body_row(1, &star, Some(5), "spansh:test").is_none(),
            "a bare star is the stars table's, not ours"
        );
        assert!(
            body_row(0, &rock, Some(5), "spansh:test").is_none(),
            "no system address, no row"
        );
    }

    /// The panel's autocomplete and the search agree on spelling: typed
    /// names resolve case- and space-blind to the stored wire symbol for
    /// hotspots and the display name for surface materials.
    #[test]
    fn typed_names_resolve_to_stored_spellings() {
        let vocab = vec![
            MaterialEntry {
                stored: "LowTemperatureDiamond".into(),
                kind: "hotspot".into(),
            },
            MaterialEntry {
                stored: "Platinum".into(),
                kind: "hotspot".into(),
            },
            MaterialEntry {
                stored: "Iron".into(),
                kind: "surface".into(),
            },
        ];
        assert_eq!(
            resolve_material(&vocab, "low temperature diamonds"),
            (Some("LowTemperatureDiamond"), None)
        );
        assert_eq!(
            resolve_material(&vocab, "PLATINUM"),
            (Some("Platinum"), None)
        );
        assert_eq!(resolve_material(&vocab, "iron"), (None, Some("Iron")));
        assert_eq!(
            resolve_material(&vocab, "gold"),
            (None, None),
            "laser goods are the ring hint's"
        );
        assert_eq!(
            ed_store::mining::ring_type_for("gold").map(|(t, _)| t),
            Some("Metallic")
        );
    }
}
