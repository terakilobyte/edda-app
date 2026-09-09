//! From "what the commander asked" to "what the finder searches".
//!
//! The profit finder takes a fully specified [`Constraints`] and a
//! [`Ship`]. Nobody types those: a request arrives with most fields blank,
//! and every blank is filled from the live journal (the current hull, its
//! cargo racks, the Powerplay pledge) or from a default. That filling-in is
//! policy -- clamps, the meaning of `"mine"` and `"any"`, and the rule that
//! an unrecognised hull **refuses** rather than searching every outpost --
//! and policy belongs here, tested, not in whichever layer happened to
//! receive the request. The Tauri command and the ship computer's tool both
//! call [`plan`] and therefore cannot drift apart.

use crate::cost::Ship;
use crate::profit::Constraints;
use ed_store::lookup::PadSize;
use serde::Deserialize;
use serde_json::Value;

/// A profit search as asked for. Everything optional falls back to the
/// commander's live state: current system, current ship, docked station.
/// Field names are the wire format for both the UI and the model.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ProfitRequest {
    pub system: Option<String>,
    /// Restrict purchases to the station the commander is docked at.
    pub from_current_station: Option<bool>,
    pub from_station_id: Option<i64>,
    pub radius_ly: Option<f64>,
    pub max_age_hours: Option<f64>,
    pub include_carriers: Option<bool>,
    pub include_prohibited: Option<bool>,
    /// Plan for a stored ship from the journal (its `ShipID`) instead of
    /// the one being flown (maintainer, 2026-09-07: the trade tab lets the
    /// commander pick a ship, or type pad and tonnes freeform). Explicit
    /// `cargo_capacity` / `jump_range_ly` / `min_pad` still override.
    pub ship_id: Option<i64>,
    pub cargo_capacity: Option<i64>,
    pub jump_range_ly: Option<f64>,
    /// Override the pad filter: a size, or `"any"` for no filter. Default
    /// derives it from the current hull, and an unknown hull is an error.
    pub min_pad: Option<String>,
    pub limit: Option<usize>,
    pub max_stations: Option<usize>,
    pub max_arrival_ls: Option<f64>,
    pub max_stops: Option<usize>,
    /// Powerplay filters; `"mine"` resolves to the pledged power.
    pub buy_power: Option<String>,
    pub buy_state: Option<String>,
    pub sell_power: Option<String>,
    pub sell_state: Option<String>,
    pub buy_power_mode: Option<String>,
    pub sell_power_mode: Option<String>,
    pub max_leg_ly: Option<f64>,
    /// Floor on the SOURCE board's supply (maintainer, 2026-09-05: a 3-ton
    /// seller headlining a 1,008-ton route is noise) and on the
    /// destination's demand.
    pub min_supply: Option<i64>,
    pub min_demand: Option<i64>,
    /// `auto` (the data-source choice decides), `local`, or `community`
    /// — the latter is the commander's consent to ask the API this once.
    pub source: Option<String>,
}

impl ProfitRequest {
    /// Default number of results when the request does not say.
    pub const DEFAULT_LIMIT: usize = 20;

    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(Self::DEFAULT_LIMIT)
    }
}

/// What the latest `Loadout` says about the ship, as the finder needs it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadoutShip {
    /// Journal hull symbol, lowercase (`cutter`, `smallcombat01_nx`).
    pub hull: Option<String>,
    pub cargo_capacity: Option<i64>,
    pub max_jump_range: Option<f64>,
    pub unladen_mass: Option<f64>,
    /// Main tank capacity in tons.
    pub fuel_main: Option<f64>,
}

impl LoadoutShip {
    /// Read the fields the finder uses from a raw `Loadout` event.
    pub fn from_loadout(v: &Value) -> Self {
        LoadoutShip {
            hull: v
                .get("Ship")
                .and_then(Value::as_str)
                .map(|s| s.trim().to_ascii_lowercase())
                .filter(|s| !s.is_empty()),
            cargo_capacity: v.get("CargoCapacity").and_then(Value::as_i64),
            max_jump_range: v.get("MaxJumpRange").and_then(Value::as_f64),
            unladen_mass: v.get("UnladenMass").and_then(Value::as_f64),
            fuel_main: v.pointer("/FuelCapacity/Main").and_then(Value::as_f64),
        }
    }
}

/// Why a request cannot become a search. Each carries a [`hint`] -- what
/// the caller can change -- kept apart from the message so a UI can show
/// one and a model can act on the other.
///
/// [`hint`]: ProfitRequestError::hint
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProfitRequestError {
    #[error("the current ship has no cargo capacity (0 t); trading needs cargo racks")]
    NoCargoRacks,
    #[error("no cargo capacity known")]
    CargoUnknown,
    #[error("not pledged to a power")]
    NotPledged,
    #[error("landing pad size unknown for hull {hull:?}")]
    PadUnknown { hull: String },
    #[error("no ship known, so the landing pad filter cannot be derived")]
    NoShip,
    #[error("{0:?} is not a pad size")]
    BadPad(String),
}

impl ProfitRequestError {
    /// What to change. Written for whoever retries -- commander or model.
    pub fn hint(&self) -> &'static str {
        match self {
            ProfitRequestError::NoCargoRacks => {
                "say so, or pass cargo_capacity to plan for a different ship"
            }
            ProfitRequestError::CargoUnknown => {
                "pass cargo_capacity, or load a ship in game so a Loadout is written"
            }
            ProfitRequestError::NotPledged => {
                "use a power's name instead of 'mine', or drop the power filter"
            }
            ProfitRequestError::PadUnknown { .. } | ProfitRequestError::NoShip => {
                "pass min_pad (small, medium or large), or 'any' to search every station knowing outposts may not fit"
            }
            ProfitRequestError::BadPad(_) => "min_pad is small, medium, large or any",
        }
    }
}

/// A request resolved against the live ship and pledge: ready to search.
#[derive(Debug, Clone)]
pub struct ProfitPlan {
    pub ship: Ship,
    pub constraints: Constraints,
}

/// Resolve the ship's pad requirement: the request's explicit choice wins;
/// otherwise the hull decides, and a hull the table does not know is an
/// error -- never a search with the filter silently off.
pub fn pad_requirement(
    explicit: Option<&str>,
    hull: Option<&str>,
) -> Result<Option<PadSize>, ProfitRequestError> {
    match explicit.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) if p.eq_ignore_ascii_case("any") => Ok(None),
        Some(p) => PadSize::parse(p)
            .map(Some)
            .ok_or_else(|| ProfitRequestError::BadPad(p.to_string())),
        None => match hull.map(str::trim).filter(|s| !s.is_empty()) {
            None => Err(ProfitRequestError::NoShip),
            Some(h) => crate::ships::pad_for_ship(h)
                .map(Some)
                .ok_or_else(|| ProfitRequestError::PadUnknown { hull: h.to_string() }),
        },
    }
}

/// Fill every blank in `req` from `live` and the pledge, clamp what needs
/// clamping, and refuse what cannot be searched honestly.
pub fn plan(
    req: &ProfitRequest,
    live: &LoadoutShip,
    pledged: Option<&str>,
) -> Result<ProfitPlan, ProfitRequestError> {
    let cargo_capacity = match req.cargo_capacity.or(live.cargo_capacity) {
        Some(c) if c > 0 => c,
        Some(_) => return Err(ProfitRequestError::NoCargoRacks),
        None => return Err(ProfitRequestError::CargoUnknown),
    };
    let jump_range_ly = req
        .jump_range_ly
        .or(live.max_jump_range)
        .unwrap_or(Ship::default().jump_range_ly);
    let ship = Ship {
        cargo_capacity,
        jump_range_ly,
        laden_range_ly: Ship::laden_range(
            jump_range_ly,
            live.unladen_mass,
            live.fuel_main,
            cargo_capacity,
        ),
    };

    let mut c = Constraints::default();
    if let Some(r) = req.radius_ly {
        c.radius_ly = r.max(1.0);
    }
    if let Some(h) = req.max_age_hours {
        c.max_age_hours = h.max(1.0);
    }
    c.include_carriers = req.include_carriers.unwrap_or(false);
    c.include_prohibited = req.include_prohibited.unwrap_or(false);
    c.min_pad = pad_requirement(req.min_pad.as_deref(), live.hull.as_deref())?;
    // 0 means no cap: the search runs off the UI thread and the whole
    // populated galaxy is a few tens of seconds, so nothing is hidden.
    c.max_stations = match req.max_stations {
        Some(0) | None => usize::MAX,
        Some(n) => n.max(100),
    };
    if let Some(ls) = req.max_arrival_ls.filter(|v| *v > 0.0) {
        c.max_arrival_ls = ls;
    }
    if let Some(n) = req.max_stops {
        c.max_stops = n.min(12);
    }
    let resolve = |p: &Option<String>| -> Result<Option<String>, ProfitRequestError> {
        match p.as_deref().map(str::trim) {
            None | Some("") | Some("any") => Ok(None),
            Some("mine") => pledged
                .map(|s| Some(s.to_string()))
                .ok_or(ProfitRequestError::NotPledged),
            Some(other) => Ok(Some(other.to_string())),
        }
    };
    let norm = |s: &Option<String>| {
        s.as_deref()
            .map(str::trim)
            .filter(|x| !x.is_empty() && *x != "any")
            .map(str::to_string)
    };
    c.buy_power = resolve(&req.buy_power)?;
    c.sell_power = resolve(&req.sell_power)?;
    c.buy_state = norm(&req.buy_state);
    c.sell_state = norm(&req.sell_state);
    if let Some(m) = &req.buy_power_mode {
        c.buy_power_mode = m.clone();
    }
    if let Some(m) = &req.sell_power_mode {
        c.sell_power_mode = m.clone();
    }
    if let Some(l) = req.max_leg_ly {
        c.max_leg_ly = l.max(0.0);
    }
    if let Some(s) = req.min_supply {
        c.min_supply = s.max(1);
    }
    if let Some(d) = req.min_demand {
        c.min_demand = d.max(1);
    }
    Ok(ProfitPlan { ship, constraints: c })
}

/// EDDN rows carry only the symbol; resolve every leg's commodity to its
/// in-game name so no table ever shows "hazardousenvironmentsuits".
pub fn resolve_commodity_names(report: &mut crate::profit::ProfitReport, catalog: &ed_journal::Catalog) {
    let name_of = |leg: &mut crate::profit::Leg| {
        if leg.commodity.eq_ignore_ascii_case(&leg.symbol) {
            if let Some(item) = catalog.by_symbol(&leg.symbol) {
                leg.commodity = item.name.clone();
            }
        }
        for x in leg.extra.iter_mut() {
            if x.commodity.eq_ignore_ascii_case(&x.symbol) {
                if let Some(item) = catalog.by_symbol(&x.symbol) {
                    x.commodity = item.name.clone();
                }
            }
        }
    };
    report.legs.iter_mut().for_each(name_of);
    for t in report.round_trips.iter_mut() {
        name_of(&mut t.out);
        name_of(&mut t.back);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_store::lookup::PadSize;
    use serde_json::json;

    fn live() -> LoadoutShip {
        LoadoutShip {
            hull: Some("cutter".into()),
            cargo_capacity: Some(700),
            max_jump_range: Some(30.0),
            unladen_mass: Some(1100.0),
            fuel_main: Some(64.0),
        }
    }

    #[test]
    fn ship_is_derived_from_a_loadout_event() {
        let v = json!({
            "event": "Loadout", "Ship": "Cutter", "CargoCapacity": 700,
            "MaxJumpRange": 30.0, "UnladenMass": 1100.0, "FuelCapacity": { "Main": 64.0, "Reserve": 1.16 }
        });
        let ship = LoadoutShip::from_loadout(&v);
        assert_eq!(ship.hull.as_deref(), Some("cutter"));
        assert_eq!(ship.cargo_capacity, Some(700));
        assert_eq!(ship.fuel_main, Some(64.0));
        let plan = plan(&ProfitRequest::default(), &ship, None).unwrap();
        assert_eq!(plan.ship.cargo_capacity, 700);
        assert!(plan.ship.laden_range_ly < 30.0 && plan.ship.laden_range_ly > 15.0, "laden {}", plan.ship.laden_range_ly);
        assert_eq!(plan.constraints.min_pad, Some(PadSize::Large));
    }

    #[test]
    fn defaults_and_clamps_live_here_not_in_the_command() {
        let req: ProfitRequest = serde_json::from_value(json!({
            "radius_ly": 0.1, "max_age_hours": 0.0, "max_stations": 5, "max_stops": 40, "max_leg_ly": -3.0
        }))
        .unwrap();
        let c = plan(&req, &live(), None).unwrap().constraints;
        assert_eq!(c.radius_ly, 1.0);
        assert_eq!(c.max_age_hours, 1.0);
        assert_eq!(c.max_stations, 100, "a station cap below 100 is raised to it");
        assert_eq!(c.max_stops, 12);
        assert_eq!(c.max_leg_ly, 0.0);
        let c = plan(&ProfitRequest::default(), &live(), None).unwrap().constraints;
        assert_eq!(c.max_stations, usize::MAX, "no cap by default");
        assert_eq!(c.max_age_hours, crate::profit::Constraints::default().max_age_hours);
    }

    #[test]
    fn mine_resolves_to_the_pledge_and_any_means_no_filter() {
        let req = ProfitRequest { buy_power: Some("mine".into()), sell_power: Some("any".into()), sell_state: Some(" Fortified ".into()), ..Default::default() };
        let c = plan(&req, &live(), Some("Li Yong-Rui")).unwrap().constraints;
        assert_eq!(c.buy_power.as_deref(), Some("Li Yong-Rui"));
        assert_eq!(c.sell_power, None);
        assert_eq!(c.sell_state.as_deref(), Some("Fortified"));
        assert_eq!(plan(&req, &live(), None).unwrap_err(), ProfitRequestError::NotPledged);
    }

    #[test]
    fn no_cargo_is_an_error_that_names_the_fix() {
        let zero = LoadoutShip { cargo_capacity: Some(0), ..live() };
        assert_eq!(plan(&ProfitRequest::default(), &zero, None).unwrap_err(), ProfitRequestError::NoCargoRacks);
        let unknown = LoadoutShip { cargo_capacity: None, ..live() };
        assert_eq!(plan(&ProfitRequest::default(), &unknown, None).unwrap_err(), ProfitRequestError::CargoUnknown);
        let req = ProfitRequest { cargo_capacity: Some(64), ..Default::default() };
        assert_eq!(plan(&req, &unknown, None).unwrap().ship.cargo_capacity, 64);
        assert!(!ProfitRequestError::CargoUnknown.hint().is_empty());
    }

    #[test]
    fn unknown_hull_refuses_rather_than_disabling_the_pad_filter() {
        let odd = LoadoutShip { hull: Some("fdev_next_hull".into()), ..live() };
        assert_eq!(
            plan(&ProfitRequest::default(), &odd, None).unwrap_err(),
            ProfitRequestError::PadUnknown { hull: "fdev_next_hull".into() }
        );
        // An explicit pad, or an explicit "any", is the commander's call.
        let req = ProfitRequest { min_pad: Some("large".into()), ..Default::default() };
        assert_eq!(plan(&req, &odd, None).unwrap().constraints.min_pad, Some(PadSize::Large));
        let req = ProfitRequest { min_pad: Some("any".into()), ..Default::default() };
        assert_eq!(plan(&req, &odd, None).unwrap().constraints.min_pad, None);
        let req = ProfitRequest { min_pad: Some("huge".into()), ..Default::default() };
        assert_eq!(plan(&req, &odd, None).unwrap_err(), ProfitRequestError::BadPad("huge".into()));
    }
}
