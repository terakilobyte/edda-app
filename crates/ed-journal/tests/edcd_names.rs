//! Exactness against EDCD's FDevIDs (maintainer, 2026-09-20: "every ship
//! name, every module name, every commodity, every material, every
//! everything has a coded name and a printed name and we should be exact").
//!
//! An instrument, not a gate yet: `EDDA_FDEVIDS=<dir with the CSVs> cargo
//! test -p ed-journal --test edcd_names -- --ignored --nocapture` prints
//! every place a name EDDA prints differs from the table, per table, and
//! writes the CSV named by `EDDA_FDEVIDS_OUT`. The gate lands once the
//! tables are vendored and the differences are fixed or explained.

use std::collections::HashMap;

fn rows(path: &str) -> Vec<HashMap<String, String>> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().unwrap().split(',').collect();
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            // FDevIDs values carry no embedded commas today; keep it simple.
            header.iter().zip(l.split(',')).map(|(k, v)| (k.to_string(), v.trim().to_string())).collect()
        })
        .collect()
}

#[test]
#[ignore]
fn every_printed_name_matches_edcd() {
    let Ok(dir) = std::env::var("EDDA_FDEVIDS") else { return };
    let out = std::env::var("EDDA_FDEVIDS_OUT").unwrap_or_else(|_| "edcd_names.csv".into());
    let mut csv = String::from("table,symbol,edcd_name,edda_name,verdict\n");
    let mut totals: Vec<(String, usize, usize)> = Vec::new();

    // Ships: the journal's Ship symbol -> the name EDDA prints.
    let (mut ok, mut bad) = (0, 0);
    for r in rows(&format!("{dir}/shipyard.csv")) {
        let ours = ed_journal::ships::display_name_or(&r["symbol"], None);
        let same = ours == r["name"];
        if same { ok += 1 } else { bad += 1 }
        if !same { csv.push_str(&format!("shipyard,{},{},{},differs\n", r["symbol"], r["name"], ours)); }
    }
    totals.push(("shipyard".into(), ok, bad));

    // Modules: the Loadout item symbol -> the outfitting name EDDA prints.
    // EDCD names the module ("Pulse Laser"); EDDA prints it with class and
    // rating ("Pulse Laser 3C/G"), so the comparison is on the name part.
    let (mut ok, mut bad) = (0, 0);
    for r in rows(&format!("{dir}/outfitting.csv")) {
        let ours = ed_journal::modules::item_name(&r["symbol"]);
        let same = ours.to_ascii_lowercase().starts_with(&r["name"].to_ascii_lowercase());
        if same { ok += 1 } else { bad += 1 }
        if !same { csv.push_str(&format!("outfitting,{},{},{},differs\n", r["symbol"], r["name"], ours.replace(',', ";"))); }
    }
    totals.push(("outfitting".into(), ok, bad));

    // Materials and commodities: the catalog's names against the tables.
    let cat = ed_journal::Catalog::load();
    for (table, key) in [("material.csv", "name"), ("commodity.csv", "name"), ("rare_commodity.csv", "name")] {
        let (mut ok, mut bad) = (0, 0);
        for r in rows(&format!("{dir}/{table}")) {
            let ours = cat.by_symbol(&r["symbol"]).map(|i| i.name.clone());
            match ours {
                Some(n) if n == r[key] => ok += 1,
                Some(n) => { bad += 1; csv.push_str(&format!("{table},{},{},{},differs\n", r["symbol"], r[key], n)); }
                None => { bad += 1; csv.push_str(&format!("{table},{},{},,missing\n", r["symbol"], r[key])); }
            }
        }
        totals.push((table.into(), ok, bad));
    }

    for (t, ok, bad) in &totals {
        csv.push_str(&format!("#totals,{t},ok={ok},differs_or_missing={bad}\n"));
        println!("{t}: ok {ok}, differs or missing {bad}");
    }
    std::fs::write(&out, csv).unwrap();
    println!("wrote {out}");
}
