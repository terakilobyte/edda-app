//! Ship symbol -> display name. The journal only adds `*_Localised` for
//! ships whose display name is not just the capitalised symbol, so plain
//! ones (`eagle`, `vulture`) arrive raw. Unknown symbols are title-cased
//! rather than guessed.

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
        assert_eq!(
            display_name("federation_dropship_mkii"),
            "Federal Assault Ship"
        );
        assert_eq!(display_name("smallcombat01_nx"), "Kestrel Mk II");
        assert_eq!(display_name("Anaconda"), "Anaconda");
    }

    #[test]
    fn unknown_symbols_are_title_cased_not_invented() {
        assert_eq!(display_name("future_ship_mk9"), "Future Ship Mk9");
        assert_eq!(
            display_name_or("eagle", Some("Eagle (Pirate)")),
            "Eagle (Pirate)"
        );
        assert_eq!(display_name_or("eagle", Some("")), "Eagle");
    }
}
