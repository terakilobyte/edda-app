//! Display names for Loadout module items and slots.
//!
//! The journal names modules by internal symbol (`int_fuelscoop_size7_class5`,
//! `hpt_beamlaser_gimbal_medium`) and slots by internal id (`Slot01_Size7`,
//! `MainEngines`). These are what the game shows in outfitting, or as close
//! as the symbol allows: "Fuel Scoop 7A", "Beam Laser (gimballed, medium)",
//! "Optional 1 (size 7)", "Thrusters".

/// The outfitting name for a module symbol.
pub fn item_name(symbol: &str) -> String {
    let s = symbol.to_ascii_lowercase();
    let parts: Vec<&str> = s.split('_').collect();
    // Size/class from `sizeN_classM` -> "NA".
    let size = parts
        .iter()
        .find_map(|p| p.strip_prefix("size").and_then(|n| n.parse::<u8>().ok()));
    let class = parts
        .iter()
        .find_map(|p| p.strip_prefix("class").and_then(|n| n.parse::<u8>().ok()));
    let rating = |c: u8| match c {
        1 => "E",
        2 => "D",
        3 => "C",
        4 => "B",
        5 => "A",
        _ => "",
    };
    let sc = match (size, class) {
        (Some(n), Some(c)) => format!(" {n}{}", rating(c)),
        (Some(n), None) => format!(" {n}"),
        _ => String::new(),
    };
    let has = |k: &str| s.contains(k);

    // Hull, cockpit, cosmetics.
    if has("_armour_") {
        let grade = match () {
            _ if has("grade1") => "Lightweight Alloy",
            _ if has("grade2") => "Reinforced Alloy",
            _ if has("grade3") => "Military Grade Composite",
            _ if has("mirrored") => "Mirrored Surface Composite",
            _ if has("reactive") => "Reactive Surface Composite",
            _ => "Bulkheads",
        };
        return grade.to_string();
    }
    if has("_cockpit") {
        return "Cockpit".into();
    }
    if has("modularcargobaydoor") {
        return "Cargo Hatch".into();
    }
    if has("voicepack") {
        return format!("Ship voice: {}", cap(parts.last().unwrap_or(&"")));
    }

    // Hardpoints and utilities.
    if let Some(rest) = s.strip_prefix("hpt_") {
        let mount = match () {
            _ if has("_fixed") => Some("fixed"),
            _ if has("_gimbal") => Some("gimballed"),
            _ if has("_turret") => Some("turreted"),
            _ => None,
        };
        let hsize = match () {
            _ if has("_tiny") => Some("0"),
            _ if has("_small") => Some("small"),
            _ if has("_medium") => Some("medium"),
            _ if has("_large") => Some("large"),
            _ if has("_huge") => Some("huge"),
            _ => None,
        };
        let base = rest.split('_').next().unwrap_or(rest);
        let name = match base {
            "pulselaser" => "Pulse Laser",
            "pulselaserburst" => "Burst Laser",
            "beamlaser" => "Beam Laser",
            "multicannon" => "Multi-cannon",
            "cannon" => "Cannon",
            "railgun" => "Rail Gun",
            "plasmaaccelerator" => "Plasma Accelerator",
            "slugshot" => "Fragment Cannon",
            "basicmissilerack" | "dumbfiremissilerack" => "Missile Rack",
            "drunkmissilerack" => "Pack-Hound Missile Rack",
            "advancedtorppylon" => "Torpedo Pylon",
            "minelauncher" => "Mine Launcher",
            "mininglaser" => "Mining Laser",
            "mining" => "Mining Tool",
            "guardian" => "Guardian Weapon",
            "atmulticannon" => "AX Multi-cannon",
            "atdumbfiremissile" => "AX Missile Rack",
            "flakmortar" => "Remote Release Flak Launcher",
            "shieldbooster" => "Shield Booster",
            "heatsinklauncher" => "Heat Sink Launcher",
            "chafflauncher" => "Chaff Launcher",
            "electroniccountermeasure" => "Electronic Countermeasure",
            "plasmapointdefence" => "Point Defence",
            "cargoscanner" => "Manifest Scanner",
            "cloudscanner" => "Frame Shift Wake Scanner",
            "crimescanner" => "Kill Warrant Scanner",
            "mrascanner" => "Pulse Wave Analyser",
            "xenoscanner" => "Xeno Scanner",
            "antiunknownshutdown" => "Shutdown Field Neutraliser",
            "causticsinklauncher" => "Caustic Sink Launcher",
            "shieldgenerator" => "Shield Generator",
            other => return cap(other),
        };
        let mut out = name.to_string();
        let detail: Vec<&str> = [mount, hsize].into_iter().flatten().collect();
        if !detail.is_empty() {
            out.push_str(&format!(" ({})", detail.join(", ")));
        }
        return out;
    }

    // Internals.
    let base = s.strip_prefix("int_").unwrap_or(&s);
    let base = base.split('_').next().unwrap_or(base);
    let name = match base {
        "hyperdrive" => {
            let mut n = "Frame Shift Drive".to_string();
            if has("overcharge") {
                n.push_str(" (SCO)");
            }
            if has("mkii") {
                n.push_str(" Mk II");
            }
            n
        }
        "engine" => "Thrusters".into(),
        "powerplant" => "Power Plant".into(),
        "powerdistributor" => "Power Distributor".into(),
        "sensors" => "Sensors".into(),
        "lifesupport" => "Life Support".into(),
        "fueltank" => "Fuel Tank".into(),
        "fuelscoop" => "Fuel Scoop".into(),
        "shieldgenerator" => "Shield Generator".into(),
        "shieldcellbank" => "Shield Cell Bank".into(),
        "hullreinforcement" => "Hull Reinforcement Package".into(),
        "modulereinforcement" => "Module Reinforcement Package".into(),
        "guardianhullreinforcement" => "Guardian Hull Reinforcement".into(),
        "guardianmodulereinforcement" => "Guardian Module Reinforcement".into(),
        "guardianshieldreinforcement" => "Guardian Shield Reinforcement".into(),
        "guardianfsdbooster" => "Guardian FSD Booster".into(),
        "guardianpowerplant" => "Guardian Power Plant".into(),
        "guardianpowerdistributor" => "Guardian Power Distributor".into(),
        "repairer" => "Auto Field-Maintenance Unit".into(),
        "refinery" => "Refinery".into(),
        "cargorack" => "Cargo Rack".into(),
        "corrosionproofcargorack" => "Corrosion Resistant Cargo Rack".into(),
        "passengercabin" => "Passenger Cabin".into(),
        "detailedsurfacescanner" => "Detailed Surface Scanner".into(),
        "supercruiseassist" => "Supercruise Assist".into(),
        "dockingcomputer" => {
            if has("advanced") {
                "Advanced Docking Computer".into()
            } else {
                "Standard Docking Computer".into()
            }
        }
        "planetapproachsuite" => "Planetary Approach Suite".into(),
        "fsdinterdictor" => "Frame Shift Drive Interdictor".into(),
        "dronecontrol" => {
            let kind = match () {
                _ if has("collection") => "Collector",
                _ if has("prospector") => "Prospector",
                _ if has("fueltransfer") => "Fuel Transfer",
                _ if has("repair") => "Repair",
                _ if has("resourcesiphon") => "Hatch Breaker",
                _ if has("recon") => "Recon",
                _ if has("decontamination") => "Decontamination",
                _ if has("rescue") => "Rescue",
                _ => "",
            };
            format!("{kind} Limpet Controller").trim().to_string()
        }
        "multidronecontrol" => "Multi Limpet Controller".into(),
        "buggybay" => "Planetary Vehicle Hangar".into(),
        "fighterbay" => "Fighter Hangar".into(),
        "metaalloyhullreinforcement" => "Meta Alloy Hull Reinforcement".into(),
        "expmodulestabiliser" => "Experimental Weapon Stabiliser".into(),
        "shutdownfieldneutraliser" => "Shutdown Field Neutraliser".into(),
        other => cap(other),
    };
    format!("{name}{sc}")
}

/// The outfitting name for a slot id.
pub fn slot_name(slot: &str) -> String {
    let s = slot;
    match s {
        "FrameShiftDrive" => return "Frame Shift Drive".into(),
        "MainEngines" => return "Thrusters".into(),
        "PowerPlant" => return "Power Plant".into(),
        "PowerDistributor" => return "Power Distributor".into(),
        "Radar" => return "Sensors".into(),
        "LifeSupport" => return "Life Support".into(),
        "FuelTank" => return "Fuel Tank".into(),
        "Armour" => return "Bulkheads".into(),
        "CargoHatch" => return "Cargo Hatch".into(),
        "ShipCockpit" => return "Cockpit".into(),
        "PlanetaryApproachSuite" => return "Planetary Approach Suite".into(),
        "VesselVoice" => return "Ship Voice".into(),
        _ => {}
    }
    if let Some(rest) = s.strip_prefix("Slot") {
        // Slot01_Size7
        let mut it = rest.split("_Size");
        let n = it.next().unwrap_or("").trim_start_matches('0');
        let size = it.next().unwrap_or("");
        return if size.is_empty() {
            format!("Optional {n}")
        } else {
            format!("Optional {n} (size {size})")
        };
    }
    for (prefix, label) in [
        ("TinyHardpoint", "Utility"),
        ("SmallHardpoint", "Small hardpoint"),
        ("MediumHardpoint", "Medium hardpoint"),
        ("LargeHardpoint", "Large hardpoint"),
        ("HugeHardpoint", "Huge hardpoint"),
        ("Military", "Military"),
    ] {
        if let Some(n) = s.strip_prefix(prefix) {
            return format!("{label} {}", n.trim_start_matches('0'));
        }
    }
    s.to_string()
}

/// Cosmetic and bookkeeping slots that outfitting does not list.
pub fn is_cosmetic_slot(slot: &str) -> bool {
    slot.starts_with("PaintJob")
        || slot.starts_with("Decal")
        || slot.starts_with("ShipKit")
        || slot.starts_with("Bobble")
        || slot.starts_with("WeaponColour")
        || slot.starts_with("EngineColour")
        || slot.starts_with("ShipName")
        || slot.starts_with("ShipID")
        || slot == "VesselVoice"
        || slot.starts_with("DataLinkScanner")
        || slot.starts_with("CodexScanner")
        || slot.starts_with("DiscoveryScanner")
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_read_like_outfitting() {
        assert_eq!(
            item_name("int_hyperdrive_overcharge_size8_class5_overchargebooster_mkii"),
            "Frame Shift Drive (SCO) Mk II 8A"
        );
        assert_eq!(item_name("int_fuelscoop_size7_class5"), "Fuel Scoop 7A");
        assert_eq!(item_name("int_engine_size6_class2"), "Thrusters 6D");
        assert_eq!(
            item_name("explorer_nx_armour_grade1_default"),
            "Lightweight Alloy"
        );
        assert_eq!(
            item_name("int_guardianfsdbooster_size5"),
            "Guardian FSD Booster 5"
        );
        assert_eq!(
            item_name("hpt_beamlaser_gimbal_medium"),
            "Beam Laser (gimballed, medium)"
        );
        assert_eq!(
            item_name("int_dronecontrol_collection_size3_class5"),
            "Collector Limpet Controller 3A"
        );
        assert_eq!(slot_name("Slot01_Size7"), "Optional 1 (size 7)");
        assert_eq!(slot_name("TinyHardpoint2"), "Utility 2");
        assert_eq!(slot_name("MainEngines"), "Thrusters");
        assert!(is_cosmetic_slot("PaintJob"));
    }
}
