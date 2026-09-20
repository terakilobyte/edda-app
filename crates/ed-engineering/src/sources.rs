//! Well-known places to collect materials, vendored from community guides.
//! Each entry names its source; treat them as leads, not first-hand data.

use serde::{Deserialize, Serialize};

const DATA: &str = include_str!("../data/material_sources.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownSite {
    pub site: String,
    /// `None` for method-based sources (HGEs) with no fixed system.
    pub system: Option<String>,
    pub body: Option<String>,
    pub method: String,
    pub materials: Vec<String>,
    pub source: String,
}

pub fn all() -> Vec<KnownSite> {
    serde_json::from_str(DATA).expect("material_sources.json is valid")
}

/// Sites listing `material` (case-insensitive display name).
pub fn for_material(material: &str) -> Vec<KnownSite> {
    let want = material.to_lowercase();
    all().into_iter().filter(|s| s.materials.iter().any(|m| m.to_lowercase() == want)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_parses_and_names_match_the_blueprint_catalog() {
        let cat = crate::Catalog::load();
        let known: std::collections::HashSet<String> = cat
            .all_ingredient_names()
            .into_iter()
            .map(|n| n.to_lowercase())
            .collect();
        for site in all() {
            for m in &site.materials {
                assert!(known.contains(&m.to_lowercase()), "{m:?} at {} is not a blueprint ingredient", site.site);
            }
        }
        assert_eq!(for_material("tellurium").len(), 1);
        assert!(for_material("nothing").is_empty());
    }
}

/// How a material of this kind is obtained in general — the floor under
/// every answer, so a shortfall never reads "no known site" (maintainer,
/// 2026-09-20: "raw materials are picked up off the ground and whatnot,
/// encoded can be farmed at crash sites and traded, manufactured can be
/// obtained in various ways"). Mechanics, not coordinates.
pub fn methods_for_kind(kind: &str) -> &'static [&'static str] {
    match kind.to_ascii_lowercase().as_str() {
        "raw" => &[
            "Prospect a planet's surface in the SRV: outcrops, metallic meteorites and geological sites on rocky, high-metal-content and icy bodies; the system map lists a body's materials and their share.",
            "Grade 4 raw materials (Antimony, Polonium, Ruthenium, Technetium, Tellurium, Yttrium) come from crystal-shard biological sites; Selenium from any body that lists it.",
            "Trade at a raw material trader: 6 of a lower grade buy 1 of the next up within a category, 1 of a higher grade buys 3 of the next down.",
        ],
        "manufactured" => &[
            "High grade emissions signal sources give grade 5 manufactured materials by the system's state: boom for Proto Light Alloys and Proto Radiolic Alloys, outbreak for Pharmaceutical Isolators, civil unrest and war for Improvised Components, election for Imperial Shielding, and an Empire or Federation system for Core Dynamics Composites.",
            "Destroyed ships drop manufactured materials as salvage; mission rewards offer them outright.",
            "Trade at a manufactured material trader: 6 of a lower grade buy 1 of the next up within a category, 1 of a higher grade buys 3 of the next down.",
        ],
        "encoded" => &[
            "Scan ships (data scans), high wakes with a wake scanner (wake data), and the data points at settlements, crashed ships and private data beacons.",
            "The crashed Anaconda at HIP 16613 and the Jameson Memorial crash site at HIP 12099 give grade 4 and 5 encoded materials from a few scans, and can be repeated after relogging.",
            "Trade at an encoded material trader: 6 of a lower grade buy 1 of the next up within a category, 1 of a higher grade buys 3 of the next down.",
        ],
        _ => &[],
    }
}

#[cfg(test)]
mod method_tests {
    #[test]
    fn every_kind_has_ways() {
        for k in ["Raw", "Manufactured", "Encoded"] {
            assert!(!super::methods_for_kind(k).is_empty(), "{k}");
        }
        assert!(super::methods_for_kind("Odyssey").is_empty());
    }
}
