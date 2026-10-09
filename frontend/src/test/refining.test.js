// The server answers a crossing early and refines behind it (#198); the
// backend polls the key and relays `route-refining` events. 2026-10-09:
// the boss saw the 478-jump early answer and never the 373-jump
// refinement, then "Try harder" said no better route (a different search).
// These pin the store's handling: watch, better-so-far on the map, the
// finish replaces the route unless it is being followed, stale ignored.
import { beforeEach, describe, expect, it } from "vitest";
import { applyRefining, routing, useRefined } from "../lib/route.svelte.js";

const route = (jumps, extra = {}) => ({ jumps, boosted_jumps: Math.floor(jumps / 2), total_ly: jumps * 100, refuel_stops: 3, injections: 0, hops: [], ...extra });

beforeEach(() => {
  routing.route = null; routing.refining = null; routing.candidates = []; routing.notice = ""; routing.loading = false;
});

describe("refinement events", () => {
  it("the watcher's first word opens the watch for the plot in flight", () => {
    routing.loading = true;
    expect(applyRefining({ early_jumps: 478, done: false, route: null })).toBe("watching");
    expect(routing.refining).toEqual({ earlyJumps: 478, best: null, done: false });
  });

  it("ignores a watcher that belongs to no plot", () => {
    expect(applyRefining({ early_jumps: 478, done: false, route: null })).toBe("stale");
    expect(routing.refining).toBeNull();
    routing.route = route(478); routing.refining = { earlyJumps: 478, best: null, done: false };
    expect(applyRefining({ early_jumps: 300, done: true, route: route(10) })).toBe("stale");
    expect(routing.route.jumps).toBe(478);
  });

  it("a better route so far becomes the candidate in the boxes and on the map", () => {
    routing.route = route(478); routing.refining = { earlyJumps: 478, best: null, done: false };
    expect(applyRefining({ early_jumps: 478, done: false, route: route(412) })).toBe("better-so-far");
    expect(routing.refining.best.jumps).toBe(412);
    expect(routing.candidates.map((c) => c.jumps)).toEqual([412]);
    expect(routing.route.jumps).toBe(478, "the route on screen does not move by itself");
  });

  it("the finished route replaces the early one and says so", () => {
    routing.route = route(478); routing.refining = { earlyJumps: 478, best: route(412), done: false }; routing.candidates = [route(412)];
    expect(applyRefining({ early_jumps: 478, done: true, route: route(373) })).toBe("final-applied");
    expect(routing.route.jumps).toBe(373);
    expect(routing.refining).toBeNull();
    expect(routing.candidates).toEqual([]);
    expect(routing.notice).toBe("Refined: 478 → 373 jumps.");
  });

  it("a followed route is not swapped underneath the commander", () => {
    routing.route = route(478); routing.refining = { earlyJumps: 478, best: null, done: false };
    expect(applyRefining({ early_jumps: 478, done: true, route: route(373) }, true)).toBe("final-waiting");
    expect(routing.route.jumps).toBe(478);
    expect(routing.refining).toEqual({ earlyJumps: 478, best: route(373), done: true });
    expect(routing.notice).toContain("Use it to switch");
    expect(useRefined()).toBe(true);
    expect(routing.route.jumps).toBe(373);
    expect(routing.refining).toBeNull();
    expect(routing.notice).toBe("Using the refined route: 478 → 373 jumps.");
  });

  it("a finish that is no better keeps the first answer", () => {
    routing.route = route(376); routing.refining = { earlyJumps: 376, best: null, done: false };
    expect(applyRefining({ early_jumps: 376, done: true, route: route(376) })).toBe("final-same");
    expect(routing.route.jumps).toBe(376);
    expect(routing.notice).toBe("Refined: the first answer stands.");
    routing.refining = { earlyJumps: 376, best: null, done: false };
    expect(applyRefining({ early_jumps: 376, done: true, route: null })).toBe("final-same");
    expect(routing.notice).toBe("Refinement did not finish in time; the first answer stands.");
  });

  it("using a candidate mid-refinement keeps watching for the finish", () => {
    routing.route = route(478); routing.refining = { earlyJumps: 478, best: route(412), done: false };
    expect(useRefined()).toBe(true);
    expect(routing.route.jumps).toBe(412);
    expect(routing.refining).toEqual({ earlyJumps: 478, best: null, done: false });
    expect(applyRefining({ early_jumps: 478, done: true, route: route(373) })).toBe("final-applied");
    expect(routing.route.jumps).toBe(373);
    expect(useRefined()).toBe(false);
  });
});
