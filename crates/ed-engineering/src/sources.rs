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
    all()
        .into_iter()
        .filter(|s| s.materials.iter().any(|m| m.to_lowercase() == want))
        .collect()
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
                assert!(
                    known.contains(&m.to_lowercase()),
                    "{m:?} at {} is not a blueprint ingredient",
                    site.site
                );
            }
        }
        assert_eq!(for_material("tellurium").len(), 1);
        assert!(for_material("nothing").is_empty());
    }
}
