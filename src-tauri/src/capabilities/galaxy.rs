//! Galaxy lookups: the request shapes the panels and the ship computer
//! share, and the two facts the journal alone can answer (the current
//! system, coordinates of a visited one). The answers themselves come
//! from the community API through `remote_lookup`, `remote_search` and
//! `remote_trade` (B.4, 2026-09-09: the local store is journal-only).

use super::{CapError, CapResult};
use ed_store::lookup::PadSize;
use ed_store::query;
use rusqlite::Connection;
use serde::Deserialize;

/// Coordinates of a system the commander has visited: the StarPos of the
/// newest FSDJump / Location / CarrierJump naming it in their own journal
/// (maintainer, 2026-09-09: `no coordinates known for "Crucis Sector FW-W
/// b1-4"` while sitting IN it, from a table that was empty by design).
/// A system never visited is `NotFound` here; the API knows the rest.
pub fn origin_coords(conn: &Connection, system: &str) -> CapResult<(f64, f64, f64)> {
    if let Some([x, y, z]) = crate::routing::journal_coords(conn, system) {
        return Ok((x as f64, y as f64, z as f64));
    }
    Err(CapError::not_found(format!("no coordinates known for system {system:?}: not in your journal"))
        .hint("the community API resolves any system by name; pass the system name, not coordinates"))
}

/// Coordinates for distance arithmetic without a network call: the
/// journal (visited systems) or the bundled bubble index (populated
/// ones); `None` for anything else - a missing distance, never a wrong
/// one.
pub fn coords_hint(conn: &Connection, galaxy: Option<&ed_galaxy::Galaxy>, name: &str) -> Option<(f64, f64, f64)> {
    if let Some([x, y, z]) = crate::routing::journal_coords(conn, name) {
        return Some((x as f64, y as f64, z as f64));
    }
    let g = galaxy?;
    let [x, y, z] = g.pos_of(g.find(name)?);
    Some((x as f64, y as f64, z as f64))
}

/// The commander's current system from the journal, if known.
pub fn current_system(conn: &Connection) -> Option<String> {
    query::location(conn).ok().flatten().and_then(|l| l.system_name)
}

/// A system name from the request, else the current one, else an error.
pub fn system_or_current(conn: &Connection, requested: Option<&str>) -> CapResult<String> {
    requested
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| current_system(conn))
        .ok_or_else(|| CapError::invalid("no origin system given and the current system is unknown").hint("pass system"))
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FindSystemRequest {
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StationsInSystemRequest {
    pub system: String,
    pub include_carriers: bool,
    pub include_minor: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FindStationRequest {
    pub name: String,
}

/// How many name matches a station search returns.
pub const FIND_STATION_LIMIT: usize = 25;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NearestServiceRequest {
    /// Origin; the current system when absent.
    pub system: Option<String>,
    pub service: String,
    pub min_pad: Option<String>,
    pub radius_ly: f64,
    pub include_carriers: bool,
}

impl Default for NearestServiceRequest {
    fn default() -> Self {
        NearestServiceRequest { system: None, service: String::new(), min_pad: None, radius_ly: 50.0, include_carriers: false }
    }
}

impl NearestServiceRequest {
    /// `"Raw Material Trader"` -> `raw_material_trader`.
    pub fn service_key(&self) -> String {
        self.service.trim().to_ascii_lowercase().replace([' ', '-'], "_")
    }
    pub fn pad(&self) -> Option<PadSize> {
        self.min_pad.as_deref().and_then(PadSize::parse)
    }
}

pub const NEAREST_SERVICE_LIMIT: usize = 25;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SystemsNearRequest {
    pub system: String,
    pub radius_ly: f64,
}

impl Default for SystemsNearRequest {
    fn default() -> Self {
        SystemsNearRequest { system: String::new(), radius_ly: 20.0 }
    }
}

pub const SYSTEMS_NEAR_LIMIT: usize = 50;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct StationMarketRequest {
    pub station_id: i64,
}

/// A search of community market data near a system. Shared by the three
/// panel commands (commodity / outfitting / shipyard) and the model's
/// `market_search`, which adds `kind`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MarketSearchRequest {
    /// `commodity`, `module` or `ship` (the panel commands imply it).
    pub kind: String,
    pub text: String,
    pub system: Option<String>,
    /// Default 100, clamped to 1..=500.
    pub radius_ly: Option<f64>,
    /// A pad size, `"any"`, or absent for the current hull -- which must be
    /// known; an unknown hull is an error, never an unfiltered search.
    pub min_pad: Option<String>,
    pub include_carriers: bool,
    /// Opt-in: show prohibited-good sales where a black market exists.
    pub include_prohibited: bool,
    /// Commodities only; default 48.
    pub max_age_hours: Option<f64>,
    /// Commodities: `buy` (commander buys) or `sell`. The panel calls it `side`.
    #[serde(alias = "action")]
    pub side: String,
    /// Default 50, at most 200.
    pub limit: Option<usize>,
    /// Commodities: minimum supply (buy) or demand (sell); 0/absent = any.
    pub min_quantity: Option<i64>,
    /// Commodities: `price` (default) or `distance`. The order IS the
    /// selection under a LIMIT (item F3): a distance-sorted panel must
    /// fetch the nearest matches, not re-sort the cheapest ones.
    pub sort: Option<String>,
}

impl Default for MarketSearchRequest {
    fn default() -> Self {
        MarketSearchRequest {
            kind: "commodity".into(),
            text: String::new(),
            system: None,
            radius_ly: None,
            min_pad: None,
            include_carriers: false,
            include_prohibited: false,
            max_age_hours: None,
            side: "buy".into(),
            limit: None,
            min_quantity: None,
            sort: None,
        }
    }
}

/// Origin name and the pad the ship needs: what the journal contributes
/// to a market search. The server resolves the name to coordinates. The
/// pad fails closed through `ed_route::request`.
pub fn market_search_context(conn: &Connection, req: &MarketSearchRequest) -> CapResult<(String, Option<PadSize>)> {
    let system = system_or_current(conn, req.system.as_deref())?;
    let hull: Option<String> = conn
        .query_row("SELECT ship FROM loadout WHERE id=1", [], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten();
    let min_pad = ed_route::request::pad_requirement(req.min_pad.as_deref(), hull.as_deref())?;
    Ok((system, min_pad))
}

#[cfg(test)]
mod origin_tests {
    use super::*;

    /// The commander's own journal knows where they are and have been
    /// (maintainer, 2026-09-09: "no coordinates known" for the system he was
    /// sitting in, from a table that is gone now).
    #[test]
    fn a_visited_system_resolves_from_the_journal() {
        let dir = tempfile::tempdir().unwrap();
        let store = ed_store::Store::open_in_memory(dir.path()).unwrap();
        let jump = serde_json::json!({
            "timestamp": "2026-09-09T03:00:00Z", "event": "FSDJump",
            "StarSystem": "Crucis Sector FW-W b1-4", "SystemAddress": 1, "StarPos": [12.5, -3.25, 40.0]
        });
        store
            .conn()
            .execute(
                "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 1, '2026-09-09T03:00:00Z', 'FSDJump', ?1)",
                [jump.to_string()],
            )
            .unwrap();
        let (x, y, z) = origin_coords(store.conn(), "crucis sector fw-w b1-4").expect("journal knows it");
        assert_eq!((x, y, z), (12.5, -3.25, 40.0));
        let err = origin_coords(store.conn(), "Never Visited").unwrap_err();
        assert!(format!("{err:?}").contains("no coordinates known"), "{err:?}");
    }
}
