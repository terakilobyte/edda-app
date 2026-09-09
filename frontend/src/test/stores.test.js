// Stores must be inert at import time: no IPC until `start()`, and
// `stop()` must undo everything, including the ticking interval.
import { describe, it, expect, beforeEach, vi } from "vitest";
import { tauri } from "./setup.js";
import { fakeTransport } from "./fakeTransport.js";
import { setTransport } from "../lib/transport.js";

describe("stores are inert until start()", () => {
  beforeEach(() => { tauri.invoke.mockClear(); tauri.listen.mockClear(); });

  it("importing the route/trade stores makes no transport calls", async () => {
    await import("../lib/route.svelte.js");
    await import("../lib/trade.svelte.js");
    await new Promise((r) => setTimeout(r, 0));
    expect(tauri.invoke.mock.calls.map((c) => c[0])).toEqual([]);
    expect(tauri.listen.mock.calls.map((c) => c[0])).toEqual([]);
  });

  it("importing the follow store makes no transport calls", async () => {
    await import("../lib/follow.svelte.js");
    await new Promise((r) => setTimeout(r, 0));
    expect(tauri.invoke.mock.calls.map((c) => c[0])).toEqual([]);
    expect(tauri.listen.mock.calls.map((c) => c[0])).toEqual([]);
  });
});

describe("route store lifecycle", () => {
  it("start() is idempotent and stop() clears the elapsed ticker", async () => {
    vi.useFakeTimers();
    const t = fakeTransport({ personas: { selected: "butler" }, route_follow_status: { active: false } });
    setTransport(t);
    const route = await import("../lib/route.svelte.js");
    route.start(); route.start();
    await vi.runOnlyPendingTimersAsync();
    expect(t.count("route-progress")).toBe(1);
    expect(t.count("route-candidate")).toBe(1);
    expect(t.calls.filter((c) => c.name === "personas")).toHaveLength(1);
    expect(route.routing.persona).toBe("butler");

    route.routing.loading = true; route.routing.startedAt = Date.now();
    await vi.advanceTimersByTimeAsync(1100);
    expect(route.routing.elapsedMs).toBeGreaterThan(0);
    const at = route.routing.elapsedMs;
    route.stop();
    await vi.advanceTimersByTimeAsync(2000);
    expect(route.routing.elapsedMs).toBe(at);
    expect(t.count("route-progress")).toBe(0);
    route.routing.loading = false;
    vi.useRealTimers();
  });

  it("showFromAi()/clearRoute() replace the source string machine", async () => {
    const route = await import("../lib/route.svelte.js");
    const r = { hops: [{ name: "Sol" }, { name: "Diso" }], jumps: 1 };
    route.showFromAi(r);
    expect(route.routing.route).toBe(r);
    expect(route.routing.origin).toBe("ai");
    route.clearRoute();
    expect(route.routing.route).toBe(null);
    expect(route.routing.to).toBe("");
    expect(route.routing.error).toBe("");
  });
});

describe("follow store", () => {
  it("deactivating a followed route KEEPS the plotted one (maintainer 2026-09-05: Stop must not nuke the tab's plan)", async () => {
    const t = fakeTransport({ route_follow_status: { active: true, jumps_left: 3, total_jumps: 5, next: null, ahead: [], destination: "Diso", source: "plot", next_index: 2 } });
    setTransport(t);
    const route = await import("../lib/route.svelte.js");
    const follow = await import("../lib/follow.svelte.js");
    follow.start();
    await new Promise((r) => setTimeout(r, 0));
    expect(follow.follow.active).toBe(true);
    route.setRoute({ hops: [] }, "tab");
    t.emit("route-follow", { active: false, jumps_left: 0, total_jumps: 0, next: null, ahead: [], destination: null, source: null, next_index: 0 });
    expect(follow.follow.active).toBe(false);
    expect(route.routing.route).not.toBe(null);
    follow.stop();
    await new Promise((r) => setTimeout(r, 0));
    expect(t.count("route-follow")).toBe(0);
  });
});

describe("trade store", () => {
  it("sortedLegs honours LEG_SORTS direction and sortBy toggles it", async () => {
    const { trade, LEG_SORTS, sortBy, sortedLegs } = await import("../lib/trade.svelte.js");
    const legs = [
      { profit_per_hour_repeat: 10, distance_ly: 30, commodity: "Gold", from: { system: "A", station: "x" }, to: { system: "B", station: "y", arrival_ls: 100 }, duration: { seconds: 5 }, buy_age_hours: 1, sell_age_hours: 2 },
      { profit_per_hour_repeat: 30, distance_ly: 10, commodity: "Beer", from: { system: "C", station: "x" }, to: { system: "D", station: "y", arrival_ls: null }, duration: { seconds: 9 }, buy_age_hours: 3, sell_age_hours: 1 },
    ];
    trade.sortKey = "rate"; trade.sortDir = LEG_SORTS.rate.dir;
    expect(sortedLegs(legs).map((l) => l.commodity)).toEqual(["Beer", "Gold"]);
    sortBy("distance");
    expect(trade.sortDir).toBe(1);
    expect(sortedLegs(legs).map((l) => l.commodity)).toEqual(["Beer", "Gold"]);
    sortBy("distance");
    expect(sortedLegs(legs).map((l) => l.commodity)).toEqual(["Gold", "Beer"]);
    sortBy("commodity");
    expect(sortedLegs(legs).map((l) => l.commodity)).toEqual(["Beer", "Gold"]);
    sortBy("arrival");
    expect(sortedLegs(legs).map((l) => l.commodity)).toEqual(["Gold", "Beer"]);
  });
});
