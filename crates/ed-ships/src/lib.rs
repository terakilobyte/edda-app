//! Ship performance from a build, the way EDSY and Coriolis compute it:
//! what a ship weighs, how far it jumps and whether its power plant
//! carries the load — for the build as flown (the journal's engineered
//! values are exact) and for a planned one (blueprint effects at a full
//! roll, from Coriolis's ranges).
//!
//! Figures come from Coriolis's vendored data (`data/coriolis`, see its
//! README and THIRD-PARTY-NOTICES) — hull masses, module masses and
//! curves, blueprint effect ranges — and from the journal `Loadout`,
//! whose `Engineering.Modifiers` carry the rolled value of every
//! engineered stat. The rule: a journal value wins over a computed one.
//!
//! Pinned against EDSY on the maintainer's own ships
//! (`tests/fixtures/edsy_*.json`); a change that moves a pin fails.
//! Phase 1 (this file): mass, jump range, power. Speed, shields, armour
//! and weapons follow, each with its pin.

mod data;
pub mod slots;

pub use slots::{Hull, ModuleKind, Slot, Slots};

use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

/// One module's figures, base or engineered, by Coriolis's field names
/// (`mass`, `power`, `optmass`, `maxfuel`, `fuelmul`, `fuelpower`, `pgen`,
/// `jumpboost`, ...).
#[derive(Debug, Clone, Serialize)]
pub struct Module {
    pub symbol: String,
    pub group: String,
    pub class: i64,
    pub rating: String,
    pub stats: HashMap<String, f64>,
}

impl Module {
    pub fn stat(&self, k: &str) -> f64 {
        self.stats.get(k).copied().unwrap_or(0.0)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Ship {
    pub name: String,
    pub hull_mass: f64,
    pub speed: f64,
    pub boost: f64,
    pub reserve_fuel: f64,
    /// Bulkhead masses by grade (index 0 = lightweight alloy).
    pub bulkhead_mass: Vec<f64>,
}

/// Blueprint effect ranges per grade: feature -> [min, max] fractions.
#[derive(Debug, Clone)]
pub struct BlueprintEffects {
    pub grades: HashMap<i64, HashMap<String, (f64, f64)>>,
}

pub struct Catalog {
    ships: HashMap<String, Ship>,
    modules: HashMap<String, Module>,
    blueprints: HashMap<String, BlueprintEffects>,
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

impl Catalog {
    pub fn load() -> Self {
        let mut ships = HashMap::new();
        for text in data::SHIPS {
            let v: Value = serde_json::from_str(text).expect("vendored ship JSON parses");
            for (_, ship) in v.as_object().into_iter().flatten() {
                let p = &ship["properties"];
                let name = p["name"].as_str().unwrap_or_default().to_string();
                let bulkhead_mass = ship["bulkheads"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|b| f(b, "mass")).collect())
                    .unwrap_or_default();
                ships.insert(
                    name.to_ascii_lowercase(),
                    Ship {
                        hull_mass: f(p, "hullMass").unwrap_or(0.0),
                        speed: f(p, "speed").unwrap_or(0.0),
                        boost: f(p, "boost").unwrap_or(0.0),
                        reserve_fuel: f(p, "reserveFuelCapacity").unwrap_or(0.0),
                        bulkhead_mass,
                        name,
                    },
                );
            }
        }
        let mut modules = HashMap::new();
        for text in data::MODULES {
            let v: Value = serde_json::from_str(text).expect("vendored module JSON parses");
            for (group, arr) in v.as_object().into_iter().flatten() {
                for m in arr.as_array().into_iter().flatten() {
                    let Some(symbol) = m["symbol"].as_str() else { continue };
                    let stats = m
                        .as_object()
                        .into_iter()
                        .flatten()
                        .filter_map(|(k, x)| x.as_f64().map(|n| (k.clone(), n)))
                        .collect();
                    modules.insert(
                        symbol.to_ascii_lowercase(),
                        Module {
                            symbol: symbol.to_string(),
                            group: group.clone(),
                            class: m["class"].as_i64().unwrap_or(0),
                            rating: m["rating"].as_str().unwrap_or("").to_string(),
                            stats,
                        },
                    );
                }
            }
        }
        let mut blueprints = HashMap::new();
        let v: Value = serde_json::from_str(data::BLUEPRINTS).expect("vendored blueprints parse");
        for (fdname, bp) in v.as_object().into_iter().flatten() {
            let mut grades = HashMap::new();
            for (g, spec) in bp["grades"].as_object().into_iter().flatten() {
                let Ok(g) = g.parse::<i64>() else { continue };
                let features = spec["features"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(k, r)| {
                        let a = r.as_array()?;
                        Some((k.clone(), (a.first()?.as_f64()?, a.get(1)?.as_f64()?)))
                    })
                    .collect();
                grades.insert(g, features);
            }
            blueprints.insert(fdname.to_ascii_lowercase(), BlueprintEffects { grades });
        }
        Catalog { ships, modules, blueprints }
    }

    /// By the journal's ship symbol (`type9_military`) or display name.
    pub fn ship(&self, symbol_or_name: &str) -> Option<&Ship> {
        let name = ed_journal::ships::display_name_or(symbol_or_name, None);
        self.ships.get(&name.to_ascii_lowercase()).or_else(|| self.ships.get(&symbol_or_name.to_ascii_lowercase()))
    }

    /// By the journal's item symbol (`int_hyperdrive_size7_class5`).
    pub fn module(&self, item: &str) -> Option<&Module> {
        self.modules.get(&item.to_ascii_lowercase())
    }

    pub fn blueprint(&self, fdname: &str) -> Option<&BlueprintEffects> {
        self.blueprints.get(&fdname.to_ascii_lowercase())
    }
}

/// The journal's modifier labels onto Coriolis's field names. Only the
/// stats phase 1 reads; a label not here is kept under its own name.
fn field_for_label(label: &str) -> &str {
    match label {
        "Mass" => "mass",
        "PowerDraw" => "power",
        "PowerCapacity" => "pgen",
        "FSDOptimalMass" | "EngineOptimalMass" | "ShieldGenOptimalMass" => "optmass",
        "MaxFuelPerJump" => "maxfuel",
        "EngineMinimumMass" => "minmass",
        "MaximumMass" => "maxmass",
        "EngineOptPerformance" => "optmul",
        "EngineMinPerformance" => "minmul",
        "EngineMaxPerformance" => "maxmul",
        other => other,
    }
}

/// The base figures for a journal item: a module from the tables; a
/// bulkhead from the ship (they live on the hull, by grade: lightweight,
/// reinforced, military, mirrored, reactive); the fixed parts the journal
/// lists but no build tool prices (the cockpit weighs nothing; the cargo
/// hatch draws its 0.6 MW). None when the tables do not know the item.
fn base_for(catalog: &Catalog, ship: Option<&Ship>, item: &str) -> Option<(String, HashMap<String, f64>)> {
    if let Some(base) = catalog.module(item) {
        return Some((base.group.clone(), base.stats.clone()));
    }
    let lower = item.to_ascii_lowercase();
    if lower.contains("_armour_") {
        let index = match () {
            _ if lower.contains("grade1") => 0,
            _ if lower.contains("grade2") => 1,
            _ if lower.contains("grade3") => 2,
            _ if lower.contains("mirrored") => 3,
            _ if lower.contains("reactive") => 4,
            _ => 0,
        };
        let mass = ship.and_then(|s| s.bulkhead_mass.get(index)).copied().unwrap_or(0.0);
        return Some(("bh".to_string(), HashMap::from([("mass".to_string(), mass)])));
    }
    if lower.ends_with("_cockpit") {
        return Some(("cockpit".to_string(), HashMap::from([("mass".to_string(), 0.0)])));
    }
    if lower.contains("cargobaydoor") {
        return Some(("hatch".to_string(), HashMap::from([("mass".to_string(), 0.0), ("power".to_string(), 0.6)])));
    }
    None
}

/// The note a commander reads: the outfitting name, no vendor named
/// (maintainer, 2026-09-20: "let's not reference coriolis, and why are we
/// not showing the friendly name here?").
fn no_figures(item: &str) -> String {
    format!("{}: no figures for it yet", ed_journal::modules::item_name(item))
}

/// A module as fitted (or planned) in one slot.
#[derive(Debug, Clone, Serialize)]
pub struct Fitted {
    pub slot: String,
    pub item: String,
    /// Coriolis knows the module; false means its mass and power are unknown (0).
    pub known: bool,
    pub group: String,
    pub on: bool,
    pub priority: i64,
    pub stats: HashMap<String, f64>,
}

impl Fitted {
    pub fn stat(&self, k: &str) -> f64 {
        self.stats.get(k).copied().unwrap_or(0.0)
    }
    fn is_hardpoint(&self) -> bool {
        let s = self.slot.to_ascii_lowercase();
        s.contains("hardpoint") && !s.starts_with("tiny")
    }
}

/// A build: the ship and every fitted module with its effective stats.
#[derive(Debug, Clone, Serialize)]
pub struct Build {
    pub ship: Option<Ship>,
    pub ship_symbol: String,
    pub modules: Vec<Fitted>,
    /// Main tank, from the journal (planned builds keep the fitted tank).
    pub fuel_capacity: f64,
    pub cargo_capacity: f64,
    /// Items Coriolis's data does not know (mass and power counted as 0).
    pub unknown_items: Vec<String>,
}

/// Phase 1 figures. `None` where the data cannot say.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    /// Hull + modules, no fuel, no cargo (the journal's UnladenMass).
    pub unladen_mass: f64,
    /// Unladen + full main tank + reserve (EDSY's "current" mass, no cargo).
    pub full_mass: f64,
    /// Full tank, no cargo (EDSY "UNL").
    pub jump_unladen: Option<f64>,
    /// Full tank and full cargo (EDSY "LDN").
    pub jump_laden: Option<f64>,
    /// One jump's fuel aboard and nothing else (EDSY "MAX").
    pub jump_max: Option<f64>,
    /// Power plant capacity, MW.
    pub power_capacity: f64,
    /// Draw with hardpoints retracted / deployed, MW.
    pub power_retracted: f64,
    pub power_deployed: f64,
}

impl Build {
    /// From a journal `Loadout` (or a SLEF `data`): every module's base
    /// figures from Coriolis, overridden by the journal's rolled values.
    pub fn from_loadout(catalog: &Catalog, loadout: &Value) -> Build {
        let ship_symbol = loadout["Ship"].as_str().unwrap_or("").to_string();
        let ship = catalog.ship(&ship_symbol).cloned();
        let mut unknown_items = Vec::new();
        let mut modules = Vec::new();
        for m in loadout["Modules"].as_array().into_iter().flatten() {
            let slot = m["Slot"].as_str().unwrap_or("").to_string();
            if ed_journal::modules::is_cosmetic_slot(&slot) {
                continue;
            }
            let item = m["Item"].as_str().unwrap_or("").to_string();
            let (known, group, mut stats) = match base_for(catalog, ship.as_ref(), &item) {
                Some((group, stats)) => (true, group, stats),
                None => {
                    unknown_items.push(item.clone());
                    (false, String::new(), HashMap::new())
                }
            };
            for md in m.pointer("/Engineering/Modifiers").and_then(Value::as_array).into_iter().flatten() {
                if let (Some(label), Some(value)) = (md["Label"].as_str(), md["Value"].as_f64()) {
                    stats.insert(field_for_label(label).to_string(), value);
                }
            }
            modules.push(Fitted {
                slot,
                item,
                known,
                group,
                on: m["On"].as_bool().unwrap_or(true),
                priority: m["Priority"].as_i64().unwrap_or(1),
                stats,
            });
        }
        Build {
            ship,
            ship_symbol,
            modules,
            fuel_capacity: loadout.pointer("/FuelCapacity/Main").and_then(Value::as_f64).unwrap_or(0.0),
            cargo_capacity: loadout["CargoCapacity"].as_f64().unwrap_or(0.0),
            unknown_items,
        }
    }

    /// A module the ship does not have yet in a slot (an imported build's
    /// swap): the slot takes the new item's base figures, unengineered,
    /// with the old module's on/priority; an empty slot gets a new row.
    pub fn refit(&mut self, catalog: &Catalog, slot: &str, item: &str) -> Result<(), String> {
        let (group, stats) = base_for(catalog, self.ship.as_ref(), item).ok_or_else(|| no_figures(item))?;
        match self.modules.iter_mut().find(|m| m.slot.eq_ignore_ascii_case(slot)) {
            Some(fitted) => {
                fitted.item = item.to_string();
                fitted.known = true;
                fitted.group = group;
                fitted.stats = stats;
            }
            None => self.modules.push(Fitted { slot: slot.to_string(), item: item.to_string(), known: true, group, on: true, priority: 1, stats }),
        }
        Ok(())
    }

    /// A planned blueprint at a full roll on one slot: every feature of the
    /// grade at the best end of its range, on the module's BASE figures
    /// (a plan replaces whatever is rolled now).
    pub fn plan(&mut self, catalog: &Catalog, slot: &str, blueprint_fdname: &str, grade: i64) -> Result<(), String> {
        let ship = self.ship.clone();
        let fitted = self.modules.iter_mut().find(|m| m.slot.eq_ignore_ascii_case(slot)).ok_or_else(|| format!("no module in slot {slot}"))?;
        let (_, base) = base_for(catalog, ship.as_ref(), &fitted.item).ok_or_else(|| no_figures(&fitted.item))?;
        let bp = catalog.blueprint(blueprint_fdname).ok_or_else(|| format!("no figures for the blueprint {blueprint_fdname}"))?;
        let features = bp.grades.get(&grade).ok_or_else(|| format!("{blueprint_fdname} has no grade {grade}"))?;
        fitted.stats = base.clone();
        for (k, (_, best)) in features {
            if let Some(b) = base.get(k) {
                fitted.stats.insert(k.clone(), b * (1.0 + best));
            }
        }
        Ok(())
    }

    fn module_mass(&self) -> f64 {
        self.modules.iter().map(|m| m.stat("mass")).sum()
    }

    pub fn fsd(&self) -> Option<&Fitted> {
        self.modules.iter().find(|m| m.group == "fsd")
    }

    /// Guardian FSD booster's fixed light-years, if fitted.
    fn booster_ly(&self) -> f64 {
        self.modules.iter().filter(|m| m.group == "gfsb").map(|m| m.stat("jumpboost")).sum()
    }

    /// Jump range with `mass` tonnes total aboard: the game's formula,
    /// `optmass / mass * (maxfuel / fuelmul)^(1 / fuelpower)`, plus the booster.
    pub fn jump_at(&self, mass: f64) -> Option<f64> {
        let fsd = self.fsd()?;
        let (optmass, maxfuel, fuelmul, fuelpower) = (fsd.stat("optmass"), fsd.stat("maxfuel"), fsd.stat("fuelmul"), fsd.stat("fuelpower"));
        if optmass <= 0.0 || maxfuel <= 0.0 || fuelmul <= 0.0 || fuelpower <= 0.0 || mass <= 0.0 {
            return None;
        }
        Some(optmass / mass * (maxfuel / fuelmul).powf(1.0 / fuelpower) + self.booster_ly())
    }

    pub fn summary(&self) -> Summary {
        let hull = self.ship.as_ref().map(|s| s.hull_mass).unwrap_or(0.0);
        let reserve = self.ship.as_ref().map(|s| s.reserve_fuel).unwrap_or(0.0);
        let unladen_mass = hull + self.module_mass();
        let full_mass = unladen_mass + self.fuel_capacity + reserve;
        let maxfuel = self.fsd().map(|f| f.stat("maxfuel")).unwrap_or(0.0);
        let power_capacity = self.modules.iter().filter(|m| m.group == "pp").map(|m| m.stat("pgen")).sum();
        let draw = |deployed: bool| -> f64 {
            self.modules.iter().filter(|m| m.on && (deployed || !m.is_hardpoint())).map(|m| m.stat("power")).sum()
        };
        Summary {
            unladen_mass,
            full_mass,
            // The reserve is on the scales but not in the jump: EDSY's 19.05
            // and the journal's MaxJumpRange both come out only without it.
            jump_unladen: self.jump_at(unladen_mass + self.fuel_capacity),
            jump_laden: self.jump_at(unladen_mass + self.fuel_capacity + self.cargo_capacity),
            jump_max: self.jump_at(unladen_mass + maxfuel.min(self.fuel_capacity)),
            power_capacity,
            power_retracted: draw(false),
            power_deployed: draw(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
    }

    #[test]
    fn the_catalog_knows_the_maintainers_type_10_and_its_drive() {
        let c = Catalog::load();
        let t10 = c.ship("type9_military").expect("Type-10 by journal symbol");
        assert_eq!((t10.name.as_str(), t10.hull_mass), ("Type-10 Defender", 1200.0));
        let fsd = c.module("int_hyperdrive_overcharge_size7_class5").expect("7A SCO drive");
        assert_eq!(fsd.group, "fsd");
        assert!(fsd.stat("optmass") > 0.0 && fsd.stat("maxfuel") > 0.0);
        assert!(c.blueprint("FSD_LongRange").is_some());
    }

    /// The pin: the maintainer's Type-10 as flown (journal ShipID 37) against
    /// EDSY 4.24.1.1 the same day. Mass to a tenth of a tonne, jump to a
    /// hundredth of a light-year, power to a tenth of a percent.
    #[test]
    fn the_type_10_as_flown_matches_edsy_on_mass_jump_and_power() {
        let c = Catalog::load();
        let pin = fixture("edsy_type10_37.json");
        let loadout = fixture("loadout_37.json");
        let build = Build::from_loadout(&c, &loadout);
        assert!(build.unknown_items.is_empty(), "Coriolis lacks: {:?}", build.unknown_items);
        let s = build.summary();

        let near = |a: f64, b: f64, tol: f64, what: &str| assert!((a - b).abs() <= tol, "{what}: ours {a} vs EDSY/journal {b}");
        near(s.unladen_mass, pin["journal"]["unladen_mass"].as_f64().unwrap(), 0.1, "unladen mass");
        near(s.full_mass, pin["edsy"]["mass"]["cur_t"].as_f64().unwrap(), 0.1, "full mass");
        near(s.jump_unladen.unwrap(), pin["edsy"]["jump_ly"]["unladen"].as_f64().unwrap(), 0.01, "jump, full tank");
        near(s.jump_laden.unwrap(), pin["edsy"]["jump_ly"]["laden"].as_f64().unwrap(), 0.01, "jump, laden");
        near(s.jump_max.unwrap(), pin["edsy"]["jump_ly"]["max"].as_f64().unwrap(), 0.01, "jump, max");
        near(s.jump_max.unwrap(), pin["journal"]["max_jump_range"].as_f64().unwrap(), 0.01, "jump, max vs the journal");
        let pct = |draw: f64| 100.0 * draw / s.power_capacity;
        near(pct(s.power_retracted), pin["edsy"]["power"]["retracted_pct"].as_f64().unwrap(), 0.1, "power retracted %");
        near(pct(s.power_deployed), pin["edsy"]["power"]["deployed_pct"].as_f64().unwrap(), 0.1, "power deployed %");
    }

    /// An imported build's swap: a heavier module in a slot weighs more
    /// and draws more, and an empty slot can take one.
    #[test]
    fn a_refit_takes_the_new_modules_figures() {
        let c = Catalog::load();
        let loadout = fixture("loadout_37.json");
        let mut build = Build::from_loadout(&c, &loadout);
        let before = build.summary();
        // The Type-10's 7D thrusters for 7A: heavier, hungrier.
        build.refit(&c, "MainEngines", "int_engine_size7_class5").unwrap();
        let after = build.summary();
        assert!(after.unladen_mass > before.unladen_mass, "{before:?} -> {after:?}");
        assert!(after.power_retracted > before.power_retracted);
        // An empty slot gets a row.
        let n = build.modules.len();
        build.refit(&c, "Slot99_Size1", "int_shieldcellbank_size1_class1").unwrap();
        assert_eq!(build.modules.len(), n + 1);
        let err = build.refit(&c, "MainEngines", "int_no_such_thing").unwrap_err();
        assert!(!err.to_lowercase().contains("coriolis"), "{err}");
        // A bulkhead swap is by grade on the hull, not a module lookup: the
        // maintainer's imported Type-10 wanted reactive armour.
        let before = build.summary().unladen_mass;
        build.refit(&c, "Armour", "type9_military_armour_reactive").unwrap();
        assert!(build.summary().unladen_mass > before, "reactive armour weighs more than lightweight");
    }

    /// A planned roll changes the figures the way the blueprint says: a
    /// G5 Increased Range on the Type-10's drive at a full roll lifts the
    /// optimal mass and the jump, and adds the blueprint's mass.
    #[test]
    fn a_planned_blueprint_moves_the_figures() {
        let c = Catalog::load();
        let loadout = fixture("loadout_37.json");
        let mut build = Build::from_loadout(&c, &loadout);
        let before = build.summary();
        build.plan(&c, "FrameShiftDrive", "FSD_LongRange", 5).unwrap();
        let after = build.summary();
        assert!(after.jump_unladen.unwrap() > before.jump_unladen.unwrap() * 1.3, "{before:?} -> {after:?}");
        assert!(after.unladen_mass > before.unladen_mass, "the range blueprint adds mass");
    }
}
