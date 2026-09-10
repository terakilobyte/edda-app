//! The ship computer's tool registry: one row per tool, and the runners.
//!
//! Descriptions and schemas are load-bearing -- they are the only
//! documentation the model gets (`docs/PLAN.md` §4.1). Tools return
//! structured data, not prose, and every error is a [`CapError`] so the
//! model reads `kind` and `hint` rather than English.

use super::{carrier, commander, galaxy, parse, CapError, CapResult, Ctx, ToolSpec};
use crate::state::AppState;
use serde::Deserialize;
use serde_json::{json, Value};

// ── Registry ──────────────────────────────────────────────────────────

pub static REGISTRY: &[ToolSpec] = &[
    ToolSpec { name: "get_ship_status", description: "Current ship state from the commander's journal: system, docked station, ship type and name, cargo count and capacity, fuel, next jump target with its star class and jumps remaining, and the current system's Powerplay control. First-hand and current. Call this for anything about where the commander is or what they are flying.", schema: no_args, run: get_ship_status },
    ToolSpec { name: "get_carrier_status", description: "The commander's own (or squadron) fleet carrier as the journal last saw it: callsign and name, location (system and body), Tritium in the tank, capacity used and free, services, bank balance, docking access, a pending jump with its departure time, and what the commander moved into the hold. First-hand. EVERY figure carries as_of and age_hours — always say the age ('as of 3 hours ago'), because the stats only refresh when Carrier Management is opened. hold_moved is what the commander moved aboard, NOT the full hold: other commanders' deposits and market sales are invisible from the journal. Use for 'where is my carrier', 'how much tritium', 'what services', 'balance'; for 'plot a route to my carrier' read its location here, then call plot_route; for 'where can I buy tritium for it' call market_search with the carrier's system as origin.", schema: no_args, run: get_carrier_status },
    ToolSpec { name: "list_ships", description: "Every ship the commander owns, from the journal, with WHERE each one is: status with_you (the one being flown), stored (system + station), in_transit (destination + minutes_to_arrival), arrived (transfer time elapsed), or aboard_carrier (the carrier's callsign and its CURRENT system, which moves). Each has ship type, name, ident, jump range, cargo, and an as_of age. Resolve 'my <ship>' in this order and never guess: 1 by name ('Kestrel'); 2 by type + name; 3 by type alone — one match answers, MORE than one means list them by name, location and status and ASK which; 4 by hull ident. Use for 'where is my Anaconda' and, before plot_route, for 'navigate to my Anaconda' — the destination is the ship's system (for a ship aboard a carrier, the carrier's current system); an in-transit ship gets its arrival time, no route. First-hand.", schema: no_args, run: list_ships },
    ToolSpec { name: "plot_carrier_route", description: "Plot a FLEET CARRIER route (500 ly per jump, tritium per jump from the carrier's real mass and tank, the game's 20-minute-per-jump floor as the ETA until the commander's own cadence is measured) from the carrier's last known position (or `from`) to `to`, and start following it: EDDA then counts down each jump, advances on arrival, and puts the next system on the clipboard via carrier_route. The answer carries hops, total fuel, an ETA, and a verdict — `short_by` means the tank plus hold tritium cannot finish the route: say so and where. Use for 'move my carrier to X', 'plot a carrier route to X'. Never use plot_route for a carrier.", schema: plot_carrier_route_schema, run: plot_carrier_route },
    ToolSpec { name: "carrier_route", description: "The carrier route being followed and actions on it. action=status: jumps remaining, the next system with its distance and tritium, ETA and whether a jump is scheduled. action=next: put the next system's name on the clipboard for the carrier's navigation panel. action=clear: stop following. Use for 'what's the next carrier jump', 'how many carrier jumps left', 'clear the carrier route'.", schema: carrier_route_schema, run: carrier_route },
    ToolSpec { name: "get_inventory", description: "Everything the commander currently holds: engineering materials (raw, manufactured, encoded) and cargo, resolved from internal journal symbols to real in-game display names. First-hand and current.", schema: no_args, run: get_inventory },
    ToolSpec { name: "list_engineers", description: "Every engineer the journal knows about, with unlock status (Unlocked / Invited / Known) and rank 1-5. Only 'Unlocked' engineers will actually do work. An engineer absent from this list has not been encountered at all.", schema: no_args, run: list_engineers },
    ToolSpec { name: "check_blueprint_access", description: "Whether the commander can actually apply a given engineering blueprint at a given grade: which engineers offer it, the commander's unlock status with each, and the highest grade reachable with currently unlocked engineers. ALWAYS call this before recommending a blueprint -- community advice does not know the commander's unlock state, and an inaccessible blueprint is not a recommendation.", schema: check_blueprint_access_schema, run: check_blueprint_access },
    ToolSpec { name: "material_shopping_list", description: "For an engineering plan, what the commander is short and how to cover it at material traders from what they already carry: exact trades (give N of X for M of Y, at the raw/manufactured/encoded trader), what is still short after trading, and the nearest traders of each kind needed from the current position. Uses real exchange rates (6:1 per grade up, 3:1 per grade down, x6 across groups). First-hand. Use get_engineering_gap first if you only want the raw shortfall.", schema: material_shopping_list_schema, run: material_shopping_list },
    ToolSpec { name: "game_control", description: "Press one of the commander's own game bindings (their Custom.binds) -- the ship computer's hands. Works only while the game window has focus. Controls: landing_gear, cargo_scoop, lights, night_vision, hardpoints, flight_assist, heat_sink, chaff, shield_cell, ecm, boost, supercruise, hyperspace, jump_or_supercruise, target_next_route, target_ahead, next_target, previous_target, next_hostile, highest_threat, next_subsystem, galaxy_map, system_map, fss, discovery_scan, hud_mode, silent_running, cargo_eject_all, orbit_lines, throttle_zero/50/75/100. Example: 'four pips to systems' = pips_reset, then pips_systems times 2. Toggles just toggle; if you cannot tell the current state, say so.", schema: game_control_schema, run: game_control },
    ToolSpec { name: "set_pips", description: "Set the power distributor to an exact split: systems / engines / weapons pips adding up to 6, each at most 4 (halves allowed). Works out the button presses itself (reset, then the shortest sequence) and reports what was reached. Use this for ANY pips order ('full pips to systems' = 4/1/1; 'four to systems, rest to engines' = 4/2/0; 'balanced' = 2/2/2); do not use game_control for pips. Needs the game window focused.", schema: set_pips_schema, run: set_pips },
    ToolSpec { name: "follow_route", description: "The route the commander is following in the game (plotted here or imported from Spansh) and actions on it. action=status: jumps left, next system, scoop/boost notes. action=target_next: put the next system into the game's galaxy map (key macro; the game window must be focused). action=skip: mark the next hop as done without jumping. action=stop: stop following. Use for spoken orders like 'target the next system', 'what's next', 'how many jumps left'. If nothing is being followed here but the game has its own plotted route (current_route), 'target next' means game_control target_next_route; if neither, say there is no route.", schema: follow_route_schema, run: follow_route },
    ToolSpec { name: "material_sources", description: "Where to collect one material: community-known farm sites (crash sites, crystal shards, Dav's Hope, HGE guidance) and, first-hand, every place the commander has actually picked it up before from their journal — with distance from the current system. Use for anything still short after material_shopping_list; offer plot_route to the chosen site.", schema: material_sources_schema, run: material_sources },
    ToolSpec { name: "ship_modules", description: "The commander's current ship modules from the latest Loadout, engineered ones first: slot, item, module type and blueprint resolved to the same names get_engineering_gap uses, current grade, quality, engineer, experimental effect. Use this to plan from the REAL current grade (pass it as from_grade) instead of from zero, and to see what is already applied. First-hand.", schema: no_args, run: ship_modules },
    ToolSpec { name: "synthesis_recipes", description: "Synthesis recipes (ammo, AFM refill, heat sinks, chaff, life support, limpets, SRV refuel/repair/ammo, FSD injection, AX and Guardian munitions) with every grade's material costs and bonus, from the vendored wiki table, diffed against the commander's live materials so each grade says how many can be made now. Pass name for one recipe (\"FSD Injection\", \"heat sink\"), omit it for the whole list of names. Tech-broker modules (Guardian and Human/anti-xeno items) are blueprints: list_blueprints with module_type \"Guardian\" or \"Human\".", schema: engineer_unlocks_schema, run: synthesis_recipes },
    ToolSpec { name: "engineer_unlocks", description: "How to meet and unlock engineers: home system and base, the invite condition, the unlock task (item and quantity), rank-up hint, and where to get the items -- vendored from the Wanderer's Toolbox step-by-step guide (source URL and fetch date included), merged with the commander's OWN unlock status from the journal so already-unlocked engineers are marked. Pass a name for one engineer, or nothing for all twenty in the guide's recommended order.", schema: engineer_unlocks_schema, run: engineer_unlocks },
    ToolSpec { name: "list_blueprints", description: "Blueprint names available for a module type, from the vendored EDEngineer dataset. Use this to discover what options exist before checking access or cost. Module types include the tech-broker catalogues: \"Guardian\" (Gauss cannons, plasma chargers, shard cannons, FSD booster, hull/module/shield reinforcements, fighters) and \"Human\" (AX and anti-xeno kit: shock cannons, enzyme and flechette racks, Sirius AX racks, meta-alloy hull, engineered FSD V1) — their material costs come from get_engineering_gap like any blueprint.", schema: list_blueprints_schema, run: list_blueprints },
    ToolSpec { name: "get_engineering_gap", description: "Materials still needed for a blueprint, cumulative from a starting grade through a target grade, diffed against the commander's live inventory. Costs come from vendored blueprint data, never estimated.", schema: get_engineering_gap_schema, run: get_engineering_gap },
    ToolSpec { name: "find_system", description: "Look up a star system: distance_ly from the commander's current position (use this for 'how far is X' -- do NOT plot a route just to get a distance), coordinates, allegiance, government, security, population, station count, and Powerplay controlling power and state. Reports whether the Powerplay reading is first-hand (the commander visited) or from community data, plus when it was observed.", schema: find_system_schema, run: find_system },
    ToolSpec { name: "stations_in_system", description: "Stations in a system, ranked by kind (starports first) then distance from arrival, with largest landing pad and services. By default returns only places a ship can dock, excluding fleet carriers and surface settlements -- a developed system has hundreds of settlements and dozens of parked carriers, which bury the handful of real stations. Set include_carriers or include_minor to see them. A null landing pad means unrecorded, not small.", schema: stations_in_system_schema, run: stations_in_system },
    ToolSpec { name: "find_station", description: "Find stations by name across the galaxy, with their system and services.", schema: find_station_schema, run: find_station },
    ToolSpec { name: "nearest_service", description: "Nearest stations offering a service, searching outward from a system (default: where the commander is). Services: market, outfitting, shipyard, raw_material_trader, manufactured_material_trader, encoded_material_trader, interstellar_factors, technology_broker, universal_cartographics, black_market, search_and_rescue, refuel, repair, restock, vista_genomics, crew_lounge, fleet_carrier_vendor, redemption_office, pioneer_supplies, missions. Results carry distance_ly (light-years from the origin system; 0 = same system) and distance_to_arrival (light-SECONDS from the star, inside the system) -- never confuse the two. Filters by minimum landing pad size and can exclude fleet carriers (which move). Use for 'nearest X', 'where can I ...'.", schema: nearest_service_schema, run: nearest_service },
    ToolSpec { name: "get_merit_model", description: "Powerplay merit model calibrated from the commander's OWN sales. Merits are linear in credit profit -- floor(profit / K) -- and K varies by station. Returns per-station K intervals with sample counts. A station with no observations, or with contradictory ones, reports no K rather than an estimate: published formulas (both 70*sqrt(tons) and the community 0.375219*sqrt(profit)) were tested against real sales and do not fit. Never estimate merits for a station this does not cover.", schema: no_args, run: get_merit_model },
    ToolSpec { name: "powerplay_seen", description: "Every system the commander has personally visited where Powerplay control was recorded, with power, state, control progress, reinforcement and undermining figures, and when it was seen. First-hand data, more precise than any external site shows.", schema: no_args, run: powerplay_seen },
    ToolSpec { name: "find_profit", description: "The profit finder: best trades near a system, ranked by credits PER HOUR (not per ton), using community market prices with their age. Uses the commander's live ship (cargo capacity, jump range, landing pad from the hull) unless overridden; if the hull's pad size is unknown the search refuses and says so -- pass min_pad. With from_current_station=true it answers 'I'm docked, what should I fill up with'; otherwise it searches every station in range as a source. Returns single legs, A-to-B round trips, and multi-stop rings (3+ stations, every leg loaded, closed back to the start -- often the best rate), plus counts of stations excluded and why (carriers, pad too small/unknown, stale prices). Time estimates are labelled 'estimated' -- compare legs with them, never quote an ETA as fact.", schema: find_profit_schema, run: find_profit },
    ToolSpec { name: "combat_stats", description: "The commander's combat record from their own journal: kills, bounty and bond credits, deaths, interdictions, most-killed ship types and best-paying factions, plus a timeline bucketed by day/week/hour. Pass `since` (ISO timestamp) to scope to a session or period. First-hand and exact.", schema: combat_stats_schema, run: combat_stats },
    ToolSpec { name: "station_market", description: "A station's full commodity board from community data: buy/sell prices, supply, demand, and when each row was last reported. Station ids come from stations_in_system, find_station, nearest_service or find_profit.", schema: station_market_schema, run: station_market },
    ToolSpec { name: "market_search", description: "Search community market data near a system for a commodity, ship module, or ship. For commodities choose action=buy (commander buys from station: cheapest first, with supply) or action=sell (commander sells to station: highest first, with demand). Module searches accept human names such as '5A fuel scoop'; ship searches accept names such as 'Mandalay'. Results include distance in ly, arrival distance in ls, landing pad, carrier status, data age/update, and price/quantity where relevant. Origin defaults to the commander's current system and pad defaults to their current ship (an unknown hull is refused: pass min_pad). Use this instead of guessing where an item is sold.", schema: market_search_schema, run: market_search },
    ToolSpec { name: "systems_near", description: "Systems within a radius of a system, nearest first, with Powerplay control and population. Useful for 'what's around me' and for reasoning about jump counts.", schema: systems_near_schema, run: systems_near },
    ToolSpec { name: "missions", description: "The commander's missions from their own journal: objective, target faction or named target, kill progress (inferred from kill events and capped at the target), destination for hand-in, reward, expiry, and status (active, ready_to_turn_in, completed, failed, abandoned, expired). Default returns only what is still in play. First-hand.", schema: missions_schema, run: missions },
    ToolSpec { name: "missions_route", description: "A short visiting order for the active missions' hand-in systems, starting from where the commander is (or a given system): nearest-neighbour with 2-opt over straight-line distances. Returns the ordered stops with per-leg light-years, the missions and rewards waiting at each, and the total tour length. Use for 'plan my mission route', 'what order should I do my missions in'. Expiry is reported but not yet weighted into the order.", schema: missions_route_schema, run: missions_route },
    ToolSpec { name: "current_route", description: "The route currently plotted in the galaxy map (from NavRoute.json), hop by hop: system, star class, whether it can be fuel-scooped, hazardous arrivals (neutron stars, white dwarfs, black holes), the best dockable station in each system with pad size, Powerplay controlling power and state, whether that power opposes the commander's pledge, and security level. Also the spoken briefing and the next-hop line. Use for anything about the trip ahead: fuel planning, where to dock, whose space is crossed, how many jumps remain. First-hand for the route, community data for stations and control.", schema: no_args, run: current_route },
    ToolSpec { name: "plot_route", description: "Plot a jump route to a system. A journey from the current system within the commander's route-coverage threshold is handed to Elite's own plotter (the answer says handed_to_game: true, with no hops); longer journeys, or any explicit origin, are planned by EDDA over the galaxy star index (every known system when the full index is built; populated systems only otherwise). Minimises jumps with A*; can use neutron stars (x4) and white dwarfs (x1.5) to supercharge. Returns each hop with star class, scoopable or not, jump length, and whether the jump was boosted, plus totals. Uses the ship's unladen range unless range_ly is given. Does NOT model fuel; max_dry_jumps limits consecutive unscoopable arrivals. Long plots (thousands of ly) can take tens of seconds.", schema: plot_route_schema, run: plot_route },
    ToolSpec { name: "commander_ranks", description: "The commander's career ranks from the journal -- Combat, Trade, Exploration, Mercenary (Soldier), Exobiologist, CQC, Federal and Imperial navy -- each with the rank NAME on the public ladder (e.g. Deadly, Tycoon, Pioneer), the percent progress toward the next rank, and the next rank's name. Also the Powerplay power, rank and merit total. Use this for any 'how far am I from Elite' or 'what rank am I' question. First-hand.", schema: no_args, run: commander_ranks },
    ToolSpec { name: "signal_watch", description: "Signal sources the ship computer announces when they appear on the sensors (spoken and on the HUD). action=list: what is watched and the full menu. action=add / remove with signal = one of: hge (high grade emissions), encoded, degraded, combat_aftermath, weapons_fire, convoy (dispersal pattern), distress, power_convoy (Power convoy distress signal), power_wreckage, power_weapons, power_cz (Power conflict zone), nonhuman (Thargoid), pirates, compromised_beacon, trading_beacon. Use for 'I'm looking for X', 'tell me when you see X', 'stop looking for X'.", schema: signal_watch_schema, run: signal_watch },
    ToolSpec { name: "say", description: "Speak a short line aloud through the ship's voice. Use ONLY when the commander asks to be told something by voice or asks you to speak; keep it to one sentence.", schema: say_schema, run: say },
];

fn no_args() -> Value {
    json!({ "type": "object", "properties": {} })
}

// ── Shared helpers ────────────────────────────────────────────────────

/// Engineering tools take a module type; accept the ways people say it
/// and answer a miss with the valid list instead of an empty result.
pub fn canonical_input(state: &AppState, input: &Value) -> CapResult<Value> {
    match input.get("module_type").and_then(Value::as_str) {
        Some(raw) if !raw.trim().is_empty() => match resolve_module_type(state, raw) {
            Some(c) => {
                let mut v = input.clone();
                v["module_type"] = json!(c);
                Ok(v)
            }
            None => Err(CapError::invalid(format!("unknown module type {raw:?}"))
                .hint("pass one of data.module_types as module_type")
                .data(json!({ "module_types": state.engineering.module_types() }))),
        },
        _ => Ok(input.clone()),
    }
}

/// "fsd", "thruster", "Power Plant" -> the blueprint database's module type.
fn resolve_module_type(state: &AppState, raw: &str) -> Option<String> {
    let norm = |s: &str| -> String {
        let mut n: String = s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        if n.ends_with('s') && n.len() > 3 {
            n.pop();
        }
        n
    };
    let mut key = norm(raw);
    for (alias, canon) in [
        ("fsd", "frameshiftdrive"),
        ("frameshift", "frameshiftdrive"),
        ("drive", "frameshiftdrive"),
        ("thruster", "thruster"),
        ("engine", "thruster"),
        ("pd", "powerdistributor"),
        ("distributor", "powerdistributor"),
        ("plant", "powerplant"),
        ("shield", "shieldgenerator"),
        ("hull", "armour"),
        ("armor", "armour"),
        ("bulkhead", "armour"),
        ("sensor", "sensor"),
        ("interdictor", "frameshiftdriveinterdictor"),
        ("booster", "shieldbooster"),
        ("hrp", "hullreinforcementpackage"),
        ("mrp", "modulereinforcementpackage"),
        ("scb", "shieldcellbank"),
        ("afmu", "autofieldmaintenanceunit"),
        ("scanner", "detailedsurfacescanner"),
        ("collector", "collectorlimpetcontroller"),
    ] {
        if key == alias {
            key = canon.to_string();
        }
    }
    let types = state.engineering.module_types();
    if let Some(t) = types.iter().find(|t| norm(t) == key) {
        return Some(t.to_string());
    }
    // Unique substring match ("cannon" -> "Multi-cannon" is not unique; "surface" -> "Detailed Surface Scanner" is).
    let hits: Vec<&&str> = types.iter().filter(|t| norm(t).contains(&key) && key.len() >= 4).collect();
    if hits.len() == 1 {
        return Some(hits[0].to_string());
    }
    None
}

fn clean_security(v: &mut Value, pointer: &str) {
    // "$SYSTEM_SECURITY_medium;" -> "medium": the model should not have to decode journal symbols.
    if let Some(sec) = v.pointer_mut(pointer) {
        if let Some(s) = sec.as_str() {
            let clean = s.trim_start_matches("$SYSTEM_SECURITY_").trim_end_matches(';').to_string();
            *sec = json!(clean);
        }
    }
}

fn to_json<T: serde::Serialize>(v: T) -> Value {
    serde_json::to_value(v).unwrap_or(json!({}))
}

// ── Commander ─────────────────────────────────────────────────────────

fn get_ship_status(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    let mut v = commander::status(ctx.state)?;
    clean_security(&mut v, "/location/system_security");
    Ok(v)
}

fn list_ships(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    let ships = commander::ships_list(ctx.state, &commander::ShipsListRequest { include_historical: false })?;
    Ok(json!({ "ships": ships, "note": "Locations are journal-derived: stored ships from the last shipyard visit (as_of), moving ships from the transfer request. A ship aboard a carrier reports the carrier's current system." }))
}

fn plot_carrier_route_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "to": { "type": "string", "description": "destination system" },
            "from": { "type": "string", "description": "origin; omit for the carrier's last known position" }
        },
        "required": ["to"]
    })
}

fn plot_carrier_route(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let to = input.get("to").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| CapError::invalid("to is required"))?;
    let from = input.get("from").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    crate::carrier_follow::plot_and_follow(ctx.state, to, from).map_err(|e| CapError::invalid(e).hint("check the system names; the carrier's position comes from Carrier Management"))
}

fn carrier_route_schema() -> Value {
    json!({ "type": "object", "properties": { "action": { "type": "string", "enum": ["status", "next", "clear"] } }, "required": ["action"] })
}

fn carrier_route(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let action = input.get("action").and_then(Value::as_str).unwrap_or("status");
    match action {
        "next" => {
            let plan = ctx.state.with_read(|s| crate::carrier_follow::load(s.conn())).ok_or_else(|| CapError::not_found("no carrier route is being followed"))?;
            let next = plan.next_hop().ok_or_else(|| CapError::not_found("the carrier route is complete"))?;
            crate::follow::set_clipboard(&next.name).map_err(CapError::internal)?;
            Ok(json!({ "next_system": next.name, "distance_ly": next.distance_ly, "fuel_t": next.fuel_t, "clipboard": true }))
        }
        "clear" => {
            ctx.state.with_store(|s| crate::carrier_follow::clear(s.conn()));
            Ok(crate::carrier_follow::view(None))
        }
        _ => Ok(crate::carrier_follow::view(ctx.state.with_read(|s| crate::carrier_follow::load(s.conn())).as_ref())),
    }
}

fn get_carrier_status(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    carrier::status(ctx.state)
}

fn get_inventory(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    Ok(json!({ "items": commander::inventory(ctx.state)?, "provenance": "journal" }))
}

fn list_engineers(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    Ok(json!({ "engineers": commander::engineers(ctx.state)?, "provenance": "journal" }))
}

fn get_merit_model(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    let m = commander::merit_model(ctx.state)?;
    Ok(json!({
        "stations": m.stations,
        "model": "merits = floor(profit / K); K is per-station and its driver is unknown",
        "warning": "Do not estimate merits for a station absent from this list.",
    }))
}

fn powerplay_seen(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    Ok(json!({ "systems": commander::powerplay_seen(ctx.state)?, "provenance": "journal" }))
}

fn combat_stats_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "since": { "type": "string", "description": "ISO-8601, e.g. 2026-08-20T00:00:00Z; omit for all time" },
            "bucket": { "type": "string", "enum": ["hour", "day", "week"], "description": "timeline granularity, default day" }
        }
    })
}

fn combat_stats(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: commander::CombatRequest = parse(input)?;
    let summary = commander::combat_summary(ctx.state, &req)?;
    let timeline = commander::combat_timeline(ctx.state, &req)?;
    Ok(json!({ "summary": summary, "timeline": timeline, "provenance": "journal" }))
}

fn missions_schema() -> Value {
    json!({ "type": "object", "properties": { "active_only": { "type": "boolean", "description": "default true" } } })
}

fn missions(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: commander::MissionsRequest = parse(input)?;
    let (list, now) = commander::missions(ctx.state, &req)?;
    Ok(json!({ "missions": list, "now": now, "provenance": "journal",
        "note": "kills_done is inferred from kill events by victim faction and may over-count kills made outside the destination system" }))
}

fn missions_route_schema() -> Value {
    json!({ "type": "object", "properties": { "system": { "type": "string", "description": "start system; default: where the commander is" } } })
}

fn missions_route(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let start_system = input.get("system").and_then(Value::as_str).map(str::to_string);
    let (list, now) = commander::missions(ctx.state, &commander::MissionsRequest { active_only: true })?;
    ctx.state.with_read(|s| {
        let conn = s.conn();
        let origin = galaxy::system_or_current(conn, start_system.as_deref())?;
        let start = galaxy::origin_coords(conn, &origin)?;
        // One stop per destination system, with everything waiting there.
        let mut stops: Vec<(String, (f64, f64, f64), Vec<&ed_store::missions::Mission>)> = Vec::new();
        let mut unlocatable: Vec<String> = Vec::new();
        for mission in &list {
            let Some(system) = mission.destination_system.as_deref().filter(|d| !d.is_empty()) else { continue };
            if let Some(stop) = stops.iter_mut().find(|(name, _, _)| name.eq_ignore_ascii_case(system)) {
                stop.2.push(mission);
                continue;
            }
            match galaxy::origin_coords(conn, system) {
                Ok(pos) => stops.push((system.to_string(), pos, vec![mission])),
                Err(_) => {
                    if !unlocatable.iter().any(|u| u.eq_ignore_ascii_case(system)) {
                        unlocatable.push(system.to_string());
                    }
                }
            }
        }
        if stops.is_empty() {
            return Ok(json!({
                "start": origin, "stops": [], "unlocatable": unlocatable, "now": now,
                "note": "no active missions with a locatable hand-in system",
            }));
        }
        let points: Vec<(f64, f64, f64)> = stops.iter().map(|(_, p, _)| *p).collect();
        let order = crate::mission_route::order_stops(start, &points);
        let mut here = start;
        let legs: Vec<Value> = order.iter().map(|&i| {
            let (name, pos, missions) = &stops[i];
            let leg_ly = ((here.0 - pos.0).powi(2) + (here.1 - pos.1).powi(2) + (here.2 - pos.2).powi(2)).sqrt();
            here = *pos;
            json!({
                "system": name,
                "leg_ly": (leg_ly * 10.0).round() / 10.0,
                "missions": missions.iter().map(|m| json!({
                    "title": m.title, "station": m.destination_station,
                    "reward": m.reward, "expiry": m.expiry, "status": m.status,
                })).collect::<Vec<_>>(),
                "reward_total": missions.iter().filter_map(|m| m.reward).sum::<i64>(),
            })
        }).collect();
        let total = crate::mission_route::tour_ly(start, &points, &order);
        Ok(json!({
            "start": origin,
            "stops": legs,
            "total_ly": (total * 10.0).round() / 10.0,
            "unlocatable": unlocatable,
            "now": now,
            "note": "straight-line light-years; plot each leg for jumps. Expiry is not yet weighted into the order.",
        }))
    })
}

fn commander_ranks(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    let mut v = to_json(commander::ranks(ctx.state)?);
    // Powerplay as one object: power, rank, merits together read better than three loose fields.
    if let Some(obj) = v.as_object_mut() {
        let power = obj.remove("powerplay_power").unwrap_or(Value::Null);
        let rank = obj.remove("powerplay_rank").unwrap_or(Value::Null);
        let merits = obj.remove("powerplay_merits").unwrap_or(Value::Null);
        obj.insert("powerplay".into(), json!({ "power": power, "rank": rank, "merits": merits }));
    }
    Ok(json!({ "ranks": v, "provenance": "journal", "note": "progress is percent toward the next rank as the game reports it; the underlying point curve is not exposed" }))
}

fn ship_modules(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    // Same resolution as the Engineering tab, via the command's logic.
    let raw = ctx.state.with_read(|s| ed_store::session::latest_event_raw(s.conn(), "Loadout").ok().flatten());
    let v = raw
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .ok_or_else(|| CapError::not_found("no Loadout in the journal yet").hint("the game writes one on load; ask the commander to check the game is running"))?;
    let mods: Vec<Value> = v
        .get("Modules")
        .and_then(Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter_map(|m| {
                    let item = m.get("Item")?.as_str()?.to_string();
                    let mt = ed_engineering::journal::module_type_for_item(&item);
                    let eng = m.get("Engineering");
                    let sym = eng.and_then(|e| e.get("BlueprintName")).and_then(Value::as_str);
                    let bp = match (sym, mt) {
                        (Some(s), Some(t)) => ed_engineering::journal::blueprint_for_symbol(s, t),
                        _ => None,
                    };
                    Some(json!({
                        "slot": m.get("Slot"), "item": item, "module_type": mt,
                        "blueprint_symbol": sym, "blueprint": bp,
                        "grade": eng.and_then(|e| e.get("Level")),
                        "quality": eng.and_then(|e| e.get("Quality")),
                        "engineer": eng.and_then(|e| e.get("Engineer")),
                        "experimental": eng.and_then(|e| e.get("ExperimentalEffect_Localised")),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "ship": v.get("Ship").and_then(Value::as_str).map(|t| ed_journal::ships::display_name_or(t, v.get("Ship_Localised").and_then(Value::as_str))),
        "ship_name": v.get("ShipName"), "modules": mods, "provenance": "journal",
        "note": "module_type/blueprint are null when the mapping is unknown; plan from grade with from_grade"
    }))
}

// ── Engineering ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(default)]
struct BlueprintAccessRequest {
    module_type: String,
    blueprint_name: String,
    grade: i64,
}

impl Default for BlueprintAccessRequest {
    fn default() -> Self {
        BlueprintAccessRequest { module_type: String::new(), blueprint_name: String::new(), grade: 5 }
    }
}

fn check_blueprint_access_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "module_type": { "type": "string", "description": "e.g. 'Power Plant', 'Frame Shift Drive'" },
            "blueprint_name": { "type": "string", "description": "e.g. 'Overcharged', 'Increased FSD Range'" },
            "grade": { "type": "integer", "description": "1-5" }
        },
        "required": ["module_type", "blueprint_name", "grade"]
    })
}

fn check_blueprint_access(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: BlueprintAccessRequest = parse(input)?;
    let state = ctx.state;
    let engineers = commander::engineers(state)?;
    let status_of = |n: &str| -> (String, Option<i64>, bool) {
        match engineers.iter().find(|e| e.name.eq_ignore_ascii_case(n)) {
            Some(e) => (e.progress.clone().unwrap_or_else(|| "Unknown".into()), e.rank, e.is_unlocked()),
            None => ("Not known".into(), None, false),
        }
    };
    let (module_type, name, grade) = (&req.module_type, &req.blueprint_name, req.grade);
    let Some(bp) = state.engineering.find(module_type, name, grade) else {
        return Err(CapError::not_found(format!("no blueprint {name:?} at grade {grade} for {module_type:?}"))
            .hint("pick a blueprint from data.available")
            .data(json!({ "available": state.engineering.blueprint_names_for(module_type) })));
    };
    let engineers_out: Vec<Value> = bp
        .engineers
        .iter()
        .map(|e| {
            let (status, rank, unlocked) = status_of(e);
            json!({ "engineer": e, "status": status, "rank": rank, "unlocked": unlocked })
        })
        .collect();
    let reachable = engineers_out.iter().any(|e| e.get("unlocked").and_then(Value::as_bool).unwrap_or(false));
    let mut max_reachable = None;
    for g in 1..=5 {
        if let Some(b) = state.engineering.find(module_type, name, g) {
            if b.engineers.iter().any(|e| status_of(e).2) {
                max_reachable = Some(g);
            }
        }
    }
    Ok(json!({
        "module_type": module_type,
        "blueprint": name,
        "grade": grade,
        "engineers": engineers_out,
        "reachable": reachable,
        "max_reachable_grade": max_reachable,
        "provenance": "journal + vendored blueprint data",
    }))
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ShoppingRequest {
    module_type: String,
    blueprint: Option<String>,
    from_grade: i64,
    target_grade: i64,
    experimental: Option<String>,
    minimum: bool,
    complete_target: bool,
}

impl Default for ShoppingRequest {
    fn default() -> Self {
        ShoppingRequest { module_type: String::new(), blueprint: None, from_grade: 0, target_grade: 5, experimental: None, minimum: false, complete_target: true }
    }
}

fn material_shopping_list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "module_type": { "type": "string" },
            "blueprint": { "type": "string", "description": "Graded blueprint name; omit for experimental-only" },
            "from_grade": { "type": "integer", "description": "Grade already applied (see ship_modules); default 0" },
            "target_grade": { "type": "integer", "description": "default 5" },
            "experimental": { "type": "string", "description": "Experimental effect name, optional" },
            "minimum": { "type": "boolean", "description": "one roll per grade instead of realistic N rolls at grade N" },
            "complete_target": { "type": "boolean", "description": "default true: N rolls at the target grade too, which completes it (each roll adds 1/N, measured from the journal). false = one roll, which merely reaches the grade" }
        },
        "required": ["module_type"]
    })
}

fn material_shopping_list(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let q: ShoppingRequest = parse(input)?;
    let galaxy = ctx.state.routing.galaxy(&ctx.state.data_dir);
    let mut r = ctx.state.with_read(|st| {
        crate::commands::shopping_for(st.conn(), galaxy.as_deref(), &ctx.state.engineering, &q.module_type, q.blueprint.as_deref(), q.from_grade, q.target_grade, q.minimum, q.complete_target, q.experimental.as_deref())
    })?;
    tauri::async_runtime::block_on(crate::commands::fill_traders(ctx.state, &mut r));
    Ok(to_json(&r))
}

fn material_sources_schema() -> Value {
    json!({ "type": "object", "properties": { "material": { "type": "string", "description": "display name, e.g. Tellurium" } }, "required": ["material"] })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct MaterialRequest {
    material: String,
}

fn material_sources(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: MaterialRequest = parse(input)?;
    let galaxy = ctx.state.routing.galaxy(&ctx.state.data_dir);
    let r = ctx.state.with_read(|st| crate::commands::sources_for(st.conn(), galaxy.as_deref(), &req.material))?;
    Ok(to_json(&r))
}

fn engineer_unlocks_schema() -> Value {
    json!({ "type": "object", "properties": { "name": { "type": "string", "description": "engineer name; omit for all" } } })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct NameRequest {
    name: Option<String>,
}

fn synthesis_recipes(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: NameRequest = parse(input)?;
    let name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let catalog = &ctx.state.engineering;
    let materials = ed_journal::Catalog::load();
    let have = |display: &str| -> i64 {
        let symbol = materials.by_name(display).map(|i| i.symbol.clone()).unwrap_or_else(|| display.replace(' ', "").to_ascii_lowercase());
        ctx.state
            .with_read(|s| Ok::<i64, CapError>(s.conn().query_row("SELECT count FROM materials WHERE symbol = ?1 COLLATE NOCASE", [symbol], |r| r.get::<_, i64>(0)).unwrap_or(0)))
            .unwrap_or(0)
    };
    let describe = |r: &ed_engineering::SynthesisRecipe| {
        let grades: Vec<Value> = r
            .grades
            .iter()
            .map(|g| {
                let ingredients: Vec<Value> = g
                    .ingredients
                    .iter()
                    .map(|i| json!({ "material": i.name, "need": i.count, "have": have(&i.name) }))
                    .collect();
                let can_make = g.ingredients.iter().map(|i| have(&i.name) / i.count.max(1)).min().unwrap_or(0);
                json!({ "grade": g.grade, "bonus": g.bonus, "ingredients": ingredients, "can_make_now": can_make })
            })
            .collect();
        json!({ "name": r.name, "category": r.category, "refills": r.refills, "grades": grades })
    };
    let provenance = catalog.synthesis_provenance();
    match name {
        Some(n) => match catalog.find_synthesis(n) {
            Some(r) => Ok(json!({ "recipe": describe(r), "source": provenance.source, "fetched": provenance.fetched, "provenance": "vendored" })),
            None => {
                let names: Vec<&str> = catalog.synthesis_recipes().iter().map(|r| r.name.as_str()).collect();
                Err(CapError::invalid(format!("no synthesis recipe matches {n:?}")).hint(format!("one of: {}", names.join(", "))))
            }
        },
        None => Ok(json!({
            "recipes": catalog.synthesis_recipes().iter().map(|r| json!({ "name": r.name, "category": r.category, "refills": r.refills })).collect::<Vec<_>>(),
            "source": provenance.source, "fetched": provenance.fetched, "provenance": "vendored",
            "hint": "pass name for a recipe's grades, costs and how many you can make now",
        })),
    }
}

fn engineer_unlocks(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: NameRequest = parse(input)?;
    let name = req.name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let known = commander::engineers(ctx.state).unwrap_or_default();
    let status_of = |n: &str| {
        known.iter().find(|e| {
            let a = e.name.to_lowercase();
            let b = n.to_lowercase();
            a == b || b.split_whitespace().last().is_some_and(|l| a.ends_with(l))
        })
    };
    let guide = ed_engineering::unlocks::guide();
    let entries: Vec<Value> = guide
        .engineers
        .iter()
        .filter(|e| name.is_none_or(|n| ed_engineering::unlocks::find(n).is_some_and(|f| f.name == e.name)))
        .map(|e| {
            let st = status_of(&e.name);
            json!({
                "name": e.name, "system": e.system, "base": e.base,
                "invite": e.invite, "unlock": e.unlock, "rank_up": e.rank_up, "notes": e.notes,
                "guide_step": e.step,
                "commander_status": st.and_then(|s| s.progress.clone()).unwrap_or_else(|| "Not known".to_string()),
                "commander_rank": st.and_then(|s| s.rank),
                "unlocked": st.is_some_and(|s| s.is_unlocked()),
            })
        })
        .collect();
    if entries.is_empty() {
        return Err(CapError::not_found(format!("no engineer matching {:?} in the guide", name.unwrap_or("")))
            .hint("use one of data.available, or omit name for all")
            .data(json!({ "available": guide.engineers.iter().map(|e| e.name.clone()).collect::<Vec<_>>() })));
    }
    Ok(json!({ "engineers": entries, "source": guide.source, "fetched": guide.fetched, "note": guide.note,
        "provenance": "vendored community guide + journal unlock status" }))
}

fn list_blueprints_schema() -> Value {
    json!({ "type": "object", "properties": { "module_type": { "type": "string" } }, "required": ["module_type"] })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ModuleTypeRequest {
    module_type: String,
}

fn list_blueprints(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: ModuleTypeRequest = parse(input)?;
    let module_type = req.module_type.trim();
    if module_type.is_empty() {
        return Ok(json!({ "module_types": ctx.state.engineering.module_types(), "note": "pass one of these as module_type to list its blueprints" }));
    }
    let eng = &ctx.state.engineering;
    Ok(json!({
        "module_type": module_type,
        "blueprints": eng.blueprint_names_for(module_type).into_iter().map(|n| {
            let grades = eng.grades_for(module_type, n);
            json!({ "name": n, "grades": grades, "experimental_effect": grades.is_empty() })
        }).collect::<Vec<_>>(),
        "note": "experimental effects have no grade; never ask for a grade on one",
    }))
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct GapRequest {
    module_type: String,
    blueprint_name: String,
    from_grade: i64,
    target_grade: i64,
    complete_target: bool,
}

impl Default for GapRequest {
    fn default() -> Self {
        GapRequest { module_type: String::new(), blueprint_name: String::new(), from_grade: 0, target_grade: 1, complete_target: true }
    }
}

fn get_engineering_gap_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "module_type": { "type": "string" },
            "blueprint_name": { "type": "string" },
            "from_grade": { "type": "integer", "description": "0 if starting from scratch" },
            "target_grade": { "type": "integer", "description": "1-5" }
        },
        "required": ["module_type", "blueprint_name", "target_grade"]
    })
}

fn get_engineering_gap(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: GapRequest = parse(input)?;
    let state = ctx.state;
    let have = state.with_read(|s| {
        let catalog = ed_journal::Catalog::load();
        let mut have = std::collections::HashMap::new();
        if let Ok(mut stmt) = s.conn().prepare("SELECT symbol, count FROM materials") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                for (symbol, count) in rows.flatten() {
                    let display = catalog.by_symbol(&symbol).map(|i| i.name.clone()).unwrap_or(symbol);
                    have.insert(display, count);
                }
            }
        }
        have
    });
    let (module_type, blueprint_name) = (&req.module_type, &req.blueprint_name);
    // An experimental effect has no grades: report its single cost.
    if state.engineering.grades_for(module_type, blueprint_name).is_empty() {
        return match state.engineering.experimental_gap(module_type, blueprint_name, &have) {
            Some(gap) => Ok(json!({ "experimental_effect": true, "gap": gap, "note": "one application, no grade" })),
            None => Err(CapError::not_found(format!("no blueprint or experimental effect {blueprint_name:?} for {module_type:?}"))
                .hint("list_blueprints shows what exists for this module type")),
        };
    }
    Ok(json!({
        "expected": state.engineering.gap_report_realistic(module_type, blueprint_name, req.from_grade, req.target_grade, req.complete_target, &have),
        "minimum": state.engineering.gap_report(module_type, blueprint_name, req.from_grade, req.target_grade, &have),
        "note": "expected assumes N rolls at grade N to unlock the next grade (measured from the commander's own crafts: 1/2/3/4) and one roll at the target; minimum is one roll per grade and is never enough past grade 1",
    }))
}

// ── Hands: game control, pips, route following, voice ─────────────────

fn game_control_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "times": { "type": "integer", "description": "repeat count, 1-8" }
        },
        "required": ["name"]
    })
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ControlRequest {
    name: String,
    times: u32,
}

impl Default for ControlRequest {
    fn default() -> Self {
        ControlRequest { name: String::new(), times: 1 }
    }
}

fn game_control(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: ControlRequest = parse(input)?;
    Ok(match ctx.fx.press(ctx.state, &req.name, req.times) {
        Ok(m) => json!({ "ok": true, "pressed": m }),
        Err(e) => json!({ "ok": false, "error": e }),
    })
}

fn set_pips_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "systems": { "type": "number" },
            "engines": { "type": "number" },
            "weapons": { "type": "number" }
        },
        "required": ["systems", "engines", "weapons"]
    })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PipsRequest {
    systems: f32,
    engines: f32,
    weapons: f32,
}

fn set_pips(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: PipsRequest = parse(input)?;
    let (presses, reached) = crate::control::pip_presses([req.systems, req.engines, req.weapons])
        .map_err(|e| CapError::invalid(e).hint("pips add up to 6, each at most 4, halves allowed"))?;
    let mut done = Vec::new();
    for (name, times) in &presses {
        match ctx.fx.press(ctx.state, name, *times) {
            Ok(m) => done.push(m),
            Err(e) => return Ok(json!({ "ok": false, "error": e, "pressed": done })),
        }
    }
    Ok(json!({ "ok": true, "reached": { "systems": reached[0], "engines": reached[1], "weapons": reached[2] }, "presses": presses.iter().map(|(n, t)| format!("{n} x{t}")).collect::<Vec<_>>() }))
}

fn follow_route_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "action": { "type": "string", "enum": ["status", "target_next", "skip", "stop"] } },
        "required": ["action"]
    })
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct ActionRequest {
    action: String,
    signal: String,
}

impl Default for ActionRequest {
    fn default() -> Self {
        ActionRequest { action: "status".into(), signal: String::new() }
    }
}

fn follow_route(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: ActionRequest = parse(input)?;
    let (state, fx) = (ctx.state, ctx.fx);
    let ar = state.with_read(|s| crate::follow::load(s.conn()));
    Ok(match (req.action.as_str(), ar) {
        (_, None) => json!({ "active": false, "note": "no route is being followed; plot or import one and press Follow in the Route tab" }),
        ("status", Some(a)) => json!({ "active": true, "spoken": crate::follow::advance_text(&a), "view": crate::follow::view(Some(&a)) }),
        ("target_next", Some(_)) => match fx.target_next(state) {
            Ok(m) => json!({ "ok": true, "result": m }),
            Err(e) => json!({ "ok": false, "error": e }),
        },
        ("skip", Some(mut a)) => {
            a.next = (a.next + 1).min(a.route.hops.len());
            let saved = fx.follow(state, &a);
            json!({ "ok": saved, "spoken": crate::follow::advance_text(&a) })
        }
        ("stop", Some(_)) => {
            let r = fx.stop_following(state);
            let game = fx.clear_in_game(state);
            json!({ "ok": r.is_ok(), "spoken": "Route cleared.", "game": game.unwrap_or_else(|e| format!("not cleared in game: {e}")) })
        }
        (other, _) => return Err(CapError::invalid(format!("unknown action {other}")).hint("action is status, target_next, skip or stop")),
    })
}

fn signal_watch_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["list", "add", "remove"] },
            "signal": { "type": "string" }
        },
        "required": ["action"]
    })
}

fn signal_watch(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let mut req: ActionRequest = parse(input)?;
    if req.action == "status" {
        req.action = "list".into();
    }
    let sig = req.signal.as_str();
    let mut ids = crate::callouts::signal_watch();
    if req.action != "list" {
        let Some((id, _, _)) = crate::callouts::signal_by_words(sig).or_else(|| crate::callouts::SIGNALS.iter().find(|(i, _, _)| *i == sig)) else {
            return Err(CapError::not_found(format!("unknown signal {sig:?}"))
                .hint("signal is one of data.signals[].id")
                .data(json!({ "signals": crate::callouts::SIGNALS.iter().map(|(i, l, _)| json!({ "id": i, "label": l })).collect::<Vec<_>>() })));
        };
        if req.action == "add" {
            ids.push(id.to_string());
        } else {
            ids.retain(|w| w != id);
        }
        crate::commands::set_signal_watch(ctx.state, ids)?;
    }
    let on = crate::callouts::signal_watch();
    Ok(json!({
        "watching": crate::callouts::SIGNALS.iter().filter(|(i, _, _)| on.iter().any(|w| w == i)).map(|(i, l, _)| json!({ "id": i, "label": l })).collect::<Vec<_>>(),
        "available": crate::callouts::SIGNALS.iter().map(|(i, _, _)| *i).collect::<Vec<_>>()
    }))
}

fn say_schema() -> Value {
    json!({ "type": "object", "properties": { "text": { "type": "string" } }, "required": ["text"] })
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SayRequest {
    text: String,
}

fn say(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: SayRequest = parse(input)?;
    let spoken = crate::commands::speakable(&req.text);
    let backend = ctx.fx.say(ctx.state, &spoken);
    Ok(json!({ "spoken": spoken, "backend": backend }))
}

// ── Galaxy ────────────────────────────────────────────────────────────

fn find_system_schema() -> Value {
    json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] })
}

fn find_system(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: galaxy::FindSystemRequest = parse(input)?;
    let here = commander::current_system_name(ctx.state);
    let found = tauri::async_runtime::block_on(crate::remote_lookup::find_system(ctx.state, &req))
        .ok_or_else(|| crate::remote_lookup::api_down("system"))?;
    let Some(sys) = found else {
        return Ok(json!({ "found": false, "system": req.name }));
    };
    let galaxy = ctx.state.routing.galaxy(&ctx.state.data_dir);
    let origin = ctx
        .state
        .with_read(|s| here.as_deref().and_then(|h| galaxy::coords_hint(s.conn(), galaxy.as_deref(), h)));
    let mut v = to_json(&sys);
    if let (Some(o), Some(c)) = (origin, sys.coords) {
        let d = ((c.0 - o.0).powi(2) + (c.1 - o.1).powi(2) + (c.2 - o.2).powi(2)).sqrt();
        v["distance_ly"] = json!((d * 10.0).round() / 10.0);
        v["distance_from"] = json!(here);
    }
    clean_security(&mut v, "/security");
    Ok(v)
}

fn stations_in_system_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "system": { "type": "string" },
            "include_carriers": { "type": "boolean", "description": "default false; carriers move, so routes through them can evaporate" },
            "include_minor": { "type": "boolean", "description": "default false; adds settlements, construction depots and installations" }
        },
        "required": ["system"]
    })
}

fn stations_in_system(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: galaxy::StationsInSystemRequest = parse(input)?;
    let v = tauri::async_runtime::block_on(crate::remote_lookup::stations_in_system(ctx.state, &req))
        .ok_or_else(|| crate::remote_lookup::api_down("stations"))?;
    Ok(json!({
        "system": req.system,
        "stations": v,
        "filtered": !req.include_carriers || !req.include_minor,
        "provenance": "community",
    }))
}

fn find_station_schema() -> Value {
    json!({ "type": "object", "properties": { "name": { "type": "string" } }, "required": ["name"] })
}

fn find_station(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: galaxy::FindStationRequest = parse(input)?;
    let stations = tauri::async_runtime::block_on(crate::remote_lookup::find_station(ctx.state, &req))
        .ok_or_else(|| crate::remote_lookup::api_down("stations"))?;
    Ok(json!({ "stations": stations }))
}

fn nearest_service_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "system": { "type": "string", "description": "System to search from; default the commander's current system" },
            "service": { "type": "string", "description": "one of the services listed above" },
            "min_pad": { "type": "string", "enum": ["small", "medium", "large"] },
            "radius_ly": { "type": "number", "description": "default 50" },
            "include_carriers": { "type": "boolean", "description": "default false" }
        },
        "required": ["service"]
    })
}

fn nearest_service(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: galaxy::NearestServiceRequest = parse(input)?;
    let service = req.service_key();
    let note = "distance_ly is light-years from the origin system (0 = same system); distance_to_arrival is light-seconds from the star inside that system";
    // Material traders: the dump only says "Material Trader"; the kind follows the station economy.
    if let Some(kind) = service.strip_suffix("_material_trader").or_else(|| service.strip_suffix("_trader")) {
        if matches!(kind, "raw" | "manufactured" | "encoded") {
            let system = ctx.state.with_read(|s| galaxy::system_or_current(s.conn(), req.system.as_deref()))?;
            let v = tauri::async_runtime::block_on(crate::remote_lookup::nearest_material_traders(ctx.state, &system, kind, req.radius_ly.max(150.0), 10))
                .ok_or_else(|| crate::remote_lookup::api_down("material traders"))?;
            return Ok(json!({ "origin": system, "service": format!("{kind} material trader"), "results": v, "note": note, "provenance": "community" }));
        }
    }
    let (system, v) = tauri::async_runtime::block_on(crate::remote_lookup::nearest_service(ctx.state, &req))
        .ok_or_else(|| crate::remote_lookup::api_down("nearest service"))?;
    let hint = if v.is_empty() {
        Some(format!(
            "nothing within {:.0} ly{}; do not repeat this call unchanged -- widen radius_ly, drop min_pad, or tell the commander",
            req.radius_ly,
            if req.pad().is_some() { " with that pad filter (stations with unknown pad size are excluded)" } else { "" }
        ))
    } else {
        None
    };
    Ok(json!({ "origin": system, "service": service, "results": v, "note": note, "hint": hint, "provenance": "community" }))
}

fn station_market_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "station_id": { "type": "integer" }
        },
        "required": ["station_id"]
    })
}

/// A station's board from the community API. An empty `commodities`
/// means the station's board was observed and lists nothing; a server
/// that does not answer is an error, never an empty board.
fn station_market(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: galaxy::StationMarketRequest = parse(input)?;
    let mut v = tauri::async_runtime::block_on(crate::commands::station_board(ctx.state, req.station_id))?;
    if let Some(obj) = v.as_object_mut() {
        if let Some(entries) = obj.remove("entries") {
            obj.insert("commodities".into(), entries);
        }
    }
    Ok(v)
}

fn market_search_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "kind": { "type": "string", "enum": ["commodity", "module", "ship"] },
            "text": { "type": "string", "description": "exact commodity name, or human module/ship search text" },
            "action": { "type": "string", "enum": ["buy", "sell"], "description": "commodities only; default buy" },
            "system": { "type": "string", "description": "origin; omit for current system" },
            "radius_ly": { "type": "number", "description": "default 100, maximum 500" },
            "max_age_hours": { "type": "number", "description": "commodities only; default 48" },
            "include_carriers": { "type": "boolean", "description": "default false" },
            "min_pad": { "type": "string", "enum": ["any", "small", "medium", "large"], "description": "omit to use current ship" }
        },
        "required": ["kind", "text"]
    })
}

/// The model gets 25 hits; the panels page through more.
const MARKET_SEARCH_TOOL_LIMIT: usize = 25;

/// Same rule as the panels: the data-source choice decides, an empty
/// local answer over a coverage gap says so (`coverage.status ==
/// "gap"`, `offer`), and `source: "community"` is the Commander's
/// consent to ask the API this once.
fn market_search(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let mut req: galaxy::MarketSearchRequest = parse(input)?;
    req.limit = Some(MARKET_SEARCH_TOOL_LIMIT);
    let kind = req.kind.clone();
    let mut v = tauri::async_runtime::block_on(crate::commands::market_search_of(ctx.state, &kind, req.clone()))?;
    // The tool's shape: `kind`, `item`, `action` alongside the shared fields.
    if let Some(obj) = v.as_object_mut() {
        let kind = match req.kind.trim().to_ascii_lowercase().as_str() {
            "module" | "outfitting" => "module",
            "ship" | "shipyard" => "ship",
            _ => "commodity",
        };
        obj.insert("kind".into(), json!(kind));
        let item = obj.remove("commodity").or_else(|| obj.remove("query")).unwrap_or(json!(req.text));
        obj.insert("item".into(), item);
        if let Some(side) = obj.remove("side") {
            obj.insert("action".into(), side);
        }
    }
    Ok(v)
}

fn systems_near_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "system": { "type": "string" },
            "radius_ly": { "type": "number", "description": "default 20" }
        },
        "required": ["system"]
    })
}

fn systems_near(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: galaxy::SystemsNearRequest = parse(input)?;
    let systems = tauri::async_runtime::block_on(crate::remote_lookup::systems_near(ctx.state, &req))
        .ok_or_else(|| crate::remote_lookup::api_down("systems near"))?;
    Ok(json!({ "origin": req.system, "systems": systems }))
}

// ── Trade ─────────────────────────────────────────────────────────────

fn find_profit_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "system": { "type": "string", "description": "Origin system; default the commander's current system" },
            "from_current_station": { "type": "boolean", "description": "Only buy at the docked station" },
            "radius_ly": { "type": "number", "description": "Search radius, default 40, max 120" },
            "max_age_hours": { "type": "number", "description": "Ignore prices older than this, default 168 (a week)" },
            "include_carriers": { "type": "boolean", "description": "default false" },
            "cargo_capacity": { "type": "integer", "description": "Override the live ship's cargo" },
            "min_pad": { "type": "string", "enum": ["any", "small", "medium", "large"], "description": "Override the pad filter; omit to derive it from the current hull" },
            "limit": { "type": "integer", "description": "default 10" },
            "max_stations": { "type": "integer", "description": "nearest-N station cap; 0 = all in radius (default)" },
            "max_stops": { "type": "integer", "description": "longest ring to search, default 5, max 12" },
            "buy_power": { "type": "string", "description": "Only buy in systems controlled by this power; 'mine' = the commander's pledge" },
            "buy_state": { "type": "string", "description": "Only buy in systems in this Powerplay state, e.g. Exploited, Fortified, Stronghold; 'none' = uncontrolled" },
            "sell_power": { "type": "string", "description": "Only sell in systems controlled by this power; 'mine' = the commander's pledge. Selling in your own power's systems can earn merits." },
            "sell_state": { "type": "string", "description": "Only sell in systems in this Powerplay state" },
            "buy_power_mode": { "type": "string", "enum": ["controls", "present", "undermining"], "description": "how buy_power matches: the power controls the system (default), is present at all, or is present but not controlling (undermining/acquiring)" },
            "sell_power_mode": { "type": "string", "enum": ["controls", "present", "undermining"], "description": "same for the sell side" },
            "max_leg_ly": { "type": "number", "description": "longest single leg considered, default 150; 0 = unlimited (slow galaxy-wide)" },
            "ship_id": { "type": "integer", "description": "plan for one of the commander's stored ships (an id from list_ships) instead of the one being flown; cargo_capacity and min_pad still override" }
        }
    })
}

/// The model's default result count; the Trade tab asks for more.
const FIND_PROFIT_TOOL_LIMIT: usize = 10;

/// The same finder the Trade tab uses, on the community API.
fn find_profit(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let mut req: ed_route::request::ProfitRequest = parse(input)?;
    // The tool never takes the panel-only fields.
    req.from_station_id = None;
    req.jump_range_ly = None;
    req.limit = req.limit.or(Some(FIND_PROFIT_TOOL_LIMIT));
    Ok(to_json(tauri::async_runtime::block_on(crate::remote_trade::report(ctx.state, &req))?))
}

// ── Routes ────────────────────────────────────────────────────────────

fn current_route(ctx: &Ctx, _: &Value) -> CapResult<Value> {
    ctx.state.with_read(|s| {
        let conn = s.conn();
        let route = ed_store::route::current(conn)?;
        let here = galaxy::current_system(conn);
        Ok(match route {
            None => json!({ "plotted": false, "note": "no route is plotted in the galaxy map" }),
            Some(r) => json!({
                "plotted": true,
                "brief": ed_store::route::brief_text(&r, ed_store::route::Narration::Full),
                "next": here.as_deref().and_then(|h| ed_store::route::next_hop_text(&r, h, ed_store::route::Narration::Full)),
                "route": r,
                "provenance": "route from the journal; stations and Powerplay control from community data",
            }),
        })
    })
}

fn plot_route_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "from": { "type": "string", "description": "origin system; default the current system" },
            "to": { "type": "string" },
            "range_ly": { "type": "number" },
            "supercharge": { "type": "boolean", "description": "default true" },
            "max_dry_jumps": { "type": "integer", "description": "0 = no limit" }
        },
        "required": ["to"]
    })
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct PlotRequest {
    from: Option<String>,
    to: String,
    range_ly: Option<f32>,
    supercharge: bool,
    max_dry_jumps: u32,
    thorough: bool,
}

impl Default for PlotRequest {
    fn default() -> Self {
        PlotRequest { from: None, to: String::new(), range_ly: None, supercharge: true, max_dry_jumps: 0, thorough: false }
    }
}

fn plot_route(ctx: &Ctx, input: &Value) -> CapResult<Value> {
    let req: PlotRequest = parse(input)?;
    let state = ctx.state;
    // The route-coverage gate the Route tab and trade following apply
    // (maintainer, 2026-09-09: "my threshold is 500 ly but 'route to my
    // carrier' used EDDA routing"): a journey from HERE within
    // game_route_max_ly, with the Galaxy Map recipe taught, goes to the
    // game's own plotter. Anything longer, any explicit origin, or an
    // untaught recipe falls through to EDDA's planner.
    if req.from.as_deref().map(str::trim).is_none_or(str::is_empty) {
        let max = state.config.lock().unwrap_or_else(|e| e.into_inner()).game_route_max_ly;
        if max > 0 {
            let galaxy = state.routing.galaxy(&state.data_dir);
            let straight = state.with_read(|s| {
                let conn = s.conn();
                let here = galaxy::current_system(conn)?;
                let a = galaxy::coords_hint(conn, galaxy.as_deref(), &here)?;
                let b = galaxy::coords_hint(conn, galaxy.as_deref(), &req.to)?;
                Some(((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2) + (a.2 - b.2).powi(2)).sqrt())
            });
            if let Some(d) = straight.filter(|d| *d <= f64::from(max)) {
                match ctx.fx.plot_in_game(state, &req.to) {
                    Ok(message) => {
                        tracing::info!(to = %req.to, straight_ly = d, max, "plot_route: within the game-route threshold, handed to the game's plotter");
                        return Ok(json!({
                            "to": req.to, "straight_ly": (d * 10.0).round() / 10.0, "game_route_max_ly": max,
                            "handed_to_game": true, "message": message,
                            "note": "within the commander's route-coverage threshold, so Elite plots this one itself: the system is on the clipboard and the next Target Next press asks the game to plot it. No EDDA hops to list.",
                        }));
                    }
                    Err(error) => tracing::info!(%error, "plot_route: game plotter unavailable, EDDA plans"),
                }
            }
        }
    }
    let g = state
        .routing
        .galaxy(&state.data_dir)
        .ok_or_else(|| CapError::unavailable("no galaxy index built yet", false).hint("the commander builds it under Settings → Galaxy index"))?;
    let (here, ship_range, ship, time_fit) = state.with_read(|s| {
        let conn = s.conn();
        let ship = crate::routing::ship_fuel(conn);
        let time_fit = ship.as_ref().and_then(|(_, _, _, label)| {
            let name = label.split(" \u{b7}").next().unwrap_or("").trim();
            crate::time_fit::fit_for_ship(conn, name)
        });
        (galaxy::current_system(conn), crate::routing::current_range(conn), ship, time_fit)
    });
    let from_name = req
        .from
        .filter(|s| !s.trim().is_empty())
        .or(here)
        .ok_or_else(|| CapError::invalid("no origin known").hint("pass from"))?;
    let a = g.find(&from_name).ok_or_else(|| CapError::not_found(format!("unknown system {from_name:?}")))?;
    let b = g.find(&req.to).ok_or_else(|| CapError::not_found(format!("unknown system {:?}", req.to)))?;
    // Same physics as the Route tab: the ship's fuel model and
    // supercharge profile, full-tank range as the planning range.
    let (fuel, boost, start_fuel, ship_label) = match &ship {
        Some((m, bst, now, label)) => (Some(*m), *bst, *now, label.clone()),
        None => (None, ed_galaxy::fuel::BoostProfile::default(), 0.0, "unknown ship".to_string()),
    };
    // The commander's safe-margins choice applies here too.
    let fuel = fuel.map(|mut m| {
        crate::routing::apply_safe_margins(&mut m, state.routing.sticky_safe_margins());
        m
    });
    let plan = ed_galaxy::router::RouteRequest {
        from: a,
        to: b,
        range_ly: req.range_ly.or(fuel.map(|m| m.range_at(m.capacity))).or(ship_range.map(|r| r as f32)).unwrap_or(30.0).max(1.0),
        supercharge: req.supercharge,
        max_dry_jumps: req.max_dry_jumps,
        weight: 1.3,
        max_expansions: 5_000_000,
        thorough: req.thorough,
        boost,
        fuel,
        start_fuel,
        injection: None,
        time_budget_ms: crate::routing::effort_budget_ms(Some("medium")),
        // Once a variant has a route the rest get a second to beat it.
        grace_ms: 1_000,
        // Item 39's collision fix: this path plans with the same physics
        // the panel does — lean by default (minimize time), the
        // commander's safe-margins choice, and their fitted clocks.
        min_fuel: true,
        stop_weight: 1.0,
        prize_k: None,
        t_jump_s: time_fit.and_then(|f| f.t_jump_s),
        stop_overhead_s: time_fit.and_then(|f| f.stop_overhead_s),
    };
    let ctl = ed_galaxy::router::Control::none();
    let straight = ed_galaxy::format::dist(g.record(a).pos(), g.record(b).pos());
    let no_route = |e: ed_galaxy::router::RouteError| CapError::not_found(e.to_string()).hint("try a larger range_ly, allow supercharge, or raise max_dry_jumps");
    // Same dispatch as the Route tab (`plan_best`: neutron-first over the
    // long-route threshold, exact plus neutron-first in the corridor under
    // it), except that a thorough long plot stays on the exact search here.
    let route = if plan.thorough && straight > ed_galaxy::long_range::LONG_ROUTE_LY {
        ed_galaxy::router::plan(&g, &plan, &ctl).map_err(no_route)?
    } else {
        let neutrons = if plan.supercharge { Some(state.routing.neutrons(&g, || {}, &|| false)?) } else { None };
        ed_galaxy::long_range::plan_best(&g, neutrons.as_deref(), &plan, &ctl).map_err(no_route)?
    };
    // A route the commander asked for is the route they mean by
    // "target it" -- follow it from here.
    let followed = ctx.fx.follow(state, &crate::follow::ActiveRoute { route: route.clone(), next: 1.min(route.hops.len()), source: "ai".into() });
    Ok(json!({
        "from": from_name, "to": req.to, "range_ly": plan.range_ly, "ship": ship_label,
        "following": followed,
        "follow_note": if followed { "this is now the followed route: follow_route target_next / the HUD countdown refer to it" } else { "could not activate following" },
        "jumps": route.jumps, "total_ly": route.total_ly, "straight_ly": route.straight_ly,
        "boosted_jumps": route.boosted_jumps, "refuel_stops": route.refuel_stops,
        "index": if g.dir.ends_with("galaxy") { "full galaxy" } else { "populated systems only" },
        "hops": route.hops.iter().map(|h| json!({
            "name": h.name, "class": h.class, "scoopable": h.scoopable,
            "jump_ly": h.distance_ly, "boosted": h.boosted, "total_ly": h.total_ly,
            "fuel_after_t": h.fuel_after, "scoop_here": h.refuel,
        })).collect::<Vec<_>>(),
        "note": if fuel.is_some() { "fuel modelled from the ship's Loadout: range at full tank, fuel per jump, scoop stops where needed" } else { "no Loadout yet, so no fuel model; check dry runs against your tank" },
        // Full route for the Route tab; the model only needs the hops above.
        "_route": route,
    }))
}
