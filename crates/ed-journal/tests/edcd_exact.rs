//! The gates behind "every everything has a coded name and a printed name
//! and we should be exact" (maintainer, 2026-09-20): EDDA's printed names
//! against EDCD's FDevIDs tables (vendored in `data/`) and against
//! Frontier's own localised strings from the maintainer's journal
//! (`tests/fixtures/frontier_names.json`). See also `edcd_names.rs`, the
//! ignored instrument that prints every difference against a fresh copy
//! of the tables.

use serde_json::Value;
use std::collections::HashMap;

fn rows(csv: &str) -> Vec<HashMap<String, String>> {
    let mut lines = csv.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| header.iter().zip(l.split(',')).map(|(k, v)| (k.to_string(), v.trim().to_string())).collect())
        .collect()
}

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/frontier_names.json")).unwrap()
}

#[test]
fn every_module_symbol_prints_edcds_name() {
    let mut bad = Vec::new();
    // A symbol's printed name is its FIRST row: pre-engineered and Powerplay
    // variants ("Enduring Feedback Rail Gun") share the base symbol and a
    // Loadout cannot tell them apart, so the base name is the honest one.
    let mut seen = std::collections::HashSet::new();
    for r in rows(include_str!("../data/outfitting.csv")) {
        if !r["entitlement"].is_empty() || !seen.insert(r["symbol"].to_ascii_lowercase()) {
            continue;
        }
        let ours = ed_journal::modules::item_name(&r["symbol"]);
        if !ours.to_ascii_lowercase().starts_with(&r["name"].to_ascii_lowercase()) {
            bad.push(format!("{} -> {:?}, EDCD {:?}", r["symbol"], ours, r["name"]));
        }
    }
    assert!(bad.is_empty(), "{} module names differ from EDCD:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn every_ship_symbol_prints_edcds_name_with_frontiers_spacing() {
    let mut bad = Vec::new();
    for r in rows(include_str!("../data/shipyard.csv")) {
        let want = ed_journal::ships::frontier_mark_spacing(&r["name"]);
        let ours = ed_journal::ships::display_name(&r["symbol"]);
        if ours != want {
            bad.push(format!("{} -> {ours:?}, EDCD {want:?}", r["symbol"]));
        }
    }
    assert!(bad.is_empty(), "{} ship names differ from EDCD:\n{}", bad.len(), bad.join("\n"));
}

/// Frontier's own strings win: the names the game wrote into the
/// maintainer's journal, ships exactly and modules on the name part.
#[test]
fn frontiers_own_names_are_printed_exactly() {
    let f = fixture();
    let mut bad = Vec::new();
    for (symbol, name) in f["ships"].as_object().unwrap() {
        let ours = ed_journal::ships::display_name(symbol);
        if &ours != name.as_str().unwrap() {
            bad.push(format!("ship {symbol}: {ours:?} vs Frontier {name}"));
        }
    }
    for (symbol, name) in f["modules"].as_object().unwrap() {
        let ours = ed_journal::modules::item_name(symbol);
        if !ours.to_ascii_lowercase().starts_with(&name.as_str().unwrap().to_ascii_lowercase()) {
            bad.push(format!("module {symbol}: {ours:?} vs Frontier {name}"));
        }
    }
    assert!(bad.is_empty(), "{} differ from Frontier's own strings:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn every_material_commodity_and_rare_is_in_the_catalog_by_edcds_name() {
    let cat = ed_journal::Catalog::load();
    let mut bad = Vec::new();
    for (table, csv) in [
        ("material", include_str!("../data/material.csv")),
        ("commodity", include_str!("../data/commodity.csv")),
        ("rare_commodity", include_str!("../data/rare_commodity.csv")),
    ] {
        for r in rows(csv) {
            match cat.by_symbol(&r["symbol"]) {
                Some(i) if i.name == r["name"] => {}
                Some(i) => bad.push(format!("{table} {}: {:?} vs EDCD {:?}", r["symbol"], i.name, r["name"])),
                None => bad.push(format!("{table} {}: missing", r["symbol"])),
            }
        }
    }
    assert!(bad.is_empty(), "{} catalog names differ from EDCD:\n{}", bad.len(), bad.join("\n"));
}
