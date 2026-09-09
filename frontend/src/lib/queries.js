// Form state → backend request. Pure, so the field mapping is testable
// against the key lists in types.js and every Rust field is accounted for.

/**
 * @param {object} q the Trade tab's form (trade.query)
 * @returns {import("./types.js").ProfitQuery}
 */
export function profitQueryFrom(q) {
  return {
    system: (q.system ?? "").trim() || null,
    from_current_station: q.fromCurrent ?? false,
    from_station_id: q.fromStationId ?? null,
    radius_ly: Number(q.radius ?? 100),
    max_age_hours: Number(q.maxAge ?? 2),
    include_carriers: q.carriers ?? false,
    include_prohibited: q.prohibited ?? false,
    // A stored ship (ships_list id) to plan for; null = the one being flown.
    ship_id: typeof q.shipId === "number" ? q.shipId : null,
    cargo_capacity: q.shipId === "custom" && q.cargo ? Number(q.cargo) : null,
    jump_range_ly: q.shipId === "custom" && q.jumpRange ? Number(q.jumpRange) : null,
    // "auto" derives the pad from the hull; "any" disables the filter.
    // "auto" derives the pad from the hull; "any" disables the filter. A
    // freeform "Other ship" carries its own pad; any stored/flown ship
    // uses its hull's.
    min_pad: (q.minPad ?? "auto") === "auto" || q.shipId !== "custom" ? null : q.minPad,
    limit: 100,
    max_stations: Number(q.maxStations ?? 0),
    max_arrival_ls: Number(q.maxArrivalLs) || null,
    max_stops: Number(q.maxStops ?? 0),
    buy_power: q.buyPower ?? "any",
    buy_state: q.buyState ?? "any",
    sell_power: q.sellPower ?? "any",
    sell_state: q.sellState ?? "any",
    buy_power_mode: q.buyPowerMode ?? "controls",
    sell_power_mode: q.sellPowerMode ?? "controls",
    max_leg_ly: Number(q.maxLegLy ?? 150),
    min_supply: Number(q.minSupply ?? 1) || 1,
    min_demand: Number(q.minDemand ?? 1) || 1,
  };
}

/**
 * @param {{from: string, to: string, supercharge: boolean, reserve: number|string, shipId: number|null, effort: string, injections: boolean, weight?: number|null, fuel?: boolean|null}} f
 * @returns {import("./types.js").PlotQuery}
 */
export function plotQueryFrom(f) {
  return {
    from: (f.from ?? "").trim() || null,
    to: (f.to ?? "").trim(),
    range_ly: null,
    supercharge: f.supercharge ?? true,
    max_dry_jumps: 0,
    weight: f.weight ?? null,
    // The quick plot by default; "Try harder" sends the same query thorough.
    thorough: f.thorough ?? false,
    fuel: f.fuel ?? null,
    reserve_t: Number(f.reserve) || 0,
    ship_id: f.shipId ?? null,
    white_dwarfs: f.whiteDwarfs ?? false,
    // The planner always minimizes time; lean stops are part of that.
    min_fuel: true,
    safe_margins: f.safeMargins ?? false,
    // Jumps-vs-refuels dial; 1 = the journal-fit time model as measured.
    // Absolute fewest jumps, whatever the search costs.
    effort: f.effort ?? "high",
    injections: f.injections ?? false,
  };
}
