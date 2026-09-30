//! Display names for Loadout module items and slots.
//!
//! The journal names modules by internal symbol (`int_fuelscoop_size7_class5`,
//! `hpt_beamlaser_gimbal_medium`) and slots by internal id (`Slot01_Size7`,
//! `MainEngines`). These are what the game shows in outfitting, or as close
//! as the symbol allows: "Fuel Scoop 7A", "Beam Laser 2E (gimballed)",
//! "Optional 1 (size 7)", "Thrusters".

/// The outfitting name for a module symbol.
/// A commander's words for a module -> a fragment of its journal symbol,
/// for the symbol-only market tables (maintainer, 2026-09-12: the Market
/// tab's module search only answered when the typed text happened to be a
/// symbol fragment). "fuel scoop" -> `fuelscoop`, "5A fuel scoop" ->
/// `fuelscoop_size5_class5`, "beam laser" -> `beamlaser`. Words the symbol
/// spells differently are mapped first; everything else just loses its
/// spaces, which is what journal symbols do. Returns None when there is
/// nothing to search on.
fn rating_class(rating: u8) -> Option<u32> {
    match rating.to_ascii_lowercase() {
        b'a' => Some(5),
        b'b' => Some(4),
        b'c' => Some(3),
        b'd' => Some(2),
        b'e' => Some(1),
        _ => None,
    }
}

/// A size and rating split off the words that name the module: the slot
/// size, the class digit (A is the best of five) and the remaining
/// alphanumeric words, lower-cased. Leading as commanders say it ("5A
/// fuel scoop", "3 d fuel scoop", "6 fuel scoop") or trailing as the
/// game's outfitting screen prints it ("Bi-Weave Shield Generator 5C",
/// "shield generator 5 a").
fn size_rating_words(text: &str) -> (Option<u32>, Option<u32>, Vec<String>) {
    let lower = text.trim().to_ascii_lowercase();
    let mut size: Option<u32> = None;
    let mut class: Option<u32> = None;
    let mut words: Vec<String> = Vec::new();
    let mut tokens: Vec<&str> = lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
    // Trailing "5c" / "5 c" / "5" after at least one word.
    let is_size_token = |w: &str| {
        let b = w.as_bytes();
        (b.len() == 1 || b.len() == 2 && rating_class(b[1]).is_some()) && b[0].is_ascii_digit() && (1..=8).contains(&((b[0] - b'0') as u32))
    };
    if tokens.len() >= 2 {
        let last = tokens[tokens.len() - 1];
        let before = tokens[tokens.len() - 2];
        if last.len() == 1 && rating_class(last.as_bytes()[0]).is_some() && tokens.len() >= 3 && before.len() == 1 && is_size_token(before) {
            size = Some((before.as_bytes()[0] - b'0') as u32);
            class = rating_class(last.as_bytes()[0]);
            tokens.truncate(tokens.len() - 2);
        } else if is_size_token(last) && !tokens[..tokens.len() - 1].iter().all(|w| is_size_token(w)) {
            let b = last.as_bytes();
            size = Some((b[0] - b'0') as u32);
            class = b.get(1).and_then(|r| rating_class(*r));
            tokens.truncate(tokens.len() - 1);
        }
    }
    for w in tokens {
        let b = w.as_bytes();
        // A rating written apart from the size ("3 D fuel scoop").
        if size.is_some() && class.is_none() && words.is_empty() && b.len() == 1 {
            if let Some(c) = rating_class(b[0]) {
                class = Some(c);
                continue;
            }
        }
        let head_size = b[0].is_ascii_digit() && (1..=8).contains(&((b[0] - b'0') as u32));
        if size.is_none() && words.is_empty() && head_size {
            size = Some((b[0] - b'0') as u32);
            let rating = if b.len() == 2 { Some(b[1]) } else { None };
            class = rating.and_then(|r| rating_class(r));
            if b.len() <= 2 {
                continue;
            }
        }
        words.push(w.to_string());
    }
    (size, class, words)
}

/// The rating letter the outfitting table prints for a class digit.
fn class_rating(class: u32) -> Option<char> {
    match class {
        5 => Some('A'),
        4 => Some('B'),
        3 => Some('C'),
        2 => Some('D'),
        1 => Some('E'),
        _ => None,
    }
}

/// A name with everything but letters and digits removed, lower-cased:
/// "Bi-Weave Shield Generator" and "bi weave shield generator" agree.
fn squash(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// What commanders call a module when it is not what EDCD calls it,
/// squashed on both sides. Each target is an exact EDCD name from the
/// vendored outfitting table (checked by `aliases_name_real_modules`).
const NAME_ALIASES: &[(&str, &str)] = &[
    ("afmu", "Auto Field-Maintenance Unit"),
    ("autofieldmaintenance", "Auto Field-Maintenance Unit"),
    ("srvhangar", "Planetary Vehicle Hangar"),
    ("vehiclehangar", "Planetary Vehicle Hangar"),
    ("scb", "Shield Cell Bank"),
    ("shieldcell", "Shield Cell Bank"),
    ("hrp", "Hull Reinforcement Package"),
    ("hullreinforcement", "Hull Reinforcement Package"),
    ("mrp", "Module Reinforcement Package"),
    ("modulereinforcement", "Module Reinforcement Package"),
    ("fsd", "Frame Shift Drive"),
    ("framshiftdrive", "Frame Shift Drive"),
    ("fsdsco", "Frame Shift Drive (SCO)"),
    ("scodrive", "Frame Shift Drive (SCO)"),
    ("scofsd", "Frame Shift Drive (SCO)"),
    ("sco", "Frame Shift Drive (SCO)"),
    ("fsdbooster", "Guardian FSD Booster"),
    ("interdictor", "Frame Shift Drive Interdictor"),
    ("fsdinterdictor", "Frame Shift Drive Interdictor"),
    ("wakescanner", "Frame Shift Wake Scanner"),
    ("kws", "Kill Warrant Scanner"),
    ("dss", "Detailed Surface Scanner"),
    ("ecm", "Electronic Countermeasure"),
    ("biweave", "Bi-Weave Shield Generator"),
    ("biweaveshield", "Bi-Weave Shield Generator"),
    ("prismatic", "Prismatic Shield Generator"),
    ("prismaticshield", "Prismatic Shield Generator"),
    ("fragcannon", "Fragment Cannon"),
    ("frag", "Fragment Cannon"),
    ("pa", "Plasma Accelerator"),
    ("plasma", "Plasma Accelerator"),
    ("heatsink", "Heat Sink Launcher"),
    ("heatsinks", "Heat Sink Launcher"),
    ("chaff", "Chaff Launcher"),
    ("pdt", "Point Defence"),
    ("pointdefense", "Point Defence"),
    ("distributor", "Power Distributor"),
    ("pd", "Power Distributor"),
    ("gauss", "Guardian Gauss Cannon"),
    ("shards", "Guardian Shard Cannon"),
    ("shardcannon", "Guardian Shard Cannon"),
    ("plasmacharger", "Guardian Plasma Charger"),
];

/// The outfitting symbols a commander means by `text`, from the vendored
/// outfitting table alone: the words name a module (EDCD's name, an alias
/// commanders use, or a symbol typed straight in) and a leading size or
/// size+rating narrows it ("5A bi-weave" is one symbol, "bi-weave" is
/// eight). An exact name wins over names that merely contain the words
/// ("shield generator" is not also the Bi-Weave and the Prismatic);
/// "docking computer" reaches both computers. Empty when the table has
/// no such module — the server then still gets the words to try
/// (maintainer, 2026-09-29: a hand-built stem `biweaveshieldgenerator`
/// went up, matched no symbol, and the search answered nothing after
/// seven seconds).
pub fn resolve_search(text: &str) -> Vec<String> {
    let (size, class, words) = size_rating_words(text);
    let joined = words.join("");
    if joined.is_empty() {
        return Vec::new();
    }
    let table = outfitting_table();
    // A symbol typed straight in, bare or wrapped.
    let typed_symbol = bare_symbol(text);
    if table.contains_key(&typed_symbol) {
        return vec![typed_symbol];
    }
    let want = NAME_ALIASES
        .iter()
        .find(|(alias, _)| *alias == joined)
        .map(|(_, name)| squash(name))
        .unwrap_or(joined);
    let matches = |exact: bool| -> Vec<String> {
        let mut out: Vec<String> = table
            .iter()
            .filter(|(_, row)| {
                let name = squash(row.name);
                if exact { name == want } else { name.contains(&want) }
            })
            .filter(|(_, row)| size.is_none_or(|n| row.class == n.to_string()))
            .filter(|(_, row)| class.and_then(class_rating).is_none_or(|r| row.rating.eq_ignore_ascii_case(&r.to_string())))
            .map(|(symbol, _)| symbol.clone())
            .collect();
        out.sort_unstable();
        out
    };
    let exact = matches(true);
    if !exact.is_empty() {
        return exact;
    }
    let containing = matches(false);
    if !containing.is_empty() || class.is_none() {
        return containing;
    }
    // "5A bi-weave": Bi-Weaves only come in C. The size is the slot and
    // stays; the rating that no module of that name has is dropped.
    resolve_search(&format!("{} {}", size.map(|n| n.to_string()).unwrap_or_default(), words.join(" ")))
}

pub fn search_fragment(text: &str) -> Option<String> {
    let (size, class, words) = size_rating_words(text);
    let joined = words.join("");
    let stem = match joined.as_str() {
        "" => return None,
        // Symbols that do not read like their names.
        "afmu" | "autofieldmaintenanceunit" | "autofieldmaintenance" => "repairer".to_string(),
        "srvhangar" | "planetaryvehiclehangar" | "vehiclehangar" => "buggybay".to_string(),
        "shieldcellbank" | "scb" | "shieldcell" => "shieldcellbank".to_string(),
        "hullreinforcement" | "hrp" => "hullreinforcement".to_string(),
        "modulereinforcement" | "mrp" => "modulereinforcement".to_string(),
        "fsdbooster" | "guardianfsdbooster" => "guardianfsdbooster".to_string(),
        "fsd" | "framshiftdrive" | "frameshiftdrive" => "hyperdrive".to_string(),
        "detailedsurfacescanner" | "dss" => "detailedsurfacescanner".to_string(),
        "fss" | "fullspectrumscanner" => "fullspectrumscanner".to_string(),
        other => other.to_string(),
    };
    let mut out = stem;
    if let Some(n) = size {
        out.push_str(&format!("_size{n}"));
        if let Some(c) = class {
            out.push_str(&format!("_class{c}"));
        }
    }
    Some(out)
}

#[cfg(test)]
mod search_fragment_tests {
    use super::search_fragment;
    /// The shapes the Market tab's placeholder promises, plus the ones
    /// whose symbol spells the thing differently.
    #[test]
    fn words_become_a_symbol_fragment() {
        assert_eq!(search_fragment("fuel scoop").as_deref(), Some("fuelscoop"));
        assert_eq!(search_fragment("5A fuel scoop").as_deref(), Some("fuelscoop_size5_class5"));
        assert_eq!(search_fragment("5a fuelscoop").as_deref(), Some("fuelscoop_size5_class5"));
        assert_eq!(search_fragment("3 D fuel scoop").as_deref(), Some("fuelscoop_size3_class2"));
        assert_eq!(search_fragment("6 fuel scoop").as_deref(), Some("fuelscoop_size6"));
        assert_eq!(search_fragment("beam laser").as_deref(), Some("beamlaser"));
        assert_eq!(search_fragment("AFMU").as_deref(), Some("repairer"));
        assert_eq!(search_fragment("SRV hangar").as_deref(), Some("buggybay"));
        assert_eq!(search_fragment("guardian fsd booster").as_deref(), Some("guardianfsdbooster"));
        assert_eq!(search_fragment("FSD").as_deref(), Some("hyperdrive"));
        // A symbol typed straight in survives unchanged apart from case.
        assert_eq!(search_fragment("int_fuelscoop_size5_class5").as_deref(), Some("intfuelscoopsize5class5"));
        assert_eq!(search_fragment("   ").as_deref(), None);
    }
}

/// EDCD's outfitting table (FDevIDs `outfitting.csv`, vendored): the FIRST
/// row per symbol (rows are in id order, so the base module precedes the
/// pre-engineered and Powerplay variants that share its symbol — a Loadout
/// cannot tell those apart). The `entitlement` column is NOT a filter:
/// Guardian, AX and Odyssey-era modules carry one on their base row, and
/// skipping them sent the maintainer's Guardian weapons to the hand table
/// as "Guardian Weapon" (2026-09-20).
/// Frontier's own `*_Localised` strings agree with it on every one of the
/// 156 modules in the maintainer's journal (2026-09-20), so it is the
/// printed name; the hand table below is only the fallback for a symbol
/// newer than the table.
struct OutfittingRow {
    name: &'static str,
    mount: &'static str,
    class: &'static str,
    rating: &'static str,
}

fn outfitting_table() -> &'static std::collections::HashMap<String, OutfittingRow> {
    static TABLE: std::sync::OnceLock<std::collections::HashMap<String, OutfittingRow>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let csv = include_str!("../data/outfitting.csv");
        let mut map = std::collections::HashMap::new();
        for line in csv.lines().skip(1) {
            // id,symbol,category,name,mount,guidance,ship,class,rating,entitlement
            let cols: Vec<&str> = line.split(',').collect();
            if cols.len() < 10 {
                continue;
            }
            map.entry(cols[1].trim().to_ascii_lowercase()).or_insert(OutfittingRow {
                name: cols[3].trim(),
                mount: cols[4].trim(),
                class: cols[7].trim(),
                rating: cols[8].trim(),
            });
        }
        map
    })
}

/// EDCD's name for an outfitting row by FDev id — the only way to name a
/// pre-engineered variant, which shares its symbol with the plain module
/// (129044376 is "Balanced Power Distributor"; its symbol prints as
/// "Power Distributor 5A"). None for an id the table does not have.
pub fn variant_name(fdev_id: i64) -> Option<&'static str> {
    static BY_ID: std::sync::OnceLock<std::collections::HashMap<i64, &'static str>> = std::sync::OnceLock::new();
    BY_ID
        .get_or_init(|| {
            include_str!("../data/outfitting.csv")
                .lines()
                .skip(1)
                .filter_map(|line| {
                    let cols: Vec<&str> = line.split(',').collect();
                    Some((cols.first()?.trim().parse().ok()?, *cols.get(3)?))
                })
                .collect()
        })
        .get(&fdev_id)
        .copied()
}

/// The merc-coin pre-engineered variants sold under a plain module's
/// symbol, by FDev id and EDCD name (the `mercgear` category rows of the
/// vendored outfitting table: Balanced Power Distributor under
/// `int_powerdistributor_size5_class5`). Empty for a symbol with none.
/// Measured 2026-09-30: every outfitting/3 board on the relay carried the
/// same merc set, so on a board without prices a twin symbol is always
/// present and says nothing about a credit sale of the plain module.
pub fn merc_variants_of(symbol: &str) -> Vec<(i64, &'static str)> {
    static TWINS: std::sync::OnceLock<std::collections::HashMap<String, Vec<(i64, &'static str)>>> = std::sync::OnceLock::new();
    TWINS
        .get_or_init(|| {
            let mut map: std::collections::HashMap<String, Vec<(i64, &'static str)>> = std::collections::HashMap::new();
            for line in include_str!("../data/outfitting.csv").lines().skip(1) {
                let cols: Vec<&str> = line.split(',').collect();
                if cols.len() < 4 || cols[2].trim() != "mercgear" {
                    continue;
                }
                if let Ok(id) = cols[0].trim().parse::<i64>() {
                    map.entry(cols[1].trim().to_ascii_lowercase()).or_default().push((id, cols[3].trim()));
                }
            }
            map
        })
        .get(&bare_symbol(symbol))
        .cloned()
        .unwrap_or_default()
}

/// A journal item symbol, bare or wrapped (`$hpt_pulselaser_fixed_small_name;`).
fn bare_symbol(symbol: &str) -> String {
    let s = symbol.trim().to_ascii_lowercase();
    s.strip_prefix('$').and_then(|r| r.strip_suffix("_name;")).map(str::to_string).unwrap_or(s)
}

/// The outfitting name for a journal item symbol: EDCD's name, then the
/// class and rating as the game's outfitting screen shows them ("Fuel
/// Scoop 7A"; a weapon "Pulse Laser 1F (fixed)", a utility "Heat Sink
/// Launcher 0I"). A weapon used to print its mount and size as words
/// with no class or rating, so a 3C shard and a 3D shard read alike
/// (maintainer, 2026-09-27: "we aren't showing the size/grade next to
/// the guardian modules").
pub fn item_name(symbol: &str) -> String {
    let s = bare_symbol(symbol);
    let Some(row) = outfitting_table().get(&s) else { return item_name_fallback(&s) };
    if s.contains("_armour_") || s.contains("_cockpit") || row.class.is_empty() {
        return row.name.to_string();
    }
    let mount = match row.mount {
        "Fixed" => " (fixed)",
        "Gimballed" => " (gimballed)",
        "Turreted" => " (turreted)",
        _ => "",
    };
    format!("{} {}{}{mount}", row.name, row.class, row.rating)
}

/// Module names that contain `text`, for a search box: EDCD's distinct
/// names from the bundled outfitting table, alphabetical, at most
/// `limit`. Typing "heat" offers "Heat Sink Launcher" (maintainer,
/// 2026-09-29: the Market tab's outfitting box completed nothing).
pub fn complete_modules(text: &str, limit: usize) -> Vec<String> {
    // "5A bi" completes to "5A Bi-Weave Shield Generator": the size and
    // rating the commander typed ride along on every offer (maintainer,
    // 2026-09-29: "why can't I search for module sizes along with the
    // name?" — the box matched "5a bi" against names and offered nothing).
    let (size, class, words) = size_rating_words(text);
    let want = words.join("");
    if want.is_empty() {
        return Vec::new();
    }
    let prefix = match (size, class.and_then(class_rating)) {
        (Some(n), Some(r)) => format!("{n}{r} "),
        (Some(n), None) => format!("{n} "),
        _ => String::new(),
    };
    let table = outfitting_table();
    // The name is complete: offer its sizes and ratings as the game
    // prints them ("Bi-Weave Shield Generator 5C"), so the size is a
    // choice on screen and not a convention to know (maintainer,
    // 2026-09-29: "why can't I search module size?"). With a size typed,
    // that size's ratings.
    {
        let rating = class.and_then(class_rating);
        let mut variants: Vec<(u32, String, String)> = table
            .values()
            .filter(|r| squash(r.name) == want && !r.class.is_empty() && !r.rating.is_empty())
            .filter(|r| size.is_none_or(|n| r.class == n.to_string()))
            .filter(|r| rating.is_none_or(|x| r.rating.eq_ignore_ascii_case(&x.to_string())))
            .map(|r| (r.class.parse::<u32>().unwrap_or(0), r.rating.to_string(), r.name.to_string()))
            .collect();
        variants.sort_unstable();
        variants.dedup();
        if !variants.is_empty() {
            return variants.into_iter().take(limit).map(|(c, r, n)| format!("{n} {c}{r}")).collect();
        }
    }
    let mut names: Vec<&str> = table
        .iter()
        .filter(|(_, r)| size.is_none_or(|n| r.class == n.to_string()))
        .map(|(_, r)| r.name)
        .filter(|n| squash(n).contains(&want))
        .collect();
    names.sort_unstable();
    names.dedup();
    // Names that START with the text first, the rest after.
    names.sort_by_key(|n| !squash(n).starts_with(&want));
    names.into_iter().take(limit).map(|n| format!("{prefix}{n}")).collect()
}

#[cfg(test)]
mod resolve_search_tests {
    use super::{complete_modules, outfitting_table, resolve_search, squash, NAME_ALIASES};

    #[test]
    fn a_symbols_merc_twins_are_known() {
        let pd = super::merc_variants_of("int_powerdistributor_size5_class5");
        assert_eq!(pd, vec![(129044376, "Balanced Power Distributor")]);
        let racks = super::merc_variants_of("Hpt_BasicMissileRack_Fixed_Medium");
        assert_eq!(racks.len(), 3, "{racks:?}");
        assert!(super::merc_variants_of("int_fuelscoop_size5_class5").is_empty());
        let twins = outfitting_table().keys().filter(|s| !super::merc_variants_of(s).is_empty()).count();
        assert_eq!(twins, 22, "the 2026-09-29 count of symbols with a merc twin");
    }

    #[test]
    fn a_variant_is_named_by_its_id() {
        assert_eq!(super::variant_name(129044376), Some("Balanced Power Distributor"));
        assert_eq!(super::variant_name(128064202), Some("Power Distributor"));
        assert_eq!(super::variant_name(1), None);
    }

    /// Every alias points at a name the vendored table really has.
    #[test]
    fn aliases_name_real_modules() {
        let names: std::collections::HashSet<String> = outfitting_table().values().map(|r| squash(r.name)).collect();
        for (alias, name) in NAME_ALIASES {
            assert!(names.contains(&squash(name)), "alias {alias:?} -> {name:?} is not in outfitting.csv");
        }
    }

    /// The search that failed on 2026-09-29, and the shapes around it.
    #[test]
    fn words_become_exact_symbols() {
        let biweave = resolve_search("Bi-Weave Shield Generator");
        assert_eq!(biweave.len(), 8, "{biweave:?}");
        assert!(biweave.iter().all(|s| s.starts_with("int_shieldgenerator_size") && s.ends_with("_class3_fast")), "{biweave:?}");
        assert_eq!(resolve_search("5C bi-weave"), vec!["int_shieldgenerator_size5_class3_fast"]);
        assert_eq!(resolve_search("5 biweave"), vec!["int_shieldgenerator_size5_class3_fast"]);
        // Trailing, as the outfitting screen prints it.
        assert_eq!(resolve_search("Bi-Weave Shield Generator 5C"), vec!["int_shieldgenerator_size5_class3_fast"]);
        assert_eq!(resolve_search("bi-weave 5"), vec!["int_shieldgenerator_size5_class3_fast"]);
        assert_eq!(resolve_search("shield generator 5 a"), vec!["int_shieldgenerator_size5_class5"]);
        assert_eq!(resolve_search("fuel scoop 7a"), vec!["int_fuelscoop_size7_class5"]);
        // A rating the family never comes in keeps the size, drops the rating.
        assert_eq!(resolve_search("5A bi-weave"), vec!["int_shieldgenerator_size5_class3_fast"]);
        assert_eq!(resolve_search("bi-weave 5A"), vec!["int_shieldgenerator_size5_class3_fast"]);
        assert_eq!(resolve_search("5A fuel scoop"), vec!["int_fuelscoop_size5_class5"]);
        assert_eq!(resolve_search("3 D fuel scoop"), vec!["int_fuelscoop_size3_class2"]);
        // An exact name does not drag in the names that contain it.
        let plain = resolve_search("shield generator");
        assert!(plain.iter().all(|s| !s.ends_with("_fast") && !s.ends_with("_strong")), "{plain:?}");
        assert!(!plain.is_empty());
        // Words that several names contain reach all of them.
        let dc = resolve_search("docking computer");
        assert!(dc.iter().any(|s| s.contains("dockingcomputer_advanced")) && dc.iter().any(|s| s.contains("dockingcomputer_standard")), "{dc:?}");
        // Aliases.
        assert_eq!(resolve_search("5A AFMU"), vec!["int_repairer_size5_class5"]);
        assert_eq!(resolve_search("6A FSD"), vec!["int_hyperdrive_size6_class5"]);
        assert_eq!(resolve_search("5A SCB"), vec!["int_shieldcellbank_size5_class5"]);
        assert!(resolve_search("heat sink").iter().all(|s| s.starts_with("hpt_heatsinklauncher")));
        // A symbol typed straight in.
        assert_eq!(resolve_search("Int_FuelScoop_Size5_Class5"), vec!["int_fuelscoop_size5_class5"]);
        assert_eq!(resolve_search("$int_fuelscoop_size5_class5_name;"), vec!["int_fuelscoop_size5_class5"]);
        // Nothing the table knows: empty, so the words go up as they are.
        assert!(resolve_search("thargoid toaster").is_empty());
        assert!(resolve_search("   ").is_empty());
    }

    /// The size prefix rides along on the completions.
    #[test]
    fn completion_keeps_the_typed_size() {
        let hits = complete_modules("5A bi", 12);
        assert_eq!(hits[0], "5A Bi-Weave Shield Generator", "{hits:?}");
        assert!(hits.iter().all(|h| h.starts_with("5A ")), "{hits:?}");
        let hits = complete_modules("heat", 12);
        assert_eq!(hits, vec!["Heat Sink Launcher"]);
        // The name complete: its sizes, as the game prints them.
        let hits = complete_modules("Bi-Weave Shield Generator", 12);
        assert_eq!(hits.len(), 8, "{hits:?}");
        assert_eq!(hits[0], "Bi-Weave Shield Generator 1C");
        assert_eq!(hits[7], "Bi-Weave Shield Generator 8C");
        let hits = complete_modules("Fuel Scoop 5", 12);
        assert_eq!(hits, vec!["Fuel Scoop 5A", "Fuel Scoop 5B", "Fuel Scoop 5C", "Fuel Scoop 5D", "Fuel Scoop 5E"], "{hits:?}");
        assert_eq!(complete_modules("Fuel Scoop 5A", 12), vec!["Fuel Scoop 5A"]);
        // A size no module of that name comes in offers nothing.
        assert!(complete_modules("8A heat", 12).is_empty());
        assert!(complete_modules("5", 12).is_empty());
    }
}

/// A module as a technology broker's recipe names it: a weapon by its
/// mount and size word ("Guardian Shard Cannon (Fixed, Large)", "Remote
/// Release Flechette Launcher (Fixed)"), anything else as `item_name`.
pub fn recipe_name(symbol: &str) -> String {
    let s = bare_symbol(symbol);
    let Some(row) = outfitting_table().get(&s) else { return item_name_fallback(&s) };
    if !s.starts_with("hpt_") || row.mount.is_empty() {
        return item_name(symbol);
    }
    let size = match row.class {
        "1" => Some("Small"),
        "2" => Some("Medium"),
        "3" => Some("Large"),
        "4" => Some("Huge"),
        _ => None,
    };
    match size {
        Some(size) => format!("{} ({}, {size})", row.name, row.mount),
        None => format!("{} ({})", row.name, row.mount),
    }
}

fn item_name_fallback(symbol: &str) -> String {
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
        // The Rhino SRV's hangars (September 2026 mining update; EDCD FDevIDs
        // #113): int_largebuggybay_* and int_mkiilargebuggybay_*, with a
        // `_free` variant for the early-access purchase.
        "largebuggybay" => "Large Planetary Vehicle Hangar".into(),
        "mkiilargebuggybay" => "Mk II Large Planetary Vehicle Hangar".into(),
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
        ("SmallMiningHardpoint", "Small mining hardpoint"),
        ("MediumMiningHardpoint", "Medium mining hardpoint"),
        ("LargeMiningHardpoint", "Large mining hardpoint"),
        ("SmallHardpoint", "Small hardpoint"),
        ("MediumHardpoint", "Medium hardpoint"),
        ("LargeHardpoint", "Large hardpoint"),
        ("HugeHardpoint", "Huge hardpoint"),
        ("Military", "Military"),
        // The Panther's cargo slots, the Caspian's cabins, the Type-11's limpet and hangar slots.
        ("Cargo", "Cargo"),
        ("Passenger", "Passenger cabin"),
        ("LimpetController", "Limpet controller"),
        ("FighterBay", "Fighter hangar"),
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
            // EDCD's outfitting name, as the outfitting screen prints it.
            "Mk II Supercharge Optimised Frame Shift Drive (SCO) 8A"
        );
        assert_eq!(item_name("int_fuelscoop_size7_class5"), "Fuel Scoop 7A");
        assert_eq!(item_name("int_engine_size6_class2"), "Thrusters 6D");
        assert_eq!(
            item_name("explorer_nx_armour_grade1_default"),
            "Lightweight Alloy"
        );
        assert_eq!(
            item_name("int_guardianfsdbooster_size5"),
            "Guardian FSD Booster 5H"
        );
        // The Rhino's hangars, as the maintainer's Type-11 carries one (2026-09-18).
        assert_eq!(item_name("int_mkiilargebuggybay_size4_class3_free"), "Mk II Large Planetary Vehicle Hangar 4F");
        assert_eq!(item_name("int_largebuggybay_size6_class3"), "Large Planetary Vehicle Hangar 6F");
        assert_eq!(item_name("int_buggybay_size2_class2"), "Planetary Vehicle Hangar 2G");
        assert_eq!(item_name("hpt_beamlaser_gimbal_medium"), "Beam Laser 2D (gimballed)");
        assert_eq!(item_name("hpt_guardian_shardcannon_fixed_large"), "Guardian Shard Cannon 3C (fixed)");
        assert_eq!(item_name("hpt_heatsinklauncher_turret_tiny"), "Heat Sink Launcher 0I");
        assert_eq!(recipe_name("hpt_guardian_shardcannon_fixed_large"), "Guardian Shard Cannon (Fixed, Large)");
        assert_eq!(recipe_name("hpt_flechettelauncher_fixed_medium"), "Remote Release Flechette Launcher (Fixed, Medium)");
        assert_eq!(recipe_name("int_guardianfsdbooster_size5"), item_name("int_guardianfsdbooster_size5"));
        let heat = complete_modules("heat", 10);
        assert!(heat.iter().any(|n| n == "Heat Sink Launcher"), "{heat:?}");
        assert!(heat.iter().all(|n| n.to_ascii_lowercase().contains("heat")), "{heat:?}");
        assert!(complete_modules("", 10).is_empty());
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
