pub mod cargo;
pub mod catalog;
pub mod inventory;
pub mod journal;
pub mod modules;
pub mod ships;
pub mod status;

pub use catalog::{Catalog, Item, Kind};
pub use inventory::Inventory;
pub use status::ShipStatus;

use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct ResolvedItem {
    pub symbol: String,
    pub name: String,
    pub category: String,
    pub count: i64,
}

/// Everything the UI needs in one call: materials + cargo, resolved to
/// display names, plus live ship status. Scans the most recent
/// `files_to_scan` journal files (default: enough to guarantee a
/// `Materials` snapshot is included) each time it's called -- cheap enough
/// to just re-run on every journal-changed file event rather than trying
/// to keep incremental state across calls.
pub fn read_all(
    journal_dir: &Path,
    files_to_scan: usize,
) -> anyhow::Result<(Vec<ResolvedItem>, ShipStatus)> {
    let catalog = Catalog::load();
    let files = journal::recent_journal_files(journal_dir, files_to_scan)?;

    let mut inv = Inventory::new();
    let mut status = ShipStatus::default();

    for path in &files {
        let file = std::fs::File::open(path)?;
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader.lines().map_while(Result::ok).collect();

        for line in &lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) {
                inventory::apply_event(&mut inv, &ev);
            }
        }
        status::scan_for_status(lines.into_iter(), &mut status);
    }

    let cargo_counts = cargo::read_cargo(journal_dir);

    let mut resolved: Vec<ResolvedItem> = Vec::new();
    for (symbol, count) in inv.into_iter().chain(cargo_counts) {
        if count <= 0 {
            continue;
        }
        let item = catalog.by_symbol(&symbol);
        resolved.push(ResolvedItem {
            symbol: symbol.clone(),
            name: item
                .map(|i| i.name.clone())
                .unwrap_or_else(|| format!("(unknown: {symbol})")),
            category: item.map(|i| i.category.clone()).unwrap_or_default(),
            count,
        });
    }
    resolved.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));

    Ok((resolved, status))
}
