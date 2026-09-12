//! Ship and module discounts: the game's static rules, not prices.
//!
//! Outfitting and shipyard data carry no prices — the feed publishes bare
//! stock lists — so "where is this cheaper" cannot be answered by
//! comparison. It does not have to be: the discounts are fixed rules
//! published by Frontier, and every one of them keys off something EDDA
//! already knows (the controlling Power and its state, the station and
//! system name, the commander's own ranks).
//!
//! Source: the Elite Dangerous wiki's Active Discounts table
//! (<https://elite-dangerous.fandom.com/wiki/Discounts>), read 2026-09-12,
//! itself sourced from Frontier's forums and GalNet. Facts only — the
//! numbers and the places they apply — never the page's prose.
//!
//! Expired discounts are deliberately absent. A rule that stops applying
//! is removed here rather than commented out: a stale 17% on Federal hulls
//! would send a commander somewhere for a discount that is not there.

use serde::{Deserialize, Serialize};

/// What a rule discounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every ship and module in stock.
    Everything,
    /// These hulls, by journal ship symbol.
    Ships(&'static [&'static str]),
    /// Any weapon hardpoint (see [`is_weapon`]).
    Weapons,
    /// Modules whose symbol contains one of these stems.
    Modules(&'static [&'static str]),
}

/// Where a rule applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Systems held by a Power in one of these states.
    Power { power: &'static str, states: &'static [&'static str] },
    /// One station, named with its system so two "Jameson Memorial"s
    /// cannot be confused.
    Station { station: &'static str, system: &'static str },
    /// Every station in one system.
    System(&'static str),
    /// Everywhere, once the commander qualifies.
    Anywhere,
}

/// One published discount.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub percent: f32,
    pub scope: Scope,
    pub place: Place,
    /// Symbol fragments the rule does NOT cover, for the rules that carve
    /// an exception out of their own scope.
    pub excluding: &'static [&'static str],
    /// True when the discount needs Elite rank in any field.
    pub needs_elite: bool,
    /// Said to the commander: why this row is cheaper.
    pub why: &'static str,
}

/// A Power's control systems. The wiki distinguishes "controlled by" from
/// "exploited by"; in the journal's vocabulary control is Stronghold or
/// Fortified and exploitation is Exploited.
const CONTROLLED: &[&str] = &["Stronghold", "Fortified"];
const EXPLOITED: &[&str] = &["Exploited"];
const HELD: &[&str] = &["Stronghold", "Fortified", "Exploited"];

const IMPERIAL_HULLS: &[&str] = &["empire_eagle", "empire_courier", "empire_trader", "cutter"];
/// Lakon's range, including the Alliance hulls Lakon builds.
const LAKON_HULLS: &[&str] = &[
    "type6", "type7", "type8", "type9", "type9_military", "independant_trader", "lakonminer", "typex", "typex_2", "typex_3",
];

/// Every discount in force. Ordered as the wiki lists them.
pub const RULES: &[Rule] = &[
    Rule {
        percent: 2.5,
        scope: Scope::Everything,
        place: Place::Anywhere,
        excluding: &[],
        needs_elite: true,
        why: "Elite rank, galaxy-wide",
    },
    Rule {
        percent: 5.0,
        scope: Scope::Ships(LAKON_HULLS),
        place: Place::System("Alioth"),
        excluding: &[],
        needs_elite: false,
        why: "Lakon hulls in Alioth (permit)",
    },
    Rule {
        percent: 10.0,
        scope: Scope::Everything,
        place: Place::Station { station: "Jameson Memorial", system: "Shinrarta Dezhra" },
        excluding: &[],
        needs_elite: true,
        why: "Jameson Memorial (permit, Elite)",
    },
    Rule {
        percent: 10.0,
        scope: Scope::Ships(IMPERIAL_HULLS),
        place: Place::Power { power: "Denton Patreus", states: CONTROLLED },
        excluding: &[],
        needs_elite: false,
        why: "Imperial hulls in Patreus control space",
    },
    Rule {
        percent: 10.0,
        scope: Scope::Ships(IMPERIAL_HULLS),
        place: Place::Station { station: "Henry O'Hare's Hangar", system: "Summerland" },
        excluding: &[],
        needs_elite: false,
        why: "Henry O'Hare's Hangar (permit)",
    },
    Rule {
        percent: 10.0,
        scope: Scope::Ships(&["federation_dropship", "vulture"]),
        place: Place::Station { station: "Daedalus", system: "Sol" },
        excluding: &[],
        needs_elite: false,
        why: "Daedalus, Sol (permit)",
    },
    Rule {
        percent: 20.0,
        scope: Scope::Ships(&["eagle"]),
        place: Place::Station { station: "Daedalus", system: "Sol" },
        excluding: &[],
        needs_elite: false,
        why: "Daedalus, Sol (permit)",
    },
    Rule {
        percent: 10.0,
        scope: Scope::Ships(&["eagle"]),
        place: Place::Station { station: "Edmondson High", system: "Beta Hydri" },
        excluding: &[],
        needs_elite: false,
        why: "Edmondson High (permit)",
    },
    Rule {
        percent: 10.0,
        scope: Scope::Weapons,
        place: Place::Power { power: "Jerome Archer", states: EXPLOITED },
        excluding: &[],
        needs_elite: false,
        why: "weapons in Archer exploited space",
    },
    Rule {
        percent: 15.0,
        scope: Scope::Everything,
        place: Place::Power { power: "Li Yong-Rui", states: HELD },
        excluding: &[],
        needs_elite: false,
        why: "Li Yong-Rui space",
    },
    Rule {
        percent: 20.0,
        scope: Scope::Weapons,
        place: Place::Power { power: "Jerome Archer", states: CONTROLLED },
        excluding: &[],
        needs_elite: false,
        why: "weapons in Archer control space",
    },
    Rule {
        percent: 20.0,
        scope: Scope::Ships(&["asp", "orca"]),
        place: Place::Station { station: "Irkutsk", system: "Alioth" },
        excluding: &[],
        needs_elite: false,
        why: "Irkutsk, Alioth (permit)",
    },
    Rule {
        percent: 20.0,
        scope: Scope::Ships(&["diamondbackxl"]),
        place: Place::Station { station: "Hamilton Gateway", system: "Wolf 406" },
        excluding: &[],
        needs_elite: false,
        why: "Hamilton Gateway, Wolf 406",
    },
    Rule {
        percent: 20.0,
        scope: Scope::Modules(&["cargorack", "hullreinforcement"]),
        place: Place::Power { power: "Edmund Mahon", states: CONTROLLED },
        excluding: &[],
        needs_elite: false,
        why: "cargo racks and hull reinforcement in Mahon control space",
    },
    Rule {
        percent: 20.0,
        scope: Scope::Ships(&[
            "sidewinder", "eagle", "viper", "cobramkiii", "diamondback", "diamondbackxl", "vulture", "ferdelance",
        ]),
        place: Place::Station { station: "Attilius Orbital", system: "CD-43 11917" },
        excluding: &[],
        needs_elite: false,
        why: "Attilius Orbital (permit)",
    },
    Rule {
        percent: 30.0,
        scope: Scope::Modules(&[
            "pulselaser", "burstlaser", "beamlaser", "cannon", "multicannon", "railgun", "chafflauncher", "heatsinklauncher",
        ]),
        // "does not apply to any Large (Class 3 or higher) or Turret
        // modules" — weapon symbols carry their mount and size.
        place: Place::Station { station: "Attilius Orbital", system: "CD-43 11917" },
        excluding: &["_large", "_huge", "_turret"],
        needs_elite: false,
        why: "Attilius Orbital, small and medium fixed/gimballed (permit)",
    },
];

/// Utility hardpoints: mounted like weapons, not weapons.
const UTILITY_STEMS: &[&str] = &[
    "chafflauncher",
    "heatsinklauncher",
    "plasmapointdefence",
    "pointdefence",
    "electroniccountermeasure",
    "shieldbooster",
    "cloudscanner",
    "crimescanner",
    "cargoscanner",
    "mrascanner",
    "xenoscanner",
    "antiunknownshutdown",
    "causticsinklauncher",
    "shutdownfieldneutraliser",
];

/// Is this module symbol a weapon? Hardpoint-mounted and not one of the
/// utility mounts that share the `hpt_` prefix.
pub fn is_weapon(symbol: &str) -> bool {
    let s = symbol.to_ascii_lowercase();
    s.starts_with("hpt_") && !UTILITY_STEMS.iter().any(|u| s.contains(u))
}

/// What is being priced.
#[derive(Debug, Clone, Copy)]
pub enum Item<'a> {
    Ship(&'a str),
    Module(&'a str),
}

/// Where it is being priced, as a market row describes it.
#[derive(Debug, Clone, Copy, Default)]
pub struct At<'a> {
    pub station: &'a str,
    pub system: &'a str,
    pub power: Option<&'a str>,
    pub power_state: Option<&'a str>,
}

/// A discount that applies here, ready to show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Applied {
    pub percent: f32,
    pub why: String,
}

fn scope_covers(scope: &Scope, excluding: &[&str], item: Item<'_>) -> bool {
    let symbol = match item {
        Item::Ship(s) | Item::Module(s) => s.to_ascii_lowercase(),
    };
    if excluding.iter().any(|x| symbol.contains(x)) {
        return false;
    }
    match (scope, item) {
        (Scope::Everything, _) => true,
        (Scope::Ships(list), Item::Ship(_)) => list.iter().any(|s| *s == symbol),
        (Scope::Ships(_), Item::Module(_)) => false,
        (Scope::Weapons, Item::Module(s)) => is_weapon(s),
        (Scope::Weapons, Item::Ship(_)) => false,
        (Scope::Modules(stems), Item::Module(_)) => stems.iter().any(|stem| symbol.contains(stem)),
        (Scope::Modules(_), Item::Ship(_)) => false,
    }
}

fn place_covers(place: &Place, at: &At<'_>) -> bool {
    match place {
        Place::Anywhere => true,
        Place::System(system) => at.system.eq_ignore_ascii_case(system),
        Place::Station { station, system } => {
            at.station.eq_ignore_ascii_case(station) && at.system.eq_ignore_ascii_case(system)
        }
        Place::Power { power, states } => {
            at.power.is_some_and(|p| p.eq_ignore_ascii_case(power))
                && at.power_state.is_some_and(|s| states.iter().any(|want| s.eq_ignore_ascii_case(want)))
        }
    }
}

/// Every discount on `item` at `at`, best first. `elite` is whether the
/// commander holds Elite in any field; without it the rules that need it
/// are left out rather than promised.
pub fn discounts(item: Item<'_>, at: &At<'_>, elite: bool) -> Vec<Applied> {
    let mut out: Vec<Applied> = RULES
        .iter()
        .filter(|r| (elite || !r.needs_elite) && place_covers(&r.place, at) && scope_covers(&r.scope, r.excluding, item))
        .map(|r| Applied { percent: r.percent, why: r.why.to_string() })
        .collect();
    out.sort_by(|a, b| b.percent.total_cmp(&a.percent));
    out
}

/// What a commander actually pays off, as a percentage. The Elite 2.5%
/// stacks with one other discount; two place-based discounts do not stack,
/// so the best of them wins.
pub fn best_percent(applied: &[Applied]) -> f32 {
    let elite: f32 = applied.iter().find(|a| a.why.starts_with("Elite rank")).map_or(0.0, |a| a.percent);
    let other: f32 = applied.iter().filter(|a| !a.why.starts_with("Elite rank")).map(|a| a.percent).fold(0.0, f32::max);
    // Multiplicative: 10% off then 2.5% off the rest.
    let kept = (1.0 - elite / 100.0) * (1.0 - other / 100.0);
    ((1.0 - kept) * 1000.0).round() / 10.0
}

/// The Powers whose space discounts `item` at all, for narrowing a search
/// to discounted stations before the rows are ranked.
pub fn powers_offering(item: Item<'_>) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = RULES
        .iter()
        .filter(|r| scope_covers(&r.scope, r.excluding, item))
        .filter_map(|r| match r.place {
            Place::Power { power, .. } => Some(power),
            _ => None,
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at<'a>(station: &'a str, system: &'a str, power: Option<&'a str>, state: Option<&'a str>) -> At<'a> {
        At { station, system, power, power_state: state }
    }

    /// The Powerplay rules, which are the ones a search can act on across
    /// thousands of systems.
    #[test]
    fn power_space_discounts_what_the_wiki_says() {
        let lyr = at("Any Dock", "Somewhere", Some("Li Yong-Rui"), Some("Exploited"));
        assert_eq!(discounts(Item::Ship("anaconda"), &lyr, false)[0].percent, 15.0);
        assert_eq!(discounts(Item::Module("int_fuelscoop_size5_class5"), &lyr, false)[0].percent, 15.0);
        // ... in every state he holds, but not in a system he merely contests.
        let contested = at("Any Dock", "Somewhere", Some("Li Yong-Rui"), Some("Contested"));
        assert!(discounts(Item::Ship("anaconda"), &contested, false).is_empty());

        // Archer: weapons only, and twice as deep in control space.
        let archer_ctl = at("Any Dock", "Somewhere", Some("Jerome Archer"), Some("Stronghold"));
        let archer_exp = at("Any Dock", "Somewhere", Some("Jerome Archer"), Some("Exploited"));
        assert_eq!(discounts(Item::Module("hpt_beamlaser_gimbal_small"), &archer_ctl, false)[0].percent, 20.0);
        assert_eq!(discounts(Item::Module("hpt_beamlaser_gimbal_small"), &archer_exp, false)[0].percent, 10.0);
        assert!(discounts(Item::Module("hpt_shieldbooster_size0_class5"), &archer_ctl, false).is_empty(), "a shield booster is not a weapon");
        assert!(discounts(Item::Ship("anaconda"), &archer_ctl, false).is_empty(), "hulls are not discounted there");

        // Mahon: two module families, control space only.
        let mahon = at("Any Dock", "Somewhere", Some("Edmund Mahon"), Some("Fortified"));
        assert_eq!(discounts(Item::Module("int_cargorack_size6_class1"), &mahon, false)[0].percent, 20.0);
        assert!(discounts(Item::Module("int_fuelscoop_size5_class5"), &mahon, false).is_empty());
        let mahon_exp = at("Any Dock", "Somewhere", Some("Edmund Mahon"), Some("Exploited"));
        assert!(discounts(Item::Module("int_cargorack_size6_class1"), &mahon_exp, false).is_empty());

        // Patreus: the four Imperial hulls.
        let patreus = at("Any Dock", "Somewhere", Some("Denton Patreus"), Some("Stronghold"));
        assert_eq!(discounts(Item::Ship("cutter"), &patreus, false)[0].percent, 10.0);
        assert!(discounts(Item::Ship("federation_corvette"), &patreus, false).is_empty());
    }

    /// The fixed places, including the two that carve exceptions.
    #[test]
    fn station_and_system_discounts() {
        let jameson = at("Jameson Memorial", "Shinrarta Dezhra", None, None);
        assert!(discounts(Item::Ship("mandalay"), &jameson, false).is_empty(), "without Elite it is not offered");
        let with_elite = discounts(Item::Ship("mandalay"), &jameson, true);
        assert_eq!(with_elite[0].percent, 10.0);
        assert_eq!(with_elite[1].percent, 2.5, "the galaxy-wide Elite discount as well");

        // A whole system: Lakon hulls in Alioth, whichever station.
        let alioth = at("Golden Gate", "Alioth", None, None);
        assert_eq!(discounts(Item::Ship("type9"), &alioth, false)[0].percent, 5.0);
        assert_eq!(discounts(Item::Ship("typex"), &alioth, false)[0].percent, 5.0, "Alliance hulls are Lakon-built");
        assert!(discounts(Item::Ship("anaconda"), &alioth, false).is_empty());

        // One station, two rules, the deeper one first.
        let daedalus = at("Daedalus", "Sol", None, None);
        assert_eq!(discounts(Item::Ship("eagle"), &daedalus, false)[0].percent, 20.0);
        assert_eq!(discounts(Item::Ship("vulture"), &daedalus, false)[0].percent, 10.0);

        // Attilius: 30% on small and medium weapons, nothing on large or turrets.
        let attilius = at("Attilius Orbital", "CD-43 11917", None, None);
        assert_eq!(discounts(Item::Module("hpt_multicannon_fixed_medium"), &attilius, false)[0].percent, 30.0);
        assert!(discounts(Item::Module("hpt_multicannon_fixed_large"), &attilius, false).is_empty());
        assert!(discounts(Item::Module("hpt_multicannon_turret_small"), &attilius, false).is_empty());
    }

    /// Elite stacks with one place-based discount; two place discounts do not.
    #[test]
    fn what_the_commander_actually_pays() {
        let jameson = at("Jameson Memorial", "Shinrarta Dezhra", None, None);
        let applied = discounts(Item::Ship("mandalay"), &jameson, true);
        assert_eq!(best_percent(&applied), 12.3, "10% then 2.5% off the rest");
        let nothing: Vec<Applied> = Vec::new();
        assert_eq!(best_percent(&nothing), 0.0);
        let elite_only = discounts(Item::Ship("mandalay"), &at("Any", "Nowhere", None, None), true);
        assert_eq!(best_percent(&elite_only), 2.5);
    }

    /// The search narrows to these Powers when the commander asks for
    /// discounts only.
    #[test]
    fn powers_worth_searching() {
        assert_eq!(powers_offering(Item::Ship("cutter")), vec!["Denton Patreus", "Li Yong-Rui"]);
        assert_eq!(powers_offering(Item::Ship("anaconda")), vec!["Li Yong-Rui"]);
        assert_eq!(powers_offering(Item::Module("hpt_beamlaser_fixed_small")), vec!["Jerome Archer", "Li Yong-Rui"]);
        assert_eq!(powers_offering(Item::Module("int_cargorack_size6_class1")), vec!["Edmund Mahon", "Li Yong-Rui"]);
    }

    /// Weapons versus the utilities that share their mount.
    #[test]
    fn weapons_are_told_from_utilities() {
        assert!(is_weapon("hpt_multicannon_gimbal_large"));
        assert!(is_weapon("Hpt_Railgun_Fixed_Medium"));
        assert!(!is_weapon("hpt_shieldbooster_size0_class5"));
        assert!(!is_weapon("hpt_chafflauncher_tiny"));
        assert!(!is_weapon("hpt_cloudscanner_size0_class2"), "a wake scanner is a utility");
        assert!(!is_weapon("int_fuelscoop_size5_class5"));
    }
}
