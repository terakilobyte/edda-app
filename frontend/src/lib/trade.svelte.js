// Shared state for the profit finder.
//
// Panels are unmounted when you switch tabs, so anything a component holds
// itself is lost -- including a search that is still running. Keeping the
// query, the result and the in-flight promise here means you can wander
// off to Combat and come back to find the search finished (or still going).
import { profitRoutes, cancelSearch } from "./api.js";
import { profitQueryFrom } from "./queries.js";
import { pickView } from "./tradeView.js";

export const trade = $state({
  query: {
    system: "",
    fromCurrent: false,
    fromStationId: null,
    radius: 100,
    maxAge: 2,
    carriers: false,
    prohibited: false,
    // Which ship the search plans for: null = the one being flown, else a
    // ships_list id; cargo / jumpRange / minPad override whichever ship.
    shipId: null,
    cargo: "",
    jumpRange: "",
    minPad: "auto",
    maxStations: 0,
    maxArrivalLs: 0,
    minSupply: 1,
    minDemand: 1,
    // Rings are opt-in (maintainer, 2026-09-05): the "rings up to" select
    // turns them on; off roughly halves a bubble-wide search.
    maxStops: 0,
    buyPower: "any",
    buyState: "any",
    sellPower: "any",
    sellState: "any",
    buyPowerMode: "controls",
    sellPowerMode: "controls",
    maxLegLy: 150,
  },
  report: null,
  error: "",
  loading: false,
  startedAt: null,
  finishedAt: null,
  elapsedMs: 0,
  view: "legs",
  sortKey: "rate",
  sortDir: -1,
});

/** Lifecycle hooks kept for App.svelte's symmetry; the search runs on the server and reports no phases. */
export function start() {}

export function stop() {
  off = null;
}

export async function stopSearch() {
  try {
    await cancelSearch();
  } catch {}
}

// Column sorters for the legs table. Distance and age sort ascending by
// default; everything else descending.
export const LEG_SORTS = {
  // Repeat rate only on single legs (maintainer, 2026-09-09); the one-way
  // rate is a ring/chain figure and is not offered as a leg sort.
  rate: { label: "cr/h repeating", get: (l) => l.profit_per_hour_repeat, dir: -1 },
  profit: { label: "Profit", get: (l) => l.profit, dir: -1 },
  perton: { label: "/ton", get: (l) => l.profit_per_ton, dir: -1 },
  tons: { label: "Tons", get: (l) => l.tons, dir: -1 },
  distance: { label: "Dist", get: (l) => l.distance_ly, dir: 1 },
  // Each leg half owns its own supercruise: Out = jumps + the SELL
  // station's arrival, Back = jumps + the BUY station's arrival (maintainer,
  // 2026-09-05 — pairing both arrivals under one "Arrival" column made
  // the times look dishonest).
  time: { label: "Out*", get: (l) => l.duration?.seconds ?? 0, dir: 1 },
  arrival: { label: "Back*", get: (l) => l.return_duration?.seconds ?? 0, dir: 1 },
  age: { label: "Price age", get: (l) => Math.max(l.buy_age_hours, l.sell_age_hours), dir: 1 },
  commodity: { label: "Commodity", get: (l) => l.commodity, dir: 1 },
  from: { label: "Buy at", get: (l) => l.from.system + l.from.station, dir: 1 },
  to: { label: "Sell at", get: (l) => l.to.system + l.to.station, dir: 1 },
};

export function sortBy(key) {
  if (trade.sortKey === key) trade.sortDir = -trade.sortDir;
  else { trade.sortKey = key; trade.sortDir = LEG_SORTS[key].dir; }
}

export function sortedLegs(legs) {
  const s = LEG_SORTS[trade.sortKey] ?? LEG_SORTS.rate;
  const dir = trade.sortDir;
  return [...legs].sort((a, b) => {
    const x = s.get(a), y = s.get(b);
    const c = typeof x === "string" ? x.localeCompare(y) : x - y;
    return c * dir;
  });
}

let ticker = null;

export async function runSearch() {
  if (trade.loading) return;
  trade.loading = true;
  trade.error = "";
  trade.startedAt = Date.now();
  trade.finishedAt = null;
  trade.elapsedMs = 0;
  ticker = setInterval(() => (trade.elapsedMs = Date.now() - trade.startedAt), 250);
  try {
    trade.report = await profitRoutes(profitQueryFrom(trade.query));
    // Loops first when they out-earn legs (maintainer, 2026-09-07).
    trade.view = pickView(trade.report);
  } catch (e) {
    trade.error = String(e);
    trade.report = null;
  } finally {
    clearInterval(ticker);
    ticker = null;
    trade.elapsedMs = Date.now() - trade.startedAt;
    trade.finishedAt = Date.now();
    trade.loading = false;
  }
}
