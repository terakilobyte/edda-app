//! Bridging the journal's names to the vendored blueprint names.
//!
//! `Loadout` and `EngineerCraft` name modules by item symbol
//! (`int_hyperdrive_size7_class5`) and blueprints by symbol
//! (`FSD_LongRange`); the EDEngineer data uses display names ("Frame Shift
//! Drive" / "Increased FSD Range"). These tables connect them. Anything not
//! listed resolves to `None` and is shown raw -- a wrong guess here would
//! plan the wrong materials.

/// Module type (as the blueprint data spells it) for a journal item symbol.
pub fn module_type_for_item(item: &str) -> Option<&'static str> {
    let i = item.to_ascii_lowercase();
    let has = |s: &str| i.contains(s);
    Some(match () {
        _ if has("int_hyperdrive") => "Frame Shift Drive",
        // `int_engine_size5_class5`, plus the Mk II ships' `int_mkiiagileboost_engine_...`.
        _ if has("int_engine") || has("_engine_") => "Thrusters",
        _ if has("int_powerplant") => "Power Plant",
        _ if has("int_powerdistributor") => "Power Distributor",
        _ if has("int_sensors") => "Sensors",
        _ if has("int_shieldgenerator") => "Shield Generator",
        _ if has("hpt_shieldbooster") => "Shield Booster",
        _ if has("int_shieldcellbank") => "Shield Cell Bank",
        _ if has("int_hullreinforcement") => "Hull Reinforcement Package",
        _ if has("_armour_") => "Armour",
        _ if has("int_fsdinterdictor") => "Frame Shift Drive Interdictor",
        _ if has("int_fuelscoop") => "Fuel Scoop",
        _ if has("int_refinery") => "Refinery",
        _ if has("int_lifesupport") => "Life Support",
        _ if has("int_repairer") => "Auto Field-Maintenance Unit",
        _ if has("int_detailedsurfacescanner") => "Surface Scanner",
        _ if has("int_dronecontrol_collection") => "Collector Limpet Controller",
        _ if has("int_dronecontrol_prospector") => "Prospector Limpet Controller",
        _ if has("int_dronecontrol_fueltransfer") => "Fuel Transfer Limpet Controller",
        _ if has("int_dronecontrol_resourcesiphon") => "Hatch Breaker Limpet Controller",
        _ if has("hpt_chafflauncher") => "Chaff Launcher",
        _ if has("hpt_heatsinklauncher") => "Heat Sink Launcher",
        _ if has("hpt_plasmapointdefence") => "Point Defence",
        _ if has("hpt_electroniccountermeasure") => "Electronic Countermeasure",
        _ if has("hpt_cargoscanner") => "Manifest Scanner",
        _ if has("hpt_cloudscanner") => "Wake Scanner",
        _ if has("hpt_crimescanner") => "Kill Warrant Scanner",
        _ if has("hpt_pulselaserburst") => "Burst Laser",
        _ if has("hpt_pulselaser") => "Pulse Laser",
        _ if has("hpt_beamlaser") => "Beam Laser",
        _ if has("hpt_multicannon") => "Multi-cannon",
        _ if has("hpt_cannon") => "Cannon",
        _ if has("hpt_slugshot") => "Fragment Cannon",
        _ if has("hpt_railgun") => "Rail Gun",
        _ if has("hpt_plasmaaccelerator") => "Plasma Accelerator",
        _ if has("missilerack") => "Missile Rack",
        _ if has("hpt_minelauncher") => "Mine Launcher",
        _ if has("hpt_advancedtorppylon") => "Torpedo Pylon",
        _ => return None,
    })
}

/// Blueprint display name for a journal blueprint symbol, given the module
/// type it was applied to (some symbols are shared, e.g. `Misc_Shielded`).
pub fn blueprint_for_symbol(symbol: &str, module_type: &str) -> Option<&'static str> {
    let s = symbol.to_ascii_lowercase();
    let m = module_type;
    let weaponish = matches!(
        m,
        "Pulse Laser" | "Burst Laser" | "Beam Laser" | "Multi-cannon" | "Cannon" | "Fragment Cannon"
            | "Rail Gun" | "Plasma Accelerator" | "Missile Rack" | "Mine Launcher" | "Torpedo Pylon"
    );
    Some(match s.as_str() {
        "fsd_longrange" => "Increased FSD Range",
        "fsd_fastboot" => "Faster FSD Boot Sequence",
        "fsd_shielded" => "Shielded FSD",
        "engine_dirty" => "Dirty Drive Tuning",
        "engine_tuned" => "Clean Drive Tuning",
        "engine_reinforced" => "Drive Strengthening",
        "powerplant_boosted" => "Overcharged",
        "powerplant_armoured" => "Armoured",
        "powerplant_stealth" => "Low Emissions",
        "powerdistributor_highfrequency" => "Charge Enhanced",
        "powerdistributor_highcapacity" => "High Charge Capacity",
        "powerdistributor_priorityengines" => "Engine Focused",
        "powerdistributor_priorityweapons" => "Weapon Focused",
        "powerdistributor_prioritysystems" => "System Focused",
        "powerdistributor_shielded" => "Shielded",
        "sensor_lightweight" => "Light Weight Scanner",
        "sensor_expanded" => "Expanded Probe Scanning Radius",
        "sensor_longrange" => "Long Range Scanner",
        "sensor_wideangle" => "Wide Angle Scanner",
        "shieldgenerator_reinforced" => "Reinforced Shields",
        "shieldgenerator_thermic" => "Thermal Resistant Shields",
        "shieldgenerator_kinetic" => "Kinetic Resistant Shields",
        "shieldgenerator_optimised" => "Enhanced, Low Power Shields",
        "shieldbooster_heavyduty" => "Heavy Duty",
        "shieldbooster_kinetic" => "Kinetic Resistant",
        "shieldbooster_thermic" => "Thermal Resistant",
        "shieldbooster_explosive" => "Blast Resistant",
        "shieldbooster_resistive" => "Resistance Augmented",
        "shieldcellbank_rapid" => "Rapid Charge",
        "shieldcellbank_specialised" => "Specialised",
        "armour_advanced" => "Lightweight",
        "armour_explosive" => "Blast Resistant",
        "armour_heavyduty" => "Heavy Duty",
        "armour_kinetic" => "Kinetic Resistant",
        "armour_thermic" => "Thermal Resistant",
        "hullreinforcement_advanced" => "Lightweight Hull Reinforcement",
        "hullreinforcement_explosive" => "Blast Resistant Hull Reinforcement",
        "hullreinforcement_heavyduty" => "Heavy Duty Hull Reinforcement",
        "hullreinforcement_kinetic" => "Kinetic Resistant Hull Reinforcement",
        "hullreinforcement_thermic" => "Thermal Resistant Hull Reinforcement",
        "fsdinterdictor_expanded" => "Expanded FSD Interdictor Capture Arc",
        "fsdinterdictor_longrange" => "Long Range FSD Interdictor",
        "weapon_efficient" => "Efficient Weapon",
        "weapon_focused" => "Focused Weapon",
        "weapon_lightweight" => "Lightweight Mount",
        "weapon_longrange" => "Long Range Weapon",
        "weapon_overcharged" => "Overcharged Weapon",
        "weapon_rapidfire" => "Rapid Fire Modification",
        "weapon_shortrange" => "Short Range Blaster",
        "weapon_sturdy" => "Sturdy Mount",
        "weapon_highcapacity" => "High Capacity Magazine",
        "weapon_doubleshot" => "Double Shot",
        "misc_lightweight" if !weaponish => "Lightweight",
        "misc_reinforced" if !weaponish => "Reinforced",
        "misc_shielded" if !weaponish => "Shielded",
        "misc_chaffcapacity" | "misc_heatsinkcapacity" | "misc_pointdefensecapacity" => "Ammo Capacity",
        "kill_warrant_scanner_longrange" | "cargoscanner_longrange" | "wakescanner_longrange" | "sensor_kill_warrant_longrange" => "Long Range Scanner",
        "kill_warrant_scanner_wideangle" | "cargoscanner_wideangle" | "wakescanner_wideangle" => "Wide Angle Scanner",
        "kill_warrant_scanner_fastscan" | "cargoscanner_fastscan" | "wakescanner_fastscan" => "Fast Scanner",
        _ => return None,
    })
}

/// Rows in the blueprint data that are synthesis recipes, not module
/// blueprints: they have no Loadout symbol and a proposed-engineering
/// export refuses them. Whole module types first, then recipe names that
/// share a type with real blueprints (launcher and life-support refills).
pub const SYNTHESIS_TYPES: &[&str] = &[
    "AFM Refill", "AX Explosive Munitions", "AX Remote Flak Munitions",
    "AX Small Calibre Munitions", "Enzyme Missile Launcher Munitions",
    "Explosive Munitions", "FSD Injection", "Flechette Launcher Munitions",
    "Guardian Gauss Cannon Munitions", "Guardian Plasma Charger Munitions",
    "Guardian Shard Cannon Munitions", "High Velocity Munitions",
    "Large Calibre Munitions", "Limpets", "Plasma Munitions",
    "SRV Ammo Restock", "SRV Refuel", "SRV Repair",
    "Shock Cannon Munitions", "Small Calibre Munitions", "Suit", "Weapon",
];
pub const SYNTHESIS_NAMES: &[&str] = &[
    "100% Refill", "50% Refill", "100% Refill, +2 Seconds Duration",
    "100% Refill, +15% Heat Dissipation", "100% Refill, +30% Heat Dissipation",
];

/// Journal blueprint symbol for a blueprint-data name, given the module
/// type it would be applied to: the exact reverse of
/// [`blueprint_for_symbol`], used to write a *proposed* `Engineering`
/// block into an exported Loadout. `None` for names this table cannot
/// place -- a wrong guess would export the wrong modification.
pub fn symbol_for_blueprint(name: &str, module_type: &str) -> Option<&'static str> {
    let m = module_type;
    let weaponish = matches!(
        m,
        "Pulse Laser" | "Burst Laser" | "Beam Laser" | "Multi-cannon" | "Cannon" | "Fragment Cannon"
            | "Rail Gun" | "Plasma Accelerator" | "Missile Rack" | "Mine Launcher" | "Torpedo Pylon"
    );
    Some(match name {
        "Increased FSD Range" => "FSD_LongRange",
        "Faster FSD Boot Sequence" => "FSD_FastBoot",
        "Shielded FSD" => "FSD_Shielded",
        "Dirty Drive Tuning" => "Engine_Dirty",
        "Clean Drive Tuning" => "Engine_Tuned",
        "Drive Strengthening" => "Engine_Reinforced",
        "Overcharged" => "PowerPlant_Boosted",
        "Armoured" => "PowerPlant_Armoured",
        "Low Emissions" => "PowerPlant_Stealth",
        "Charge Enhanced" => "PowerDistributor_HighFrequency",
        "High Charge Capacity" => "PowerDistributor_HighCapacity",
        "Engine Focused" => "PowerDistributor_PriorityEngines",
        "Weapon Focused" => "PowerDistributor_PriorityWeapons",
        "System Focused" => "PowerDistributor_PrioritySystems",
        "Light Weight Scanner" => "Sensor_LightWeight",
        "Expanded Probe Scanning Radius" => "Sensor_Expanded",
        "Long Range Scanner" => match m {
            "Kill Warrant Scanner" => "Kill_Warrant_Scanner_LongRange",
            "Manifest Scanner" => "CargoScanner_LongRange",
            "Wake Scanner" => "WakeScanner_LongRange",
            _ => "Sensor_LongRange",
        },
        "Wide Angle Scanner" => match m {
            "Kill Warrant Scanner" => "Kill_Warrant_Scanner_WideAngle",
            "Manifest Scanner" => "CargoScanner_WideAngle",
            "Wake Scanner" => "WakeScanner_WideAngle",
            _ => "Sensor_WideAngle",
        },
        "Fast Scanner" => match m {
            "Kill Warrant Scanner" => "Kill_Warrant_Scanner_FastScan",
            "Manifest Scanner" => "CargoScanner_FastScan",
            _ => "WakeScanner_FastScan",
        },
        "Reinforced Shields" => "ShieldGenerator_Reinforced",
        "Thermal Resistant Shields" => "ShieldGenerator_Thermic",
        "Kinetic Resistant Shields" => "ShieldGenerator_Kinetic",
        "Enhanced, Low Power Shields" => "ShieldGenerator_Optimised",
        "Heavy Duty" if m == "Shield Booster" => "ShieldBooster_HeavyDuty",
        "Kinetic Resistant" if m == "Shield Booster" => "ShieldBooster_Kinetic",
        "Thermal Resistant" if m == "Shield Booster" => "ShieldBooster_Thermic",
        "Blast Resistant" if m == "Shield Booster" => "ShieldBooster_Explosive",
        "Resistance Augmented" => "ShieldBooster_Resistive",
        "Rapid Charge" => "ShieldCellBank_Rapid",
        "Specialised" => "ShieldCellBank_Specialised",
        "Lightweight" if m == "Armour" => "Armour_Advanced",
        "Blast Resistant" if m == "Armour" => "Armour_Explosive",
        "Heavy Duty" if m == "Armour" => "Armour_HeavyDuty",
        "Kinetic Resistant" if m == "Armour" => "Armour_Kinetic",
        "Thermal Resistant" if m == "Armour" => "Armour_Thermic",
        "Lightweight Hull Reinforcement" => "HullReinforcement_Advanced",
        "Blast Resistant Hull Reinforcement" => "HullReinforcement_Explosive",
        "Heavy Duty Hull Reinforcement" => "HullReinforcement_HeavyDuty",
        "Kinetic Resistant Hull Reinforcement" => "HullReinforcement_Kinetic",
        "Thermal Resistant Hull Reinforcement" => "HullReinforcement_Thermic",
        "Expanded FSD Interdictor Capture Arc" => "FSDinterdictor_Expanded",
        "Long Range FSD Interdictor" => "FSDinterdictor_LongRange",
        "Efficient Weapon" => "Weapon_Efficient",
        "Focused Weapon" => "Weapon_Focused",
        "Lightweight Mount" => "Weapon_LightWeight",
        "Long Range Weapon" => "Weapon_LongRange",
        "Overcharged Weapon" => "Weapon_Overcharged",
        "Rapid Fire Modification" => "Weapon_RapidFire",
        "Short Range Blaster" => "Weapon_ShortRange",
        "Sturdy Mount" => "Weapon_Sturdy",
        "High Capacity Magazine" => "Weapon_HighCapacity",
        "Double Shot" => "Weapon_DoubleShot",
        "Lightweight" if !weaponish => "Misc_LightWeight",
        "Reinforced" if !weaponish => "Misc_Reinforced",
        "Shielded" if !weaponish => "Misc_Shielded",
        "Ammo Capacity" => match m {
            "Heat Sink Launcher" => "Misc_HeatSinkCapacity",
            "Point Defence" => "Misc_PointDefenseCapacity",
            _ => "Misc_ChaffCapacity",
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_commanders_fsd_resolves() {
        assert_eq!(module_type_for_item("int_hyperdrive_overcharge_size7_class5"), Some("Frame Shift Drive"));
        assert_eq!(blueprint_for_symbol("FSD_LongRange", "Frame Shift Drive"), Some("Increased FSD Range"));
    }

    #[test]
    fn weapons_and_utilities_resolve_by_item_prefix() {
        assert_eq!(module_type_for_item("Hpt_PulseLaserBurst_Gimbal_Medium"), Some("Burst Laser"));
        assert_eq!(module_type_for_item("int_mkiiagileboost_engine_size5_class5"), Some("Thrusters"), "Kestrel Mk II thrusters");
        assert_eq!(module_type_for_item("hpt_pulselaser_fixed_small"), Some("Pulse Laser"));
        assert_eq!(blueprint_for_symbol("Weapon_Efficient", "Beam Laser"), Some("Efficient Weapon"));
        assert_eq!(blueprint_for_symbol("Misc_Shielded", "Fuel Scoop"), Some("Shielded"));
        assert_eq!(blueprint_for_symbol("Misc_Shielded", "Beam Laser"), None, "weapons have no Misc_Shielded");
    }

    #[test]
    fn every_mapped_pair_exists_in_the_vendored_data() {
        let cat = crate::Catalog::load();
        for (item, symbol) in [
            ("int_hyperdrive_size5_class5", "FSD_LongRange"),
            ("int_engine_size5_class5", "Engine_Dirty"),
            ("int_powerplant_size5_class5", "PowerPlant_Boosted"),
            ("int_powerdistributor_size5_class5", "PowerDistributor_HighFrequency"),
            ("int_sensors_size2_class5", "Sensor_LightWeight"),
            ("int_shieldgenerator_size5_class5", "ShieldGenerator_Reinforced"),
            ("hpt_shieldbooster_size0_class5", "ShieldBooster_HeavyDuty"),
            ("hpt_multicannon_gimbal_medium", "Weapon_Overcharged"),
        ] {
            let mt = module_type_for_item(item).unwrap();
            let bp = blueprint_for_symbol(symbol, mt).unwrap();
            assert!(!cat.grades_for(mt, bp).is_empty(), "{mt} / {bp} missing from data");
        }
    }

    /// Every graded blueprint in the vendored data either round-trips
    /// name -> symbol -> name, or is on the known-unmapped list (module
    /// types the journal tables do not cover yet). This is what makes a
    /// proposed-engineering export trustworthy.
    #[test]
    fn vendored_blueprints_round_trip_through_the_symbol_tables() {
        let cat = crate::Catalog::load();
        let mut unmapped = std::collections::BTreeSet::new();
        for (module_type, name) in cat.graded_pairs() {
            match symbol_for_blueprint(&name, &module_type) {
                None => {
                    unmapped.insert((module_type.clone(), name.clone()));
                }
                Some(symbol) => {
                    assert_eq!(
                        blueprint_for_symbol(symbol, &module_type),
                        Some(name.as_str()),
                        "{module_type} / {name} -> {symbol} does not round-trip"
                    );
                }
            }
        }
        // Module types with no journal mapping at all (no Loadout symbol
        // table yet): additions here should shrink, never grow.
        for (module_type, name) in &unmapped {
            assert!(
                SYNTHESIS_TYPES.contains(&module_type.as_str()) || SYNTHESIS_NAMES.contains(&name.as_str()),
                "{module_type} / {name}: no symbol and not a known synthesis recipe; map it"
            );
        }
    }

    #[test]
    fn unknown_symbols_are_none_not_guessed() {
        assert_eq!(module_type_for_item("int_mysterybox"), None);
        assert_eq!(blueprint_for_symbol("Future_Blueprint", "Thrusters"), None);
    }
}
