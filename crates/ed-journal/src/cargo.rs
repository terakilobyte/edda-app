//! Reads the game's live `Cargo.json`, which Frontier keeps up to date on
//! every cargo change -- unlike Materials, there's no snapshot+delta
//! reconstruction needed here, just read the file.

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct CargoEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Count")]
    count: i64,
}

#[derive(Debug, Deserialize)]
struct CargoFile {
    #[serde(rename = "Inventory", default)]
    inventory: Vec<CargoEntry>,
}

/// symbol (lowercase) -> count currently in the cargo hold.
pub fn read_cargo(journal_dir: &Path) -> HashMap<String, i64> {
    let path = journal_dir.join("Cargo.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return HashMap::new();
    };
    let Ok(parsed) = serde_json::from_str::<CargoFile>(&text) else {
        return HashMap::new();
    };
    parsed
        .inventory
        .into_iter()
        .map(|e| (e.name.to_lowercase(), e.count))
        .collect()
}
