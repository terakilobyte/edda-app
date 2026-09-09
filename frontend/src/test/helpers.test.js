// The small shared helpers: format, ui, storage, lifecycle, queries, merge.
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { fakeTransport } from "./fakeTransport.js";
import { setTransport } from "../lib/transport.js";

describe("format.js", async () => {
  const f = await import("../lib/format.js");
  it("renders dashes for missing values", () => {
    expect(f.fmtInt(null)).toBe("—");
    expect(f.fmtCr(undefined)).toBe("—");
    expect(f.fmtLy(null)).toBe("—");
    expect(f.fmtLs(null)).toBe("—");
    expect(f.fmtMin(null)).toBe("—");
    expect(f.fmtCrShort(null)).toBe("—");
    expect(f.fmtTs(null)).toBe("");
    expect(f.fmtAge(null)).toBe("unknown");
    expect(f.fmtAge(Infinity)).toBe("unknown");
  });
  it("formats numbers", () => {
    expect(f.fmtInt(1234.6)).toBe((1235).toLocaleString());
    expect(f.fmtCr(1500)).toBe(`${(1500).toLocaleString()} cr`);
    expect(f.fmtCrShort(2_500_000_000)).toBe("2.50B cr");
    expect(f.fmtCrShort(1_500_000)).toBe("1.5M cr");
    expect(f.fmtCrShort(12_300)).toBe("12k cr");
    expect(f.fmtCrShort(-12_300)).toBe("-12k cr");
    expect(f.fmtCrShort(999)).toBe("999 cr");
    expect(f.fmtLy(12.34)).toBe("12.3 ly");
    expect(f.fmtLs(1234.4)).toBe(`${(1234).toLocaleString()} ls`);
    expect(f.fmtMin(150)).toBe("3 min");
    expect(f.fmtAge(0.5)).toBe("30 min");
    expect(f.fmtAge(30)).toBe("30 h");
    expect(f.fmtAge(72)).toBe("3 d");
    expect(f.fmtTs("2026-08-29T10:11:12Z")).toBe("2026-08-29 10:11:12");
  });
});

describe("ui.js", async () => {
  const { prioClass, fuelPct } = await import("../lib/ui.js");
  it("maps priorities to classes", () => {
    expect([0, 1, 2, 3, 5].map(prioClass)).toEqual(["", "accent", "warn", "bad", "bad"]);
  });
  it("fuelPct is null-safe in both windows: unknown is null, not 0", () => {
    expect(fuelPct(null)).toBe(null);
    expect(fuelPct({})).toBe(null);
    expect(fuelPct({ fuel_main: 4, fuel_capacity: 0 })).toBe(null);
    expect(fuelPct({ fuel_main: 4, fuel_capacity: 16 })).toBe(25);
    expect(fuelPct({ fuel_main: 40, fuel_capacity: 16 })).toBe(100);
    expect(fuelPct({ fuel_main: 0, fuel_capacity: 16 })).toBe(0);
  });
});

describe("storage.svelte.js", async () => {
  const { KEYS, persisted } = await import("../lib/storage.svelte.js");
  let store;
  beforeEach(() => {
    store = new Map();
    globalThis.localStorage = {
      getItem: (k) => (store.has(k) ? store.get(k) : null),
      setItem: (k, v) => store.set(k, String(v)),
      removeItem: (k) => store.delete(k),
    };
  });
  afterEach(() => { delete globalThis.localStorage; delete globalThis.window; });

  it("declares the keys the app uses", () => {
    expect(Object.values(KEYS)).toEqual(expect.arrayContaining(["edda.tab", "edda.pinnedLoop", "edda.plotEffort", "edda.speak"]));
  });

  it("reads the stored value, falls back to the default, and writes on set", () => {
    store.set(KEYS.plotEffort, "low");
    const effort = persisted(KEYS.plotEffort, "high");
    expect(effort.value).toBe("low");
    effort.value = "medium";
    expect(store.get(KEYS.plotEffort)).toBe("medium");
    const speak = persisted(KEYS.speak, false);
    expect(speak.value).toBe(false);
    speak.value = true;
    expect(store.get(KEYS.speak)).toBe("1");
    expect(persisted(KEYS.speak, false).value).toBe(true);
  });

  it("survives a throwing localStorage", () => {
    globalThis.localStorage = { getItem() { throw new Error("denied"); }, setItem() { throw new Error("denied"); } };
    const box = persisted(KEYS.routeMap, true);
    expect(box.value).toBe(true);
    expect(() => { box.value = false; }).not.toThrow();
    expect(box.value).toBe(false);
  });

  it("json values round-trip and null clears", () => {
    const pin = persisted(KEYS.pinnedLoop, null, { json: true });
    pin.value = { a: 1 };
    expect(store.get(KEYS.pinnedLoop)).toBe('{"a":1}');
    expect(persisted(KEYS.pinnedLoop, null, { json: true }).value).toEqual({ a: 1 });
    pin.value = null;
    expect(persisted(KEYS.pinnedLoop, null, { json: true }).value).toBe(null);
  });

  it("sync: follows the other window's writes until disposed", () => {
    const handlers = new Set();
    globalThis.window = { addEventListener: (_, h) => handlers.add(h), removeEventListener: (_, h) => handlers.delete(h) };
    const pin = persisted(KEYS.pinnedLoop, null, { json: true, sync: true });
    store.set(KEYS.pinnedLoop, '{"b":2}');
    for (const h of handlers) h({ key: KEYS.pinnedLoop });
    expect(pin.value).toEqual({ b: 2 });
    pin.dispose();
    expect(handlers.size).toBe(0);
  });
});

describe("lifecycle.svelte.js", async () => {
  const { createJournalResource, listenerSet } = await import("../lib/lifecycle.svelte.js");
  const tick = () => new Promise((r) => setTimeout(r, 0));

  it("journalResource loads once, subscribes once, reloads on change, unsubscribes on stop", async () => {
    const t = fakeTransport({ get_status: () => ({ ok: true }) });
    setTransport(t);
    const { getStatus } = await import("../lib/api.js");
    let status = null, loads = 0;
    const res = createJournalResource(async () => { loads++; status = await getStatus(); });
    expect(loads).toBe(0);
    res.start(); res.start();
    await tick();
    expect(loads).toBe(1);
    expect(status).toEqual({ ok: true });
    expect(t.count("journal-changed")).toBe(1);
    t.emit("journal-changed");
    await tick();
    expect(loads).toBe(2);
    res.stop();
    await tick();
    expect(t.count("journal-changed")).toBe(0);
    t.emit("journal-changed");
    await tick();
    expect(loads).toBe(2);
  });

  it("journalResource exposes the normalised error and clears it on success", async () => {
    let fail = true;
    const t = fakeTransport({ get_status: () => (fail ? Promise.reject("store busy") : { ok: 1 }) });
    setTransport(t);
    const { getStatus } = await import("../lib/api.js");
    const res = createJournalResource(async () => { await getStatus(); });
    res.start();
    await tick();
    expect(res.error).toBe("store busy");
    fail = false;
    await res.refresh();
    expect(res.error).toBe("");
    res.stop();
  });

  it("listenerSet disposes every unsubscribe it was given, sync or promised", async () => {
    const calls = [];
    const set = listenerSet();
    set.add(() => calls.push("a"));
    set.add(Promise.resolve(() => calls.push("b")));
    set.add(undefined);
    set.dispose();
    await tick();
    expect(calls.sort()).toEqual(["a", "b"]);
  });
});

describe("queries.js", async () => {
  const { profitQueryFrom, plotQueryFrom } = await import("../lib/queries.js");
  const { PROFIT_QUERY_KEYS, PLOT_QUERY_KEYS } = await import("../lib/types.js");

  it("the profit query carries every ProfitQuery field, including the ones the form omitted", () => {
    const q = profitQueryFrom({ system: " Sol ", fromCurrent: true, radius: "40", maxAge: 48, carriers: false, cargo: "", minPad: "auto", maxStations: 0, maxArrivalLs: 0, maxStops: 5, buyPower: "any", buyState: "any", sellPower: "any", sellState: "any", buyPowerMode: "controls", sellPowerMode: "controls", maxLegLy: 150 });
    expect(Object.keys(q).sort()).toEqual([...PROFIT_QUERY_KEYS].sort());
    expect(q).toMatchObject({ system: "Sol", from_current_station: true, radius_ly: 40, min_pad: null, cargo_capacity: null, max_arrival_ls: null, limit: 100, from_station_id: null, jump_range_ly: null });
    expect(profitQueryFrom({ system: "", shipId: "custom", minPad: "large", cargo: "64", maxArrivalLs: 2000 })).toMatchObject({ system: null, ship_id: null, min_pad: "large", cargo_capacity: 64, max_arrival_ls: 2000 });
    // A stored or flown ship brings its own hull, hold and range: freeform fields are ignored.
    expect(profitQueryFrom({ shipId: 7, minPad: "large", cargo: "64", jumpRange: "30" })).toMatchObject({ ship_id: 7, min_pad: null, cargo_capacity: null, jump_range_ly: null });
    // A picked stored ship travels as ship_id; freeform cargo/jump still override it.
    expect(profitQueryFrom({ shipId: 7, cargo: "", jumpRange: "" })).toMatchObject({ ship_id: 7, cargo_capacity: null, jump_range_ly: null });
    expect(profitQueryFrom({ shipId: "custom", cargo: "1008", jumpRange: "22.5" })).toMatchObject({ ship_id: null, cargo_capacity: 1008, jump_range_ly: 22.5 });
  });

  it("trade search defaults freshness to 2 h — the mapper's fallback and the query state agree", async () => {
    // A query with no maxAge (malformed or pre-migration) falls back to 2 h,
    // matching the Trade tab's own default so the two can never diverge.
    expect(profitQueryFrom({ system: "Sol" }).max_age_hours).toBe(2);
    const { trade } = await import("../lib/trade.svelte.js");
    expect(trade.query.maxAge).toBe(2);
  });

  it("the plot query carries every PlotQuery field", () => {
    const q = plotQueryFrom({ from: " ", to: " Colonia ", supercharge: true, reserve: "3", shipId: 7, effort: "low", injections: false });
    expect(Object.keys(q).sort()).toEqual([...PLOT_QUERY_KEYS].sort());
    expect(q).toMatchObject({ from: null, to: "Colonia", range_ly: null, supercharge: true, max_dry_jumps: 0, thorough: false, reserve_t: 3, ship_id: 7, effort: "low", injections: false, weight: null, fuel: null });
    // White dwarfs are opt-in at every layer: absent means off, and the
    // panel sends the choice explicitly.
    expect(q.white_dwarfs).toBe(false);
    expect(plotQueryFrom({ to: "Colonia", whiteDwarfs: true }).white_dwarfs).toBe(true);
    // Item 39: the planner minimizes time, and lean stops are part of
    // that — every panel plot is min-fuel; there is no knob.
    expect(q.min_fuel).toBe(true);
    expect(plotQueryFrom({ to: "Colonia" }).min_fuel).toBe(true);
    // The default plot is the quick one with only neutrons on; "Try
    // harder" sends the same query thorough.
    const d = plotQueryFrom({ to: "Colonia" });
    expect([d.thorough, d.white_dwarfs, d.injections, d.supercharge]).toEqual([false, false, false, true]);
    expect(plotQueryFrom({ to: "Colonia", thorough: true }).thorough).toBe(true);
  });

  it("a thorough route replaces the quick one only when strictly better", async () => {
    const { betterRoute } = await import("../lib/route.svelte.js");
    const r = (jumps, boosted_jumps, total_ly) => ({ jumps, boosted_jumps, total_ly });
    expect(betterRoute(r(57, 56, 23178), r(58, 55, 23302))).toBe(true);
    expect(betterRoute(r(58, 55, 23302), r(58, 55, 23302))).toBe(false);
    expect(betterRoute(r(58, 54, 23400), r(58, 55, 23302))).toBe(true);
    expect(betterRoute(r(58, 55, 23300), r(58, 55, 23302))).toBe(true);
    expect(betterRoute(r(59, 50, 20000), r(58, 55, 23302))).toBe(false);
    expect(betterRoute(null, r(58, 55, 23302))).toBe(false);
  });
});

describe("galaxyMerge.js", async () => {
  const { mergeSystem } = await import("../lib/galaxyMerge.js");
  it("journal data wins; coords fall back index → EDSM", () => {
    const local = { name: "Sol", coords: null, allegiance: "Federation", station_count: 3 };
    const indexed = { name: "SOL", pos: [0, 0, 0], id64: 10, class: "G" };
    const external = { name: "Sol", coords: [1, 1, 1], allegiance: "Empire" };
    expect(mergeSystem(local, indexed, external)).toEqual({ ...local, coords: [0, 0, 0] });
    expect(mergeSystem({ ...local, coords: [5, 5, 5] }, indexed, external).coords).toEqual([5, 5, 5]);
    expect(mergeSystem(local, null, external).coords).toEqual([1, 1, 1]);
  });
  it("builds a system from EDSM and the index when the journal has none", () => {
    const indexed = { name: "Beagle Point", pos: [1, 2, 3], id64: 99, class: "K" };
    expect(mergeSystem(null, indexed, null)).toEqual({ name: "Beagle Point", id64: 99, coords: [1, 2, 3], allegiance: null, government: null, primary_economy: null, security: null, population: null, primary_star: "K", scoopable: null, station_count: 0, external: false });
    const ext = { name: "Beagle Point", coords: null, allegiance: "None", government: "None", primary_economy: "None", security: "Anarchy", population: 0, primary_star: "K", scoopable: true };
    expect(mergeSystem(null, indexed, ext)).toMatchObject({ coords: [1, 2, 3], security: "Anarchy", scoopable: true, external: true, id64: 99 });
    expect(mergeSystem(null, null, ext)).toMatchObject({ name: "Beagle Point", coords: null, id64: null, external: true });
  });
  it("is null when nothing knows the system", () => {
    expect(mergeSystem(null, null, null)).toBe(null);
  });
});

describe("route status lines", async () => {
  const { LINES } = await import("../lib/loadingLines.js");
  it("every persona has a plot pool and standard covers import and neutrons", () => {
    for (const p of Object.values(LINES)) expect(p.plot.length).toBeGreaterThan(1);
    expect(LINES.standard.import).toHaveLength(1);
    expect(LINES.standard.neutrons).toHaveLength(1);
  });
  it("plotStatusLine picks from the persona's pool for the phase and rotates every 5 s", async () => {
    const { routing, plotStatusLine } = await import("../lib/route.svelte.js");
    routing.persona = "robotic"; routing.progress = { phase: "plot" }; routing.elapsedMs = 0;
    const first = plotStatusLine();
    expect(LINES.robotic.plot).toContain(first);
    expect(plotStatusLine()).toBe(first);
    routing.elapsedMs = 5000;
    expect(plotStatusLine()).not.toBe(first);
    routing.progress = { phase: "import" };
    expect(plotStatusLine()).toBe(LINES.standard.import[0]);
    routing.persona = "unknown"; routing.progress = { phase: "neutrons" };
    expect(plotStatusLine()).toBe(LINES.standard.neutrons[0]);
  });
});

describe("voices.js", async () => {
  const { voiceName, serverVoiceName } = await import("../lib/voices.js");
  it("names Piper models", () => {
    expect(voiceName("en_GB-alan-medium.onnx")).toBe("Alan · British");
    expect(voiceName("en_US-some_new_voice-high")).toBe("Some New Voice · American");
    expect(voiceName("de_DE-thorsten-medium")).toBe("Thorsten · DE");
    expect(voiceName("weird")).toBe("weird");
    expect(voiceName("")).toBe("");
  });
  it("names Kokoro voices", () => {
    expect(serverVoiceName("af_heart")).toBe("Heart · American female");
    expect(serverVoiceName("bm_george")).toBe("George · British male");
    expect(serverVoiceName("alloy")).toBe("alloy");
    expect(serverVoiceName(null)).toBe("");
  });
});

describe("FSD integrity projection", () => {
  it("etaPlan prices jumps, boost line-ups and scoop stops, and formats", async () => {
    const { etaPlan, fmtDuration } = await import("../lib/ui.js");
    // Three hops: plain jump, a neutron that grants the next boost and is
    // scooped at, then the boosted arrival. Rate 0.5 t/s, drive cap 5 t.
    const hops = [
      { class: "G", boosted: false, refuel: false, scoopable: true, fuel_after: 27.0 },
      { class: "Neutron", boosted: false, refuel: true, scoopable: false, fuel_after: 32.0 },
      { class: "M", boosted: true, refuel: false, scoopable: false, fuel_after: 26.0 },
    ];
    const plan = etaPlan(hops, { scoop_rate_t_per_s: 0.5, max_fuel_per_jump: 5.0, capacity: 32.0 });
    // Hop 0: 50 s jump. Hop 1: 50 s jump + neutron line-up 35 s (it boosts
    // the NEXT hop) + scoop (32 - (27 - 5)) / 0.5 = 20 s + companion
    // supercruise allowance 240 s (refuel at a non-scoopable star).
    // Hop 2: 50 s jump.
    expect(plan.perHop[0]).toBe(50);
    expect(plan.perHop[1]).toBe(50 + 35 + 20 + 240);
    expect(plan.perHop[2]).toBe(50);
    expect(plan.totalS).toBe(445);
    expect(plan.scoopS).toBe(20);
    // A scoopable refuel star has no companion allowance.
    const direct = etaPlan(
      [{ class: "K", boosted: false, refuel: true, scoopable: true, fuel_after: 30.0 }],
      { scoop_rate_t_per_s: 0.5, max_fuel_per_jump: 5.0, capacity: 32.0 },
    );
    expect(direct.perHop[0]).toBe(50 + (30 - (32 - 5)) / 0.5);
    // White dwarfs take longer to line up than neutrons.
    const wd = etaPlan(
      [
        { class: "WhiteDwarf", boosted: false, refuel: false, fuel_after: 30.0 },
        { class: "M", boosted: true, refuel: false, fuel_after: 26.0 },
      ],
      { scoop_rate_t_per_s: 0.5, max_fuel_per_jump: 5.0, capacity: 32.0 },
    );
    expect(wd.perHop[0]).toBe(50 + 60);
    // No fuel data or no scoop info: still a jump-count ETA, no scoop time.
    const dry = etaPlan([{ class: "M", boosted: false, refuel: false, fuel_after: null }], null);
    expect(dry.perHop[0]).toBe(50);
    expect(dry.scoopS).toBe(0);
    expect(fmtDuration(495)).toBe("8 m");
    expect(fmtDuration(6135)).toBe("1 h 42 m");
    expect(fmtDuration(45)).toBe("45 s");
  });

  it("loses the drive's static value per boost and marks the boost that crosses 81 %", async () => {
    const { fsdIntegrityPlan } = await import("../lib/ui.js");
    const hops = [{ boosted: false }, ...Array.from({ length: 25 }, () => ({ boosted: true }))];
    const plan = fsdIntegrityPlan(hops, 1.0, 0.01);
    expect(plan.boosts).toBe(25);
    expect(plan.after[0]).toBeCloseTo(1.0);
    expect(plan.after[1]).toBeCloseTo(0.99);
    // 19 boosts keep it at 81 %; the 20th boost (hop 20) would go below.
    expect(plan.after[19]).toBeCloseTo(0.81);
    expect(plan.repairBefore).toBe(20);
    // A repair is assumed there, like a scoop for fuel: back to 100 %,
    // then that boost costs its 1 %, and the count goes on.
    expect(plan.repairs).toEqual([20]);
    expect(plan.after[20]).toBeCloseTo(0.99);
    expect(plan.end).toBeCloseTo(0.94);
    // A long highway run repairs every 19 boosts.
    const long = fsdIntegrityPlan(Array.from({ length: 60 }, () => ({ boosted: true })), 1.0, 0.01);
    expect(long.repairs).toEqual([19, 38, 57]);
    expect(long.end).toBeCloseTo(0.97);
    // From a worn drive the first repair comes sooner.
    expect(fsdIntegrityPlan(hops, 0.85, 0.01).repairBefore).toBe(5);
    // The Mk II SCO takes no damage: never a repair.
    const mk2 = fsdIntegrityPlan(hops, 1.0, 0);
    expect(mk2.repairBefore).toBeNull();
    expect(mk2.end).toBe(1.0);
    // No data, no projection.
    expect(fsdIntegrityPlan(hops, null, 0.01)).toBeNull();
    expect(fsdIntegrityPlan(hops, 1.0, null)).toBeNull();
  });
});

describe("fuelLabel", async () => {
  const { fuelLabel } = await import("../lib/ui.js");
  it("shows a dash, not an empty tank, when the game is not connected", () => {
    expect(fuelLabel(null)).toBe("—");
    expect(fuelLabel({ fuel_main: null, fuel_capacity: 32 })).toBe("—");
    expect(fuelLabel({ fuel_main: 12.34, fuel_capacity: 32 })).toBe("12.3 / 32 t");
    expect(fuelLabel({ fuel_main: 4, fuel_capacity: null })).toBe("4.0 / ? t");
  });
});

