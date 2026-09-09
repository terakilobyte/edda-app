//! Ship physics from a `Loadout` — the journal event, or the same event
//! wrapped in SLEF (the Ship Loadout Event Format EDSY and Coriolis
//! export: `[{"header": {...}, "data": {<Loadout>}}]`). One derivation
//! for the desktop app's own journal and the web router's paste box
//! (maintainer, 2026-09-09: "site path, SLEF only, go"), so both plot with
//! the same fuel model.
//!
//! What it reads: `UnladenMass`, `FuelCapacity.Main`, `MaxJumpRange`
//! (the game's own figure, engineering included — the model's optimal
//! mass is derived from it), the FSD item (size, rating, SCO, Mk II),
//! its engineered `MaxFuelPerJump` when present, and a Guardian FSD
//! booster's size. `CargoCapacity` is reported, not planned with: the
//! caller says what is aboard.

use serde::Serialize;
use serde_json::Value;

use crate::fuel::{base_max_fuel, guardian_booster_ly, BoostProfile, FuelModel, MK2_SCO_MAX_FUEL};

/// What a `Loadout` says about the ship's drive, as the planner needs it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LoadoutPhysics {
    /// Journal hull symbol, lowercase (`cutter`, `panther_mk2`).
    pub ship: String,
    pub ship_name: Option<String>,
    pub model: FuelModel,
    pub boost: BoostProfile,
    pub fsd_size: u8,
    pub fsd_rating: char,
    pub sco: bool,
    pub mk2: bool,
    /// Guardian FSD booster, ly (0 if none).
    pub booster_ly: f32,
    pub cargo_capacity: f32,
    /// The Loadout's own `MaxJumpRange`, for the label.
    pub max_jump_range: f32,
}

impl LoadoutPhysics {
    /// One line for a label: hull, drive, cap, tank, booster.
    pub fn summary(&self) -> String {
        format!(
            "size {}{}{}{} · cap {:.1} t/jump · tank {:.0} t{}",
            self.fsd_size,
            self.fsd_rating,
            if self.sco { " SCO" } else { "" },
            if self.mk2 { " Mk II" } else { "" },
            self.model.max_fuel_per_jump,
            self.model.capacity,
            if self.booster_ly > 0.0 { format!(" · booster +{} ly", self.booster_ly) } else { String::new() },
        )
    }
}

/// Why a paste is not a loadout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadoutError {
    NotJson(String),
    NotALoadout,
    Missing(&'static str),
    NoDrive,
}

impl std::fmt::Display for LoadoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadoutError::NotJson(e) => write!(f, "not JSON: {e}"),
            LoadoutError::NotALoadout => {
                write!(f, "not a Loadout: expected a journal Loadout event or a SLEF export ([{{\"header\", \"data\"}}])")
            }
            LoadoutError::Missing(what) => write!(f, "the Loadout has no {what}"),
            LoadoutError::NoDrive => write!(f, "no frame shift drive in the Loadout's modules"),
        }
    }
}

impl std::error::Error for LoadoutError {}

/// The `Loadout` object inside a paste: a SLEF array's first `data`
/// whose event is `Loadout`, or a bare `Loadout` object. Case-blind on
/// the event name; a SLEF with several builds takes the first.
pub fn loadout_from_paste(text: &str) -> Result<Value, LoadoutError> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|e| LoadoutError::NotJson(e.to_string()))?;
    let is_loadout = |v: &Value| {
        v.get("event").and_then(Value::as_str).is_some_and(|e| e.eq_ignore_ascii_case("loadout"))
            || (v.get("Modules").is_some() && v.get("Ship").is_some())
    };
    match &value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.get("data"))
            .find(|data| is_loadout(data))
            .cloned()
            .ok_or(LoadoutError::NotALoadout),
        Value::Object(_) if is_loadout(&value) => Ok(value),
        Value::Object(_) => value
            .get("data")
            .filter(|data| is_loadout(data))
            .cloned()
            .ok_or(LoadoutError::NotALoadout),
        _ => Err(LoadoutError::NotALoadout),
    }
}

/// The physics of a `Loadout`, with `cargo` tonnes aboard. `observed_cap`
/// is the most fuel this hull has been seen burning in one jump (the
/// desktop's first-hand figure); it can only raise the drive cap.
pub fn physics_from_loadout(v: &Value, cargo: f32, observed_cap: Option<f32>) -> Result<LoadoutPhysics, LoadoutError> {
    let f = |k: &str| v.get(k).and_then(Value::as_f64).map(|x| x as f32);
    let unladen = f("UnladenMass").ok_or(LoadoutError::Missing("UnladenMass"))?;
    let capacity = v
        .pointer("/FuelCapacity/Main")
        .and_then(Value::as_f64)
        .map(|x| x as f32)
        .ok_or(LoadoutError::Missing("FuelCapacity.Main"))?;
    let max_range = f("MaxJumpRange").ok_or(LoadoutError::Missing("MaxJumpRange"))?;
    let ship = v.get("Ship").and_then(Value::as_str).unwrap_or("").trim().to_ascii_lowercase();
    let ship_name = v.get("ShipName").and_then(Value::as_str).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let modules = v.get("Modules").and_then(Value::as_array);
    let mut fsd_item = String::new();
    let mut fsd_cap_mod: Option<f32> = None;
    let mut booster = 0.0f32;
    if let Some(ms) = modules {
        for m in ms {
            let item = m.get("Item").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
            if item.contains("int_hyperdrive") {
                fsd_item = item.clone();
                if let Some(mods) = m.pointer("/Engineering/Modifiers").and_then(Value::as_array) {
                    for md in mods {
                        if md.get("Label").and_then(Value::as_str).is_some_and(|l| l.eq_ignore_ascii_case("MaxFuelPerJump")) {
                            fsd_cap_mod = md.get("Value").and_then(Value::as_f64).map(|x| x as f32);
                        }
                    }
                }
            } else if item.contains("guardianfsdbooster") {
                let size = item.split("size").nth(1).and_then(|s| s.chars().next()).and_then(|c| c.to_digit(10)).unwrap_or(0) as u8;
                booster = guardian_booster_ly(size);
            }
        }
    }
    if fsd_item.is_empty() {
        return Err(LoadoutError::NoDrive);
    }
    let size = fsd_item.split("size").nth(1).and_then(|s| s.chars().next()).and_then(|c| c.to_digit(10)).unwrap_or(5) as u8;
    let rating = fsd_item
        .split("class")
        .nth(1)
        .and_then(|s| s.chars().next())
        .and_then(|c| c.to_digit(10))
        .map(|d| match d {
            5 => 'A',
            4 => 'B',
            3 => 'C',
            2 => 'D',
            _ => 'E',
        })
        .unwrap_or('A');
    let sco = fsd_item.contains("overcharge");
    let mk2 = fsd_item.contains("mkii");
    // Fuel cap: engineered value if present, else the base table (+4 % for
    // SCO), and never below the most this ship has actually burned in one
    // jump -- first-hand beats the table.
    let table = if mk2 { Some(MK2_SCO_MAX_FUEL) } else { base_max_fuel(size, rating).map(|b| if sco { b * 1.04 } else { b }) };
    let cap = [fsd_cap_mod, observed_cap, table].into_iter().flatten().fold(0.0f32, f32::max);
    if cap <= 0.0 {
        return Err(LoadoutError::NoDrive);
    }
    let model = FuelModel::from_loadout(unladen, capacity, cap, size, sco, mk2, max_range, booster, cargo);
    let boost = if mk2 { BoostProfile::MK2_SCO } else { BoostProfile::default() };
    Ok(LoadoutPhysics {
        ship,
        ship_name,
        model,
        boost,
        fsd_size: size,
        fsd_rating: rating,
        sco,
        mk2,
        booster_ly: booster,
        cargo_capacity: f("CargoCapacity").unwrap_or(0.0),
        max_jump_range: max_range,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An EDSY-style SLEF export: a Cutter with an engineered 7A, a
    /// size-5 Guardian booster and a scoop.
    const SLEF: &str = r#"[{"header":{"appName":"EDSY","appVersion":"4.0"},"data":{"event":"Loadout","Ship":"Cutter","ShipName":"Treasure Goblin","UnladenMass":1163.6,"CargoCapacity":720,"MaxJumpRange":25.83,"FuelCapacity":{"Main":32,"Reserve":1.16},"Modules":[{"Slot":"FrameShiftDrive","Item":"Int_Hyperdrive_Size7_Class5","On":true,"Engineering":{"BlueprintName":"FSD_LongRange","Level":5,"Modifiers":[{"Label":"FSDOptimalMass","Value":2902.4,"OriginalValue":1800},{"Label":"MaxFuelPerJump","Value":12.8,"OriginalValue":12.8}]}},{"Slot":"Slot01_Size6","Item":"Int_GuardianFSDBooster_Size5","On":true},{"Slot":"Slot02_Size6","Item":"Int_FuelScoop_Size6_Class5","On":true}]}}]"#;

    #[test]
    fn a_slef_paste_becomes_the_same_physics_the_journal_would() {
        let loadout = loadout_from_paste(SLEF).unwrap();
        let p = physics_from_loadout(&loadout, 0.0, None).unwrap();
        assert_eq!((p.ship.as_str(), p.ship_name.as_deref()), ("cutter", Some("Treasure Goblin")));
        assert_eq!((p.fsd_size, p.fsd_rating, p.sco, p.mk2), (7, 'A', false, false));
        assert_eq!(p.booster_ly, 10.5);
        assert_eq!(p.model.max_fuel_per_jump, 12.8);
        assert_eq!(p.model.capacity, 32.0);
        assert_eq!(p.cargo_capacity, 720.0);
        // The model reproduces the Loadout's own range at one jump's fuel
        // aboard (that is how optimal mass is derived from it).
        let one_jump = p.model.range_at(p.model.max_fuel_per_jump);
        assert!((one_jump - 25.83).abs() < 0.05, "{one_jump}");
        assert!(p.model.range_at(32.0) < one_jump, "a full tank is heavier");
        assert_eq!(p.boost, BoostProfile::default());
        assert!(p.summary().contains("size 7A") && p.summary().contains("booster +10.5 ly"), "{}", p.summary());

        // The bare event and a single SLEF object are the same paste.
        let bare = serde_json::to_string(&loadout).unwrap();
        assert_eq!(physics_from_loadout(&loadout_from_paste(&bare).unwrap(), 0.0, None).unwrap(), p);
        let single = format!(r#"{{"header":{{}},"data":{}}}"#, bare);
        assert_eq!(physics_from_loadout(&loadout_from_paste(&single).unwrap(), 0.0, None).unwrap(), p);
    }

    #[test]
    fn a_paste_that_is_not_a_loadout_says_so() {
        assert!(matches!(loadout_from_paste("hello"), Err(LoadoutError::NotJson(_))));
        assert_eq!(loadout_from_paste(r#"{"event":"Docked"}"#), Err(LoadoutError::NotALoadout));
        assert_eq!(loadout_from_paste(r#"[{"header":{},"data":{"event":"Shipyard"}}]"#), Err(LoadoutError::NotALoadout));
        let no_drive = serde_json::json!({"event":"Loadout","Ship":"sidewinder","UnladenMass":25.0,"MaxJumpRange":7.5,"FuelCapacity":{"Main":2},"Modules":[]});
        assert_eq!(physics_from_loadout(&no_drive, 0.0, None), Err(LoadoutError::NoDrive));
        let no_mass = serde_json::json!({"event":"Loadout","Ship":"sidewinder","Modules":[{"Item":"int_hyperdrive_size2_class1"}]});
        assert_eq!(physics_from_loadout(&no_mass, 0.0, None), Err(LoadoutError::Missing("UnladenMass")));
    }

    /// The Mk II SCO drive takes its own cap and the six-times neutron
    /// profile; a stock drive without an engineered cap takes the table,
    /// and an observed burn above the table raises it.
    #[test]
    fn drive_variants_pick_their_cap_and_profile() {
        let mk2 = serde_json::json!({"event":"Loadout","Ship":"panther_mk2","UnladenMass":1200.0,"MaxJumpRange":30.0,"FuelCapacity":{"Main":64},
            "Modules":[{"Item":"int_hyperdrive_overcharge_mkii_size7_class5"}]});
        let p = physics_from_loadout(&mk2, 0.0, None).unwrap();
        assert!(p.mk2 && p.sco);
        assert_eq!(p.model.max_fuel_per_jump, MK2_SCO_MAX_FUEL);
        assert_eq!(p.boost, BoostProfile::MK2_SCO);
        let stock = serde_json::json!({"event":"Loadout","Ship":"asp","UnladenMass":280.0,"MaxJumpRange":38.0,"FuelCapacity":{"Main":32},
            "Modules":[{"Item":"int_hyperdrive_size5_class5"}]});
        assert_eq!(physics_from_loadout(&stock, 0.0, None).unwrap().model.max_fuel_per_jump, 5.0);
        assert_eq!(physics_from_loadout(&stock, 0.0, Some(5.4)).unwrap().model.max_fuel_per_jump, 5.4);
        assert_eq!(physics_from_loadout(&stock, 100.0, None).unwrap().model.cargo, 100.0);
    }
}
