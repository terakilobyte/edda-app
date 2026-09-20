//! A whole build planned at once: every engineerable slot with a chosen
//! blueprint, grade and experimental, reported as ONE material list, one
//! shopping list and one engineer itinerary (maintainer, 2026-09-19: "my
//! type 10 has 9 weapon hardpoints — I'd like to be able to plan out all 9
//! at once and get the list").
//!
//! The per-module machinery already existed (gap reports, engineer access,
//! the shopping list from what the commander carries); this module pools
//! it across slots. Materials pool by name, so nine Focused pulse lasers
//! need nine times the iron — the number a commander actually has to
//! collect, which no per-module view can show.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use crate::commands::{self, EngineerAccess, ShoppingReport};
use crate::state::AppState;
use ed_engineering::RequirementLine;

fn five() -> i64 {
    5
}

/// One slot's plan, as the Ships tab sends it.
#[derive(Debug, Clone, Deserialize)]
pub struct PlanItem {
    /// The journal's `Slot` value ("LargeHardpoint1").
    pub slot: String,
    /// Blueprint-data module type ("Pulse Laser").
    pub module_type: String,
    /// Blueprint name ("Focused Weapon"); None when only an experimental is planned.
    #[serde(default)]
    pub blueprint: Option<String>,
    /// The grade already on the module (0 when unengineered).
    #[serde(default)]
    pub from_grade: i64,
    #[serde(default = "five")]
    pub target_grade: i64,
    #[serde(default)]
    pub experimental: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PlanItemReport {
    pub slot: String,
    pub slot_name: String,
    pub item_name: String,
    pub module_type: String,
    pub blueprint: Option<String>,
    pub from_grade: i64,
    pub target_grade: i64,
    pub experimental: Option<String>,
    /// This slot's own materials (blueprint rolls plus the experimental).
    pub lines: Vec<RequirementLine>,
    /// Every engineer who works the blueprint, with their cap and status.
    pub engineers: Vec<EngineerAccess>,
    /// Someone unlocked offers the target grade.
    pub reachable: bool,
    pub max_reachable_grade: Option<i64>,
    /// The engineer this slot is routed to in the itinerary, if any.
    pub assigned_to: Option<String>,
}

/// One stop on the itinerary: an unlocked engineer and the slots to bring.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct PlanEngineer {
    pub engineer: String,
    pub rank: Option<i64>,
    pub jobs: Vec<String>,
}

/// A swapped-in module a technology broker sells: its unlock, broken
/// down (maintainer, 2026-09-20: "refer to the recipe for the component
/// and break it down that way"). Materials pool into the plan's list;
/// commodities are bought at a market and listed apart.
#[derive(Debug, Serialize)]
pub struct Unlock {
    pub slot_name: String,
    pub item_name: String,
    /// "Guardian" or "Human": which technology broker.
    pub broker: String,
    pub materials: Vec<RequirementLine>,
    pub commodities: Vec<(String, i64)>,
}

#[derive(Debug, Serialize)]
pub struct BuildPlanReport {
    pub ship: String,
    pub ship_name: Option<String>,
    pub items: Vec<PlanItemReport>,
    /// Every material the whole plan needs, pooled, against the inventory.
    pub materials: Vec<RequirementLine>,
    pub fully_met: bool,
    /// Fewest engineers that cover every reachable slot, each with its slots.
    pub engineers: Vec<PlanEngineer>,
    /// Slots no unlocked engineer can take to the asked grade.
    pub unassigned: Vec<String>,
    /// Trades and farming for the pooled shortfall; None when nothing is short.
    pub shopping: Option<ShoppingReport>,
    /// Technology-broker unlocks the swaps need, materials already pooled above.
    pub unlocks: Vec<Unlock>,
}

/// Materials pooled by name across every slot. `have` is the same for all
/// lines of one material, so the pooled line keeps it.
pub fn pool(per_item: &[Vec<RequirementLine>]) -> Vec<RequirementLine> {
    let mut by_name: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for lines in per_item {
        for l in lines {
            let e = by_name.entry(l.material.clone()).or_insert((0, l.have));
            e.0 += l.need;
            e.1 = l.have;
        }
    }
    by_name
        .into_iter()
        .map(|(material, (need, have))| RequirementLine { material, need, have })
        .collect()
}

/// A slot that can be done by these engineers (unlocked ones offering the
/// asked grade), labelled for the itinerary.
pub struct Job {
    pub label: String,
    pub candidates: Vec<(String, Option<i64>)>,
}

/// Fewest stops: greedy set cover. Each round takes the engineer who can
/// still do the most remaining slots (ties to the name, so two reads give
/// the same itinerary); slots nobody can do are reported, never dropped.
pub fn assign(jobs: &[Job]) -> (Vec<PlanEngineer>, Vec<String>) {
    let mut remaining: Vec<usize> = (0..jobs.len()).collect();
    let mut out: Vec<PlanEngineer> = Vec::new();
    loop {
        let mut counts: BTreeMap<&str, (usize, Option<i64>)> = BTreeMap::new();
        for &i in &remaining {
            for (name, rank) in &jobs[i].candidates {
                let e = counts.entry(name.as_str()).or_insert((0, *rank));
                e.0 += 1;
            }
        }
        // Most slots first; BTreeMap order breaks ties alphabetically.
        let Some((best, (n, rank))) = counts.into_iter().max_by(|a, b| a.1 .0.cmp(&b.1 .0).then(b.0.cmp(a.0))) else {
            break;
        };
        if n == 0 {
            break;
        }
        let (mine, rest): (Vec<usize>, Vec<usize>) =
            remaining.iter().partition(|&&i| jobs[i].candidates.iter().any(|(c, _)| c == best));
        out.push(PlanEngineer { engineer: best.to_string(), rank, jobs: mine.iter().map(|&i| jobs[i].label.clone()).collect() });
        remaining = rest;
        if remaining.is_empty() {
            break;
        }
    }
    let unassigned = remaining.iter().map(|&i| jobs[i].label.clone()).collect();
    (out, unassigned)
}

/// The report, everything but the trader lookup (which is async and done
/// by the command).
pub fn report(state: &AppState, ship_id: Option<i64>, items: &[PlanItem], swaps: &[(String, String)], minimum: bool, complete: bool) -> Result<BuildPlanReport, String> {
    if items.is_empty() && swaps.is_empty() {
        return Err("nothing planned: pick a blueprint for at least one module".into());
    }
    let loadout = commands::ship_loadout(state, ship_id)?;
    let (have, engineers) = state.with_read(|s| {
        (
            commands::material_inventory(s.conn()),
            ed_store::query::engineers(s.conn()).unwrap_or_default(),
        )
    });
    let catalog = &state.engineering;

    let mut reports: Vec<PlanItemReport> = Vec::new();
    let mut per_item: Vec<Vec<RequirementLine>> = Vec::new();
    let mut jobs: Vec<Job> = Vec::new();
    for it in items {
        let module = loadout.modules.iter().find(|m| m.slot == it.slot);
        let (slot_name, item_name) = match module {
            Some(m) => (m.slot_name.clone(), m.item_name.clone()),
            None => (it.slot.clone(), String::new()),
        };
        let label = format!("{slot_name}: {}", it.blueprint.as_deref().unwrap_or("experimental only"));
        let mut lines: Vec<RequirementLine> = Vec::new();
        let mut access: Vec<EngineerAccess> = Vec::new();
        let (mut reachable, mut max_reachable) = (false, None);
        if let Some(bp) = &it.blueprint {
            if it.target_grade <= it.from_grade {
                return Err(format!("{slot_name}: target grade {} is not above the fitted grade {}", it.target_grade, it.from_grade));
            }
            let gap = if minimum {
                catalog.gap_report(&it.module_type, bp, it.from_grade, it.target_grade, &have)
            } else {
                catalog.gap_report_realistic(&it.module_type, bp, it.from_grade, it.target_grade, complete, &have)
            };
            if gap.lines.is_empty() && catalog.find(&it.module_type, bp, it.target_grade).is_none() {
                return Err(format!("{slot_name}: no blueprint {bp:?} at grade {} for {:?}", it.target_grade, it.module_type));
            }
            lines.extend(gap.lines);
            if let Some((a, r, m)) = commands::access_for(&engineers, catalog, &it.module_type, bp, it.target_grade) {
                access = a;
                reachable = r;
                max_reachable = m;
            }
            jobs.push(Job {
                label: format!("{label} G{}", it.target_grade),
                candidates: access
                    .iter()
                    .filter(|a| a.unlocked && a.max_grade >= it.target_grade)
                    .map(|a| (a.engineer.clone(), a.rank))
                    .collect(),
            });
        }
        if let Some(x) = &it.experimental {
            let gap = catalog
                .experimental_gap(&it.module_type, x, &have)
                .ok_or_else(|| format!("{slot_name}: no experimental effect {x:?} for {:?}", it.module_type))?;
            lines.extend(gap.lines);
        }
        per_item.push(lines.clone());
        reports.push(PlanItemReport {
            slot: it.slot.clone(),
            slot_name,
            item_name,
            module_type: it.module_type.clone(),
            blueprint: it.blueprint.clone(),
            from_grade: it.from_grade,
            target_grade: it.target_grade,
            experimental: it.experimental.clone(),
            lines,
            engineers: access,
            reachable,
            max_reachable_grade: max_reachable,
            assigned_to: None,
        });
    }

    // Swaps to technology-broker modules: the unlock recipe, materials
    // pooled with the rest, commodities listed to buy.
    let jc = ed_journal::Catalog::load();
    let mut unlocks = Vec::new();
    for (slot, item) in swaps {
        let item_name = ed_journal::modules::item_name(item);
        let Some(recipe) = catalog.unlock_recipe(&item_name) else { continue };
        let mut mats = Vec::new();
        let mut comms = Vec::new();
        for ing in &recipe.ingredients {
            match jc.by_name(&ing.name) {
                Some(i) if i.kind == ed_journal::Kind::Commodity => comms.push((ing.name.clone(), ing.quantity)),
                _ => mats.push(RequirementLine { material: ing.name.clone(), need: ing.quantity, have: *have.get(&ing.name).unwrap_or(&0) }),
            }
        }
        per_item.push(mats.clone());
        unlocks.push(Unlock {
            slot_name: ed_journal::modules::slot_name(slot),
            item_name,
            broker: recipe.module_type.clone(),
            materials: mats,
            commodities: comms,
        });
    }
    let materials = pool(&per_item);
    let fully_met = materials.iter().all(|l| l.have >= l.need);
    let (itinerary, unassigned) = assign(&jobs);
    for stop in &itinerary {
        for r in reports.iter_mut().filter(|r| r.blueprint.is_some()) {
            let label = format!("{}: {} G{}", r.slot_name, r.blueprint.as_deref().unwrap_or(""), r.target_grade);
            if stop.jobs.contains(&label) {
                r.assigned_to = Some(stop.engineer.clone());
            }
        }
    }

    let shopping = if fully_met {
        None
    } else {
        let need: HashMap<String, i64> = materials.iter().map(|l| (l.material.clone(), l.need)).collect();
        let galaxy = state.routing.galaxy(&state.data_dir);
        Some(state.with_read(|s| commands::shopping_from_need(s.conn(), galaxy.as_deref(), need, have.clone()))?)
    };

    Ok(BuildPlanReport {
        ship: loadout.ship,
        ship_name: loadout.ship_name,
        items: reports,
        materials,
        fully_met,
        engineers: itinerary,
        unassigned,
        shopping,
        unlocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(m: &str, need: i64, have: i64) -> RequirementLine {
        RequirementLine { material: m.into(), need, have }
    }

    /// Nine pulse lasers need nine times the iron: the pooled line is the
    /// number to collect, and "have" is the one inventory, not nine copies.
    #[test]
    fn materials_pool_by_name_across_slots() {
        let per_item = vec![
            vec![line("Iron", 3, 10), line("Nickel", 2, 0)],
            vec![line("Iron", 3, 10)],
            vec![line("Iron", 3, 10), line("Nickel", 1, 0)],
        ];
        let pooled = pool(&per_item);
        assert_eq!(pooled.len(), 2);
        assert_eq!((pooled[0].material.as_str(), pooled[0].need, pooled[0].have), ("Iron", 9, 10));
        assert_eq!((pooled[1].material.as_str(), pooled[1].need, pooled[1].have), ("Nickel", 3, 0));
    }

    fn job(label: &str, who: &[&str]) -> Job {
        Job { label: label.into(), candidates: who.iter().map(|w| (w.to_string(), Some(5))).collect() }
    }

    /// The itinerary is the fewest stops: one engineer who covers eight
    /// lasers and the distributor beats visiting two, and the slot only
    /// one engineer can do still gets its stop. A slot nobody unlocked
    /// can do is named, not dropped.
    #[test]
    fn the_itinerary_takes_the_fewest_engineers_and_names_what_nobody_can_do() {
        let mut jobs: Vec<Job> = (1..=8)
            .map(|i| job(&format!("Large Hardpoint {i}: Focused Weapon G4"), &["The Dweller", "Broo Tarquin"]))
            .collect();
        jobs.push(job("Power Distributor: Charge Enhanced G5", &["The Dweller"]));
        jobs.push(job("Frame Shift Drive: Increased FSD Range G5", &["Felicity Farseer"]));
        jobs.push(job("Thrusters: Dirty Drive Tuning G5", &[]));
        let (stops, unassigned) = assign(&jobs);
        assert_eq!(stops.iter().map(|s| (s.engineer.as_str(), s.jobs.len())).collect::<Vec<_>>(), [("The Dweller", 9), ("Felicity Farseer", 1)]);
        assert_eq!(unassigned, ["Thrusters: Dirty Drive Tuning G5"]);
    }

    /// Equal coverage falls to the name, so two reads give one itinerary.
    #[test]
    fn ties_break_by_name() {
        let jobs = vec![job("a", &["Zed", "Abe"]), job("b", &["Zed", "Abe"])];
        let (stops, _) = assign(&jobs);
        assert_eq!(stops.len(), 1);
        assert_eq!(stops[0].engineer, "Abe");
    }
}
