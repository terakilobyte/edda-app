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
    pub commodities: Vec<CommodityLine>,
    /// Already unlocked, and how the journal knows: nothing to gather.
    pub unlocked: Option<String>,
    /// A pre-engineered variant: paid for at the broker with every
    /// purchase, not unlocked once (maintainer, 2026-09-27: "the
    /// pre-engineered mods are one-time buy, not a permanent unlock").
    pub per_unit: bool,
    /// What the data could not say: a variant with no recipe on file.
    pub note: Option<String>,
    /// Already in storage, and where: nothing to buy or unlock for it.
    pub stored: Option<String>,
    /// Where this is bought, when the broker is not any broker of its
    /// type: the Sirius megaships (maintainer, 2026-09-29: "where I can
    /// buy the pre-engineered heat sink launchers").
    #[serde(rename = "where")]
    pub where_: Option<String>,
    /// The same module fitted on other owned ships, by ship: known from
    /// the journal, not spent (moving it strips that ship).
    pub also_on: Vec<String>,
    /// How many of this the build swaps in, and where: two modified
    /// shards are one purchase twice, and `materials` is what BOTH need
    /// (maintainer, 2026-09-27: "I don't think we're summing this properly").
    pub units: i64,
    pub slots: Vec<String>,
}

/// A commodity the unlock needs: bought at a market, unless the hold
/// already carries it. The sellers are the one network ask of the report,
/// filled by the command (maintainer, 2026-09-27: "why aren't we offering
/// to perform a market search, or just doing one?").
#[derive(Debug, Serialize, Clone)]
pub struct CommodityLine {
    pub name: String,
    pub need: i64,
    /// In the hold now.
    pub have: i64,
    /// The nearest markets with the shortfall in stock (market search rows).
    pub sellers: Vec<serde_json::Value>,
    /// The system the sellers were searched from.
    pub sellers_from: Option<String>,
    /// Why the list is what it is: outside the bubble, or the ask failed.
    pub sellers_note: Option<String>,
}

/// Every module fitted on the OTHER owned ships: (ship name, item symbol,
/// bought-engineering blueprint if any, its grade).
fn modules_on_other_ships(conn: &rusqlite::Connection, ship_id: Option<i64>) -> Vec<(String, String, Option<String>, Option<i64>)> {
    let mut out = Vec::new();
    let Ok(mut stmt) = conn.prepare("SELECT ship_id, ship, ship_name, raw FROM ships WHERE owned = 1 ORDER BY loadout_ts DESC") else { return out };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?))) else { return out };
    for (id, ship, name, raw) in rows.flatten() {
        if Some(id) == ship_id {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else { continue };
        let label = match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => format!("{n} ({})", ed_journal::ships::display_name(&ship)),
            None => ed_journal::ships::display_name(&ship),
        };
        for m in v.get("Modules").and_then(|m| m.as_array()).into_iter().flatten() {
            let Some(item) = m.get("Item").and_then(|i| i.as_str()) else { continue };
            let eng = m.get("Engineering").filter(|e| e.get("Engineer").and_then(|x| x.as_str()).is_none());
            out.push((
                label.clone(),
                item.to_ascii_lowercase(),
                eng.and_then(|e| e.get("BlueprintName").and_then(|b| b.as_str())).map(str::to_string),
                eng.and_then(|e| e.get("Level").and_then(|l| l.as_i64())),
            ));
        }
    }
    out
}

/// What the hold carries, by display name.
fn cargo_by_name(conn: &rusqlite::Connection, jc: &ed_journal::Catalog) -> std::collections::HashMap<String, i64> {
    let mut out = std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT symbol, count FROM cargo WHERE count > 0") {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
            for (symbol, count) in rows.flatten() {
                let name = jc.by_symbol(&symbol).map(|i| i.name.clone()).unwrap_or(symbol);
                *out.entry(name).or_insert(0) += count;
            }
        }
    }
    out
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
    /// Every commodity the unlocks need, pooled across them against the
    /// hold, with the sellers: two modified shards at two Power Converters
    /// each are four to buy, said once and in the open (maintainer,
    /// 2026-09-27: "they each take 2 power convertors but we only list that
    /// in grey text that's easy to miss").
    pub commodities: Vec<CommodityLine>,
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
pub fn report(state: &AppState, ship_id: Option<i64>, hull: Option<&str>, items: &[PlanItem], swaps: &[commands::ProposedSwap], minimum: bool, complete: bool) -> Result<BuildPlanReport, String> {
    if items.is_empty() && swaps.is_empty() {
        return Err("nothing planned: pick a blueprint for at least one module".into());
    }
    let raw: serde_json::Value = serde_json::from_str(&commands::loadout_raw(state, ship_id, hull)?).map_err(|e| e.to_string())?;
    let learned = commands::learned_presets(state);
    commands::check_swaps(&raw, swaps, &learned)?;
    let loadout = commands::ship_loadout(state, ship_id, hull)?;
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
        let swapped = swaps.iter().find(|s| s.slot.eq_ignore_ascii_case(&it.slot)).map(|s| {
            s.preset.as_deref().and_then(|id| commands::slots().preset(id).cloned().or_else(|| learned.iter().find(|p| p.id == id).cloned())).map(|p| p.name.clone()).unwrap_or_else(|| ed_journal::modules::item_name(&s.item))
        });
        let (slot_name, item_name) = match (module, swapped) {
            (_, Some(name)) => (ed_journal::modules::slot_name(&it.slot), name),
            (Some(m), None) => (m.slot_name.clone(), m.item_name.clone()),
            (None, None) => (ed_journal::modules::slot_name(&it.slot), String::new()),
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
    let (cargo, proven, mut storage, storage_ts, fitted_elsewhere) = state.with_read(|s| {
        (
            cargo_by_name(s.conn(), &jc),
            ed_store::query::unlocked_modules(s.conn()).unwrap_or_default(),
            ed_store::query::stored_modules(s.conn()).unwrap_or_default(),
            ed_store::query::stored_modules_ts(s.conn()).ok().flatten(),
            modules_on_other_ships(s.conn(), ship_id),
        )
    });
    // Units bought at a broker AFTER the last storage snapshot: the game
    // writes StoredModules at the next dock or outfitting screen, not at
    // the purchase, so six shards bought in three minutes showed as one
    // in storage and five to buy (maintainer, 2026-09-29: "are we not
    // summing correctly?"). Each such event, matched to its variant by
    // what it paid, is one unit owned until the snapshot catches up.
    let mut bought_since: Vec<(String, Vec<(String, i64)>, String)> = proven
        .iter()
        .filter_map(|(item, p)| match p {
            ed_store::query::UnlockProof::Broker { ts, paid, .. } if storage_ts.as_deref().is_none_or(|s| ts.as_str() > s) => Some((item.clone(), paid.clone(), ts.clone())),
            _ => None,
        })
        .collect();
    let mut unlocks = Vec::new();
    for s in swaps {
        let (slot, item) = (&s.slot, &s.item);
        if item.eq_ignore_ascii_case(ed_ships::EMPTY) {
            continue;
        }
        let preset = s.preset.as_deref().and_then(|id| commands::slots().preset(id).cloned().or_else(|| learned.iter().find(|p| p.id == id).cloned()));
        // The slot already carries this very module with this very bought
        // engineering: the swap is done, whatever a saved plan still says
        // (maintainer, 2026-09-29: "edda isn't realized I already have them
        // equipped").
        let fitted_already = loadout.modules.iter().any(|m| {
            m.slot.eq_ignore_ascii_case(slot)
                && m.item.eq_ignore_ascii_case(item)
                && m.engineer.is_none()
                && match &preset {
                    Some(p) => m.blueprint_symbol.as_deref().is_some_and(|b| p.blueprint.eq_ignore_ascii_case(b) || p.blueprints.iter().any(|x| x.eq_ignore_ascii_case(b))) && m.grade.is_none_or(|g| g == p.level),
                    None => m.blueprint_symbol.is_none(),
                }
        });
        if fitted_already {
            continue;
        }
        let plain_name = ed_journal::modules::item_name(item);
        let plain_recipe_name = ed_journal::modules::recipe_name(item);
        // A pre-engineered variant has its own recipe at the broker
        // ("Modified Shard Cannon (Fixed, Medium)", "Engineered FSD V1");
        // the plain module's is the fallback when the data has none.
        let recipe = match &preset {
            // A variant's recipe is its own or nothing: the plain module's
            // unlock is not what the broker charges for a bought variant.
            Some(p) => p.unlock.as_deref().and_then(|u| catalog.unlock_recipe(u)),
            None => catalog.unlock_recipe(&plain_recipe_name),
        };
        let item_name = preset.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| plain_name.clone());
        let paid_is = |paid: &[(String, i64)], recipe: &ed_engineering::Blueprint| -> bool {
            let mut want: Vec<(String, i64)> = recipe.ingredients.iter().map(|i| (i.name.to_ascii_lowercase(), i.quantity)).collect();
            let mut got: Vec<(String, i64)> = paid
                .iter()
                .map(|(sym, n)| (jc.by_symbol(sym).map(|i| i.name.to_ascii_lowercase()).unwrap_or_else(|| sym.to_ascii_lowercase()), *n))
                .collect();
            want.sort();
            got.sort();
            want == got
        };
        let same_engineering = |bp: Option<&str>, lvl: Option<i64>| match &preset {
            Some(p) => bp.is_some_and(|b| p.blueprint.eq_ignore_ascii_case(b) || p.blueprints.iter().any(|x| x.eq_ignore_ascii_case(b))) && lvl.is_none_or(|l| l == p.level),
            None => bp.is_none(),
        };
        // Fitted on another owned ship: said, never spent (maintainer,
        // 2026-09-28: "if a build calls for a ship module that the player
        // already has, we should know from the journal then").
        let also_on: Vec<String> = fitted_elsewhere
            .iter()
            .filter(|(_, it, bp, lvl)| it.eq_ignore_ascii_case(item) && same_engineering(bp.as_deref(), *lvl))
            .map(|(ship, _, _, _)| ship.clone())
            .collect();
        // A unit already in storage — the plain module, or the variant with
        // this very engineering — is owned: nothing to buy or unlock for
        // this slot (maintainer, 2026-09-28: "I'm about to buy those
        // modshards"). Each stored unit satisfies one slot.
        let stored_at = storage.iter().position(|m| m.item.eq_ignore_ascii_case(item) && same_engineering(m.blueprint.as_deref(), m.level));
        let bought_at = match (&preset, recipe) {
            (Some(_), Some(r)) => bought_since.iter().position(|(it, paid, _)| it.eq_ignore_ascii_case(item) && paid_is(paid, r)),
            _ => None,
        };
        if let (None, Some(i)) = (stored_at, bought_at) {
            let (_, _, ts) = bought_since.remove(i);
            unlocks.push(Unlock {
                slot_name: ed_journal::modules::slot_name(slot),
                item_name,
                broker: preset.as_ref().map(|p| p.broker.clone()).unwrap_or_default(),
                materials: Vec::new(),
                commodities: Vec::new(),
                unlocked: None,
                per_unit: false,
                note: None,
                stored: Some(format!("bought at the broker at {} — in storage there once the game lists it", ts.get(11..16).unwrap_or(&ts))),
                also_on,
                where_: None,
                units: 1,
                slots: Vec::new(),
            });
            continue;
        }
        if let Some(i) = stored_at {
            let m = storage.remove(i);
            let where_ = match (&m.system, m.in_transit) {
                (_, true) => "in storage, in transit".to_string(),
                (Some(sys), false) => format!(
                    "in storage at {sys}{}",
                    match (m.transfer_cost, m.transfer_time_s) {
                        (Some(c), Some(t)) if c > 0 => format!(" · transfer {} cr, {} min", c, (t + 59) / 60),
                        _ => String::new(),
                    }
                ),
                (None, false) => "in storage".to_string(),
            };
            unlocks.push(Unlock {
                slot_name: ed_journal::modules::slot_name(slot),
                item_name,
                broker: preset.as_ref().map(|p| p.broker.clone()).unwrap_or_else(|| recipe.map(|r| r.module_type.clone()).unwrap_or_default()),
                materials: Vec::new(),
                commodities: Vec::new(),
                unlocked: None,
                per_unit: false,
                note: None,
                stored: Some(where_),
                where_: None,
                also_on,
                units: 1,
                slots: Vec::new(),
            });
            continue;
        }
        let Some(recipe) = recipe else {
            if let Some(p) = &preset {
                unlocks.push(Unlock {
                    slot_name: ed_journal::modules::slot_name(slot),
                    item_name,
                    broker: p.broker.clone(),
                    materials: Vec::new(),
                    commodities: Vec::new(),
                    unlocked: None,
                    per_unit: p.per_unit,
                    note: Some(match p.broker.as_str() {
                        "community goal" => "a community-goal reward: no broker sells it".to_string(),
                        _ => format!("no recipe for this variant in EDDA's data (source: {})", p.source.as_deref().unwrap_or("unknown")),
                    }),
                    units: 1,
                    slots: Vec::new(),
                    stored: None,
                    also_on,
                where_: None,
                });
            }
            continue;
        };
        // Already unlocked: the journal's broker event, or a ship that
        // carried it (maintainer, 2026-09-27: six shards fitted, and the
        // plan still asked for the unlock's materials). The plain and the
        // modified variant share a symbol: a fitted module proves the
        // variant whose blueprint it carries, and a broker event proves
        // the recipe whose materials it paid.
        let plain_recipe = catalog.unlock_recipe(&plain_recipe_name);
        let key = item.to_ascii_lowercase();
        // A pre-engineered variant is bought each time, so nothing in the
        // journal makes the next one free: no proof is looked for. For the
        // plain module, a fitted plain one or a broker event that paid the
        // plain recipe (or paid something no recipe of this symbol
        // accounts for — the data may lag the game) proves the unlock; a
        // payment that matches a variant's recipe was a purchase, not it.
        let proof = if preset.is_some() {
            None
        } else {
            proven.iter().filter(|(k, _)| *k == key).map(|(_, p)| p).find(|p| match p {
                ed_store::query::UnlockProof::Fitted { blueprint, .. } => blueprint.is_none(),
                ed_store::query::UnlockProof::Broker { paid, .. } => {
                    paid_is(paid, recipe)
                        || (!plain_recipe.is_some_and(|r| paid_is(paid, r)) && !preset_recipes_of(catalog, &learned, item).iter().any(|r| paid_is(paid, r)))
                }
            })
        };
        if let Some(proof) = proof {
            let how = match proof {
                ed_store::query::UnlockProof::Broker { broker, ts, .. } => {
                    let kind = if broker.eq_ignore_ascii_case("guardian") { "Guardian" } else if broker.eq_ignore_ascii_case("human") { "Human" } else { broker.as_str() };
                    format!("unlocked at a {kind} technology broker on {}", ts.get(..10).unwrap_or(ts))
                }
                ed_store::query::UnlockProof::Fitted { ship, ship_name, .. } => {
                    let hull = ed_journal::ships::display_name(ship);
                    match ship_name {
                        Some(n) => format!("already fitted on {n} ({hull}), so the unlock is done"),
                        None => format!("already fitted on the {hull}, so the unlock is done"),
                    }
                }
            };
            unlocks.push(Unlock {
                slot_name: ed_journal::modules::slot_name(slot),
                item_name,
                broker: recipe.module_type.clone(),
                materials: Vec::new(),
                commodities: Vec::new(),
                unlocked: Some(how),
                per_unit: false,
                note: None,
                units: 1,
                slots: Vec::new(),
                stored: None,
                also_on,
                where_: None,
            });
            continue;
        }
        let mut mats = Vec::new();
        let mut comms = Vec::new();
        for ing in &recipe.ingredients {
            match jc.by_name(&ing.name) {
                Some(i) if i.kind == ed_journal::Kind::Commodity => comms.push(CommodityLine {
                    name: ing.name.clone(),
                    need: ing.quantity,
                    have: *cargo.get(&ing.name).unwrap_or(&0),
                    sellers: Vec::new(),
                    sellers_from: None,
                    sellers_note: None,
                }),
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
            unlocked: None,
            per_unit: preset.as_ref().is_some_and(|p| p.per_unit),
            note: None,
            units: 1,
            slots: Vec::new(),
            stored: None,
            also_on,
            where_: recipe.name.starts_with("Sirius ").then(|| {
                format!(
                    "bought per unit from the technology brokers on Sirius Corporation's megaships: {}",
                    crate::remote_lookup::SIRIUS_BROKER_SHIPS.iter().map(|(s, sys)| format!("{s} ({sys})")).collect::<Vec<_>>().join(", ")
                )
            }),
        });
    }
    // One line per variant and recipe, not one per slot: the ticks then
    // compare the hold with what every unit needs together.
    let mut grouped: Vec<Unlock> = Vec::new();
    for u in unlocks {
        match grouped.iter_mut().find(|g| g.item_name == u.item_name && g.unlocked == u.unlocked && g.note == u.note && g.per_unit == u.per_unit && g.stored == u.stored) {
            Some(g) => {
                g.units += 1;
                g.slots.push(u.slot_name.clone());
                for m in &u.materials {
                    match g.materials.iter_mut().find(|l| l.material == m.material) {
                        Some(l) => l.need += m.need,
                        None => g.materials.push(m.clone()),
                    }
                }
                for c in &u.commodities {
                    match g.commodities.iter_mut().find(|l| l.name == c.name) {
                        Some(l) => l.need += c.need,
                        None => g.commodities.push(c.clone()),
                    }
                }
            }
            None => grouped.push(Unlock { units: 1, slots: vec![u.slot_name.clone()], ..u }),
        }
    }
    let unlocks = grouped;
    let mut commodities: Vec<CommodityLine> = Vec::new();
    for c in unlocks.iter().flat_map(|u| u.commodities.iter()) {
        match commodities.iter_mut().find(|l| l.name == c.name) {
            Some(l) => l.need += c.need,
            None => commodities.push(c.clone()),
        }
    }
    let materials = pool(&per_item);
    let fully_met = materials.iter().all(|l| l.have >= l.need) && commodities.iter().all(|c| c.have >= c.need);
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
        commodities,
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

/// The broker recipes of every pre-engineered variant of `item` the
/// tables or the commander's data know — what a broker event's payment
/// is matched against before it is taken as the plain module's.
fn preset_recipes_of<'a>(catalog: &'a ed_engineering::Catalog, learned: &[ed_ships::Preset], item: &str) -> Vec<&'a ed_engineering::Blueprint> {
    commands::slots()
        .presets_for(item)
        .into_iter()
        .cloned()
        .chain(learned.iter().filter(|p| p.item.eq_ignore_ascii_case(item)).cloned())
        .filter_map(|p| p.unlock.as_deref().and_then(|u| catalog.unlock_recipe(u)))
        .collect()
}

#[cfg(test)]
mod sirius_sink_tests {
    /// The maintainer's question of 2026-09-27: "is this also taking into
    /// account the materials I need for the modded heat sinks?" A ship
    /// with four plain launchers, a build with four Sirius ones: four
    /// swaps to the Sirius preset, one broker line for four units, and
    /// the materials are the wiki's recipe four times over.
    #[test]
    fn four_sirius_heat_sinks_cost_the_recipe_four_times() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let plain = |slot: &str| serde_json::json!({ "Slot": slot, "Item": "hpt_heatsinklauncher_turret_tiny", "On": true, "Priority": 0 });
        let loadout = serde_json::json!({
            "timestamp": "2026-09-27T10:00:00Z", "event": "Loadout", "Ship": "python_nx", "ShipID": 33, "ShipName": "", "ShipIdent": "",
            "UnladenMass": 600.0, "FuelCapacity": { "Main": 32.0 }, "MaxJumpRange": 30.0,
            "Modules": [plain("TinyHardpoint1"), plain("TinyHardpoint2"), plain("TinyHardpoint3"), plain("TinyHardpoint4")]
        });
        state.with_store(|s| {
            s.conn()
                .execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 1, '2026-09-27T10:00:00Z', 'Loadout', ?1)", [loadout.to_string()])
                .unwrap();
            ed_store::derive::derive_all(s.conn()).unwrap();
        });
        let sirius = |slot: &str| serde_json::json!({
            "Slot": slot, "Item": "hpt_heatsinklauncher_turret_tiny", "On": true, "Priority": 0,
            "Engineering": { "BlueprintName": "Misc_HeatSinkCapacity", "Level": 1, "Quality": 1.0, "Modifiers": [
                { "Label": "Mass", "Value": 0.65, "OriginalValue": 1.3, "LessIsGood": 1 },
                { "Label": "AmmoMaximum", "Value": 5.0, "OriginalValue": 3.0, "LessIsGood": 0 },
                { "Label": "ReloadTime", "Value": 17.5, "OriginalValue": 10.0, "LessIsGood": 1 }
            ] }
        });
        let slef = serde_json::json!([{ "header": { "appName": "EDSY" }, "data": {
            "event": "Loadout", "Ship": "python_nx", "ShipID": 33,
            "Modules": [sirius("TinyHardpoint1"), sirius("TinyHardpoint2"), sirius("TinyHardpoint3"), sirius("TinyHardpoint4")]
        }}]);
        let imported = crate::build_import::import(&state, Some(33), None, &slef.to_string()).expect("the import");
        assert_eq!(imported.swaps.len(), 4, "{:?}", imported.swaps);
        for sw in &imported.swaps {
            assert_eq!(sw.preset.as_deref(), Some("pe:hpt_heatsinklauncher_turret_tiny:misc_heatsinkcapacity:1"), "{sw:?}");
        }
        let swaps: Vec<crate::commands::ProposedSwap> = imported.swaps.iter().map(|sw| crate::commands::ProposedSwap { slot: sw.slot.clone(), item: sw.want_item.clone(), preset: sw.preset.clone() }).collect();
        let report = super::report(&state, Some(33), None, &[], &swaps, false, true).expect("the plan");
        assert_eq!(report.unlocks.len(), 1, "{:?}", report.unlocks);
        let u = &report.unlocks[0];
        assert_eq!((u.units, u.per_unit, u.unlocked.is_none(), u.note.is_none()), (4, true, true, true), "{u:?}");
        assert!(u.item_name.starts_with("Sirius Modified Heat Sink Launcher"), "{}", u.item_name);
        let need = |m: &str| u.materials.iter().find(|l| l.material == m).map(|l| l.need);
        assert_eq!((need("Mechanical Scrap"), need("Niobium"), need("Vanadium"), need("Mechanical Components")), (Some(32), Some(24), Some(24), Some(20)), "{:?}", u.materials);
        // And the pooled table above the broker section carries the same.
        let pooled = |m: &str| report.materials.iter().find(|l| l.material == m).map(|l| l.need);
        assert_eq!(pooled("Mechanical Scrap"), Some(32));

        // A grade 1 Ammo Capacity ROLL on the same launchers (mass doubled, as the
        // blueprint says) is not a Sirius: no swap, an engineering row instead.
        let roll = |slot: &str| serde_json::json!({
            "Slot": slot, "Item": "hpt_heatsinklauncher_turret_tiny", "On": true, "Priority": 0,
            "Engineering": { "BlueprintName": "Misc_HeatSinkCapacity", "Level": 1, "Quality": 0.9, "Modifiers": [
                { "Label": "Mass", "Value": 2.47, "OriginalValue": 1.3, "LessIsGood": 1 },
                { "Label": "AmmoMaximum", "Value": 4.0, "OriginalValue": 3.0, "LessIsGood": 0 },
                { "Label": "ReloadTime", "Value": 14.5, "OriginalValue": 10.0, "LessIsGood": 1 }
            ] }
        });
        let slef = serde_json::json!([{ "header": { "appName": "EDSY" }, "data": {
            "event": "Loadout", "Ship": "python_nx", "ShipID": 33,
            "Modules": [roll("TinyHardpoint1"), roll("TinyHardpoint2"), roll("TinyHardpoint3"), roll("TinyHardpoint4")]
        }}]);
        let imported = crate::build_import::import(&state, Some(33), None, &slef.to_string()).expect("the import");
        assert!(imported.swaps.is_empty(), "a roll is not a swap: {:?}", imported.swaps);
        assert_eq!(imported.items.len(), 4, "{:?}", imported.items);
        assert!(imported.items.iter().all(|it| it.blueprint.as_deref() == Some("Ammo Capacity") && it.target_grade == 1), "{:?}", imported.items);
    }

    /// Two modified 2A shards over plain ones: one broker line for two
    /// units, and the four Power Converters they need pooled in the open.
    #[test]
    fn two_modified_shards_pool_four_power_converters() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let plain = |slot: &str| serde_json::json!({ "Slot": slot, "Item": "hpt_guardian_shardcannon_fixed_medium", "On": true, "Priority": 0 });
        let loadout = serde_json::json!({
            "timestamp": "2026-09-27T10:00:00Z", "event": "Loadout", "Ship": "python_nx", "ShipID": 33, "ShipName": "", "ShipIdent": "",
            "UnladenMass": 600.0, "FuelCapacity": { "Main": 32.0 }, "MaxJumpRange": 30.0,
            "Modules": [plain("MediumHardpoint1"), plain("MediumHardpoint2")]
        });
        state.with_store(|s| {
            s.conn()
                .execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 1, '2026-09-27T10:00:00Z', 'Loadout', ?1)", [loadout.to_string()])
                .unwrap();
            ed_store::derive::derive_all(s.conn()).unwrap();
        });
        let modified = |slot: &str| serde_json::json!({
            "Slot": slot, "Item": "hpt_guardian_shardcannon_fixed_medium", "On": true, "Priority": 0,
            "Engineering": { "BlueprintName": "Weapon_LongRange", "Level": 1, "Quality": 1.0, "ExperimentalEffect": "special_super_penetrator_cooled" }
        });
        let slef = serde_json::json!([{ "header": { "appName": "EDSY" }, "data": {
            "event": "Loadout", "Ship": "python_nx", "ShipID": 33, "Modules": [modified("MediumHardpoint1"), modified("MediumHardpoint2")]
        }}]);
        let imported = crate::build_import::import(&state, Some(33), None, &slef.to_string()).expect("the import");
        assert_eq!(imported.swaps.len(), 2, "{:?}", imported.swaps);
        let swaps: Vec<crate::commands::ProposedSwap> = imported.swaps.iter().map(|sw| crate::commands::ProposedSwap { slot: sw.slot.clone(), item: sw.want_item.clone(), preset: sw.preset.clone() }).collect();
        let report = super::report(&state, Some(33), None, &[], &swaps, false, true).expect("the plan");
        assert_eq!(report.unlocks.len(), 1);
        assert_eq!((report.unlocks[0].units, report.unlocks[0].per_unit), (2, true));
        assert_eq!(report.commodities.iter().map(|c| (c.name.as_str(), c.need, c.have)).collect::<Vec<_>>(), vec![("Power Converter", 4, 0)]);
        assert!(!report.fully_met, "four Power Converters short");

        // One bought and in storage at Mbooni (the journal's StoredModules, replayed
        // into the table): one line for it, one unit still to buy, two converters.
        state.with_store(|s| {
            s.conn().execute(
                "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 2, '2026-09-27T11:00:00Z', 'StoredModules', ?1)",
                [r#"{ "timestamp":"2026-09-27T11:00:00Z", "event":"StoredModules", "MarketID":1, "StationName":"Prospect's Deep", "StarSystem":"Mbooni", "Items":[ { "Name":"$hpt_guardian_shardcannon_fixed_medium_name;", "StarSystem":"Mbooni", "StorageSlot":7, "EngineerModifications":"Weapon_LongRange", "Level":1, "Quality":1.0, "Hot":false, "TransferCost":51000, "TransferTime":720, "BuyPrice":420807 } ] }"#],
            ).unwrap();
            ed_store::derive::derive_all(s.conn()).unwrap();
        });
        let report = super::report(&state, Some(33), None, &[], &swaps, false, true).expect("the plan");
        let stored = report.unlocks.iter().find(|u| u.stored.is_some()).expect("the stored one");
        assert_eq!((stored.units, stored.stored.as_deref()), (1, Some("in storage at Mbooni · transfer 51000 cr, 12 min")));
        let to_buy = report.unlocks.iter().find(|u| u.stored.is_none()).expect("the one to buy");
        assert_eq!((to_buy.units, to_buy.per_unit), (1, true));
        assert_eq!(report.commodities.iter().map(|c| (c.name.as_str(), c.need)).collect::<Vec<_>>(), vec![("Power Converter", 2)]);

        // The second bought at the broker AFTER the snapshot (the game lists it in
        // storage only at the next dock): it counts as owned now, nothing to buy.
        state.with_store(|s| {
            s.conn().execute(
                "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 3, '2026-09-27T11:05:00Z', 'TechnologyBroker', ?1)",
                [r#"{ "timestamp":"2026-09-27T11:05:00Z", "event":"TechnologyBroker", "BrokerType":"guardian", "MarketID":1, "ItemsUnlocked":[{ "Name":"Hpt_Guardian_ShardCannon_Fixed_Medium" }], "Commodities":[{ "Name":"powerconverter", "Count":2 }], "Materials":[{ "Name":"guardian_weaponblueprint", "Count":1 },{ "Name":"guardian_sentinel_wreckagecomponents", "Count":5 },{ "Name":"guardian_techcomponent", "Count":5 },{ "Name":"germanium", "Count":4 }] }"#],
            ).unwrap();
        });
        let report = super::report(&state, Some(33), None, &[], &swaps, false, true).expect("the plan");
        assert!(report.unlocks.iter().all(|u| u.stored.is_some()), "{:?}", report.unlocks);
        assert!(report.unlocks.iter().any(|u| u.stored.as_deref().is_some_and(|s| s.starts_with("bought at the broker at 11:05"))), "{:?}", report.unlocks);
        assert!(report.commodities.is_empty() && report.fully_met, "nothing left to buy: {:?}", report.commodities);

        // Fitted, as the journal wrote the maintainer's six (lowercase blueprint,
        // grade 1, a broker's EngineerID and no engineer): the saved swaps are
        // done — no broker line at all — and a fresh import finds nothing to swap.
        let fitted = |slot: &str| serde_json::json!({
            "Slot": slot, "Item": "hpt_guardian_shardcannon_fixed_medium", "On": true, "Priority": 0,
            "Engineering": { "EngineerID": 300001, "BlueprintID": 1, "BlueprintName": "weapon_longrange", "Level": 1, "Quality": 0.0, "Modifiers": [] }
        });
        let loadout = serde_json::json!({
            "timestamp": "2026-09-27T12:00:00Z", "event": "Loadout", "Ship": "python_nx", "ShipID": 33, "ShipName": "", "ShipIdent": "",
            "UnladenMass": 600.0, "FuelCapacity": { "Main": 32.0 }, "MaxJumpRange": 30.0,
            "Modules": [fitted("MediumHardpoint1"), fitted("MediumHardpoint2")]
        });
        state.with_store(|s| {
            s.conn()
                .execute("INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', 4, '2026-09-27T12:00:00Z', 'Loadout', ?1)", [loadout.to_string()])
                .unwrap();
            ed_store::derive::derive_all(s.conn()).unwrap();
        });
        let report = super::report(&state, Some(33), None, &[], &swaps, false, true).expect("the plan");
        assert!(report.unlocks.is_empty(), "fitted already: {:?}", report.unlocks);
        let imported = crate::build_import::import(&state, Some(33), None, &slef.to_string()).expect("the import");
        assert!(imported.swaps.is_empty(), "a fresh import sees them fitted: {:?}", imported.swaps);
    }
}
