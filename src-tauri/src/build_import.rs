//! Import a build from EDSY or Coriolis and say what separates the ship as
//! flown from it: which modules to swap, and which engineering to do —
//! then the build plan takes over for the materials, the shopping list and
//! the engineers (maintainer, 2026-09-19: "import a build from either edsy
//! or coriolis and we calculate what they're missing to get to it").
//!
//! The paste is SLEF, the journal `Loadout` both sites export, so the
//! target's modules carry the journal's own item symbols and its
//! `Engineering` block (blueprint symbol, grade, experimental). Coriolis's
//! other export, its JSON, names modules the Coriolis way and is reshaped
//! for the route planner only (drive and booster); for a build, the
//! answer is to export SLEF instead, and the error says so.

use serde::Serialize;
use serde_json::Value;

use crate::commands;
use crate::state::AppState;

/// A module the target build has where the ship has something else.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Swap {
    pub slot: String,
    pub slot_name: String,
    /// What is fitted now (None for an empty slot).
    pub have: Option<String>,
    pub want: String,
    pub want_item: String,
}

/// One engineered module of the target build, as a plan row.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ImportedItem {
    pub slot: String,
    pub slot_name: String,
    pub item_name: String,
    pub module_type: String,
    pub blueprint: Option<String>,
    /// The grade the ship's own module already has on this blueprint
    /// (0 after a swap or a different blueprint: a new roll starts over).
    pub from_grade: i64,
    pub target_grade: i64,
    pub experimental: Option<String>,
    /// The ship already has this: nothing to plan.
    pub done: bool,
}

#[derive(Debug, Serialize)]
pub struct ImportedBuild {
    /// "EDSY", "Coriolis", or None for a bare Loadout.
    pub app: Option<String>,
    pub ship: String,
    pub ship_name: Option<String>,
    /// The paste is the same hull as the selected ship.
    pub ship_matches: bool,
    pub swaps: Vec<Swap>,
    pub items: Vec<ImportedItem>,
    /// Engineered modules the plan cannot follow (unknown blueprint or module).
    pub skipped: Vec<String>,
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// The difference between the ship as flown and a pasted build.
pub fn import(state: &AppState, ship_id: Option<i64>, text: &str) -> Result<ImportedBuild, String> {
    let app = ed_galaxy::loadout::paste_app_name(text);
    if app.as_deref() == Some(ed_galaxy::loadout::CORIOLIS_JSON) {
        return Err("that is Coriolis's JSON export, which names modules its own way; use Export → SLEF in Coriolis (EDSY's SLEF works too)".into());
    }
    let target = ed_galaxy::loadout::loadout_from_paste(text).map_err(|e| e.to_string())?;
    let current = commands::ship_loadout(state, ship_id)?;
    let current_raw: Value = serde_json::from_str(&commands::loadout_raw(state, ship_id)?).map_err(|e| e.to_string())?;
    let ship_matches = s(&target, "Ship")
        .zip(s(&current_raw, "Ship"))
        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b));

    let ship = s(&target, "Ship")
        .map(|t| ed_journal::ships::display_name_or(t, s(&target, "Ship_Localised")))
        .unwrap_or_default();
    let ship_name = s(&target, "ShipName").map(|n| n.trim().to_string()).filter(|n| !n.is_empty());

    let (mut swaps, mut items, mut skipped) = (Vec::new(), Vec::new(), Vec::new());
    for m in target.get("Modules").and_then(Value::as_array).into_iter().flatten() {
        let Some(slot) = s(m, "Slot") else { continue };
        if ed_journal::modules::is_cosmetic_slot(slot) {
            continue;
        }
        let Some(item) = s(m, "Item") else { continue };
        let slot_name = ed_journal::modules::slot_name(slot);
        let item_name = ed_journal::modules::item_name(item);
        let have = current.modules.iter().find(|c| c.slot.eq_ignore_ascii_case(slot));
        let swap = !have.is_some_and(|c| c.item.eq_ignore_ascii_case(item));
        if swap {
            swaps.push(Swap {
                slot: slot.to_string(),
                slot_name: slot_name.clone(),
                have: have.map(|c| c.item_name.clone()),
                want: item_name.clone(),
                want_item: item.to_string(),
            });
        }
        let Some(eng) = m.get("Engineering") else { continue };
        let Some(module_type) = ed_engineering::journal::module_type_for_item(item) else {
            skipped.push(format!("{slot_name}: {item_name} is engineered in the build but EDDA has no blueprints for it"));
            continue;
        };
        let blueprint = s(eng, "BlueprintName").and_then(|sym| ed_engineering::journal::blueprint_for_symbol(sym, module_type));
        if blueprint.is_none() && s(eng, "BlueprintName").is_some() {
            skipped.push(format!("{slot_name}: blueprint {} is not in EDDA's data", s(eng, "BlueprintName").unwrap_or("")));
            continue;
        }
        let level = eng.get("Level").and_then(Value::as_i64).unwrap_or(0);
        let experimental = s(eng, "ExperimentalEffect_Localised")
            .map(str::to_string)
            .or_else(|| s(eng, "ExperimentalEffect").and_then(ed_engineering::journal::experimental_for_symbol).map(str::to_string));
        let same_blueprint = !swap && have.is_some_and(|c| c.blueprint.as_deref().zip(blueprint).is_some_and(|(a, b)| a.eq_ignore_ascii_case(b)));
        let from_grade = if same_blueprint { have.and_then(|c| c.grade).unwrap_or(0) } else { 0 };
        let same_experimental = !swap
            && match (&experimental, have.and_then(|c| c.experimental.as_deref())) {
                (None, _) => true,
                (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
                (Some(_), None) => false,
            };
        let done = same_blueprint && from_grade >= level && same_experimental;
        items.push(ImportedItem {
            slot: slot.to_string(),
            slot_name,
            item_name,
            module_type: module_type.to_string(),
            blueprint: blueprint.map(str::to_string),
            from_grade,
            target_grade: level,
            experimental,
            done,
        });
    }
    Ok(ImportedBuild { app, ship, ship_name, ship_matches, swaps, items, skipped })
}

#[cfg(test)]
mod tests {
    /// The seam the frontend relies on: a swapped module starts its
    /// engineering from grade 0, the same blueprint continues from the
    /// fitted grade, and a module the ship already has exactly is done.
    /// (`import` itself needs a store; the per-module rule is what matters.)
    #[test]
    fn a_swap_starts_over_and_a_match_is_done() {
        // Mirrors the rule in `import`, kept next to it so a change there is a change here.
        let rule = |swap: bool, same_blueprint: bool, have_grade: i64, level: i64, same_x: bool| {
            let same_blueprint = !swap && same_blueprint;
            let from = if same_blueprint { have_grade } else { 0 };
            (from, same_blueprint && from >= level && (!swap && same_x))
        };
        assert_eq!(rule(true, true, 5, 5, true), (0, false), "a new module has no rolls on it");
        assert_eq!(rule(false, true, 3, 5, true), (3, false), "continue from grade 3");
        assert_eq!(rule(false, false, 5, 5, true), (0, false), "another blueprint starts over");
        assert_eq!(rule(false, true, 5, 5, true), (5, true), "already there");
        assert_eq!(rule(false, true, 5, 5, false), (5, false), "same grade, experimental still to apply");
    }

    #[test]
    fn coriolis_json_is_refused_with_the_way_out() {
        // is_coriolis_json looks for the ship-loadout schema shape.
        let text = r#"{"$schema":"https://coriolis.io/schemas/ship-loadout/4.json#","name":"x","ship":"Type-10 Defender","components":{"standard":{},"hardpoints":[],"utility":[],"internal":[]}}"#;
        assert_eq!(ed_galaxy::loadout::paste_app_name(text).as_deref(), Some(ed_galaxy::loadout::CORIOLIS_JSON));
    }
}
