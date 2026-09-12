//! Landing pad size per ship hull.
//!
//! Keyed on the journal's internal `Ship` symbol (lowercase, as written in
//! `Loadout`). A hull absent from this table returns `None` and callers must
//! treat that as "unknown", never as "small": the pad filter fails closed,
//! per `docs/PLAN.md` Phase 4.

use ed_domain::station::PadSize;

/// The pad a hull needs. `None` for an unrecognised symbol.
pub fn pad_for_ship(symbol: &str) -> Option<PadSize> {
    use PadSize::*;
    let s = symbol.trim().to_ascii_lowercase();
    let pad = match s.as_str() {
        // Small
        "sidewinder" | "eagle" | "empire_eagle" | "hauler" | "adder" | "viper" | "viper_mkiv"
        | "cobramkiii" | "cobramkiv" | "diamondback" | "diamondbackxl" | "dolphin"
        | "vulture" | "empire_courier" | "independant_trader" | "smallcombat01_nx" => match s.as_str() {
            // Keelback and Dolphin are medium despite their size class peers.
            "independant_trader" | "dolphin" => Medium,
            _ => Small,
        },
        // Medium
        "type6" | "type7" | "type8" | "asp" | "asp_scout" | "federation_dropship"
        | "federation_dropship_mkii" | "federation_gunship" | "python" | "python_nx"
        | "krait_mkii" | "krait_light" | "typex" | "typex_2" | "typex_3" | "mamba"
        // mediumtransport01: never in the commander's journals, so no
        // display name yet — but Frontier's class-in-the-symbol naming
        // (smallcombat01_nx = small) makes the pad inference safe. The
        // sweep source is the server's ships table (every hull EDDN has
        // seen for sale): 48 hulls, all now covered.
        | "ferdelance" | "orca" | "mandalay" | "cobramkv" | "mediumtransport01" => Medium,
        // Large
        "anaconda" | "federation_corvette" | "cutter" | "empire_trader" | "type9"
        // Kestrel Mk II (smallcombat01_nx) is small-class; the Type-11
        // Prospector (lakonminer) is a large-class Lakon mining hull.
        // Caspian Explorer (explorer_nx): large pad, field-reported
        // 2026-09-05 ("landing pad size unknown… - large").
        | "type9_military" | "belugaliner" | "corsair" | "panthermkii" | "lakonminer"
        | "explorer_nx" => Large,
        _ => return None,
    };
    Some(pad)
}

/// Human name for a hull symbol, for speech and display.
///
/// The journal's `LoadGame` usually carries `Ship_Localised`, which is the
/// authoritative source and should be preferred. This table is the
/// fallback for the events that omit it (`Loadout` does), and it includes
/// the symbols seen in this commander's own journal. Unknown symbols are
/// tidied rather than returned raw, so a greeting never says
/// "smallcombat01_nx systems online".
pub fn display_name(symbol: &str) -> String {
    // One table, in ed-journal, so kills, status and speech agree.
    ed_journal::ships::display_name(symbol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_names_are_spoken_not_symbolised() {
        assert_eq!(display_name("SmallCombat01_NX"), "Kestrel Mk II");
        assert_eq!(display_name("krait_mkii"), "Krait Mk II");
        assert_eq!(display_name("future_hull_nx"), "Future Hull");
    }

    #[test]
    fn the_big_three_need_large_pads() {
        for s in ["Anaconda", "cutter", "federation_corvette"] {
            assert_eq!(pad_for_ship(s), Some(PadSize::Large), "{s}");
        }
    }

    #[test]
    fn medium_hulls_fit_outposts() {
        assert_eq!(pad_for_ship("python"), Some(PadSize::Medium));
        assert_eq!(pad_for_ship("krait_mkii"), Some(PadSize::Medium));
        assert_eq!(pad_for_ship("type8"), Some(PadSize::Medium));
    }

    #[test]
    fn the_boss_fleet_is_large_pad() {
        // Field-confirmed 2026-09-05: the Caspian Explorer (explorer_nx,
        // the hull that warned "pad size unknown"), the Panther Clipper
        // Mk II, and the Imperial Clipper are all large.
        for s in ["explorer_nx", "panthermkii", "empire_trader"] {
            assert_eq!(pad_for_ship(s), Some(PadSize::Large), "{s}");
        }
        assert_eq!(super::display_name("explorer_nx"), "Caspian Explorer");
    }

    #[test]
    fn hulls_the_journal_can_name_have_a_pad_size() {
        // ed_journal::ships knows these two; the pad table must too, or the
        // profit finder refuses to search for a ship it can greet by name.
        assert_eq!(
            pad_for_ship("SmallCombat01_NX"),
            Some(PadSize::Small),
            "Kestrel Mk II"
        );
        assert_eq!(
            pad_for_ship("lakonminer"),
            Some(PadSize::Large),
            "Type-11 Prospector"
        );
    }

    #[test]
    fn an_unknown_hull_is_unknown_not_small() {
        // Guessing "small" here would let a filter pass a ship that does
        // not fit. Unknown must stay unknown.
        assert_eq!(pad_for_ship("fdev_next_hull"), None);
    }
}
