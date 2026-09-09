//! Main-star classes: the single authority for what a star means to a
//! pilot.
//!
//! The journal's `StarClass` letter and Spansh's `subType` string describe
//! the same thing; both parse onto this enum, and every question about a
//! star -- does it refuel you (KGBFOAM), does it supercharge the FSD
//! (neutron x4, white dwarf x1.5), is dropping in hazardous -- is answered
//! here and nowhere else. Storage and file formats that need a compact
//! encoding own their own mapping onto these variants.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StarClass {
    Unknown,
    O,
    B,
    A,
    F,
    G,
    K,
    M,
    L,
    T,
    Y,
    /// T Tauri and Herbig Ae/Be protostars.
    Proto,
    /// Wolf-Rayet, carbon and S-type stars: exotic, not scoopable.
    Exotic,
    WhiteDwarf,
    Neutron,
    BlackHole,
}

impl StarClass {
    /// Every variant, in declaration order.
    pub const ALL: [StarClass; 16] = {
        use StarClass::*;
        [Unknown, O, B, A, F, G, K, M, L, T, Y, Proto, Exotic, WhiteDwarf, Neutron, BlackHole]
    };

    /// From Spansh's `subType`, e.g. "K (Yellow-Orange) Star", "Neutron Star",
    /// "White Dwarf (DA) Star", "Black Hole", "T Tauri Star".
    pub fn from_subtype(s: &str) -> Self {
        use StarClass::*;
        let l = s.trim();
        if l.starts_with("Neutron") {
            return Neutron;
        }
        if l.starts_with("White Dwarf") {
            return WhiteDwarf;
        }
        if l.contains("Black Hole") {
            return BlackHole;
        }
        if l.starts_with("T Tauri") || l.starts_with("Herbig") {
            return Proto;
        }
        if l.starts_with("Wolf-Rayet")
            || l.starts_with("C ")
            || l.starts_with("CN")
            || l.starts_with("CJ")
            || l.starts_with("MS")
            || l.starts_with("S ")
        {
            return Exotic;
        }
        match l.chars().next() {
            Some('O') => O,
            Some('B') => B,
            Some('A') => A,
            Some('F') => F,
            Some('G') => G,
            Some('K') => K,
            Some('M') => M,
            Some('L') => L,
            Some('T') => T,
            Some('Y') => Y,
            _ => Unknown,
        }
    }

    /// From the journal's `StarClass` / `StarType` ("K", "DA", "N", "H",
    /// "TTS", "AeBe", "W", "MS", "SupermassiveBlackHole", ...).
    pub fn from_journal(s: &str) -> Self {
        use StarClass::*;
        match s.trim() {
            "N" => Neutron,
            "H" | "SupermassiveBlackHole" => BlackHole,
            "TTS" | "AeBe" => Proto,
            s if s.starts_with('D') => WhiteDwarf,
            s if s.starts_with('W') || s.starts_with('C') || s == "MS" || s == "S" => Exotic,
            "O" => O,
            "B" => B,
            "A" => A,
            "F" => F,
            "G" => G,
            "K" => K,
            "M" => M,
            "L" => L,
            "T" => T,
            "Y" => Y,
            _ => Unknown,
        }
    }

    /// Stable persistence name ("K", "WhiteDwarf", "Neutron", ...): the
    /// variant's identifier. This is what the `star_overrides.class`
    /// column holds; [`from_name`](Self::from_name) reads it back.
    pub fn name(self) -> &'static str {
        use StarClass::*;
        match self {
            Unknown => "Unknown",
            O => "O",
            B => "B",
            A => "A",
            F => "F",
            G => "G",
            K => "K",
            M => "M",
            L => "L",
            T => "T",
            Y => "Y",
            Proto => "Proto",
            Exotic => "Exotic",
            WhiteDwarf => "WhiteDwarf",
            Neutron => "Neutron",
            BlackHole => "BlackHole",
        }
    }

    /// Inverse of [`name`](Self::name). `None` for anything that is not a
    /// stored name -- including "Unknown", which is never worth keeping.
    pub fn from_name(s: &str) -> Option<Self> {
        let s = s.trim();
        Self::ALL.iter().copied().find(|c| *c != StarClass::Unknown && c.name() == s)
    }

    /// The journal-style letter, for display.
    pub fn letter(self) -> &'static str {
        use StarClass::*;
        match self {
            O => "O",
            B => "B",
            A => "A",
            F => "F",
            G => "G",
            K => "K",
            M => "M",
            L => "L",
            T => "T",
            Y => "Y",
            Proto => "TTS",
            Exotic => "W",
            WhiteDwarf => "D",
            Neutron => "N",
            BlackHole => "H",
            Unknown => "?",
        }
    }

    /// KGBFOAM can be fuel-scooped.
    pub fn scoopable(self) -> bool {
        use StarClass::*;
        matches!(self, O | B | A | F | G | K | M)
    }

    /// FSD supercharge multiplier on the next jump, for a standard drive.
    pub fn boost(self) -> f32 {
        match self {
            StarClass::Neutron => 4.0,
            StarClass::WhiteDwarf => 1.5,
            _ => 1.0,
        }
    }

    /// Jet cones or an event horizon at the arrival point.
    pub fn hazardous(self) -> bool {
        self.hazard_label().is_some()
    }

    /// What to warn about on arrival, spoken form.
    pub fn hazard_label(self) -> Option<&'static str> {
        match self {
            StarClass::Neutron => Some("neutron star"),
            StarClass::WhiteDwarf => Some("white dwarf"),
            StarClass::BlackHole => Some("black hole"),
            _ => None,
        }
    }
}

impl std::fmt::Display for StarClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}


/// The 4-bit code each class takes in the EDGX star file and the
/// service's `stars.class` column (`docs/BINARY-FORMATS.md`). A
/// file-format contract: never renumber, add new classes at the end.
/// Lives here so the store's EDDN writer and the router agree without
/// the store depending on the router.
pub trait StarClassCode: Sized {
    fn from_code(c: u8) -> Self;
    fn code(self) -> u8;
}

impl StarClassCode for StarClass {
    fn from_code(c: u8) -> Self {
        use StarClass::*;
        match c {
            1 => O, 2 => B, 3 => A, 4 => F, 5 => G, 6 => K, 7 => M, 8 => L, 9 => T, 10 => Y,
            11 => Proto, 12 => Exotic, 13 => WhiteDwarf, 14 => Neutron, 15 => BlackHole,
            _ => Unknown,
        }
    }

    fn code(self) -> u8 {
        use StarClass::*;
        match self {
            Unknown => 0, O => 1, B => 2, A => 3, F => 4, G => 5, K => 6, M => 7, L => 8, T => 9, Y => 10,
            Proto => 11, Exotic => 12, WhiteDwarf => 13, Neutron => 14, BlackHole => 15,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spansh_subtypes_map_to_classes() {
        assert_eq!(StarClass::from_subtype("K (Yellow-Orange) Star"), StarClass::K);
        assert_eq!(StarClass::from_subtype("Neutron Star"), StarClass::Neutron);
        assert_eq!(StarClass::from_subtype("White Dwarf (DA) Star"), StarClass::WhiteDwarf);
        assert_eq!(StarClass::from_subtype("Black Hole"), StarClass::BlackHole);
        assert_eq!(StarClass::from_subtype("T Tauri Star"), StarClass::Proto);
        assert_eq!(StarClass::from_subtype("Wolf-Rayet N Star"), StarClass::Exotic);
        assert_eq!(StarClass::from_subtype("M (Red dwarf) Star"), StarClass::M);
    }

    #[test]
    fn journal_letters_map_to_classes() {
        assert_eq!(StarClass::from_journal("N"), StarClass::Neutron);
        assert_eq!(StarClass::from_journal("DA"), StarClass::WhiteDwarf);
        assert_eq!(StarClass::from_journal("DC"), StarClass::WhiteDwarf);
        assert_eq!(StarClass::from_journal("H"), StarClass::BlackHole);
        assert_eq!(StarClass::from_journal("SupermassiveBlackHole"), StarClass::BlackHole);
        assert_eq!(StarClass::from_journal("TTS"), StarClass::Proto);
        assert_eq!(StarClass::from_journal("AeBe"), StarClass::Proto);
        assert_eq!(StarClass::from_journal("MS"), StarClass::Exotic);
        assert_eq!(StarClass::from_journal("W"), StarClass::Exotic);
        assert_eq!(StarClass::from_journal("K"), StarClass::K);
        assert_eq!(StarClass::from_journal("X"), StarClass::Unknown);
    }

    /// The cases the old string-prefix helpers got wrong: "AeBe" and "MS"
    /// start with a scoopable letter but are not scoopable, "TTS" is a
    /// protostar and not class T, and a supermassive black hole is a hazard.
    #[test]
    fn journal_edge_cases_through_the_single_authority() {
        assert!(!StarClass::from_journal("AeBe").scoopable());
        assert!(!StarClass::from_journal("MS").scoopable());
        assert!(!StarClass::from_journal("TTS").scoopable());
        assert_ne!(StarClass::from_journal("TTS"), StarClass::T);
        assert!(!StarClass::from_journal("DA").scoopable());
        assert!(StarClass::from_journal("DA").hazardous());
        assert_eq!(StarClass::from_journal("DA").hazard_label(), Some("white dwarf"));
        assert!(StarClass::from_journal("SupermassiveBlackHole").hazardous());
        assert_eq!(StarClass::from_journal("SupermassiveBlackHole").hazard_label(), Some("black hole"));
    }

    #[test]
    fn scoopable_covers_kgbfoam_and_nothing_else() {
        for c in ["K", "G", "B", "F", "O", "A", "M"] {
            assert!(StarClass::from_journal(c).scoopable(), "{c} should be scoopable");
        }
        for c in ["L", "T", "Y", "D", "N", "H", "TTS", "W", "AeBe", "MS", "DA"] {
            assert!(!StarClass::from_journal(c).scoopable(), "{c} should not be scoopable");
        }
    }

    #[test]
    fn scoop_and_boost_follow_the_class() {
        assert!(StarClass::K.scoopable() && !StarClass::L.scoopable() && !StarClass::Neutron.scoopable());
        assert_eq!(StarClass::Neutron.boost(), 4.0);
        assert_eq!(StarClass::WhiteDwarf.boost(), 1.5);
        assert_eq!(StarClass::G.boost(), 1.0);
        assert!(StarClass::BlackHole.hazardous());
        assert!(!StarClass::G.hazardous());
    }

    #[test]
    fn names_round_trip_and_match_debug() {
        for c in StarClass::ALL {
            assert_eq!(c.name(), format!("{c:?}"), "stored rows were written with the Debug form");
            assert_eq!(c.to_string(), c.name());
            if c == StarClass::Unknown {
                assert_eq!(StarClass::from_name(c.name()), None);
            } else {
                assert_eq!(StarClass::from_name(c.name()), Some(c));
            }
        }
        assert_eq!(StarClass::from_name("Neutron Star"), None);
    }
}
