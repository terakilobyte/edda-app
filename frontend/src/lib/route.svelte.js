// Shared state for the route plotter, so a plotted route -- and a plot in
// progress -- survive tab switches, can be set from the ship computer's
// plot_route tool, or requested from other tabs ("route me to this trader").
//
// Nothing here talks to the backend until `start()`; `stop()` undoes it.
import { plotRoute, cancelRoute, onRouteProgress, onRouteCandidate, onRouteReplanned, personas, importSpanshRoute } from "./api.js";
import { linePool } from "./loadingLines.js";

export const routing = $state({
  /** @type {import("./types.js").Route|null} */
  route: null,
  origin: "tab",      // "tab" | "ai": where the shown route came from (the follow source label)
  from: "",
  to: "",
  request: null,      // {to, from?} -> RoutePanel plots it and clears it
  openTab: false,     // App switches to the Route tab and clears it
  // In-flight plot. Lives here so leaving the tab does not lose it.
  loading: false,
  error: "",
  /** @type {import("./types.js").RouteProgress|null} */
  progress: null,
  // The last plot query (for "Try harder"), whether it ran, and its verdict.
  lastQuery: null,
  triedHarder: false,
  notice: "",
  startedAt: null,
  elapsedMs: 0,
  budgetMs: 120000,   // the plotter's time budget for this plot
  best: null,         // best complete candidate so far (a long plot runs several variants)
  candidates: [],     // other candidates shown on the map, newest last
  persona: "standard",
});

let offs = null;
let ticker = null;

/** Subscribe to plot progress and start the elapsed-time ticker. Idempotent. */
export function start() {
  if (offs) return;
  offs = [
    onRouteProgress((e) => { routing.progress = e.payload; }),
    // A variant finished: the best so far is the blue line, the rest are
    // shown in yellow (at most six on the map).
    onRouteCandidate((e) => {
      const r = e.payload;
      if (!routing.loading || !r?.hops?.length) return;
      if (!routing.best || r.jumps < routing.best.jumps) {
        if (routing.best) routing.candidates = [...routing.candidates, routing.best];
        routing.best = r;
      } else {
        const key = (c) => `${c.jumps}:${c.total_ly}`;
        routing.candidates = [...routing.candidates.filter((c) => key(c) !== key(r)), r];
      }
      if (routing.candidates.length > 6) routing.candidates = routing.candidates.slice(-6);
    }),
    // The follower re-planned after a detour: show the new route.
    onRouteReplanned((e) => setRoute(e.payload, "tab")),
  ];
  personas().then((v) => { if (v?.selected) routing.persona = v.selected; }).catch(() => {});
  ticker = setInterval(() => { if (routing.loading && routing.startedAt) routing.elapsedMs = Date.now() - routing.startedAt; }, 500);
}

export function stop() {
  offs?.forEach((off) => off());
  offs = null;
  clearInterval(ticker);
  ticker = null;
}

/** Strictly better under the planner's own tie-break: fewer jumps, then
 * fewer boosts, then fewer ly. Equal is not better -- a thorough plot that
 * only ties keeps the quick route. */
export function betterRoute(candidate, current) {
  if (!candidate) return false;
  if (!current) return true;
  const rank = (r) => [r.jumps, r.boosted_jumps, r.total_ly];
  const [a, b] = [rank(candidate), rank(current)];
  for (let i = 0; i < a.length; i++) {
    if (a[i] < b[i]) return true;
    if (a[i] > b[i]) return false;
  }
  return false;
}

export function setRoute(route, origin = "tab", from = "", to = "") {
  routing.route = route;
  routing.origin = origin;
  if (from) routing.from = from;
  if (to) routing.to = to;
}

/** The ship computer plotted a route: show it in the Route tab. */
export function showFromAi(route) {
  setRoute(route, "ai");
}

/** Nothing to follow, nothing to show (route cleared by button, voice or ship computer). */
export function clearRoute() {
  routing.route = null;
  routing.to = "";
  routing.error = "";
}

/// Ask the Route tab to plot to `to` (from the current system unless given).
export function requestRoute(to, from = null) {
  routing.request = { to, from };
  routing.openTab = true;
}

/// Run a plot; the result lands in the store whichever tab is showing.
/** @param {import("./types.js").PlotQuery} query */
export async function runPlot(query) {
  if (routing.loading) return;
  // The persona may have been changed in Settings since the store loaded.
  try { const v = await personas(); if (v?.selected) routing.persona = v.selected; } catch {}
  routing.loading = true; routing.error = ""; routing.route = null; routing.progress = null;
  routing.notice = ""; routing.lastQuery = query; routing.triedHarder = false;
  routing.startedAt = Date.now(); routing.elapsedMs = 0;
  routing.best = null; routing.candidates = [];
  routing.budgetMs = query.thorough ? ({ low: 30000, medium: 60000 }[query.effort] ?? 120000) : 15000;
  routing.from = query.from ?? ""; routing.to = query.to ?? "";
  try {
    const route = await plotRoute(query);
    setRoute(route, "tab", query.from ?? "", query.to ?? "");
  } catch (e) {
    routing.error = String(e);
  } finally {
    routing.loading = false;
    routing.best = null; routing.candidates = [];
  }
}

/** The thorough portfolio from the same inputs as the last plot. The
 * route on screen is replaced only if the thorough one is strictly better
 * under the planner's tie-break; otherwise it says so and keeps it. */
export async function tryHarder() {
  if (routing.loading || !routing.lastQuery || !routing.route) return;
  const query = { ...routing.lastQuery, thorough: true };
  const current = routing.route;
  routing.loading = true; routing.error = ""; routing.progress = null; routing.notice = "";
  routing.startedAt = Date.now(); routing.elapsedMs = 0;
  routing.best = null; routing.candidates = [];
  routing.budgetMs = { low: 30000, medium: 60000 }[query.effort] ?? 120000;
  try {
    const route = await plotRoute(query);
    if (betterRoute(route, current)) {
      setRoute(route, "tab", query.from ?? "", query.to ?? "");
      routing.notice = `Thorough plot: ${current.jumps - route.jumps > 0 ? `${current.jumps - route.jumps} fewer jump${current.jumps - route.jumps === 1 ? "" : "s"}` : "same jumps, fewer boosts"}.`;
    } else {
      routing.notice = "No better route found.";
    }
  } catch (e) {
    routing.error = String(e);
  } finally {
    routing.loading = false; routing.triedHarder = true;
    routing.best = null; routing.candidates = [];
  }
}

export async function stopPlot() {
  try { await cancelRoute(); } catch {}
}

// A random line from the persona's pool, changing every ~5 s and never
// repeating the one just shown.
let lineSlot = -1;
let lineIdx = 0;
export function plotStatusLine() {
  const p = routing.progress;
  const phase = p?.phase === "import" || p?.phase === "neutrons" ? p.phase : "plot";
  const pool = linePool(routing.persona, phase);
  const slot = Math.floor(routing.elapsedMs / 5000);
  if (slot !== lineSlot) {
    lineSlot = slot;
    if (pool.length > 1) {
      let next = Math.floor(Math.random() * pool.length);
      if (next === lineIdx) next = (next + 1) % pool.length;
      lineIdx = next;
    } else {
      lineIdx = 0;
    }
  }
  return pool[lineIdx % pool.length];
}

// The search's own phase, from the planner's progress events: which
// stage it is in, how many nodes it has expanded, how far the best
// partial plan still has to go.
function phaseLine(p) {
  const n = p.expansions ? `${p.expansions.toLocaleString()} expansions` : "";
  const left = p.remaining_ly > 0 ? `${Math.round(p.remaining_ly).toLocaleString()} ly to go` : "";
  switch (p.phase) {
    case "neutrons": return "building the highway index";
    case "coarse": return ["highway search", n, left].filter(Boolean).join(" · ");
    case "refine": return p.legs ? `refining leg ${p.leg}/${p.legs}` : "refining legs";
    case "exact": return ["exact search", n, left].filter(Boolean).join(" · ");
    case "injection": return "trying FSD injections";
    case "import": return "importing";
    default: return [p.phase, n, left].filter(Boolean).join(" · ");
  }
}

// What is actually known: the search phase, the best complete route so
// far, and how long this can take at most. No guessed time left.
export function plotDetailLine() {
  const parts = [];
  const p = routing.progress;
  if (p?.phase) parts.push(phaseLine(p));
  if (routing.best) parts.push(`best so far: ${routing.best.jumps} jumps`);
  const s = Math.round(routing.elapsedMs / 1000);
  const b = routing.budgetMs;
  const budget = b >= 60000 ? `${Math.round(b / 60000)} min` : `${Math.round(b / 1000)} s`;
  parts.push(`${s} s, up to ${budget}`);
  return parts.join(" · ");
}

/// Bring a route plotted on spansh.co.uk into the store (results link or job id).
export async function importSpansh(link) {
  if (routing.loading) return;
  routing.loading = true; routing.error = ""; routing.progress = { phase: "import", expansions: 0, remaining_ly: 0 };
  routing.startedAt = Date.now(); routing.elapsedMs = 0;
  try {
    const route = await importSpanshRoute(link);
    setRoute(route, "tab", route.hops[0]?.name ?? "", route.hops[route.hops.length - 1]?.name ?? "");
  } catch (e) {
    routing.error = String(e);
  } finally {
    routing.loading = false;
  }
}
