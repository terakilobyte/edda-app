use crate::capabilities::{carrier, commander, galaxy, CapError};
use crate::state::AppState;
use ed_engineering::{Blueprint, GapReport};
use ed_store::lookup::{self, StationInfo, StationWithService, SystemInfo};
use ed_store::query::{self, Engineer, Location, NavTarget};
use serde::Serialize;
use tauri::State;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Ship status for the header panel and the overlay.
#[derive(Debug, Serialize)]
pub struct ShipStatus {
    pub location: Option<Location>,
    pub nav: Option<NavTarget>,
    pub ship: Option<String>,
    pub ship_name: Option<String>,
    pub cargo_capacity: Option<i64>,
    pub cargo_count: i64,
    pub fuel_main: Option<f64>,
    pub fuel_capacity: Option<f64>,
    /// Powerplay for the current system, if known.
    pub controlling_power: Option<String>,
    pub power_state: Option<String>,
}

#[tauri::command]
pub async fn get_status(state: State<'_, AppState>) -> Result<ShipStatus, String> {
    state.with_read(|store| {
        let conn = store.conn();
        let location = query::location(conn).map_err(err)?;
        let nav = query::nav_target(conn).map_err(err)?;

        let (ship, ship_name, cargo_capacity): (Option<String>, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT ship, ship_name, cargo_capacity FROM loadout WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap_or((None, None, None));

        let cargo_count: i64 = conn
            .query_row("SELECT COALESCE(SUM(count), 0) FROM cargo", [], |r| r.get(0))
            .unwrap_or(0);

        // Fuel level lives in Status.json, which the game rewrites
        // continuously; the tank size is per-ship and comes from the last
        // Loadout. A Kestrel holds 16 t, so a fixed "32" made a full tank
        // look half empty.
        let fuel_main = conn
            .query_row(
                "SELECT json_extract(raw, '$.Fuel.FuelMain') FROM snapshots WHERE name = 'Status.json'",
                [],
                |r| r.get::<_, Option<f64>>(0),
            )
            .ok()
            .flatten();
        let fuel_capacity = conn
            .query_row(
                "SELECT json_extract(raw, '$.FuelCapacity.Main') FROM events
                 WHERE event = 'Loadout' ORDER BY file DESC, offset DESC LIMIT 1",
                [],
                |r| r.get::<_, Option<f64>>(0),
            )
            .ok()
            .flatten();

        let (controlling_power, power_state) = location
            .as_ref()
            .and_then(|l| l.system_name.as_deref())
            .and_then(|s| query::powerplay_for_system(conn, s).ok().flatten())
            .map(|p| (p.controlling_power, p.powerplay_state))
            .unwrap_or((None, None));

        Ok(ShipStatus {
            location,
            nav,
            ship: ship.as_deref().map(ed_route::ships::display_name),
            ship_name,
            cargo_capacity,
            cargo_count,
            fuel_main,
            fuel_capacity,
            controlling_power,
            power_state,
        })
    })
}

#[derive(Debug, Serialize)]
pub struct InventoryItem {
    pub symbol: String,
    pub name: String,
    pub category: String,
    pub count: i64,
}

#[tauri::command]
pub async fn get_inventory(state: State<'_, AppState>) -> Result<Vec<InventoryItem>, String> {
    state.with_read(|store| {
        let catalog = ed_journal::Catalog::load();
        let mut out = Vec::new();
        let mut stmt = store
            .conn()
            .prepare(
                "SELECT symbol, count, 'Material' FROM materials WHERE count > 0
                 UNION ALL
                 SELECT symbol, count, 'Cargo' FROM cargo WHERE count > 0",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .map_err(err)?;
        for row in rows {
            let (symbol, count, kind) = row.map_err(err)?;
            let item = catalog.by_symbol(&symbol);
            out.push(InventoryItem {
                name: item
                    .map(|i| i.name.clone())
                    // A lookup miss is shown, never silently treated as zero.
                    .unwrap_or_else(|| format!("(unknown: {symbol})")),
                category: item.map(|i| i.category.clone()).unwrap_or(kind),
                symbol,
                count,
            });
        }
        out.sort_by(|a, b| a.category.cmp(&b.category).then(a.name.cmp(&b.name)));
        Ok(out)
    })
}

#[derive(Debug, Serialize)]
pub struct CommodityOption {
    pub name: String,
    pub detail: String,
}

/// The canonical commodity catalog is tiny and embedded in the executable.
/// Returning it in one shot lets the webview autocomplete entirely in memory.
#[tauri::command]
pub async fn list_commodities() -> Vec<CommodityOption> {
    let catalog = ed_journal::Catalog::load();
    let mut items: Vec<_> = catalog
        .commodities()
        .map(|item| CommodityOption {
            name: item.name.clone(),
            detail: item.category.clone(),
        })
        .collect();
    items.sort_unstable_by(|a, b| a.name.cmp(&b.name));
    items
}

// ── Engineering ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_module_types(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(commander::module_types(&state))
}

#[derive(Debug, Serialize)]
pub struct BlueprintOption {
    pub name: String,
    /// Grades that exist; empty means an experimental effect.
    pub grades: Vec<i64>,
}

#[tauri::command]
pub async fn list_blueprint_names(
    state: State<'_, AppState>,
    module_type: String,
) -> Result<Vec<BlueprintOption>, String> {
    Ok({
        state
            .engineering
            .blueprint_names_for(&module_type)
            .into_iter()
            .map(|n| BlueprintOption {
                name: n.to_string(),
                grades: state.engineering.grades_for(&module_type, n),
            })
            .collect()
    })
}

#[tauri::command]
pub async fn search_blueprints(
    state: State<'_, AppState>,
    text: String,
) -> Result<Vec<Blueprint>, String> {
    Ok({
        state
            .engineering
            .search(&text)
            .into_iter()
            .take(50).cloned()
            .collect()
    })
}

#[tauri::command]
pub async fn check_blueprint(
    state: State<'_, AppState>,
    module_type: String,
    name: String,
    from_grade: i64,
    target_grade: i64,
    minimum: Option<bool>,
    complete: Option<bool>,
) -> Result<GapReport, String> {
    let have = state.with_read(|store| {
        let catalog = ed_journal::Catalog::load();
        let mut have = std::collections::HashMap::new();
        if let Ok(mut stmt) = store.conn().prepare("SELECT symbol, count FROM materials") {
            if let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            {
                for row in rows.flatten() {
                    // gap_report keys on display names, so resolve here.
                    let name = catalog
                        .by_symbol(&row.0)
                        .map(|i| i.name.clone())
                        .unwrap_or(row.0);
                    have.insert(name, row.1);
                }
            }
        }
        have
    });
    // Realistic by default: N rolls at grade N to unlock the next.
    Ok(if minimum.unwrap_or(false) {
        state
            .engineering
            .gap_report(&module_type, &name, from_grade, target_grade, &have)
    } else {
        state.engineering.gap_report_realistic(
            &module_type,
            &name,
            from_grade,
            target_grade,
            complete.unwrap_or(true),
            &have,
        )
    })
}

/// Live material inventory keyed by display name.
fn material_inventory(conn: &rusqlite::Connection) -> std::collections::HashMap<String, i64> {
    let catalog = ed_journal::Catalog::load();
    let mut have = std::collections::HashMap::new();
    if let Ok(mut stmt) = conn.prepare("SELECT symbol, count FROM materials") {
        if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        {
            for (symbol, count) in rows.flatten() {
                let name = catalog
                    .by_symbol(&symbol)
                    .map(|i| i.name.clone())
                    .unwrap_or(symbol);
                have.insert(name, count);
            }
        }
    }
    have
}

#[derive(Debug, Serialize)]
pub struct TraderStop {
    pub kind: ed_engineering::trader::TraderKind,
    pub nearest: Vec<StationWithService>,
}

/// A plan's shortfall turned into trades at material traders.
#[derive(Debug, Serialize)]
pub struct ShoppingReport {
    /// Every line the plan needs, after the shopping list is applied.
    pub short: Vec<(String, i64)>,
    pub list: ed_engineering::trader::ShoppingList,
    pub traders: Vec<TraderStop>,
    /// Farm-then-trade plans for what is still short after trading.
    pub farm: Vec<FarmPlan>,
    pub origin_system: Option<String>,
}

/// Shared by the command and the ship computer tool: gap → surplus → trades
/// → nearest traders of each kind needed, from the commander's position.
pub fn shopping_for(
    conn: &rusqlite::Connection,
    galaxy: Option<&ed_galaxy::Galaxy>,
    engineering: &ed_engineering::Catalog,
    module_type: &str,
    blueprint: Option<&str>,
    from_grade: i64,
    target_grade: i64,
    minimum: bool,
    complete: bool,
    experimental: Option<&str>,
) -> Result<ShoppingReport, String> {
    let have = material_inventory(conn);
    // Pool every line the plan needs (blueprint + experimental) by material.
    let mut need: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    if let Some(bp) = blueprint {
        let gap = if minimum {
            engineering.gap_report(module_type, bp, from_grade, target_grade, &have)
        } else {
            engineering.gap_report_realistic(
                module_type,
                bp,
                from_grade,
                target_grade,
                complete,
                &have,
            )
        };
        for l in gap.lines {
            *need.entry(l.material).or_default() += l.need;
        }
    }
    if let Some(x) = experimental {
        let gap = engineering
            .experimental_gap(module_type, x, &have)
            .ok_or_else(|| format!("no experimental effect {x:?} for {module_type:?}"))?;
        for l in gap.lines {
            *need.entry(l.material).or_default() += l.need;
        }
    }
    if need.is_empty() {
        return Err("nothing selected to plan".into());
    }
    let mut short: Vec<(String, i64)> = need
        .iter()
        .filter_map(|(m, n)| {
            let h = *have.get(m).unwrap_or(&0);
            (h < *n).then(|| (m.clone(), n - h))
        })
        .collect();
    short.sort();
    // Spare = what we carry beyond what this plan itself consumes.
    let surplus: std::collections::HashMap<String, i64> = have
        .iter()
        .map(|(m, h)| (m.clone(), h - need.get(m).copied().unwrap_or(0)))
        .filter(|(_, s)| *s > 0)
        .collect();
    let jc = ed_journal::Catalog::load();
    let meta: Vec<ed_engineering::trader::MaterialMeta> = jc
        .materials()
        .filter_map(|i| {
            Some(ed_engineering::trader::MaterialMeta {
                name: i.name.clone(),
                kind: ed_engineering::trader::TraderKind::parse(&i.category)?,
                group: i.group.clone(),
                grade: i.grade,
            })
        })
        .collect();
    let list = ed_engineering::trader::shopping_list(&short, &surplus, &meta);

    let origin_system = query::location(conn)
        .ok()
        .flatten()
        .and_then(|l| l.system_name);
    let origin = origin_system.as_deref().and_then(|n| galaxy::coords_hint(conn, galaxy, n));
    let dist = |name: &str| -> Option<f64> {
        let o = origin?;
        let c = galaxy::coords_hint(conn, galaxy, name)?;
        Some(((c.0 - o.0).powi(2) + (c.1 - o.1).powi(2) + (c.2 - o.2).powi(2)).sqrt())
    };
    // What trading can't cover: farm something (ideally high grade, same
    // group) at a known site and trade it in. Direct pickups included.
    let farmable: Vec<(String, String, Option<String>, Option<String>)> =
        ed_engineering::sources::all()
            .into_iter()
            .filter(|s| s.system.is_some())
            .flat_map(|s| {
                s.materials
                    .iter()
                    .map(move |m| (m.clone(), s.site.clone(), s.system.clone(), s.body.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
    let farm: Vec<FarmPlan> = list
        .still_short
        .iter()
        .filter_map(|(name, n)| {
            let target = meta.iter().find(|m| m.name.eq_ignore_ascii_case(name))?;
            let mut options: Vec<FarmOptionAt> =
                ed_engineering::trader::farm_options(target, *n, &farmable, &meta)
                    .into_iter()
                    .map(|o| FarmOptionAt {
                        distance_ly: o.system.as_deref().and_then(dist),
                        option: o,
                    })
                    .collect();
            // Fewest units to collect first; distance breaks ties within a factor of 3.
            options.sort_by(|a, b| {
                let (ca, cb) = (a.option.collect as f64, b.option.collect as f64);
                if ca.max(cb) / ca.min(cb).max(1.0) > 3.0 {
                    ca.partial_cmp(&cb).unwrap()
                } else {
                    a.distance_ly
                        .unwrap_or(f64::MAX)
                        .partial_cmp(&b.distance_ly.unwrap_or(f64::MAX))
                        .unwrap()
                }
            });
            options.truncate(4);
            Some(FarmPlan {
                material: name.clone(),
                needed: *n,
                options,
            })
        })
        .collect();
    let mut trader_kinds = list.traders_needed.clone();
    for plan in &farm {
        for o in &plan.options {
            if o.option.rate.is_some() && !trader_kinds.contains(&o.option.kind) {
                trader_kinds.push(o.option.kind);
            }
        }
    }
    // The nearest traders come from the API; `fill_traders` adds them.
    let traders = trader_kinds.iter().map(|k| TraderStop { kind: *k, nearest: Vec::new() }).collect();
    Ok(ShoppingReport {
        short,
        list,
        traders,
        farm,
        origin_system,
    })
}

#[derive(Debug, Serialize)]
pub struct FarmOptionAt {
    #[serde(flatten)]
    pub option: ed_engineering::trader::FarmOption,
    pub distance_ly: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct FarmPlan {
    pub material: String,
    pub needed: i64,
    pub options: Vec<FarmOptionAt>,
}

/// Where to collect a material: community-known sites plus the commander's
/// own pickups from the journal, each with distance from the current system.
#[derive(Debug, Serialize)]
pub struct MaterialSources {
    pub material: String,
    pub known: Vec<KnownSiteAt>,
    pub witnessed: Vec<WitnessedAt>,
    pub origin_system: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct KnownSiteAt {
    #[serde(flatten)]
    pub site: ed_engineering::sources::KnownSite,
    pub distance_ly: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct WitnessedAt {
    #[serde(flatten)]
    pub source: ed_store::materials::WitnessedSource,
    pub distance_ly: Option<f64>,
}

pub fn sources_for(conn: &rusqlite::Connection, galaxy: Option<&ed_galaxy::Galaxy>, material: &str) -> Result<MaterialSources, String> {
    let origin_system = query::location(conn)
        .ok()
        .flatten()
        .and_then(|l| l.system_name);
    let origin = origin_system.as_deref().and_then(|n| galaxy::coords_hint(conn, galaxy, n));
    let dist = |name: &str| -> Option<f64> {
        let o = origin?;
        let c = galaxy::coords_hint(conn, galaxy, name)?;
        Some(((c.0 - o.0).powi(2) + (c.1 - o.1).powi(2) + (c.2 - o.2).powi(2)).sqrt())
    };
    let mut known: Vec<KnownSiteAt> = ed_engineering::sources::for_material(material)
        .into_iter()
        .map(|site| KnownSiteAt {
            distance_ly: site.system.as_deref().and_then(dist),
            site,
        })
        .collect();
    known.sort_by(|a, b| {
        a.distance_ly
            .unwrap_or(f64::MAX)
            .partial_cmp(&b.distance_ly.unwrap_or(f64::MAX))
            .unwrap()
    });
    let witnessed = ed_store::materials::witnessed_sources(conn, material)
        .map_err(err)?
        .into_iter()
        .take(8)
        .map(|source| WitnessedAt {
            distance_ly: dist(&source.system),
            source,
        })
        .collect();
    Ok(MaterialSources {
        material: material.to_string(),
        known,
        witnessed,
        origin_system,
    })
}

#[tauri::command]
pub async fn material_sources(
    state: State<'_, AppState>,
    material: String,
) -> Result<MaterialSources, String> {
    let galaxy = state.routing.galaxy(&state.data_dir);
    state.with_read(|s| sources_for(s.conn(), galaxy.as_deref(), &material))
}

/// The nearest trader of each kind the plan needs, from the community
/// API, around the commander's system (150 ly, five each).
pub(crate) async fn fill_traders(state: &AppState, report: &mut ShoppingReport) {
    let Some(system) = report.origin_system.clone() else { return };
    for stop in &mut report.traders {
        let kind = format!("{:?}", stop.kind).to_lowercase();
        stop.nearest = crate::remote_lookup::nearest_material_traders(state, &system, &kind, 150.0, 5)
            .await
            .unwrap_or_default();
    }
}

#[tauri::command]
pub async fn material_shopping(
    state: State<'_, AppState>,
    module_type: String,
    blueprint: Option<String>,
    from_grade: Option<i64>,
    target_grade: Option<i64>,
    minimum: Option<bool>,
    complete: Option<bool>,
    experimental: Option<String>,
) -> Result<ShoppingReport, String> {
    let galaxy = state.routing.galaxy(&state.data_dir);
    let mut report = state.with_read(|s| {
        shopping_for(
            s.conn(),
            galaxy.as_deref(),
            &state.engineering,
            &module_type,
            blueprint.as_deref().filter(|b| !b.is_empty()),
            from_grade.unwrap_or(0),
            target_grade.unwrap_or(5),
            minimum.unwrap_or(false),
            complete.unwrap_or(true),
            experimental.as_deref().filter(|x| !x.is_empty()),
        )
    })?;
    fill_traders(&state, &mut report).await;
    Ok(report)
}

/// Which engineers can work a blueprint, and whether the commander has them.
///
/// This is the check that has to run *before* any recommendation: an
/// inaccessible blueprint is not a suggestion, and no external site knows
/// your unlock state.
#[derive(Debug, Serialize)]
pub struct EngineerAccess {
    pub engineer: String,
    pub status: String,
    pub rank: Option<i64>,
    pub unlocked: bool,
}

#[derive(Debug, Serialize)]
pub struct BlueprintAccess {
    pub module_type: String,
    pub name: String,
    pub grade: i64,
    pub engineers: Vec<EngineerAccess>,
    pub reachable: bool,
    /// Highest grade actually reachable with currently unlocked engineers.
    pub max_reachable_grade: Option<i64>,
}

#[tauri::command]
pub async fn blueprint_access(
    state: State<'_, AppState>,
    module_type: String,
    name: String,
    grade: i64,
) -> Result<BlueprintAccess, String> {
    let engineers = state
        .with_read(|s| query::engineers(s.conn()))
        .map_err(err)?;
    let status_of = |n: &str| -> (String, Option<i64>, bool) {
        match engineers.iter().find(|e| e.name.eq_ignore_ascii_case(n)) {
            Some(e) => (
                e.progress.clone().unwrap_or_else(|| "Unknown".into()),
                e.rank,
                e.is_unlocked(),
            ),
            // Absent from the journal entirely is worse than "Known".
            None => ("Not known".to_string(), None, false),
        }
    };

    let bp = state
        .engineering
        .find(&module_type, &name, grade)
        .ok_or_else(|| format!("no blueprint {name:?} grade {grade} for {module_type:?}"))?;

    let access: Vec<EngineerAccess> = bp
        .engineers
        .iter()
        .map(|e| {
            let (status, rank, unlocked) = status_of(e);
            EngineerAccess {
                engineer: e.clone(),
                status,
                rank,
                unlocked,
            }
        })
        .collect();
    let reachable = access.iter().any(|a| a.unlocked);

    let mut max_reachable_grade = None;
    for g in 1..=5 {
        if let Some(b) = state.engineering.find(&module_type, &name, g) {
            if b.engineers.iter().any(|e| status_of(e).2) {
                max_reachable_grade = Some(g);
            }
        }
    }

    Ok(BlueprintAccess {
        module_type,
        name,
        grade,
        engineers: access,
        reachable,
        max_reachable_grade,
    })
}

/// One module on the current ship, with its engineering resolved to the
/// blueprint data's names where the mapping is known.
#[derive(Debug, Serialize)]
pub struct ShipModule {
    pub slot: String,
    /// Outfitting name of the slot ("Optional 1 (size 7)", "Thrusters").
    pub slot_name: String,
    pub item: String,
    /// Outfitting name of the module ("Fuel Scoop 7A").
    pub item_name: String,
    /// Blueprint-data module type ("Frame Shift Drive"), if the item is known.
    pub module_type: Option<String>,
    pub blueprint_symbol: Option<String>,
    /// Blueprint-data name ("Increased FSD Range"), if the symbol is known.
    pub blueprint: Option<String>,
    pub grade: Option<i64>,
    pub quality: Option<f64>,
    pub engineer: Option<String>,
    pub experimental: Option<String>,
}

/// The latest `Loadout` for one ship (by the game's ShipID), or the current ship.
fn loadout_raw(state: &AppState, ship_id: Option<i64>) -> Result<String, String> {
    match ship_id {
        None => state.with_read(|s| ed_store::session::latest_event_raw(s.conn(), "Loadout")).map_err(err)?.ok_or_else(|| "no Loadout in the journal yet".into()),
        Some(id) => state
            .with_read(|s| {
                s.conn()
                    .query_row("SELECT raw FROM events WHERE event = 'Loadout' AND json_extract(raw, '$.ShipID') = ?1 ORDER BY ts DESC LIMIT 1", [id], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())
            })
            .map_err(|_| format!("no Loadout for ship {id}")),
    }
}

/// Ships with a Loadout in the journal, currently owned by default.
/// Item 52 A: the carrier card's data — the same read the model's tool uses.
#[tauri::command]
pub async fn carrier_status(state: State<'_, AppState>) -> Result<serde_json::Value, CapError> {
    carrier::status(&state)
}

#[tauri::command]
pub async fn ships_list(state: State<'_, AppState>, include_historical: Option<bool>) -> Result<Vec<commander::ShipSummary>, CapError> {
    commander::ships_list(&state, &commander::ShipsListRequest { include_historical: include_historical.unwrap_or(false) })
}

/// A theorycrafted modification for one slot of an exported build.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ProposedEngineering {
    /// The Loadout slot to modify (the journal's `Slot` value).
    pub slot: String,
    /// Blueprint-data module type and name ("Frame Shift Drive",
    /// "Increased FSD Range"); translated to the journal symbol here.
    pub module_type: String,
    pub blueprint: String,
    pub grade: i64,
}

/// Replace one module's `Engineering` block with a proposed blueprint at
/// full grade. Modifiers are dropped: EDSY and Coriolis recompute the
/// grade's nominal values, which is what a plan (not yet rolled) means.
fn apply_proposed_engineering(data: &mut serde_json::Value, proposed: &ProposedEngineering) -> Result<(), String> {
    let symbol = ed_engineering::journal::symbol_for_blueprint(&proposed.blueprint, &proposed.module_type)
        .ok_or_else(|| format!("no journal symbol for {} / {}", proposed.module_type, proposed.blueprint))?;
    let modules = data
        .get_mut("Modules")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("Loadout has no Modules")?;
    let module = modules
        .iter_mut()
        .find(|m| m.get("Slot").and_then(serde_json::Value::as_str) == Some(proposed.slot.as_str()))
        .ok_or_else(|| format!("no module in slot {}", proposed.slot))?;
    // Kept if already applied: the experimental effect survives a grade plan.
    let experimental = module
        .get("Engineering")
        .and_then(|e| e.get("ExperimentalEffect"))
        .cloned();
    let mut engineering = serde_json::json!({
        "BlueprintName": symbol,
        "Level": proposed.grade,
        "Quality": 1.0,
    });
    if let Some(effect) = experimental {
        engineering["ExperimentalEffect"] = effect;
    }
    module["Engineering"] = engineering;
    Ok(())
}

/// The ship's build as SLEF (the Loadout event with a header), which EDSY
/// and Coriolis import as-is. With `proposed`, one slot's engineering is
/// replaced by the plan (at nominal full-grade values) so the build can be
/// theorycrafted before any materials are spent.
#[tauri::command]
pub async fn ship_slef(state: State<'_, AppState>, ship_id: Option<i64>, proposed: Option<ProposedEngineering>) -> Result<String, String> {
    let raw = loadout_raw(&state, ship_id)?;
    let mut data: serde_json::Value = serde_json::from_str(&raw).map_err(err)?;
    if let Some(proposed) = &proposed {
        apply_proposed_engineering(&mut data, proposed)?;
    }
    let slef = serde_json::json!([{ "header": { "appName": "EDDA", "appVersion": env!("CARGO_PKG_VERSION"), "appURL": "https://edda-app.com/" }, "data": data }]);
    serde_json::to_string_pretty(&slef).map_err(err)
}

#[derive(Debug, Serialize)]
pub struct ShipLinks {
    pub edsy: String,
    pub coriolis: String,
}

/// EDSY and Coriolis open with the build loaded: both take the Loadout
/// event gzip-compressed and base64-encoded in the URL (as EDMC does).
#[tauri::command]
pub async fn ship_links(
    state: State<'_, AppState>,
    ship_id: Option<i64>,
) -> Result<ShipLinks, String> {
    use base64::Engine;
    use std::io::Write;
    let raw = loadout_raw(&state, ship_id)?;
    let compact = serde_json::from_str::<serde_json::Value>(&raw)
        .map_err(err)?
        .to_string();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(compact.as_bytes()).map_err(err)?;
    let bytes = gz.finish().map_err(err)?;
    let encoded = base64::engine::general_purpose::URL_SAFE.encode(bytes);
    Ok(ShipLinks {
        edsy: format!("https://edsy.org/#/I={encoded}"),
        coriolis: format!("https://coriolis.io/import?data={encoded}"),
    })
}

/// The ship's modules from the latest `Loadout` (or one ship's, by ShipID), engineered ones first.
#[tauri::command]
pub async fn ship_modules(
    state: State<'_, AppState>,
    ship_id: Option<i64>,
) -> Result<ShipLoadout, String> {
    let raw = loadout_raw(&state, ship_id)?;
    let v: serde_json::Value = serde_json::from_str(&raw).map_err(err)?;
    let mut out: Vec<ShipModule> = v
        .get("Modules")
        .and_then(serde_json::Value::as_array)
        .map(|mods| {
            mods.iter()
                .filter_map(|m| {
                    let s = |k: &str| {
                        m.get(k)
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    };
                    let slot = s("Slot").unwrap_or_default();
                    if ed_journal::modules::is_cosmetic_slot(&slot) {
                        return None;
                    }
                    let item = s("Item")?;
                    let module_type =
                        ed_engineering::journal::module_type_for_item(&item).map(str::to_string);
                    let eng = m.get("Engineering");
                    let e = |k: &str| {
                        eng.and_then(|e| e.get(k))
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    };
                    let symbol = e("BlueprintName");
                    let blueprint = match (&symbol, &module_type) {
                        (Some(sym), Some(mt)) => {
                            ed_engineering::journal::blueprint_for_symbol(sym, mt)
                                .map(str::to_string)
                        }
                        _ => None,
                    };
                    Some(ShipModule {
                        slot_name: ed_journal::modules::slot_name(&slot),
                        item_name: ed_journal::modules::item_name(&item),
                        slot,
                        item,
                        module_type,
                        blueprint_symbol: symbol,
                        blueprint,
                        grade: eng
                            .and_then(|e| e.get("Level"))
                            .and_then(serde_json::Value::as_i64),
                        quality: eng
                            .and_then(|e| e.get("Quality"))
                            .and_then(serde_json::Value::as_f64),
                        engineer: e("Engineer"),
                        experimental: e("ExperimentalEffect_Localised")
                            .or_else(|| e("ExperimentalEffect")),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|m| (m.grade.is_none(), m.slot.clone()));
    let s = |k: &str| {
        v.get(k)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    Ok(ShipLoadout {
        ship: s("Ship")
            .map(|t| ed_journal::ships::display_name_or(&t, s("Ship_Localised").as_deref()))
            .unwrap_or_default(),
        ship_name: s("ShipName")
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty()),
        ship_ident: s("ShipIdent"),
        modules: out,
    })
}

#[derive(Debug, Serialize)]
pub struct ShipLoadout {
    pub ship: String,
    pub ship_name: Option<String>,
    pub ship_ident: Option<String>,
    pub modules: Vec<ShipModule>,
}

/// An experimental effect: its materials against inventory, and who can
/// apply it. No grade exists, so none is asked for.
#[derive(Debug, Serialize)]
pub struct ExperimentalCheck {
    pub gap: GapReport,
    pub engineers: Vec<EngineerAccess>,
    pub reachable: bool,
}

#[tauri::command]
pub async fn check_experimental(
    state: State<'_, AppState>,
    module_type: String,
    name: String,
) -> Result<ExperimentalCheck, String> {
    let (have, engineers) = state.with_read(|store| {
        let catalog = ed_journal::Catalog::load();
        let mut have = std::collections::HashMap::new();
        if let Ok(mut stmt) = store.conn().prepare("SELECT symbol, count FROM materials") {
            if let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            {
                for (symbol, count) in rows.flatten() {
                    have.insert(
                        catalog
                            .by_symbol(&symbol)
                            .map(|i| i.name.clone())
                            .unwrap_or(symbol),
                        count,
                    );
                }
            }
        }
        (have, query::engineers(store.conn()).unwrap_or_default())
    });
    let gap = state
        .engineering
        .experimental_gap(&module_type, &name, &have)
        .ok_or_else(|| format!("no experimental effect {name:?} for {module_type:?}"))?;
    let bp = state
        .engineering
        .find_experimental(&module_type, &name)
        .ok_or("missing")?;
    let access: Vec<EngineerAccess> = bp
        .engineers
        .iter()
        .map(|n| {
            let e = engineers.iter().find(|e| e.name.eq_ignore_ascii_case(n));
            EngineerAccess {
                engineer: n.clone(),
                status: e
                    .and_then(|e| e.progress.clone())
                    .unwrap_or_else(|| "Not known".into()),
                rank: e.and_then(|e| e.rank),
                unlocked: e.is_some_and(|e| e.is_unlocked()),
            }
        })
        .collect();
    let reachable = access.iter().any(|a| a.unlocked);
    Ok(ExperimentalCheck {
        gap,
        engineers: access,
        reachable,
    })
}

#[tauri::command]
pub async fn list_engineers(state: State<'_, AppState>) -> Result<Vec<Engineer>, CapError> {
    commander::engineers(&state)
}

// ── Galaxy lookup ────────────────────────────────────────────────────
//
// Thin adapters: named IPC arguments become the capability's request
// struct, the response goes back as-is. Defaults live in the request.

#[tauri::command]
pub async fn find_system(state: State<'_, AppState>, name: String) -> Result<Option<SystemInfo>, CapError> {
    let req = galaxy::FindSystemRequest { name };
    crate::remote_lookup::find_system(&state, &req).await.ok_or_else(|| crate::remote_lookup::api_down("system"))
}

#[tauri::command]
pub async fn stations_in_system(
    state: State<'_, AppState>,
    system: String,
    include_carriers: Option<bool>,
    include_minor: Option<bool>,
) -> Result<Vec<StationInfo>, CapError> {
    let d = galaxy::StationsInSystemRequest::default();
    let req = galaxy::StationsInSystemRequest {
        system,
        include_carriers: include_carriers.unwrap_or(d.include_carriers),
        include_minor: include_minor.unwrap_or(d.include_minor),
    };
    crate::remote_lookup::stations_in_system(&state, &req).await.ok_or_else(|| crate::remote_lookup::api_down("stations"))
}

#[tauri::command]
pub async fn find_station(state: State<'_, AppState>, name: String) -> Result<Vec<StationInfo>, CapError> {
    let req = galaxy::FindStationRequest { name };
    crate::remote_lookup::find_station(&state, &req).await.ok_or_else(|| crate::remote_lookup::api_down("stations"))
}

#[tauri::command]
pub async fn nearest_service(
    state: State<'_, AppState>,
    system: String,
    service: String,
    min_pad: Option<String>,
    radius_ly: Option<f64>,
    include_carriers: Option<bool>,
) -> Result<Vec<StationWithService>, CapError> {
    let d = galaxy::NearestServiceRequest::default();
    let req = galaxy::NearestServiceRequest {
        system: Some(system),
        service,
        min_pad,
        radius_ly: radius_ly.unwrap_or(d.radius_ly),
        include_carriers: include_carriers.unwrap_or(d.include_carriers),
    };
    crate::remote_lookup::nearest_service(&state, &req)
        .await
        .map(|(_, hits)| hits)
        .ok_or_else(|| crate::remote_lookup::api_down("nearest service"))
}

// ── Powerplay & merits ───────────────────────────────────────────────

#[tauri::command]
pub async fn merit_model(state: State<'_, AppState>) -> Result<ed_store::merits::MeritModel, CapError> {
    commander::merit_model(&state)
}

#[tauri::command]
pub async fn powerplay_seen(state: State<'_, AppState>) -> Result<Vec<ed_store::query::PowerplayState>, CapError> {
    commander::powerplay_seen(&state)
}

/// Force a journal re-sync. Normally the watcher does this.
#[tauri::command]
pub async fn sync_now(state: State<'_, AppState>) -> Result<String, String> {
    state.with_store(|s| {
        let stats = s.sync().map_err(err)?;
        Ok(format!(
            "{} new events, {} ms",
            stats.ingest.events_inserted, stats.elapsed_ms
        ))
    })
}

#[tauri::command]
pub async fn ai_ask(
    state: State<'_, AppState>,
    question: String,
) -> Result<crate::ai::Answer, String> {
    crate::ai::ask(&state, &question).await.map_err(err)
}

/// Forget the conversation so far.
#[tauri::command]
pub async fn ai_reset(state: State<'_, AppState>) -> Result<(), String> {
    state.chat.lock().unwrap_or_else(|e| e.into_inner()).clear();
    Ok(())
}

// ── Combat ───────────────────────────────────────────────────────────

fn combat_request(since: Option<String>, bucket: Option<String>) -> commander::CombatRequest {
    let d = commander::CombatRequest::default();
    commander::CombatRequest { since, bucket: bucket.unwrap_or(d.bucket) }
}

#[tauri::command]
pub async fn combat_summary(state: State<'_, AppState>, since: Option<String>) -> Result<query::CombatSummary, CapError> {
    commander::combat_summary(&state, &combat_request(since, None))
}

#[tauri::command]
pub async fn combat_timeline(
    state: State<'_, AppState>,
    since: Option<String>,
    bucket: Option<String>,
) -> Result<Vec<ed_store::session::CombatBucket>, CapError> {
    commander::combat_timeline(&state, &combat_request(since, bucket))
}

#[tauri::command]
pub async fn recent_kills(state: State<'_, AppState>, limit: Option<usize>) -> Result<Vec<ed_store::session::KillRow>, CapError> {
    let d = commander::RecentKillsRequest::default();
    commander::recent_kills(&state, &commander::RecentKillsRequest { limit: limit.unwrap_or(d.limit) })
}

#[tauri::command]
pub async fn merit_timeline(
    state: State<'_, AppState>,
    since: Option<String>,
    bucket: Option<String>,
) -> Result<Vec<ed_store::session::MeritBucket>, CapError> {
    commander::merit_timeline(&state, &combat_request(since, bucket))
}

// ── Markets and routing ──────────────────────────────────────────────

/// One market search of a fixed kind, on the community API - the only
/// market there is (B.4, 2026-09-09). Every search logs its shape, row
/// count and elapsed ms in `remote_search` (doctrine rule 2).
pub(crate) async fn market_search_of(state: &AppState, kind: &str, mut query: galaxy::MarketSearchRequest) -> Result<serde_json::Value, CapError> {
    query.kind = kind.to_string();
    crate::remote_search::search(state, &query).await
}

#[tauri::command]
pub async fn commodity_search(state: State<'_, AppState>, query: galaxy::MarketSearchRequest) -> Result<serde_json::Value, CapError> {
    market_search_of(&state, "commodity", query).await
}

#[tauri::command]
pub async fn outfitting_search(state: State<'_, AppState>, query: galaxy::MarketSearchRequest) -> Result<serde_json::Value, CapError> {
    market_search_of(&state, "module", query).await
}

#[tauri::command]
pub async fn shipyard_search(state: State<'_, AppState>, query: galaxy::MarketSearchRequest) -> Result<serde_json::Value, CapError> {
    market_search_of(&state, "ship", query).await
}

/// The Mining page (maintainer shape, 2026-09-06): the commander's own marks
/// first, then ring hotspots, then landable bodies ranked by surface
/// concentration — one search, three honesty levels. The marks are the
/// journal's; hotspots, rings and bodies come from POST
/// /v1/mining/search around the journal's own coordinates (B.4 +
/// the assistant's d7bbc2c). A server that does not answer leaves the marks
/// and says `data_installed: false`, never an empty "nothing here".
#[tauri::command]
pub async fn mining_search(
    state: State<'_, AppState>,
    text: String,
    radius_ly: Option<f64>,
) -> Result<serde_json::Value, CapError> {
    let conn = state.read_conn().map_err(|e| CapError::unavailable(e, true))?;
    let started = std::time::Instant::now();
    let text = text.trim().to_string();
    let radius = radius_ly.unwrap_or(100.0).clamp(1.0, 500.0);
    let (system, origin, marks) = {
        let text = text.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let system = galaxy::system_or_current(&conn, None)?;
            let origin = galaxy::origin_coords(&conn, &system)?;
            let marks = ed_store::mining::marks_near(&conn, Some(origin), &text)
                .map_err(|e| CapError::internal(e.to_string()))?;
            Ok::<_, CapError>((system, origin, marks))
        })
        .await
        .map_err(|e| CapError::internal(e.to_string()))??
    };
    let remote = if text.is_empty() { None } else { crate::remote_lookup::mining_search(&state, &text, &system, origin, radius).await };
    let ring_hint = ed_store::mining::ring_type_for(&text).map(|(t, why)| serde_json::json!({"type": t, "why": why}));
    let mut out = remote.unwrap_or_else(|| serde_json::json!({
        "origin": system,
        "hotspots": [],
        "bodies": [],
        "rings": [],
        "known_hotspot": false,
        "known_surface": false,
        "ring_hint": ring_hint,
        "data_installed": false,
    }));
    tracing::info!(%text, radius, marks = marks.len(), served = out["data_installed"].as_bool().unwrap_or(false), ms = started.elapsed().as_millis() as u64, "mining search");
    out["marks"] = serde_json::to_value(marks).map_err(|e| CapError::internal(e.to_string()))?;
    Ok(out)
}

/// The autocomplete's source of truth: the server's hotspot/surface
/// vocabulary (GET /v1/mining/materials) with the laser-mined goods
/// that resolve to a ring type; the laser list alone when unanswered.
#[tauri::command]
pub async fn mining_materials(state: State<'_, AppState>) -> Result<serde_json::Value, CapError> {
    let lasers = [
        "Gold", "Silver", "Palladium", "Osmium", "Bertrandite", "Indite", "Gallite",
        "Praseodymium", "Samarium", "Bauxite", "Cobalt", "Rutile", "Water",
        "Liquid Oxygen", "Lithium Hydroxide",
    ];
    Ok(crate::remote_lookup::mining_materials(&state).await.unwrap_or_else(|| {
        serde_json::json!({ "entries": [], "laser": lasers })
    }))
}

/// "Mark iridium here": a private breadcrumb at the CURRENT system.
#[tauri::command]
pub async fn mark_add(
    state: State<'_, AppState>,
    label: String,
    body: Option<String>,
    note: Option<String>,
) -> Result<serde_json::Value, CapError> {
    if label.trim().is_empty() {
        return Err(CapError::invalid("a mark needs a label").hint("what is here — Iridium, Gold hotspot…"));
    }
    state.with_store(|s| -> Result<serde_json::Value, String> {
        // One resolver for where the commander is, at the finest grain
        // the game is currently publishing.
        let fix = ed_store::mining::here(s.conn()).map_err(|e| e.to_string())?;
        if fix.system.trim().is_empty() {
            return Err("no current system yet — EDDA needs one journal event first".into());
        }
        let id = ed_store::mining::mark_add(
            s.conn(),
            &label,
            &fix,
            body.as_deref(),
            note.as_deref().filter(|n| !n.trim().is_empty()),
        )
        .map_err(|e| e.to_string())?;
        // The grain is worth a log line: it is the difference between a
        // bookmark you can fly back to and one you have to hunt for.
        tracing::info!(%label, system = %fix.system, grain = fix.grain(), "bookmark added");
        let grain = fix.grain();
        let saved_body = body
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_string)
            .or_else(|| fix.body.clone());
        Ok(serde_json::json!({
            "id": id,
            "system": fix.system,
            "station": fix.station,
            "body": saved_body,
            "latitude": fix.latitude,
            "longitude": fix.longitude,
            "grain": grain,
        }))
    })
    .map_err(|e| CapError::internal(e))
}

/// Where a bookmark saved right now would land, so the UI can say so
/// before the commander commits — and keep saying it as they move.
#[tauri::command]
pub async fn mark_here(state: State<'_, AppState>) -> Result<serde_json::Value, CapError> {
    state
        .with_store(|s| -> Result<serde_json::Value, String> {
            let fix = ed_store::mining::here(s.conn()).map_err(|e| e.to_string())?;
            let mut v = serde_json::to_value(&fix).map_err(|e| e.to_string())?;
            v["grain"] = serde_json::Value::from(fix.grain());
            Ok(v)
        })
        .map_err(CapError::internal)
}

#[tauri::command]
pub async fn mark_remove(state: State<'_, AppState>, id: i64) -> Result<bool, CapError> {
    state
        .with_store(|s| -> Result<bool, String> { ed_store::mining::mark_remove(s.conn(), id).map_err(|e| e.to_string()) })
        .map_err(CapError::internal)
}

/// A station's board from the community API: `{station_id, entries,
/// provenance}` — the same shape the model's tool returns.
#[tauri::command]
pub async fn station_market(state: State<'_, AppState>, station_id: i64) -> Result<serde_json::Value, CapError> {
    station_board(&state, station_id).await
}

pub(crate) async fn station_board(state: &AppState, station_id: i64) -> Result<serde_json::Value, CapError> {
    let entries = crate::remote_lookup::station_board(state, station_id)
        .await
        .ok_or_else(|| crate::remote_lookup::api_down("station board"))?;
    Ok(serde_json::json!({ "station_id": station_id, "entries": entries, "provenance": "server" }))
}

#[tauri::command]
pub async fn systems_near(state: State<'_, AppState>, system: String, radius_ly: Option<f64>) -> Result<Vec<lookup::NearbySystem>, CapError> {
    let d = galaxy::SystemsNearRequest::default();
    let req = galaxy::SystemsNearRequest { system, radius_ly: radius_ly.unwrap_or(d.radius_ly) };
    crate::remote_lookup::systems_near(&state, &req).await.ok_or_else(|| crate::remote_lookup::api_down("systems near"))
}

/// The profit finder, on the community API (B.4, 2026-09-09). `query`
/// is `ed_route::request::ProfitRequest`, the same struct the ship
/// computer's `find_profit` parses; the policy behind it is tested there.
#[tauri::command]
pub async fn profit_routes(
    state: State<'_, AppState>,
    query: ed_route::request::ProfitRequest,
) -> Result<ed_route::profit::ProfitReport, CapError> {
    let started = std::time::Instant::now();
    // The freshness window the commander searched at (Option<f64>, Copy) —
    // captured before `query` moves — feeds the aggregate that picks a
    // better default. A number only; the closed kind is server-validated.
    let max_age = query.max_age_hours;
    let result = crate::remote_trade::report(&state, &query).await;
    crate::telemetry::record_timing("trade", started.elapsed().as_millis(), result.is_ok());
    if let Some(h) = max_age {
        crate::telemetry::record_search("trade_max_age_hours", h.max(0.0).round().min(f64::from(u32::MAX)) as u32);
    }
    result
}

// ── Voice, callouts, overlay ─────────────────────────────────────────

/// Stop the running profit search. The search runs on the server now;
/// the button stays so the panel's contract holds while the request
/// times out (there is no job to cancel).
#[tauri::command]
pub fn cancel_search(state: State<AppState>) {
    state.jobs.cancel(crate::jobs::PROFIT_SEARCH);
}

/// The plotted route, enriched with fuel, hazard, docking and Powerplay
/// facts, plus the spoken briefing text.
#[derive(Debug, Serialize)]
pub struct RouteView {
    pub route: Option<ed_store::route::RouteBrief>,
    pub brief: Option<String>,
    pub next: Option<String>,
    /// Item 39: hops where the burn model says fuel is needed AND
    /// available (via "scoop" or "station").
    pub fuel_marks: Vec<crate::routing::GameFuelMark>,
}

#[tauri::command]
pub async fn current_route(state: State<'_, AppState>) -> Result<RouteView, String> {
    let (route, brief, next, dock_query) = state.with_read(|s| {
        let conn = s.conn();
        let route = ed_store::route::current(conn).map_err(err)?;
        let here = query::location(conn)
            .map_err(err)?
            .and_then(|l| l.system_name);
        // The Route tab is a read, not the voice: it shows the whole
        // briefing whatever else is running.
        use ed_store::route::Narration;
        let brief = route.as_ref().map(|r| ed_store::route::brief_text(r, Narration::Full));
        let next = match (&route, &here) {
            (Some(r), Some(h)) => ed_store::route::next_hop_text(r, h, Narration::Full),
            _ => None,
        };
        let dock_query = route.as_ref().and_then(|r| crate::routing::game_route_dock_query(conn, r));
        Ok::<_, String>((route, brief, next, dock_query))
    })?;
    // Item 39: fuel icons on the game route mean "you will need fuel by
    // here, and here has it" — computed by the burn model, never implied
    // by mere scoopability. Docks come from one /v1/stations?systems=
    // call (B.4 gap 3); unanswered means no dock known.
    let docks = match &dock_query {
        Some((systems, pad)) if !systems.is_empty() => {
            crate::remote_lookup::docks_by_systems(&state, systems, *pad).await.unwrap_or_default()
        }
        _ => Default::default(),
    };
    let fuel_marks = match &route {
        Some(r) => state.with_read(|s| crate::routing::game_route_fuel_marks_for(s.conn(), r, &docks)),
        None => Vec::new(),
    };
    Ok(RouteView { route, brief, next, fuel_marks })
}

/// Distinct powers and Powerplay states in the galaxy tables, for filters.
#[derive(Debug, Serialize)]
pub struct PowerplayOptions {
    pub powers: Vec<String>,
    pub states: Vec<String>,
    pub pledged: Option<String>,
}

#[tauri::command]
pub async fn powerplay_options(state: State<'_, AppState>) -> Result<PowerplayOptions, String> {
    state.with_read(|s| {
        let conn = s.conn();
        let pledged = ed_store::session::latest_event_raw(conn, "Powerplay")
            .ok()
            .flatten()
            .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
            .and_then(|v| v.get("Power").and_then(serde_json::Value::as_str).map(str::to_string));
        let (powers, states) = powerplay_vocabulary(Vec::new(), Vec::new());
        Ok(PowerplayOptions {
            powers,
            states,
            pledged,
        })
    })
}

/// The Powerplay vocabulary the journal writes (powers as `Powers[]` /
/// `ControllingPower` spell them, states as `PowerplayState`). The
/// server applies the filters; this list is what the panel offers.
pub const POWERS: &[&str] = &[
    "A. Lavigny-Duval",
    "Aisling Duval",
    "Archon Delaine",
    "Denton Patreus",
    "Edmund Mahon",
    "Felicia Winters",
    "Jerome Archer",
    "Li Yong-Rui",
    "Nakato Kaine",
    "Pranav Antal",
    "Yuri Grom",
    "Zemina Torval",
];
pub const POWER_STATES: &[&str] = &["Stronghold", "Fortified", "Exploited", "Contested", "Unoccupied"];

pub(crate) fn powerplay_vocabulary(mut powers: Vec<String>, mut states: Vec<String>) -> (Vec<String>, Vec<String>) {
    for p in POWERS {
        if !powers.iter().any(|x| x.eq_ignore_ascii_case(p)) {
            powers.push((*p).to_string());
        }
    }
    for st in POWER_STATES {
        if !states.iter().any(|x| x.eq_ignore_ascii_case(st)) {
            states.push((*st).to_string());
        }
    }
    powers.sort_by_key(|p| p.to_ascii_lowercase());
    states.sort_by_key(|st| POWER_STATES.iter().position(|k| k.eq_ignore_ascii_case(st)).unwrap_or(usize::MAX));
    (powers, states)
}

/// Career and Powerplay ranks from the journal, with ladder names.
#[tauri::command]
pub async fn ranks(state: State<'_, AppState>) -> Result<ed_store::session::Ranks, String> {
    state
        .with_read(|s| ed_store::session::ranks(s.conn()))
        .map_err(err)
}

#[derive(Debug, Serialize)]
pub struct VoiceStatus {
    pub backend: ed_voice::Backend,
    pub model: Option<String>,
    pub muted: bool,
}

#[tauri::command]
pub async fn voice_status(state: State<'_, AppState>) -> Result<VoiceStatus, String> {
    Ok({
        VoiceStatus {
            backend: state.voice.backend(),
            model: state.voice.model(),
            muted: state.voice.is_muted(),
        }
    })
}

#[derive(Debug, Serialize)]
pub struct PersonaView {
    pub personas: Vec<crate::persona::Persona>,
    pub selected: String,
}

#[tauri::command]
pub async fn personas(state: State<'_, AppState>) -> Result<PersonaView, String> {
    Ok({
        let selected = state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .persona
            .clone()
            .unwrap_or_else(|| "standard".into());
        PersonaView {
            personas: crate::persona::PERSONAS.to_vec(),
            selected,
        }
    })
}

/// Select a persona: remembered, switches to its default voice if that
/// model is present, and says its sample line so you hear the result.
#[tauri::command]
pub async fn set_persona(state: State<'_, AppState>, id: String) -> Result<PersonaView, String> {
    let p = crate::persona::by_id(&id);
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.persona = Some(p.id.to_string());
        cfg.save(&state.data_dir).map_err(err)?;
    }
    // Personality changes the wording only; the voice is chosen separately.
    state.voice.say(p.sample);
    personas(state).await
}

/// Model files present under `.data/voices/`.
#[tauri::command]
pub async fn voice_models(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(ed_voice::list_models(&state.data_dir))
}

#[tauri::command]
pub async fn voice_use_windows(state: State<'_, AppState>) -> Result<VoiceStatus, String> {
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.voice_server_enabled = false;
        cfg.save(&state.data_dir).map_err(err)?;
    }
    state.voice.audio().set_server(None);
    state.voice.use_windows();
    crate::telemetry::set_voice_engine("voice_windows");
    state.voice.say("Windows voice selected.");
    voice_status(state).await
}

#[derive(Clone, Copy)]
struct VoiceSpec { model: &'static str, path: &'static str, label: &'static str, size_mb: u32 }
const VOICE_CATALOG: &[VoiceSpec] = &[
    VoiceSpec { model: "en_US-lessac-high.onnx", path: "en/en_US/lessac/high", label: "Elise · American", size_mb: 122 },
    VoiceSpec { model: "en_GB-alan-medium.onnx", path: "en/en_GB/alan/medium", label: "Alan · British", size_mb: 64 },
    VoiceSpec { model: "en_US-amy-medium.onnx", path: "en/en_US/amy/medium", label: "Amy · American", size_mb: 64 },
    VoiceSpec { model: "en_US-ryan-high.onnx", path: "en/en_US/ryan/high", label: "Ryan · American", size_mb: 122 },
];

#[derive(Debug, Serialize)]
pub struct VoiceCatalogEntry { pub model: String, pub label: String, pub size_mb: u32, pub installed: bool }

#[tauri::command]
pub async fn voice_catalog(state: State<'_, AppState>) -> Result<Vec<VoiceCatalogEntry>, String> {
    let voices = state.data_dir.join("voices");
    Ok(VOICE_CATALOG.iter().map(|v| VoiceCatalogEntry {
        model: v.model.into(), label: v.label.into(), size_mb: v.size_mb,
        installed: voices.join(v.model).is_file() && voices.join(format!("{}.json", v.model)).is_file(),
    }).collect())
}

async fn install_curated_voice(state: &AppState, model: &str) -> Result<String, String> {
    let spec = VOICE_CATALOG.iter().find(|v| v.model == model).copied().ok_or_else(|| "unknown curated voice".to_string())?;
    let data_dir = state.data_dir.clone();
    let install_dir = data_dir.clone();
    let client = state.http_blocking.clone();
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
        let voices = install_dir.join("voices");
        std::fs::create_dir_all(&voices)?;
        let piper = voices.join("piper");
        if !crate::platform::piper_installed(&piper) {
            // Every release archive unpacks to a `piper/` folder, whatever
            // the host: zip on Windows, tar.gz elsewhere.
            let url = crate::platform::piper_download_url()
                .ok_or_else(|| anyhow::anyhow!("no Piper build is published for {}/{}", std::env::consts::OS, std::env::consts::ARCH))?;
            let bytes = client.get(&url).send()?.error_for_status()?.bytes()?;
            if url.ends_with(".zip") {
                let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
                for i in 0..zip.len() {
                    let mut entry = zip.by_index(i)?;
                    let Some(relative) = entry.enclosed_name() else { continue };
                    let out = voices.join(relative);
                    if entry.is_dir() { std::fs::create_dir_all(&out)?; continue; }
                    if let Some(parent) = out.parent() { std::fs::create_dir_all(parent)?; }
                    let mut file = std::fs::File::create(out)?;
                    std::io::copy(&mut entry, &mut file)?;
                }
            } else {
                // tar preserves the executable bit piper needs.
                let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
                tar::Archive::new(gz).unpack(&voices)?;
            }
        }
        let base = format!("https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/{}", spec.path);
        for name in [spec.model.to_string(), format!("{}.json", spec.model)] {
            let out = voices.join(&name);
            if out.is_file() { continue; }
            let partial = voices.join(format!("{name}.partial"));
            let mut response = client.get(format!("{base}/{name}?download=true")).send()?.error_for_status()?;
            let mut file = std::fs::File::create(&partial)?;
            std::io::copy(&mut response, &mut file)?;
            std::fs::rename(partial, out)?;
        }
        Ok(())
    }).await.map_err(err)?.map_err(err)?;
    state.voice.reload(&data_dir);
    state.voice.set_model(spec.model);
    state.voice.say(format!("{} installed and ready.", spec.label));
    Ok(spec.model.into())
}

#[tauri::command]
pub async fn voice_install(state: State<'_, AppState>, model: String) -> Result<String, String> {
    install_curated_voice(&state, &model).await
}

#[tauri::command]
pub async fn voice_remove(state: State<'_, AppState>, model: String) -> Result<Vec<String>, String> {
    if model.contains('/') || model.contains('\\') || !model.ends_with(".onnx") {
        return Err("invalid voice model name".into());
    }
    if state.voice.model().is_some_and(|m| model.starts_with(&m)) {
        return Err("select another voice before removing the one currently in use".into());
    }
    let voices = state.data_dir.join("voices");
    let model_path = voices.join(&model);
    let config_path = voices.join(format!("{model}.json"));
    tauri::async_runtime::spawn_blocking(move || -> std::io::Result<()> {
        if model_path.is_file() { std::fs::remove_file(model_path)?; }
        if config_path.is_file() { std::fs::remove_file(config_path)?; }
        Ok(())
    }).await.map_err(err)?.map_err(err)?;
    Ok(ed_voice::list_models(&state.data_dir))
}

/// Install EDDA's curated local neural voice. Downloads happen only after
/// this explicit command and run off the UI thread.
#[tauri::command]
pub async fn voice_install_default(state: State<'_, AppState>) -> Result<String, String> {
    const MODEL: &str = "en_US-lessac-high.onnx";
    install_curated_voice(&state, MODEL).await
}

#[derive(Debug, Serialize)]
pub struct VoiceServerView {
    pub enabled: bool,
    pub config: ed_voice::ServerConfig,
    pub voices: Vec<String>,
}

fn voice_server_view(
    audio: &ed_voice::Audio,
    enabled: bool,
    mut config: ed_voice::ServerConfig,
    probe: bool,
) -> VoiceServerView {
    let voices = if probe && !config.url.trim().is_empty() {
        audio.server_voices(&config)
    } else {
        Vec::new()
    };
    config.api_key = config.api_key.as_ref().map(|_| "•••".into());
    VoiceServerView {
        enabled,
        config,
        voices,
    }
}

/// Ask a server which voices it has, before committing to it. Async: the
/// HTTP round trip must never run on the UI thread.
#[tauri::command]
pub async fn voice_server_probe(
    state: State<'_, AppState>,
    config: ed_voice::ServerConfig,
) -> Result<Vec<String>, String> {
    let mut c = config;
    if c.api_key.as_deref() == Some("•••") {
        c.api_key = state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .voice_server
            .as_ref()
            .and_then(|p| p.api_key.clone());
    }
    let audio = state.voice.audio().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let v = audio.server_voices(&c);
        if v.is_empty() {
            // No listing endpoint: check the server is at least there.
            return audio.server_reachable(&c).map(|_| Vec::new()).map_err(|e| format!("{e:#}"));
        }
        Ok(v)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn voice_server_get(state: State<'_, AppState>) -> Result<VoiceServerView, String> {
    let (enabled, config) = {
        let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        (
            cfg.voice_server_enabled,
            cfg.voice_server.clone().unwrap_or_default(),
        )
    };
    let audio = state.voice.audio().clone();
    tauri::async_runtime::spawn_blocking(move || voice_server_view(&audio, enabled, config, enabled))
            .await
            .map_err(|e| e.to_string())
}

/// Save the speech server and switch to it (or back to the built-in voice).
#[tauri::command]
pub async fn voice_server_set(
    state: State<'_, AppState>,
    enabled: bool,
    config: ed_voice::ServerConfig,
) -> Result<VoiceServerView, String> {
    let saved = {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        let mut c = config;
        // A masked key means "keep the one I have".
        if c.api_key.as_deref() == Some("•••") {
            c.api_key = cfg.voice_server.as_ref().and_then(|p| p.api_key.clone());
        }
        cfg.voice_server = Some(c.clone());
        cfg.voice_server_enabled = enabled;
        cfg.save(&state.data_dir).map_err(err)?;
        state.voice.audio().set_server(if enabled { Some(c.clone()) } else { None });
        c
    };
    state.voice.say("Hello, Commander. All systems online.");
    let audio = state.voice.audio().clone();
    tauri::async_runtime::spawn_blocking(move || voice_server_view(&audio, enabled, saved, enabled))
            .await
            .map_err(|e| e.to_string())
}

#[derive(Debug, Serialize)]
pub struct SignalWatchItem {
    pub id: &'static str,
    pub label: &'static str,
    pub on: bool,
}

fn signal_watch_items() -> Vec<SignalWatchItem> {
    let on = crate::callouts::signal_watch();
    crate::callouts::SIGNALS
        .iter()
        .map(|(id, label, _)| SignalWatchItem {
            id,
            label,
            on: on.iter().any(|w| w == id),
        })
        .collect()
}

#[tauri::command]
pub async fn signal_watch_get() -> Result<Vec<SignalWatchItem>, String> {
    Ok(signal_watch_items())
}

/// Replace the set of watched signals; remembered for the next launch.
#[tauri::command]
pub async fn signal_watch_set(
    state: State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<SignalWatchItem>, String> {
    set_signal_watch(&state, ids)?;
    Ok(signal_watch_items())
}

/// The callout kinds, with a plain name each, and whether they are on.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CalloutKind {
    pub id: &'static str,
    pub label: &'static str,
    pub on: bool,
}

pub const CALLOUT_KINDS: &[(&str, &str)] = &[
    ("greeting", "Greeting on start"),
    ("hazard", "Hazards: neutron star and white dwarf caution"),
    ("fuel", "Fuel warnings"),
    ("arrival", "Arrivals"),
    ("scan", "Scans and discoveries"),
    ("material", "Material pickups"),
    ("mission", "Missions"),
    ("route", "The game's plotted route"),
    ("follow", "Following an app route"),
    ("signal", "Signal watch"),
    ("session", "Session summary"),
    ("ship", "Ship changes"),
];

#[tauri::command]
pub async fn callouts_get(state: State<'_, AppState>) -> Result<Vec<CalloutKind>, String> {
    let off = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .callouts_off
        .clone();
    Ok(CALLOUT_KINDS
        .iter()
        .map(|(id, label)| CalloutKind {
            id,
            label,
            on: !off.iter().any(|o| o == id),
        })
        .collect())
}

#[tauri::command]
pub async fn callouts_set(
    state: State<'_, AppState>,
    off: Vec<String>,
) -> Result<Vec<CalloutKind>, String> {
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.callouts_off = off
            .into_iter()
            .filter(|o| CALLOUT_KINDS.iter().any(|(id, _)| id == o))
            .collect();
        cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    }
    callouts_get(state).await
}

pub fn set_signal_watch(state: &AppState, ids: Vec<String>) -> Result<(), String> {
    crate::callouts::set_signal_watch(ids);
    let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    cfg.signal_watch = Some(crate::callouts::signal_watch());
    cfg.save(&state.data_dir).map_err(err)
}

/// Switch voice; the choice is remembered for the next launch.
#[tauri::command]
pub async fn set_voice(state: State<'_, AppState>, model: String) -> Result<String, String> {
    if !ed_voice::list_models(&state.data_dir).contains(&model)
    {
        return Err(format!("no such voice model: {model}"));
    }
    state.voice.set_model(&model);
    // A Piper model selection only wins when no server is configured;
    // report what will actually speak.
    crate::telemetry::set_voice_engine(match state.voice.backend() {
        ed_voice::Backend::Server => "voice_kokoro",
        ed_voice::Backend::Piper => "voice_piper",
        ed_voice::Backend::Sapi => "voice_windows",
        _ => "voice_none",
    });
    state.voice.say("Hello, Commander. All systems online.");
    Ok(model)
}

/// Markdown read aloud is "asterisk asterisk". Strip the syntax, keep the words.
pub fn speakable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let l = line.trim_start();
        let l = l
            .trim_start_matches(['#', '>'])
            .trim_start();
        let l = l
            .strip_prefix("- ")
            .or_else(|| l.strip_prefix("* "))
            .or_else(|| l.strip_prefix("• "))
            .unwrap_or(l);
        for ch in l.chars() {
            if !matches!(ch, '*' | '_' | '`' | '~' | '#') {
                out.push(ch);
            }
        }
        out.push(' ');
    }
    pronounce(&crate::phonetics::speak_system_names(
        &out.split_whitespace().collect::<Vec<_>>().join(" "),
    ))
}

#[cfg(test)]
mod speakable_phonetics_tests {
    #[test]
    fn procgen_system_names_reach_the_voice_as_nato() {
        let said = super::speakable("Warning: Wredguia WD-K d8-1 would be a fuel trap.");
        assert!(said.contains("Whiskey Delta dash Kilo Delta 8 dash 1"), "{said}");
        assert!(!said.contains("WD-K"), "{said}");
    }
}

/// Things a TTS engine gets wrong in this domain. "Mk II" is "Mark 2",
/// not "em kay eye eye"; the unit abbreviations are spoken in full.
pub fn pronounce(text: &str) -> String {
    let roman = [
        ("XII", "12"),
        ("XI", "11"),
        ("X", "10"),
        ("IX", "9"),
        ("VIII", "8"),
        ("VII", "7"),
        ("VI", "6"),
        ("V", "5"),
        ("IV", "4"),
        ("III", "3"),
        ("II", "2"),
        ("I", "1"),
    ];
    let mut words: Vec<String> = Vec::new();
    let toks: Vec<&str> = text.split(' ').collect();
    let mut i = 0;
    while i < toks.len() {
        let w = toks[i];
        let bare = w.trim_end_matches(['.', ',', ';', ':']);
        let tail = &w[bare.len()..];
        if bare.eq_ignore_ascii_case("mk") {
            words.push("Mark".into());
            if let Some(next) = toks.get(i + 1) {
                let nb =
                    next.trim_end_matches(['.', ',', ';', ':']);
                if let Some((_, n)) = roman.iter().find(|(r, _)| nb.eq_ignore_ascii_case(r)) {
                    words.push(format!("{n}{}", &next[nb.len()..]));
                    i += 2;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        // ISO dates: "2026-08-26" -> "August 26th, 2026".
        if bare.len() == 10 && bare.as_bytes()[4] == b'-' && bare.as_bytes()[7] == b'-' {
            if let (Ok(m), Ok(d)) = (bare[5..7].parse::<usize>(), bare[8..10].parse::<u32>()) {
                const MONTHS: [&str; 12] = [
                    "January",
                    "February",
                    "March",
                    "April",
                    "May",
                    "June",
                    "July",
                    "August",
                    "September",
                    "October",
                    "November",
                    "December",
                ];
                if (1..=12).contains(&m) && (1..=31).contains(&d) {
                    let suffix = match d {
                        1 | 21 | 31 => "st",
                        2 | 22 => "nd",
                        3 | 23 => "rd",
                        _ => "th",
                    };
                    words.push(format!(
                        "{} {d}{suffix}, {}{tail}",
                        MONTHS[m - 1],
                        &bare[0..4]
                    ));
                    i += 1;
                    continue;
                }
            }
        }
        let replaced = match bare {
            "ly" => Some("light years"),
            "ls" => Some("light seconds"),
            "cr" | "Cr" => Some("credits"),
            "t" => Some("tons"),
            "FSD" => Some("F S D"),
            _ => None,
        };
        match replaced {
            Some(r) => words.push(format!("{r}{tail}")),
            None => words.push(w.to_string()),
        }
        i += 1;
    }
    words.join(" ")
}

#[cfg(test)]
mod speech_tests {
    use super::*;

    #[test]
    fn marks_and_units_are_spoken_in_full() {
        assert_eq!(
            pronounce("Kestrel Mk II systems online."),
            "Kestrel Mark 2 systems online."
        );
        assert_eq!(
            pronounce("Krait Mk II, Cobra Mk III."),
            "Krait Mark 2, Cobra Mark 3."
        );
        assert_eq!(
            pronounce("18.6 ly away, 2,594 ls out, 5 t."),
            "18.6 light years away, 2,594 light seconds out, 5 tons."
        );
    }

    #[test]
    fn markdown_is_stripped_before_speech() {
        assert_eq!(
            speakable(
                "**Silver** at _Cake_: `29,940` cr
- one
# two"
            ),
            "Silver at Cake: 29,940 credits one two"
        );
    }
}

#[tauri::command]
pub fn say(state: State<AppState>, text: String) {
    state.voice.say(speakable(&text));
}

/// Barge-in then speak: cuts whatever is playing or queued and starts
/// this line. Setup tabs use it so switching pages never queues narration.
#[tauri::command]
pub fn say_now(state: State<AppState>, text: String) {
    state.voice.interrupt();
    state.voice.say(speakable(&text));
}

#[tauri::command]
pub fn voice_interrupt(state: State<AppState>) {
    state.voice.interrupt();
}

#[tauri::command]
pub async fn set_muted(state: State<'_, AppState>, muted: bool) -> Result<bool, String> {
    Ok({
        state.voice.set_muted(muted);
        muted
    })
}

#[tauri::command]
pub async fn recent_callouts(
    state: State<'_, AppState>,
) -> Result<Vec<crate::callouts::Callout>, String> {
    Ok({
        let q = state.callouts.lock().unwrap_or_else(|e| e.into_inner());
        q.iter().cloned().collect()
    })
}

#[tauri::command]
pub async fn set_overlay_interactive(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    interactive: bool,
) -> Result<bool, String> {
    let result = crate::overlay::set_interactive(&app, &state, interactive).map_err(err)?;
    if !interactive {
        crate::watcher::deliver(&app, vec![(crate::callouts::Callout::new("session", "", 1, true, "Overlay locked. Open EDDA Settings to unlock and reposition it.".into()), None)]);
    }
    Ok(result)
}

#[tauri::command]
pub fn overlay_visible(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    use tauri::Manager;
    let Some(w) = app.get_webview_window("overlay") else {
        return Ok(());
    };
    if visible { w.show() } else { w.hide() }.map_err(err)
}

// ── Missions ─────────────────────────────────────────────────────────

/// Now as a journal-style timestamp, for expiry comparisons.
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // Civil-from-days (Howard Hinnant), the inverse of query::epoch_secs.
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[tauri::command]
pub async fn missions(
    state: State<'_, AppState>,
    active_only: Option<bool>,
) -> Result<Vec<ed_store::missions::Mission>, String> {
    let now = now_iso();
    state
        .with_read(|s| {
            if active_only.unwrap_or(true) {
                ed_store::missions::active(s.conn(), &now)
            } else {
                ed_store::missions::missions(s.conn(), "", &now)
            }
        })
        .map_err(err)
}

// ── Ship computer configuration ──────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct AiConfigView {
    pub has_key: bool,
    /// Last four characters, so the user can tell which key is stored.
    pub key_hint: Option<String>,
    pub model: Option<String>,
    /// True when the environment variable is overriding the saved key.
    pub env_override: bool,
    pub config_path: String,
    /// Whether web search / fetch are offered to the model.
    pub research: bool,
    /// "anthropic" or "openai".
    pub provider: String,
    pub openai_base_url: Option<String>,
    pub openai_model: Option<String>,
    pub openai_has_key: bool,
    pub openai_key_hint: Option<String>,
}

#[tauri::command]
pub async fn get_ai_config(state: State<'_, AppState>) -> Result<AiConfigView, String> {
    Ok({
        let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        let env_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty());
        let key = env_key.clone().or_else(crate::state::secrets::get_api_key);
        AiConfigView {
            has_key: key.is_some(),
            key_hint: key.map(|k| format!("…{}", &k[k.len().saturating_sub(4)..])),
            model: cfg.anthropic_model.clone(),
            env_override: env_key.is_some(),
            config_path: "Windows Credential Manager (edda / anthropic_api_key)".to_string(),
            research: cfg.research_enabled(),
            provider: cfg
                .ai_provider
                .clone()
                .unwrap_or_else(|| "anthropic".into()),
            openai_base_url: cfg.openai_base_url.clone(),
            openai_model: cfg.openai_model.clone(),
            openai_has_key: crate::state::secrets::get_key(crate::state::secrets::OPENAI_KEY)
                .is_some(),
            openai_key_hint: crate::state::secrets::get_key(crate::state::secrets::OPENAI_KEY)
                .map(|k| format!("…{}", &k[k.len().saturating_sub(4)..])),
        }
    })
}

/// Save key and/or model. The key goes to the OS credential store; an
/// empty key removes it. `None` leaves either field as it is.
#[tauri::command]
pub async fn set_ai_config(
    state: State<'_, AppState>,
    api_key: Option<String>,
    model: Option<String>,
    research: Option<bool>,
    provider: Option<String>,
    openai_base_url: Option<String>,
    openai_model: Option<String>,
    openai_key: Option<String>,
) -> Result<AiConfigView, String> {
    if let Some(k) = api_key {
        if k.trim().is_empty() {
            crate::state::secrets::clear_api_key()?;
        } else {
            crate::state::secrets::set_api_key(&k)?;
        }
    }
    if let Some(k) = openai_key {
        if k.trim().is_empty() {
            crate::state::secrets::clear_key(crate::state::secrets::OPENAI_KEY)?;
        } else {
            crate::state::secrets::set_key(crate::state::secrets::OPENAI_KEY, &k)?;
        }
    }
    if model.is_some()
        || research.is_some()
        || provider.is_some()
        || openai_base_url.is_some()
        || openai_model.is_some()
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = model {
            cfg.anthropic_model = Some(m.trim().to_string()).filter(|m| !m.is_empty());
        }
        if let Some(r) = research {
            cfg.research = Some(r);
        }
        if let Some(p) = provider {
            cfg.ai_provider = Some(if p == "openai" {
                "openai".to_string()
            } else {
                "anthropic".to_string()
            });
        }
        if let Some(u) = openai_base_url {
            cfg.openai_base_url =
                Some(u.trim().trim_end_matches('/').to_string()).filter(|u| !u.is_empty());
        }
        if let Some(m) = openai_model {
            cfg.openai_model = Some(m.trim().to_string()).filter(|m| !m.is_empty());
        }
        cfg.save(&state.data_dir).map_err(err)?;
    }
    get_ai_config(state).await
}

// ── Maintenance ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct DbStats {
    pub path: String,
    pub journal_bytes: u64,
    pub total_bytes: u64,
    pub events: i64,
    pub kills: i64,
    pub merit_awards: i64,
    /// Reads that ran on the shared writer because a private connection
    /// could not be opened. Should stay 0.
    pub read_fallbacks: u64,
    /// Background jobs running right now.
    pub jobs: Vec<&'static str>,
}

/// SQLite may keep recently committed pages in `-wal`; report the complete
/// on-disk footprint rather than only the main file's logical page count.
fn sqlite_family_bytes(path: &std::path::Path) -> u64 {
    let base = path.as_os_str().to_string_lossy();
    [
        base.to_string(),
        format!("{base}-wal"),
        format!("{base}-shm"),
    ]
    .iter()
    .filter_map(|p| std::fs::metadata(p).ok())
    .map(|m| m.len())
    .sum()
}

#[tauri::command]
pub async fn db_stats(state: State<'_, AppState>) -> Result<DbStats, String> {
    let journal_bytes = sqlite_family_bytes(&state.db_path);
    state.with_read(|s| {
        let conn = s.conn();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
        Ok(DbStats {
            path: state.db_path.display().to_string(),
            journal_bytes,
            total_bytes: journal_bytes,
            events: count("SELECT COUNT(*) FROM events"),
            kills: count("SELECT COUNT(*) FROM combat_kills"),
            merit_awards: count("SELECT COUNT(*) FROM merit_events"),
            read_fallbacks: state.read_fallbacks(),
            jobs: state.jobs.running(),
        })
    })
}

/// Reclaim space from the journal database.
#[tauri::command]
pub async fn vacuum(state: State<'_, AppState>) -> Result<String, String> {
    let store = state.store.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = store.lock().unwrap_or_else(|e| e.into_inner());
        let conn = guard.conn();
        let size = || {
            conn.query_row(
                "SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
        };
        let before = size();
        let started = std::time::Instant::now();
        conn.execute_batch("VACUUM").map_err(err)?;
        let after = size();
        Ok(format!(
            "VACUUM done in {:.0}s: {:.2} GB -> {:.2} GB",
            started.elapsed().as_secs_f64(),
            before as f64 / 1e9,
            after as f64 / 1e9
        ))
    })
    .await
    .map_err(err)?
}

#[cfg(test)]
mod slef_tests {
    use super::{apply_proposed_engineering, ProposedEngineering};

    /// The proposed block lands on the right slot with the journal symbol,
    /// keeps an applied experimental, and unknown names are refused.
    #[test]
    fn proposed_engineering_rewrites_one_slot() {
        let mut data = serde_json::json!({
            "Ship": "mandalay",
            "Modules": [
                { "Slot": "FrameShiftDrive", "Item": "int_hyperdrive_size5_class5",
                  "Engineering": { "BlueprintName": "FSD_LongRange", "Level": 3, "Quality": 0.77, "ExperimentalEffect": "special_fsd_heavy",
                                   "Modifiers": [{"Label": "PowerDraw", "Value": 0.5}] } },
                { "Slot": "PowerPlant", "Item": "int_powerplant_size4_class5" }
            ]
        });
        let proposed = ProposedEngineering { slot: "FrameShiftDrive".into(), module_type: "Frame Shift Drive".into(), blueprint: "Increased FSD Range".into(), grade: 5 };
        apply_proposed_engineering(&mut data, &proposed).unwrap();
        let eng = &data["Modules"][0]["Engineering"];
        assert_eq!(eng["BlueprintName"], "FSD_LongRange");
        assert_eq!(eng["Level"], 5);
        assert_eq!(eng["Quality"], 1.0);
        assert_eq!(eng["ExperimentalEffect"], "special_fsd_heavy", "an applied experimental survives the plan");
        assert!(eng.get("Modifiers").is_none(), "rolled modifiers do not describe a plan");
        // An unengineered slot gains a block.
        let proposed = ProposedEngineering { slot: "PowerPlant".into(), module_type: "Power Plant".into(), blueprint: "Armoured".into(), grade: 4 };
        apply_proposed_engineering(&mut data, &proposed).unwrap();
        assert_eq!(data["Modules"][1]["Engineering"]["BlueprintName"], "PowerPlant_Armoured");
        // Refusals: wrong slot, unmappable name.
        let missing = ProposedEngineering { slot: "Slot99".into(), module_type: "Power Plant".into(), blueprint: "Armoured".into(), grade: 4 };
        assert!(apply_proposed_engineering(&mut data, &missing).is_err());
        let synth = ProposedEngineering { slot: "PowerPlant".into(), module_type: "AFM Refill".into(), blueprint: "AFM Refill".into(), grade: 1 };
        assert!(apply_proposed_engineering(&mut data, &synth).is_err());
    }
}

/// The live activity heatmap layer: decayed cell intensities, positions
/// and numbers only — by design nothing in it can name a system (ledger
/// 2026-09-04, the anti-piracy constraint).
#[tauri::command]
pub async fn activity_heatmap(
    heat: tauri::State<'_, std::sync::Arc<crate::heatmap::Heatmap>>,
) -> Result<crate::heatmap::HeatSnapshot, String> {
    Ok(heat.snapshot(crate::heatmap::now_ms()))
}

/// The feedback payload, built pure so the caps and shape are testable.
/// Anonymous by design (ledger 2026-09-05): version, OS, the commander's
/// words, and — only with explicit consent — the tail of today's log.
#[derive(serde::Serialize, Debug, PartialEq)]
pub struct FeedbackPayload {
    pub version: String,
    pub os: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_tail: Option<String>,
    pub created_at: String,
}

pub fn feedback_payload(text: &str, log_tail: Option<String>, created_at: String) -> FeedbackPayload {
    // Server truncates too (the contract), but a polite client does not
    // ship 4 MB of enthusiasm in the first place.
    let clip = |s: &str, cap: usize| -> String {
        if s.len() <= cap { s.to_owned() } else {
            let mut at = cap;
            while at > 0 && !s.is_char_boundary(at) { at -= 1; }
            s[..at].to_owned()
        }
    };
    FeedbackPayload {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        text: clip(text.trim(), 4096),
        log_tail: log_tail.map(|l| {
            // The tail is what matters: keep the LAST 32 KB.
            let bytes = l.as_bytes();
            if bytes.len() <= 32_768 { l } else {
                let mut from = bytes.len() - 32_768;
                while from < bytes.len() && !l.is_char_boundary(from) { from += 1; }
                l[from..].to_owned()
            }
        }),
        created_at,
    }
}

/// The newest log file's content, if any (`logs/edda.log.YYYY-MM-DD`,
/// daily-rolled — newest by name sorts last).
fn latest_log(data_dir: &std::path::Path) -> Option<String> {
    let dir = data_dir.join("logs");
    let mut logs: Vec<_> = std::fs::read_dir(&dir).ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("edda.log")))
        .collect();
    logs.sort();
    std::fs::read_to_string(logs.last()?).ok()
}

/// Send a problem report to the community API — the site's "report it in
/// the app" promise, kept. Fails politely: the report text stays in the
/// box on error, nothing is lost.
#[tauri::command]
pub async fn feedback_send(
    state: tauri::State<'_, crate::state::AppState>,
    text: String,
    include_log: bool,
) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err("say a few words about what went wrong first".into());
    }
    let Some(api) = crate::exchange::endpoint(&state) else {
        return Err("no community server is configured".into());
    };
    let log_tail = if include_log { latest_log(&state.data_dir) } else { None };
    let payload = feedback_payload(&text, log_tail, chrono::Utc::now().to_rfc3339());
    let response = state
        .http
        .post(format!("{api}/v1/feedback"))
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("could not reach the server: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("server said {}", response.status()));
    }
    tracing::info!(chars = payload.text.len(), with_log = payload.log_tail.is_some(), "feedback sent");
    Ok("Sent. Thank you, commander.".into())
}

#[cfg(test)]
mod feedback_tests {
    use super::*;

    /// The payload honors the wire contract: caps applied client-side
    /// (text 4 KB, log tail LAST 32 KB), char-boundary safe, version and
    /// OS attached, no identity fields anywhere in the shape.
    #[test]
    fn feedback_payload_caps_and_stays_anonymous() {
        let long_text = "x".repeat(10_000);
        let log = format!("{}END", "y".repeat(40_000));
        let p = feedback_payload(&long_text, Some(log), "2026-09-05T00:00:00Z".into());
        assert_eq!(p.text.len(), 4096);
        let tail = p.log_tail.unwrap();
        assert!(tail.len() <= 32_768);
        assert!(tail.ends_with("END"), "the TAIL survives, not the head");
        assert!(!p.version.is_empty());
        assert!(p.os.contains(' '));
        let json = serde_json::to_string(&feedback_payload("hi", None, "t".into())).unwrap();
        for forbidden in ["commander", "name", "cmdr", "email", "id64"] {
            assert!(!json.contains(forbidden), "{forbidden} must not ride: {json}");
        }
        assert!(!json.contains("log_tail"), "absent consent, absent field");
        // Multibyte text at the cap must not panic.
        let emoji = "🚀".repeat(2_000);
        let p = feedback_payload(&emoji, None, "t".into());
        assert!(p.text.len() <= 4096);
    }
}

/// The telemetry consent state (opt-out: absent choice reads as on).
#[tauri::command]
pub async fn telemetry_prefs(state: tauri::State<'_, crate::state::AppState>) -> Result<bool, String> {
    Ok(crate::telemetry::consented(&state.config))
}

#[tauri::command]
pub async fn telemetry_prefs_set(
    state: tauri::State<'_, crate::state::AppState>,
    enabled: bool,
) -> Result<(), String> {
    let mut config = state.config.lock().unwrap_or_else(|e| e.into_inner());
    config.send_telemetry = Some(enabled);
    config.save(&state.data_dir).map_err(|e| e.to_string())
}

#[cfg(test)]
mod powerplay_vocabulary_tests {
    use super::*;

    /// An empty local table (the API-only client) still offers every
    /// power and state; a populated one keeps its own spellings and is
    /// not duplicated.
    #[test]
    fn the_dropdowns_are_never_empty_and_never_doubled() {
        let (powers, states) = powerplay_vocabulary(Vec::new(), Vec::new());
        assert_eq!(powers.len(), POWERS.len());
        assert_eq!(states, ["Stronghold", "Fortified", "Exploited", "Contested", "Unoccupied"]);
        let (powers, states) = powerplay_vocabulary(vec!["Li Yong-Rui".into(), "Some New Power".into()], vec!["stronghold".into()]);
        assert_eq!(powers.iter().filter(|p| p.eq_ignore_ascii_case("Li Yong-Rui")).count(), 1);
        assert!(powers.iter().any(|p| p == "Some New Power"));
        assert_eq!(states.iter().filter(|s| s.eq_ignore_ascii_case("stronghold")).count(), 1);
        assert_eq!(states[0], "stronghold", "the table's own spelling is kept");
    }
}
