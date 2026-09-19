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
    /// A loadout was found but lacks the numbers the physics needs. Every
    /// missing one is named, because "no UnladenMass" sent a commander
    /// hunting for a single field when their whole export lacked three
    /// (maintainer, 2026-09-15: a Coriolis SLEF paste — Coriolis writes
    /// only `Ship` and `Modules`; EDSY's export carries the rest).
    Missing(Vec<&'static str>),
    NoDrive,
}

/// The three numbers a paste must carry for the physics to be derived,
/// in the order the message names them.
pub const REQUIRED: [&str; 3] = ["UnladenMass", "FuelCapacity.Main", "MaxJumpRange"];

/// Which app wrote a SLEF paste, from its header — `"EDSY"`, `"Coriolis"`
/// — or None for a bare journal `Loadout` or anything without a header.
pub fn paste_app_name(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    if is_coriolis_json(&value) {
        return Some(CORIOLIS_JSON.to_owned());
    }
    let header = match &value {
        Value::Array(items) => items.iter().find_map(|i| i.get("header")),
        Value::Object(_) => value.get("header"),
        _ => None,
    }?;
    header.get("appName").and_then(Value::as_str).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

impl std::fmt::Display for LoadoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadoutError::NotJson(e) => write!(f, "not JSON: {e}"),
            LoadoutError::NotALoadout => {
                write!(f, "not a Loadout: expected a journal Loadout event or a SLEF export ([{{\"header\", \"data\"}}])")
            }
            LoadoutError::Missing(keys) => {
                let list = match keys.as_slice() {
                    [] => String::from("the numbers the physics needs"),
                    [one] => (*one).to_string(),
                    [head @ .., last] => format!("{} or {last}", head.join(", ")),
                };
                write!(f, "the loadout has no {list}")
            }
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
        Value::Object(_) if is_coriolis_json(&value) => loadout_from_coriolis_json(&value),
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

/// What `paste_app_name` says for Coriolis's own JSON export (the
/// `ship-loadout` schema), as opposed to Coriolis's SLEF, which it names
/// "Coriolis" from the header and which carries no numbers.
pub const CORIOLIS_JSON: &str = "Coriolis JSON";

/// A Coriolis JSON export: the `ship-loadout` schema, or the shape of one
/// (`components` and `stats` objects) when the `$schema` line was cut.
fn is_coriolis_json(v: &Value) -> bool {
    v.get("$schema").and_then(Value::as_str).is_some_and(|s| s.contains("coriolis.io/schemas/ship-loadout"))
        || (v.get("components").is_some_and(Value::is_object) && v.get("stats").is_some_and(Value::is_object))
}

/// A Coriolis JSON export reshaped into the journal `Loadout` the physics
/// reads (maintainer, 2026-09-19: "coriolis json, edsy slef, or edda
/// slef"). Coriolis's `stats` carry the numbers on its own definitions,
/// checked against the maintainer's Caspian Explorer export and
/// Coriolis's `Ship.js`:
///
/// * `dryMass` is hull + modules with no fuel — the journal's
///   `UnladenMass` (`unladenMass` in Coriolis INCLUDES a full tank).
/// * `fuelCapacity` is the main tank; `reserveFuelCapacity` the reserve.
/// * `maxRange` is the range with one jump's fuel and no cargo — the
///   journal's `MaxJumpRange`. Fed through `FuelModel::from_loadout`, the
///   full-tank range comes back within 0.01 ly of Coriolis's own
///   `fullTankRange` (72.13 vs 72.14 on the Explorer).
///
/// The drive and booster are synthesised as journal item names from the
/// FSD's class, rating and name ("Mk II", "SCO") and the Guardian
/// booster's class, since that is what `physics_from_loadout` keys on.
fn loadout_from_coriolis_json(v: &Value) -> Result<Value, LoadoutError> {
    let stats = v.get("stats").and_then(Value::as_object);
    let stat = |k: &str| stats.and_then(|s| s.get(k)).and_then(Value::as_f64);
    let (dry, tank, max_range) = (stat("dryMass"), stat("fuelCapacity"), stat("maxRange"));
    let missing: Vec<&'static str> = [dry.is_none(), tank.is_none(), max_range.is_none()]
        .iter()
        .zip(REQUIRED)
        .filter_map(|(absent, key)| absent.then_some(key))
        .collect();
    if !missing.is_empty() {
        return Err(LoadoutError::Missing(missing));
    }
    let standard = v.pointer("/components/standard");
    let fsd = standard.and_then(|s| s.get("frameShiftDrive")).ok_or(LoadoutError::NoDrive)?;
    let class = fsd.get("class").and_then(Value::as_u64).ok_or(LoadoutError::NoDrive)?;
    let rating = fsd.get("rating").and_then(Value::as_str).and_then(|r| r.chars().next()).unwrap_or('A');
    let rating_class = match rating.to_ascii_uppercase() {
        'A' => 5,
        'B' => 4,
        'C' => 3,
        'D' => 2,
        _ => 1,
    };
    let fsd_name = fsd.get("name").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    let sco = fsd_name.contains("sco") || fsd_name.contains("overcharge");
    let mk2 = fsd_name.contains("mk ii") || fsd_name.contains("mkii") || fsd_name.contains("mk2");
    let fsd_item = format!(
        "Int_Hyperdrive{}_Size{class}_Class{rating_class}{}",
        if sco { "_Overcharge" } else { "" },
        if mk2 { "_Overchargebooster_MkII" } else { "" }
    );
    let mut modules = vec![serde_json::json!({"Slot": "FrameShiftDrive", "Item": fsd_item, "On": true})];
    if let Some(internal) = v.pointer("/components/internal").and_then(Value::as_array) {
        for m in internal.iter().filter(|m| !m.is_null()) {
            let group = m.get("group").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
            let on = m.get("enabled").and_then(Value::as_bool).unwrap_or(true);
            if group.contains("guardian") && group.contains("booster") && on {
                if let Some(size) = m.get("class").and_then(Value::as_u64) {
                    modules.push(serde_json::json!({"Slot": "Internal", "Item": format!("Int_GuardianFSDBooster_Size{size}"), "On": true}));
                }
            }
        }
    }
    let ship = v
        .pointer("/references/0/shipId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| v.get("ship").and_then(Value::as_str).map(|s| s.to_ascii_lowercase().replace(' ', "_")))
        .unwrap_or_default();
    let mut out = serde_json::json!({
        "event": "Loadout",
        "Ship": ship,
        "UnladenMass": dry,
        "FuelCapacity": {"Main": tank, "Reserve": stat("reserveFuelCapacity").unwrap_or(0.0)},
        "MaxJumpRange": max_range,
        "CargoCapacity": stat("cargoCapacity").unwrap_or(0.0),
        "Modules": modules,
    });
    if let Some(name) = v.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) {
        out["ShipName"] = Value::String(name.to_owned());
    }
    Ok(out)
}

/// The physics of a `Loadout`, with `cargo` tonnes aboard. `observed_cap`
/// is the most fuel this hull has been seen burning in one jump (the
/// desktop's first-hand figure); it can only raise the drive cap.
pub fn physics_from_loadout(v: &Value, cargo: f32, observed_cap: Option<f32>) -> Result<LoadoutPhysics, LoadoutError> {
    let f = |k: &str| v.get(k).and_then(Value::as_f64).map(|x| x as f32);
    let unladen = f("UnladenMass");
    let capacity = v.pointer("/FuelCapacity/Main").and_then(Value::as_f64).map(|x| x as f32);
    let max_range = f("MaxJumpRange");
    let missing: Vec<&'static str> = [unladen.is_none(), capacity.is_none(), max_range.is_none()]
        .iter()
        .zip(REQUIRED)
        .filter_map(|(absent, key)| absent.then_some(key))
        .collect();
    if !missing.is_empty() {
        return Err(LoadoutError::Missing(missing));
    }
    let (unladen, capacity, max_range) = (unladen.unwrap_or(0.0), capacity.unwrap_or(0.0), max_range.unwrap_or(0.0));
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
    /// What Coriolis actually exports (2026-09-15, a Caspian Explorer): a
    /// header naming the app, then `Ship` and `Modules` — no mass, no tank,
    /// no range. Trimmed to the modules that matter.
    const CORIOLIS: &str = r#"[{"header":{"appName":"Coriolis","appVersion":"4.0","appURL":"https://coriolis.io/outfit/explorer_nx?code=x"},"data":{"Ship":"Explorer_NX","Modules":[{"Slot":"FrameShiftDrive","Item":"Int_Hyperdrive_Overcharge_Size8_Class5_Overchargebooster_MkII","On":true,"Engineering":{"BlueprintName":"FSD_LongRange","Level":5,"Modifiers":[{"Label":"EngineOptimalMass","Value":7528.04,"OriginalValue":4670}]}},{"Slot":"FuelTank","Item":"Int_FuelTank_Size7_Class3","On":true},{"Slot":"Slot04_Size5","Item":"Int_GuardianFSDBooster_Size5","On":true}]}}]"#;

    /// A Coriolis paste IS a loadout, and the error says everything it lacks
    /// in one breath — not the first missing field alone.
    #[test]
    fn a_coriolis_export_is_a_loadout_missing_all_three_numbers() {
        let v = loadout_from_paste(CORIOLIS).expect("Ship + Modules is a loadout");
        assert_eq!(v["Ship"], "Explorer_NX");
        let err = physics_from_loadout(&v, 0.0, None).unwrap_err();
        assert_eq!(err, LoadoutError::Missing(vec!["UnladenMass", "FuelCapacity.Main", "MaxJumpRange"]));
        assert_eq!(err.to_string(), "the loadout has no UnladenMass, FuelCapacity.Main or MaxJumpRange");
        assert_eq!(paste_app_name(CORIOLIS).as_deref(), Some("Coriolis"));
        assert_eq!(paste_app_name(SLEF).as_deref(), Some("EDSY"));
        assert_eq!(paste_app_name(r#"{"event":"Loadout","Ship":"asp","Modules":[]}"#), None, "a bare journal event names no app");
    }

    /// The maintainer's Caspian Explorer, exported from Coriolis as JSON
    /// (2026-09-19). Coriolis shows it as MAX 77.75 ly, full tank 72.14 ly,
    /// 128 t tank, 1,323.3 t dry, 8A FSD Mk II (SCO), 5H Guardian booster.
    const CORIOLIS_JSON_EXPORT: &str = include_str!("../tests/fixtures/coriolis-caspian-explorer.json");

    #[test]
    fn a_coriolis_json_export_is_the_journal_loadout_and_agrees_with_coriolis_to_the_hundredth() {
        assert_eq!(paste_app_name(CORIOLIS_JSON_EXPORT).as_deref(), Some(CORIOLIS_JSON));
        let loadout = loadout_from_paste(CORIOLIS_JSON_EXPORT).expect("a Coriolis JSON export is a loadout");
        assert_eq!(loadout["Ship"], "explorer_nx");
        assert_eq!(loadout["ShipName"], "a");
        assert_eq!(loadout["UnladenMass"], 1323.3, "dryMass, not unladenMass (which includes the tank)");
        assert_eq!(loadout["FuelCapacity"]["Main"], 128.0);
        assert_eq!(loadout["FuelCapacity"]["Reserve"], 1.14);
        assert_eq!(loadout["MaxJumpRange"], 77.75);
        let p = physics_from_loadout(&loadout, 0.0, None).unwrap();
        assert_eq!((p.fsd_size, p.fsd_rating, p.sco, p.mk2), (8, 'A', true, true));
        assert_eq!(p.booster_ly, guardian_booster_ly(5));
        assert_eq!(p.model.capacity, 128.0);
        let full_tank = p.model.range_at(128.0);
        assert!((full_tank - 72.14).abs() < 0.05, "Coriolis says 72.14 ly on a full tank; we derive {full_tank}");
        let one_jump = p.model.range_at(p.model.max_fuel_per_jump);
        assert!((one_jump - 77.75).abs() < 0.05, "Coriolis says 77.75 ly MAX; we derive {one_jump}");
    }

    /// A Coriolis JSON export whose `stats` block was cut is still a
    /// Coriolis export — refused with every missing number named, not
    /// "not a loadout".
    #[test]
    fn a_coriolis_json_export_without_its_stats_names_every_missing_number() {
        let mut v: Value = serde_json::from_str(CORIOLIS_JSON_EXPORT).unwrap();
        v["stats"] = serde_json::json!({"hullMass": 950});
        let text = v.to_string();
        assert_eq!(paste_app_name(&text).as_deref(), Some(CORIOLIS_JSON));
        assert_eq!(loadout_from_paste(&text), Err(LoadoutError::Missing(REQUIRED.to_vec())));
        v["stats"] = serde_json::json!({"dryMass": 1323.3, "fuelCapacity": 128});
        assert_eq!(loadout_from_paste(&v.to_string()), Err(LoadoutError::Missing(vec!["MaxJumpRange"])));
    }

    /// A plain 5A drive with no booster synthesises the stock item name.
    #[test]
    fn a_stock_drive_in_a_coriolis_json_export_is_a_stock_hyperdrive() {
        let v = serde_json::json!({
            "$schema": "https://coriolis.io/schemas/ship-loadout/4.json#", "name": "", "ship": "Asp Explorer",
            "components": {"standard": {"frameShiftDrive": {"class": 5, "rating": "A", "enabled": true}}, "internal": [null]},
            "stats": {"dryMass": 280.0, "fuelCapacity": 32, "maxRange": 38.0}
        });
        let loadout = loadout_from_paste(&v.to_string()).unwrap();
        assert_eq!(loadout["Ship"], "asp_explorer");
        assert_eq!(loadout["Modules"][0]["Item"], "Int_Hyperdrive_Size5_Class5");
        assert_eq!(loadout["Modules"].as_array().unwrap().len(), 1);
        let p = physics_from_loadout(&loadout, 0.0, None).unwrap();
        assert_eq!((p.fsd_size, p.fsd_rating, p.sco, p.mk2, p.booster_ly), (5, 'A', false, false, 0.0));
    }

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
        // Only the mass missing: named alone. All three missing: all named.
        let no_mass = serde_json::json!({"event":"Loadout","Ship":"sidewinder","MaxJumpRange":7.5,"FuelCapacity":{"Main":2},"Modules":[{"Item":"int_hyperdrive_size2_class1"}]});
        assert_eq!(physics_from_loadout(&no_mass, 0.0, None), Err(LoadoutError::Missing(vec!["UnladenMass"])));
        let bare = serde_json::json!({"event":"Loadout","Ship":"sidewinder","Modules":[{"Item":"int_hyperdrive_size2_class1"}]});
        assert_eq!(physics_from_loadout(&bare, 0.0, None), Err(LoadoutError::Missing(vec!["UnladenMass", "FuelCapacity.Main", "MaxJumpRange"])));
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
