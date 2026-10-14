//! Engineer unlock requirements, vendored from a community guide.
//!
//! Blueprints come from EDEngineer's data; *how to meet an engineer* is not
//! in that dataset, so this file carries it: invite condition, unlock task,
//! rank-up hint, home base. It is reference material with a source and a
//! fetch date, not ground truth -- the journal's `EngineerProgress` says
//! what the commander has actually unlocked, and that always wins.

use serde::{Deserialize, Serialize};

const UNLOCKS_JSON: &str = include_str!("../data/engineer_unlocks.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineerUnlock {
    pub name: String,
    pub system: String,
    pub base: String,
    pub invite: String,
    pub unlock: String,
    pub rank_up: Option<String>,
    pub notes: Option<String>,
    /// Position in the guide's recommended order; `None` for the omitted ones.
    pub step: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockGuide {
    pub source: String,
    pub fetched: String,
    pub note: String,
    pub engineers: Vec<EngineerUnlock>,
}

pub fn guide() -> UnlockGuide {
    serde_json::from_str(UNLOCKS_JSON).expect("bundled engineer_unlocks.json must parse")
}

/// Case-insensitive lookup, tolerant of the journal's spelling differences
/// ("Tod 'The Blaster' McQuinn" vs "Tod McQuinn").
pub fn find(name: &str) -> Option<EngineerUnlock> {
    let want = name.to_lowercase();
    let squash = |s: &str| s.to_lowercase().replace(['\'', '"', '.'], "");
    guide().engineers.into_iter().find(|e| {
        let have = e.name.to_lowercase();
        have == want
            || squash(&have).contains(&squash(&want))
            || want.split_whitespace().last().is_some_and(|last| have.ends_with(last))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_parses_with_every_ship_engineer_placed() {
        let g = guide();
        // The guide's twenty, plus the five ship engineers added 2026-10-14
        // with locations from the wiki's Engineers table (the engineer tour
        // needs every one of them placed).
        assert_eq!(g.engineers.len(), 25);
        assert!(g.source.contains("wanderer-toolbox"));
        assert_eq!(g.engineers.iter().filter(|e| e.step.is_some()).count(), 14);
        for (name, system, base) in [
            ("Chloe Sedesi", "Shenve", "Cinder Dock"),
            ("Etienne Dorn", "Los", "Kraken's Retreat"),
            ("Marsha Hicks", "Tir", "The Watchtower"),
            ("Mel Brandon", "Luchtaine", "The Brig"),
            ("Petra Olmanova", "Asura", "Sanctuary"),
        ] {
            let e = find(name).unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!((e.system.as_str(), e.base.as_str()), (system, base), "{name}");
        }
        assert!(g.engineers.iter().all(|e| !e.system.is_empty() && !e.base.is_empty()), "every engineer has a system and a base");
    }

    #[test]
    fn journal_spellings_resolve() {
        assert_eq!(find("Tod McQuinn").unwrap().system, "Wolf 397");
        assert_eq!(find("felicity farseer").unwrap().unlock, "Donate 1 Meta Alloy");
        assert!(find("Nobody").is_none());
    }
}
