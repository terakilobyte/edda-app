//! Ship symbol -> display name. The journal only adds `*_Localised` for
//! ships whose display name is not just the capitalised symbol, so plain
//! ones (`eagle`, `vulture`) arrive raw. Unknown symbols are title-cased
//! rather than guessed.

/// Every ship the journal names, symbol first. The single source for
/// [`display_name`], [`resolve`] and [`complete`], so a hull added here is
/// searchable and completable the same day it is nameable.
pub const SHIPS: &[(&str, &str)] = &[
    ("adder", "Adder"),
    ("anaconda", "Anaconda"),
    ("asp", "Asp Explorer"),
    ("asp_scout", "Asp Scout"),
    ("belugaliner", "Beluga Liner"),
    ("cobramkiii", "Cobra Mk III"),
    ("cobramkiv", "Cobra Mk IV"),
    ("cobramkv", "Cobra Mk V"),
    ("corsair", "Corsair"),
    ("cutter", "Imperial Cutter"),
    ("diamondback", "Diamondback Scout"),
    ("diamondbackxl", "Diamondback Explorer"),
    ("dolphin", "Dolphin"),
    ("eagle", "Eagle"),
    ("empire_courier", "Imperial Courier"),
    ("empire_eagle", "Imperial Eagle"),
    ("empire_trader", "Imperial Clipper"),
    ("explorer_nx", "Caspian Explorer"),
    ("federation_corvette", "Federal Corvette"),
    ("federation_dropship", "Federal Dropship"),
    ("federation_dropship_mkii", "Federal Assault Ship"),
    ("federation_gunship", "Federal Gunship"),
    ("ferdelance", "Fer-de-Lance"),
    ("hauler", "Hauler"),
    ("independant_trader", "Keelback"),
    ("krait_light", "Krait Phantom"),
    ("krait_mkii", "Krait Mk II"),
    ("lakonminer", "Type-11 Prospector"),
    ("mamba", "Mamba"),
    ("mandalay", "Mandalay"),
    ("orca", "Orca"),
    ("panthermkii", "Panther Clipper Mk II"),
    ("python", "Python"),
    ("python_nx", "Python Mk II"),
    ("sidewinder", "Sidewinder"),
    ("smallcombat01_nx", "Kestrel Mk II"),
    ("testbuggy", "SRV Scarab"),
    ("type6", "Type-6 Transporter"),
    ("type7", "Type-7 Transporter"),
    ("type8", "Type-8 Transporter"),
    ("type9", "Type-9 Heavy"),
    ("type9_military", "Type-10 Defender"),
    ("typex", "Alliance Chieftain"),
    ("typex_2", "Alliance Crusader"),
    ("typex_3", "Alliance Challenger"),
    ("viper", "Viper Mk III"),
    ("viper_mkiv", "Viper Mk IV"),
    ("vulture", "Vulture"),
];

/// Letters and digits only, lowercased: the shape both sides of a name
/// comparison are reduced to, so "Type-10 Defender", "type 10 defender"
/// and "Type10Defender" are one string.
fn flat(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

/// A commander's words for a hull -> its journal symbol. The market and
/// shipyard tables carry symbols only (maintainer, 2026-09-12: "I can't
/// find a Type-10 Defender" — the search matched the typed text against
/// `type9_military` and found nothing), so a display name has to become a
/// symbol before it goes on the wire. Matches a display name, a symbol, or
/// either with the punctuation and spacing rubbed out; falls back to a
/// unique containment hit ("defender", "chieftain") and refuses an
/// ambiguous one ("type" matches nine hulls).
pub fn resolve(text: &str) -> Option<&'static str> {
    let q = flat(text);
    if q.is_empty() {
        return None;
    }
    if let Some((sym, _)) = SHIPS.iter().find(|(sym, name)| flat(sym) == q || flat(name) == q) {
        return Some(sym);
    }
    let mut hits = SHIPS.iter().filter(|(sym, name)| flat(sym).contains(&q) || flat(name).contains(&q));
    match (hits.next(), hits.next()) {
        (Some((sym, _)), None) => Some(sym),
        _ => None,
    }
}

/// Display names matching `prefix` for a completion list, alphabetical.
/// Matches anywhere in the name or symbol, punctuation-insensitively, so
/// "10" finds the Type-10 and "krait" finds both Kraits.
pub fn complete(prefix: &str, limit: usize) -> Vec<&'static str> {
    let q = flat(prefix);
    let mut out: Vec<&'static str> = SHIPS
        .iter()
        .filter(|(sym, name)| q.is_empty() || flat(name).contains(&q) || flat(sym).contains(&q))
        .map(|(_, name)| *name)
        .collect();
    out.sort_unstable();
    out.truncate(limit);
    out
}

pub fn display_name(symbol: &str) -> String {
    let s = symbol.trim().to_ascii_lowercase();
    let known = match s.as_str() {
        "adder" => "Adder",
        "anaconda" => "Anaconda",
        "asp" => "Asp Explorer",
        "asp_scout" => "Asp Scout",
        "belugaliner" => "Beluga Liner",
        "cobramkiii" => "Cobra Mk III",
        "cobramkiv" => "Cobra Mk IV",
        "cobramkv" => "Cobra Mk V",
        "corsair" => "Corsair",
        "cutter" => "Imperial Cutter",
        "diamondback" => "Diamondback Scout",
        "diamondbackxl" => "Diamondback Explorer",
        "dolphin" => "Dolphin",
        "eagle" => "Eagle",
        "empire_courier" => "Imperial Courier",
        "empire_eagle" => "Imperial Eagle",
        "empire_trader" => "Imperial Clipper",
        "explorer_nx" => "Caspian Explorer",
        "federation_corvette" => "Federal Corvette",
        "federation_dropship" => "Federal Dropship",
        "federation_dropship_mkii" => "Federal Assault Ship",
        "federation_gunship" => "Federal Gunship",
        "ferdelance" => "Fer-de-Lance",
        "hauler" => "Hauler",
        "independant_trader" => "Keelback",
        "krait_light" => "Krait Phantom",
        "lakonminer" => "Type-11 Prospector",
        "testbuggy" => "SRV Scarab",
        "krait_mkii" => "Krait Mk II",
        "mamba" => "Mamba",
        "mandalay" => "Mandalay",
        "orca" => "Orca",
        "panthermkii" => "Panther Clipper Mk II",
        "python" => "Python",
        "python_nx" => "Python Mk II",
        "sidewinder" => "Sidewinder",
        "smallcombat01_nx" => "Kestrel Mk II",
        "type6" => "Type-6 Transporter",
        "type7" => "Type-7 Transporter",
        "type8" => "Type-8 Transporter",
        "type9" => "Type-9 Heavy",
        "type9_military" => "Type-10 Defender",
        "typex" => "Alliance Chieftain",
        "typex_2" => "Alliance Crusader",
        "typex_3" => "Alliance Challenger",
        "viper" => "Viper Mk III",
        "viper_mkiv" => "Viper Mk IV",
        "vulture" => "Vulture",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    // Unknown: drop the "_nx" variant suffix and title-case the rest so it
    // at least reads as a name.
    s.trim_end_matches("_nx")
        .split(['_', ' '])
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod market_name_tests {
    use super::{complete, display_name, resolve, SHIPS};

    /// The table and `display_name` must not drift apart: every symbol in
    /// one is named the same in the other.
    #[test]
    fn the_catalog_and_display_name_agree() {
        for (sym, name) in SHIPS {
            assert_eq!(&display_name(sym), name, "{sym}");
        }
    }

    /// Maintainer, 2026-09-12: a Type-10 Defender was unfindable because
    /// its symbol is `type9_military`. Every hull whose display name does
    /// not contain its symbol needs this path.
    #[test]
    fn a_commanders_words_become_a_symbol() {
        assert_eq!(resolve("Type-10 Defender"), Some("type9_military"));
        assert_eq!(resolve("type 10 defender"), Some("type9_military"));
        assert_eq!(resolve("Type10Defender"), Some("type9_military"));
        assert_eq!(resolve("Imperial Cutter"), Some("cutter"));
        assert_eq!(resolve("Krait Mk II"), Some("krait_mkii"));
        assert_eq!(resolve("krait mkii"), Some("krait_mkii"));
        assert_eq!(resolve("Fer-de-Lance"), Some("ferdelance"));
        assert_eq!(resolve("Keelback"), Some("independant_trader"));
        assert_eq!(resolve("Panther Clipper Mk II"), Some("panthermkii"));
        // A symbol typed straight in still works, and so does a unique part.
        assert_eq!(resolve("type9_military"), Some("type9_military"));
        assert_eq!(resolve("defender"), Some("type9_military"));
        assert_eq!(resolve("chieftain"), Some("typex"));
        // Ambiguous or unknown resolves to nothing rather than a guess.
        assert_eq!(resolve("type"), None, "nine hulls start with Type");
        assert_eq!(resolve("krait"), None, "Phantom and Mk II both");
        assert_eq!(resolve("millennium falcon"), None);
        assert_eq!(resolve("   "), None);
    }

    /// Completion is by display name, and finds a hull by any part of it.
    #[test]
    fn completion_offers_display_names() {
        assert_eq!(complete("type-10", 10), vec!["Type-10 Defender"]);
        assert_eq!(complete("10", 10), vec!["Type-10 Defender"]);
        assert_eq!(complete("krait", 10), vec!["Krait Mk II", "Krait Phantom"]);
        assert_eq!(complete("imperial", 10), vec!["Imperial Clipper", "Imperial Courier", "Imperial Cutter", "Imperial Eagle"]);
        assert_eq!(complete("cutter", 10), vec!["Imperial Cutter"], "by symbol too");
        assert_eq!(complete("zzz", 10), Vec::<&str>::new());
        assert_eq!(complete("", 3).len(), 3, "empty prefix offers the head of the catalog");
        assert!(complete("", 999).len() == SHIPS.len());
    }
}

/// Prefer the journal's own localised name when present.
pub fn display_name_or(symbol: &str, localised: Option<&str>) -> String {
    match localised {
        Some(l) if !l.trim().is_empty() => l.to_string(),
        _ => display_name(symbol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_symbols_get_their_names() {
        assert_eq!(display_name("eagle"), "Eagle");
        assert_eq!(display_name("federation_dropship_mkii"), "Federal Assault Ship");
        assert_eq!(display_name("smallcombat01_nx"), "Kestrel Mk II");
        assert_eq!(display_name("Anaconda"), "Anaconda");
    }

    #[test]
    fn unknown_symbols_are_title_cased_not_invented() {
        assert_eq!(display_name("future_ship_mk9"), "Future Ship Mk9");
        assert_eq!(display_name_or("eagle", Some("Eagle (Pirate)")), "Eagle (Pirate)");
        assert_eq!(display_name_or("eagle", Some("")), "Eagle");
    }
}
