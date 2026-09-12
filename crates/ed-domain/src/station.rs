//! What kind of place a station is, and what can land there.
//!
//! Both enums are the vocabulary the route planner, the store's lookups
//! and the app share when they ask "can I dock here": they carry no
//! database knowledge, only the classification rules.

use serde::{Deserialize, Serialize};

/// The kind of station, coarse enough to rank by.
///
/// This exists because a raw station list is nearly useless: Deciat has 199
/// "stations", of which 136 are parked fleet carriers and most of the rest
/// are surface settlements. Sorting by arrival distance alone puts carriers
/// at 0 ls on top and buries Farseer Inc. Ranking by kind first is what
/// makes the answer match what a commander means by "what's in this system".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationClass {
    /// Large orbital: Coriolis, Orbis, Ocellus, Dodec, asteroid base, mega ship.
    Starport,
    /// Surface port with full services.
    PlanetaryPort,
    /// Small orbital or surface outpost.
    Outpost,
    /// Odyssey surface settlement -- on-foot, no ship services worth routing to.
    Settlement,
    /// Fleet carrier. Has a market, but moves.
    Carrier,
    /// Colonisation construction site.
    ConstructionDepot,
    /// Installation, beacon, or anything the dump left untyped.
    Other,
}

impl StationClass {
    /// From the dump's free-text station type.
    pub fn of(kind: Option<&str>) -> Self {
        let Some(k) = kind else {
            return StationClass::Other;
        };
        // Two spellings arrive here: the Spansh dump's ("Coriolis
        // Starport", "Planetary Port") and the journal's own StationType
        // ("Coriolis", "CraterPort", "SurfaceStation") as EDDN Docked
        // events write it into the server's stations table.
        let lower = k.to_ascii_lowercase();
        if lower.contains("carrier") {
            StationClass::Carrier
        } else if lower.contains("construction depot") || lower.contains("constructiondepot") {
            StationClass::ConstructionDepot
        } else if lower.contains("settlement") {
            StationClass::Settlement
        } else if lower.contains("starport")
            || lower.contains("asteroid base")
            || lower.contains("mega ship")
            || matches!(
                lower.as_str(),
                "coriolis" | "orbis" | "ocellus" | "bernal" | "asteroidbase" | "megaship"
            )
        {
            StationClass::Starport
        } else if lower == "planetary port"
            || matches!(lower.as_str(), "craterport" | "surfacestation")
        {
            StationClass::PlanetaryPort
        } else if lower.contains("outpost") {
            StationClass::Outpost
        } else {
            StationClass::Other
        }
    }

    /// Can a ship dock here and use services?
    pub fn is_dockable(self) -> bool {
        matches!(
            self,
            StationClass::Starport
                | StationClass::PlanetaryPort
                | StationClass::Outpost
                | StationClass::Carrier
        )
    }

    /// Sort weight: what a commander most likely means first.
    pub fn rank(self) -> i64 {
        match self {
            StationClass::Starport => 0,
            StationClass::PlanetaryPort => 1,
            StationClass::Outpost => 2,
            StationClass::Carrier => 3,
            StationClass::ConstructionDepot => 4,
            StationClass::Settlement => 5,
            StationClass::Other => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PadSize {
    Small,
    Medium,
    Large,
}

/// On the wire a pad is whatever [`PadSize::parse`] accepts - `l` or
/// `large`, any case - not only the serialized spelling. the assistant session
/// (2026-09-08) caught a v2 trade body with `"min_pad":"l"` failing the
/// derive and falling through to the legacy answer without a word.
impl<'de> Deserialize<'de> for PadSize {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        PadSize::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "pad size is s/m/l or small/medium/large, not {raw:?}"
            ))
        })
    }
}

impl PadSize {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "s" | "small" => Some(PadSize::Small),
            "m" | "medium" => Some(PadSize::Medium),
            "l" | "large" => Some(PadSize::Large),
            _ => None,
        }
    }

    /// Largest pad a station offers, from its per-size counts.
    ///
    /// A station with no recorded pads returns `None` rather than `Small`.
    /// (See also [`is_carrier_callsign`] for the other identity the data
    /// sometimes withholds.)
    /// Guessing here fails open, and a route that sends a Cutter to an
    /// outpost is worse than one that admits it doesn't know.
    pub fn from_counts(
        large: Option<i64>,
        medium: Option<i64>,
        small: Option<i64>,
    ) -> Option<Self> {
        match (large.unwrap_or(0), medium.unwrap_or(0), small.unwrap_or(0)) {
            (l, _, _) if l > 0 => Some(PadSize::Large),
            (_, m, _) if m > 0 => Some(PadSize::Medium),
            (_, _, s) if s > 0 => Some(PadSize::Small),
            _ => None,
        }
    }

    pub fn fits(self, required: PadSize) -> bool {
        self >= required
    }

    /// The pad a ship needs, by its journal `Ship` identifier (the
    /// lowercase internal name a `Loadout` carries). `None` for a name
    /// this table does not know — callers should fail closed and treat an
    /// unknown ship as needing a Large pad, for the same reason
    /// [`PadSize::from_counts`] refuses to guess.
    pub fn for_journal_ship(ship: &str) -> Option<Self> {
        Some(match ship.trim().to_ascii_lowercase().as_str() {
            "adder" | "cobramkiii" | "cobramkiv" | "cobramkv" | "diamondback" | "diamondbackxl"
            | "dolphin" | "eagle" | "empire_courier" | "empire_eagle" | "hauler" | "sidewinder"
            | "viper" | "viper_mkiv" | "vulture" => PadSize::Small,
            "asp"
            | "asp_scout"
            | "corsair"
            | "federation_dropship"
            | "federation_dropship_mkii"
            | "federation_gunship"
            | "ferdelance"
            | "independant_trader"
            | "krait_light"
            | "krait_mkii"
            | "mamba"
            | "mandalay"
            | "python"
            | "python_nx"
            | "type6"
            | "type8"
            | "typex"
            | "typex_2"
            | "typex_3" => PadSize::Medium,
            "anaconda"
            | "belugaliner"
            | "cutter"
            | "empire_trader"
            | "federation_corvette"
            | "orca"
            | "panthermkii"
            | "type7"
            | "type9"
            | "type9_military" => PadSize::Large,
            _ => return None,
        })
    }
}

/// Does this station name carry a fleet-carrier callsign (XXX-XXX of
/// uppercase letters and digits)? The `is_carrier` flag is the primary
/// signal, but it lies in two measured ways (2026-09-04): fresh installs
/// hold no station identity yet, and 63 carriers in the bootstrapped
/// data sit misflagged at 0 — while 52,005 of 54,054 correctly-flagged
/// carriers match this pattern and zero real stations do. Use as
/// `flagged || is_carrier_callsign(name)`, never instead of the flag.
pub fn is_carrier_callsign(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 7
        && bytes[3] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 3 || b.is_ascii_uppercase() || b.is_ascii_digit())
}

#[cfg(test)]
mod pad_tests {
    use super::{is_carrier_callsign, PadSize};

    #[test]
    fn a_pad_deserializes_from_any_spelling_and_serializes_long() {
        for (raw, want) in [
            ("\"l\"", PadSize::Large),
            ("\"Large\"", PadSize::Large),
            ("\"m\"", PadSize::Medium),
            ("\"small\"", PadSize::Small),
        ] {
            assert_eq!(serde_json::from_str::<PadSize>(raw).unwrap(), want, "{raw}");
        }
        assert!(serde_json::from_str::<PadSize>("\"xl\"").is_err());
        assert_eq!(serde_json::to_string(&PadSize::Large).unwrap(), "\"large\"");
        assert_eq!(
            serde_json::from_str::<Option<PadSize>>("null").unwrap(),
            None
        );
    }

    #[test]
    fn carrier_callsigns_are_recognized_and_stations_are_not() {
        for carrier in ["W1V-8BQ", "N2T-21V", "K7F-83H", "X0J-43Z"] {
            assert!(is_carrier_callsign(carrier), "{carrier}");
        }
        for station in [
            "Metz Enterprise",
            "Garay Terminal",
            "abc-def",
            "AB-CDEF",
            "A1B-2C3D",
            "SWZN",
        ] {
            assert!(!is_carrier_callsign(station), "{station}");
        }
    }

    /// The classics people get wrong: the Type-7 needs a Large pad, the
    /// Type-8 only a Medium; unknown ships are None so callers fail
    /// closed to Large.
    #[test]
    fn journal_ship_pads_cover_the_gotchas() {
        assert_eq!(PadSize::for_journal_ship("Type7"), Some(PadSize::Large));
        assert_eq!(PadSize::for_journal_ship("type8"), Some(PadSize::Medium));
        assert_eq!(
            PadSize::for_journal_ship("empire_trader"),
            Some(PadSize::Large)
        );
        assert_eq!(PadSize::for_journal_ship("mandalay"), Some(PadSize::Medium));
        assert_eq!(PadSize::for_journal_ship("dolphin"), Some(PadSize::Small));
        assert_eq!(PadSize::for_journal_ship("shiny_new_ship"), None);
    }
}

/// The journal's StationServices key for a service name as the Spansh
/// dump spells it ("Material Trader" → `materialtrader`,
/// "Interstellar Factors Contact" → `facilitator`), so `station_services`
/// holds ONE vocabulary whichever source wrote the row. Already-journal
/// keys pass through unchanged. Unknown names fall back to the journal's
/// own convention: lowercase, no spaces.
pub fn journal_service_key(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "interstellar factors contact" | "interstellar factors" => "facilitator".into(),
        "technology broker" | "tech broker" => "techbroker".into(),
        "universal cartographics" => "exploration".into(),
        "black market" => "blackmarket".into(),
        "search and rescue" => "searchrescue".into(),
        "restock" => "rearm".into(),
        "refinery contact" => "refinery".into(),
        "fleet carrier vendor" => "carriervendor".into(),
        "redemption office" => "voucherredemption".into(),
        "workshop" => "engineer".into(),
        "system colonisation" => "registeringcolonisation".into(),
        "construction services" => "colonisationcontribution".into(),
        "fleet carrier administration" | "carrier management" => "carriermanagement".into(),
        "fleet carrier fuel" | "carrier fuel" => "carrierfuel".into(),
        "market" => "commodities".into(),
        "dock" => "dock".into(),
        _ => lower.replace([' ', '-', '_'], ""),
    }
}

/// The journal spells an economy as a symbol (`$economy_HighTech;`)
/// where the Spansh dump spells it in words ("High Tech"). One
/// vocabulary wins, and it is the dump's, because that is what the
/// client's material-trader filter compares against. Already-plain
/// names pass through. Unknown symbols are unwrapped and split on the
/// capitals, so a new economy still arrives readable rather than raw.
pub fn economy_from_journal(name: &str) -> String {
    let raw = name.trim();
    let inner = unwrap_symbol(raw, "economy");
    match inner.to_ascii_lowercase().as_str() {
        "agri" | "agriculture" => "Agriculture".into(),
        "hightech" | "high tech" => "High Tech".into(),
        "privateenterprise" | "private enterprise" => "Private Enterprise".into(),
        "none" => "None".into(),
        _ => split_capitals(inner),
    }
}

/// The government's twin (`$government_PrisonColony;` -> "Prison Colony").
pub fn government_from_journal(name: &str) -> String {
    let raw = name.trim();
    let inner = unwrap_symbol(raw, "government");
    split_capitals(inner)
}

/// `$economy_HighTech;` -> `HighTech`; anything else unchanged.
fn unwrap_symbol<'a>(raw: &'a str, kind: &str) -> &'a str {
    let prefix = format!("${kind}_");
    raw.strip_prefix(prefix.as_str())
        .map(|rest| rest.strip_suffix(';').unwrap_or(rest))
        .unwrap_or(raw)
}

/// `HighTech` -> `High Tech`; a name that already has spaces is left
/// alone so dump spellings survive untouched.
fn split_capitals(inner: &str) -> String {
    if inner.contains(' ') || inner.is_empty() {
        return inner.to_owned();
    }
    let mut out = String::with_capacity(inner.len() + 2);
    for (i, c) in inner.char_indices() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod journal_spelling_tests {
    use super::*;

    /// The journal's StationType (what EDDN writes into the server) and
    /// the dump's names land on the same class.
    #[test]
    fn journal_and_dump_spellings_agree() {
        for (journal, dump) in [
            ("Coriolis", "Coriolis Starport"),
            ("Orbis", "Orbis Starport"),
            ("Ocellus", "Ocellus Starport"),
            ("AsteroidBase", "Asteroid base"),
            ("MegaShip", "Mega ship"),
            ("Outpost", "Outpost"),
            ("CraterOutpost", "Planetary Outpost"),
            ("CraterPort", "Planetary Port"),
            ("FleetCarrier", "Drake-Class Carrier"),
            ("OnFootSettlement", "Settlement"),
            ("PlanetaryConstructionDepot", "Planetary Construction Depot"),
        ] {
            assert_eq!(
                StationClass::of(Some(journal)),
                StationClass::of(Some(dump)),
                "{journal} vs {dump}"
            );
        }
        assert_eq!(StationClass::of(Some("Coriolis")), StationClass::Starport);
        assert_eq!(
            StationClass::of(Some("SurfaceStation")),
            StationClass::PlanetaryPort
        );
    }

    /// The journal's economy symbols land on the dump's words, because
    /// the client's material-trader filter compares against the dump
    /// spelling: Extraction/Refinery = raw, Industrial = manufactured,
    /// High Tech/Military = encoded.
    #[test]
    fn journal_economy_symbols_become_dump_words() {
        for (journal, dump) in [
            ("$economy_Industrial;", "Industrial"),
            ("$economy_HighTech;", "High Tech"),
            ("$economy_Extraction;", "Extraction"),
            ("$economy_Refinery;", "Refinery"),
            ("$economy_Military;", "Military"),
            ("$economy_Agri;", "Agriculture"),
            ("$economy_Tourism;", "Tourism"),
            ("$economy_PrivateEnterprise;", "Private Enterprise"),
        ] {
            assert_eq!(economy_from_journal(journal), dump, "{journal}");
            // The dump's own spelling survives a second pass unchanged.
            assert_eq!(economy_from_journal(dump), dump, "{dump} round trip");
        }
        assert_eq!(
            government_from_journal("$government_PrisonColony;"),
            "Prison Colony"
        );
        assert_eq!(government_from_journal("Corporate"), "Corporate");
    }

    /// Dump service names land on the journal's keys — the vocabulary
    /// EDDN already writes and /v1/stations validates against.
    #[test]
    fn dump_service_names_become_journal_keys() {
        assert_eq!(journal_service_key("Material Trader"), "materialtrader");
        assert_eq!(
            journal_service_key("Interstellar Factors Contact"),
            "facilitator"
        );
        assert_eq!(
            journal_service_key("Universal Cartographics"),
            "exploration"
        );
        assert_eq!(journal_service_key("Black Market"), "blackmarket");
        assert_eq!(journal_service_key("Restock"), "rearm");
        assert_eq!(journal_service_key("Vista Genomics"), "vistagenomics");
        assert_eq!(journal_service_key("materialtrader"), "materialtrader");
        assert_eq!(journal_service_key("Apex Interstellar"), "apexinterstellar");
    }
}
