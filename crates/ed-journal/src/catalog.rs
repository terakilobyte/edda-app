//! Resolves Elite Dangerous internal journal "symbol" names (e.g.
//! `tg_propulsionelement`, `hyperspacetrajectories`) to their real in-game
//! display names.
//!
//! Why this exists: journal events store items under short internal names
//! that frequently do NOT match the display name at all, and guessing at
//! them silently produces wrong results (a lookup miss reads the same as
//! "count is zero"). This resolves that by embedding the canonical tables
//! published by the EDCD/FDevIDs project
//! (<https://github.com/EDCD/FDevIDs>) directly in the binary at compile
//! time, so name resolution never depends on network access and can never
//! silently drift from what's actually in the journal. The FDevIDs
//! repository carries no license file; the tables are Frontier's factual
//! ID assignments as collected by the community, reproduced here with
//! attribution (see THIRD-PARTY-NOTICES.md at the repository root).
//!
//! To refresh the vendored data, re-download `material.csv` and
//! `commodity.csv` from that repo into `data/` and rebuild.

use std::collections::HashMap;

const MATERIAL_CSV: &str = include_str!("../data/material.csv");
const COMMODITY_CSV: &str = include_str!("../data/commodity.csv");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Material,
    Commodity,
}

#[derive(Debug, Clone)]
pub struct Item {
    /// Internal journal symbol, exactly as it appears in FDevIDs (mixed case).
    pub symbol: String,
    /// In-game display name.
    pub name: String,
    /// For materials: Raw / Manufactured / Encoded. For commodities: market category.
    pub category: String,
    /// Materials only: trader column (`Conductive`, or `4` for a raw element category).
    pub group: String,
    /// Materials only: 1..=5 (FDevIDs "rarity"). 0 for commodities.
    pub grade: u8,
    pub kind: Kind,
}

pub struct Catalog {
    by_symbol: HashMap<String, Item>, // keyed lowercase
    by_name: HashMap<String, String>, // lowercase display name -> lowercase symbol
}

impl Catalog {
    /// Every catalogued commodity. Useful for small, in-memory UI pickers;
    /// this list is embedded in the binary and requires no database access.
    pub fn commodities(&self) -> impl Iterator<Item = &Item> {
        self.by_symbol
            .values()
            .filter(|i| i.kind == Kind::Commodity)
    }

    /// Every catalogued material (not commodities).
    pub fn materials(&self) -> impl Iterator<Item = &Item> {
        self.by_symbol.values().filter(|i| i.kind == Kind::Material)
    }

    pub fn load() -> Self {
        let mut by_symbol = HashMap::new();
        let mut by_name = HashMap::new();

        let mut push =
            |symbol: &str, name: &str, category: &str, group: &str, grade: u8, kind: Kind| {
                let key = symbol.to_lowercase();
                by_name
                    .entry(name.to_lowercase())
                    .or_insert_with(|| key.clone());
                by_symbol.insert(
                    key,
                    Item {
                        symbol: symbol.to_string(),
                        name: name.to_string(),
                        category: category.to_string(),
                        group: group.to_string(),
                        grade,
                        kind,
                    },
                );
            };

        let mut mat_reader = csv::Reader::from_reader(MATERIAL_CSV.as_bytes());
        for row in mat_reader.records().flatten() {
            // id,symbol,rarity,type,category,name
            if let (Some(symbol), Some(ty), Some(name)) = (row.get(1), row.get(3), row.get(5)) {
                let grade = row.get(2).and_then(|r| r.parse().ok()).unwrap_or(0);
                push(
                    symbol,
                    name,
                    ty,
                    row.get(4).unwrap_or(""),
                    grade,
                    Kind::Material,
                );
            }
        }

        let mut com_reader = csv::Reader::from_reader(COMMODITY_CSV.as_bytes());
        for row in com_reader.records().flatten() {
            // id,symbol,category,name
            if let (Some(symbol), Some(category), Some(name)) = (row.get(1), row.get(2), row.get(3))
            {
                push(symbol, name, category, "", 0, Kind::Commodity);
            }
        }

        Catalog { by_symbol, by_name }
    }

    pub fn by_symbol(&self, symbol: &str) -> Option<&Item> {
        self.by_symbol.get(&symbol.to_lowercase())
    }

    pub fn by_name(&self, name: &str) -> Option<&Item> {
        self.by_name
            .get(&name.to_lowercase())
            .and_then(|sym| self.by_symbol.get(sym))
    }

    /// Substring search over both display names and internal symbols.
    pub fn search(&self, text: &str) -> Vec<&Item> {
        let needle = text.to_lowercase();
        let mut hits: Vec<&Item> = self
            .by_symbol
            .values()
            .filter(|i| {
                i.name.to_lowercase().contains(&needle) || i.symbol.to_lowercase().contains(&needle)
            })
            .collect();
        hits.sort_by(|a, b| a.name.cmp(&b.name));
        hits
    }

    /// Display name for a symbol, falling back to a marked-unknown string
    /// rather than silently dropping unresolvable items. An "(unknown: ...)"
    /// name showing up in the UI is a visible bug report, not a silent gap.
    pub fn display_name(&self, symbol: &str) -> String {
        match self.by_symbol(symbol) {
            Some(item) => item.name.clone(),
            None => format!("(unknown: {symbol})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_previously_missed_internal_names() {
        let cat = Catalog::load();
        // These three are the exact internal names that were guessed wrong
        // earlier in development (tg_propulsionelement, hyperspacetrajectories)
        // -- regression-tested here so that mistake can't quietly recur.
        assert_eq!(
            cat.by_symbol("tg_propulsionelement")
                .map(|i| i.name.as_str()),
            Some("Propulsion Elements")
        );
        assert_eq!(
            cat.by_symbol("hyperspacetrajectories")
                .map(|i| i.name.as_str()),
            Some("Eccentric Hyperspace Trajectories")
        );
        assert_eq!(
            cat.by_symbol("dataminedwake").map(|i| i.name.as_str()),
            Some("Datamined Wake Exceptions")
        );
        assert_eq!(
            cat.by_symbol("thargoidtitandrivecomponent")
                .map(|i| i.name.as_str()),
            Some("Titan Drive Component")
        );
    }

    #[test]
    fn name_lookup_is_case_insensitive() {
        let cat = Catalog::load();
        assert!(cat.by_name("PROPULSION elements").is_some());
    }

    #[test]
    fn unknown_symbol_is_marked_not_silently_dropped() {
        let cat = Catalog::load();
        assert_eq!(
            cat.display_name("totally_made_up_symbol"),
            "(unknown: totally_made_up_symbol)"
        );
    }
}
