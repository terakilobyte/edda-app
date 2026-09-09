// Request and response shapes the frontend actually sends and receives,
// copied by hand from the Rust structs named on each. Keep the key lists
// next to the typedefs: `queries.js` builds requests from them and the
// tests check that every backend field is present.
//
// A generated binding (ts-rs/specta) would replace this file; until then,
// when a Rust struct changes, change it here too.

// ── crates/ed-route/src/request.rs: ProfitRequest (was commands.rs ProfitQuery) ──
/**
 * @typedef {object} ProfitQuery
 * @property {string|null} system
 * @property {boolean|null} from_current_station  Restrict purchases to the docked station.
 * @property {number|null} from_station_id
 * @property {number|null} radius_ly
 * @property {number|null} max_age_hours
 * @property {boolean|null} include_carriers
 * @property {number|null} cargo_capacity
 * @property {number|null} jump_range_ly
 * @property {string|null} min_pad  Override the pad filter; default derives it from the hull.
 * @property {number|null} limit
 * @property {number|null} max_stations
 * @property {number|null} max_arrival_ls
 * @property {number|null} max_stops
 * @property {string|null} buy_power   Powerplay filters; "mine" resolves to the pledged power.
 * @property {string|null} buy_state
 * @property {string|null} sell_power
 * @property {string|null} sell_state
 * @property {string|null} buy_power_mode
 * @property {string|null} sell_power_mode
 * @property {number|null} max_leg_ly
 * @property {number|null} min_supply
 * @property {number|null} min_demand
 */
export const PROFIT_QUERY_KEYS = Object.freeze([
  "system", "from_current_station", "from_station_id", "radius_ly", "max_age_hours", "include_carriers", "include_prohibited",
  "ship_id", "cargo_capacity", "jump_range_ly", "min_pad", "limit", "max_stations", "max_arrival_ls", "max_stops",
  "buy_power", "buy_state", "sell_power", "sell_state", "buy_power_mode", "sell_power_mode", "max_leg_ly",
  "min_supply", "min_demand",
]);

// ── src-tauri/src/routing.rs: PlotQuery ─────────────────────────────
/**
 * @typedef {object} PlotQuery
 * @property {string|null} from
 * @property {string} to
 * @property {number|null} range_ly
 * @property {boolean|null} supercharge
 * @property {number|null} max_dry_jumps
 * @property {number|null} weight
 * @property {boolean|null} thorough
 * @property {boolean|null} fuel        Plan with fuel physics (default true).
 * @property {number|null} reserve_t    Tonnes to keep in the tank at all times.
 * @property {number|null} ship_id      Plan for this ship (journal ShipID).
 * @property {"high"|"medium"|"low"|null} effort
 * @property {boolean|null} injections  Use synthesisable FSD injections when nothing else crosses a gap.
 */
export const PLOT_QUERY_KEYS = Object.freeze([
  "from", "to", "range_ly", "supercharge", "max_dry_jumps", "weight", "thorough", "fuel", "reserve_t", "ship_id", "effort", "injections", "white_dwarfs", "min_fuel", "safe_margins",
]);

// ── crates/ed-galaxy/src/router.rs: Hop, Route ──────────────────────
/**
 * @typedef {object} Hop
 * @property {number} idx
 * @property {number} id64
 * @property {string} name
 * @property {[number, number, number]} pos
 * @property {string} class            StarClass: "neutron" | "white_dwarf" | "black_hole" | "unknown" | …
 * @property {boolean} scoopable
 * @property {number} distance_ly      Distance jumped to get here.
 * @property {boolean} boosted         This jump used a supercharge from the previous star.
 * @property {number} total_ly         Cumulative from the start.
 * @property {number|null} fuel_after  Tonnes in the tank on arrival; null without a fuel model.
 * @property {boolean} refuel          The plan expects you to scoop here.
 * @property {string|null} injection   Synthesise this FSD injection before the jump to this hop.
 */
/**
 * @typedef {object} Route
 * @property {number} range_ly         The unboosted range the plan was made with.
 * @property {Hop[]} hops
 * @property {number} jumps
 * @property {number} total_ly
 * @property {number} straight_ly
 * @property {number} boosted_jumps
 * @property {number} expansions
 * @property {number} elapsed_ms
 * @property {number} refuel_stops
 * @property {number} injections
 * @property {number|null} ship_id
 * @property {string|null} ship
 */

// ── src-tauri/src/follow.rs: ActiveRoute, FollowView ────────────────
/**
 * @typedef {object} ActiveRoute
 * @property {Route} route
 * @property {number} next     Index into `route.hops` of the next system to jump to.
 * @property {string} source
 */
/**
 * @typedef {object} FollowView
 * @property {boolean} active
 * @property {string|null} source
 * @property {number} total_jumps
 * @property {number} jumps_left
 * @property {Hop|null} next
 * @property {Hop[]} ahead             The next few hops from the cursor (for the HUD).
 * @property {string|null} destination
 * @property {number} next_index
 */
/** The store's shape: FollowView plus the frontend-only fields. */
export const FOLLOW_IDLE = Object.freeze({ active: false, jumps_left: 0, total_jumps: 0, next: null, ahead: [], destination: null, source: null, next_index: 0 });

// ── src-tauri/src/routing.rs: route-progress payload ────────────────
/**
 * @typedef {object} RouteProgress
 * @property {number} expansions
 * @property {number} remaining_ly
 * @property {"import"|"neutrons"|"plot"|string} phase
 * @property {number} [leg]
 * @property {number} [legs]
 */

// ── src-tauri/src/routing.rs: galaxy-import-progress payload ────────
/**
 * @typedef {object} GalaxyImportProgress
 * @property {"full"|"populated"} source
 * @property {number} systems
 * @property {number} gb_read
 * @property {number} secs
 * @property {number} [skipped]
 */

// ── src-tauri/src/lib.rs: maintenance payload ───────────────────────
/** @typedef {string} MaintenanceNote  e.g. "market index built" */
