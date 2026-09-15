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

        // Vendored rows are trimmed on the way in: one trailing space in
        // material.csv ("Untypical Shield Scans ") keyed the catalog by a
        // name no lookup could ever spell, so a commander holding 131 of
        // them was told they had none (maintainer, 2026-09-12).
        let mut push =
            |symbol: &str, name: &str, category: &str, group: &str, grade: u8, kind: Kind| {
                let (symbol, name, category, group) = (symbol.trim(), name.trim(), category.trim(), group.trim());
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

    /// Every symbol in the catalog, for tests and audits.
    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        self.by_symbol.values().map(|i| i.symbol.as_str())
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
    use super::{Catalog, Kind};

    /// The bug this guards: `material.csv` had "Untypical Shield Scans "
    /// with a trailing space, so the inventory map was keyed by a name the
    /// blueprint tables spell without one. A commander with 131 of them
    /// was told they had 0 (maintainer, 2026-09-12).
    #[test]
    fn a_trailing_space_cannot_hide_a_material_again() {
        let c = Catalog::load();
        let scans = c.by_symbol("ShieldDensityReports").expect("the encoded shield material");
        assert_eq!(scans.name, "Untypical Shield Scans");
        assert_eq!(c.by_name("Untypical Shield Scans").map(|i| i.symbol.as_str()), Some("ShieldDensityReports"));
        assert_eq!(c.by_name("untypical shield scans").map(|i| i.symbol.as_str()), Some("ShieldDensityReports"));

        // No vendored row may carry stray spacing in any field.
        let symbols: Vec<String> = c.symbols().map(str::to_owned).collect();
        assert!(symbols.len() > 300, "the whole catalog loaded: {} entries", symbols.len());
        for symbol in &symbols {
            let item = c.by_symbol(symbol).expect("a listed symbol resolves");
            for (what, field) in [("symbol", &item.symbol), ("name", &item.name), ("category", &item.category), ("group", &item.group)] {
                assert_eq!(field.trim(), field.as_str(), "{what} of {symbol} carries stray whitespace: {field:?}");
            }
        }
    }

    /// Every MATERIAL must be findable by the name it displays, because
    /// the inventory is keyed by name. One display name is shared with a
    /// commodity in the game's own data ("Wreckage Components", salvage
    /// and the Thargoid material), and the catalog keeps one of the two;
    /// that collision is listed here so a NEW one fails this test instead
    /// of silently costing a commander their count.
    #[test]
    fn every_material_is_findable_by_its_display_name() {
        let c = Catalog::load();
        const SHARED_WITH_A_COMMODITY: &[&str] = &["wreckage components"];
        let mut checked = 0;
        for symbol in c.symbols().map(str::to_owned).collect::<Vec<_>>() {
            let item = c.by_symbol(&symbol).expect("a listed symbol resolves");
            if item.kind != Kind::Material || SHARED_WITH_A_COMMODITY.contains(&item.name.to_lowercase().as_str()) {
                continue;
            }
            checked += 1;
            assert_eq!(
                c.by_name(&item.name).map(|i| i.symbol.as_str()),
                Some(item.symbol.as_str()),
                "{} does not round-trip through its display name {:?}",
                item.symbol,
                item.name
            );
        }
        assert!(checked > 100, "materials were actually checked: {checked}");
    }

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
