import { describe, it, expect } from "vitest";
import { SECTIONS, DEFAULT_LAYOUT, PRESETS, normalise, move, toggleHidden, toggleCompact, preset, resolve, isHidden, isCompact } from "../lib/hudLayout.js";

const ids = SECTIONS.map((s) => s.id);

describe("normalise", () => {
  it("makes any stored value whole: every section once, unknowns dropped, missing appended", () => {
    expect(normalise(null)).toEqual({ order: ids, hidden: [], compact: [] });
    expect(normalise("junk")).toEqual({ order: ids, hidden: [], compact: [] });
    const l = normalise({ order: ["callouts", "bogus", "location", "callouts"], hidden: ["route", "bogus", "route"], compact: "no" });
    expect(l.order.slice(0, 2)).toEqual(["callouts", "location"]);
    expect(new Set(l.order)).toEqual(new Set(ids));
    expect(l.order.length).toBe(ids.length);
    expect(l.hidden).toEqual(["route"]);
    expect(l.compact).toEqual([]);
  });

  it("every preset is whole", () => {
    for (const [name, p] of Object.entries(PRESETS)) {
      const n = normalise(p);
      expect(n.order, name).toEqual(p.order);
      expect(n.hidden, name).toEqual(p.hidden);
      expect(n.compact, name).toEqual(p.compact);
    }
    expect(preset("nope")).toEqual(normalise(DEFAULT_LAYOUT));
  });
});

describe("editing", () => {
  it("moves a section and stops at the edges", () => {
    const l = normalise(DEFAULT_LAYOUT);
    expect(move(l, "powerplay", -1).order.slice(0, 2)).toEqual(["powerplay", "location"]);
    expect(move(l, "location", -1).order).toEqual(l.order);
    expect(move(l, "callouts", 1).order).toEqual(l.order);
    expect(move(l, "nope", 1).order).toEqual(l.order);
  });

  it("toggles hidden and compact independently", () => {
    let l = normalise(DEFAULT_LAYOUT);
    l = toggleHidden(l, "trade");
    expect(isHidden(l, "trade")).toBe(true);
    expect(isCompact(l, "trade")).toBe(false);
    l = toggleCompact(l, "trade");
    expect(isCompact(l, "trade")).toBe(true);
    l = toggleHidden(l, "trade");
    expect(isHidden(l, "trade")).toBe(false);
    expect(isCompact(l, "trade")).toBe(true);
  });
});

describe("resolve", () => {
  it("prefers the layout remembered for the ship being flown", () => {
    const global = preset("trade");
    const byShip = { "37": preset("combat") };
    expect(resolve(global, byShip, 37).hidden).toEqual(PRESETS.combat.hidden);
    expect(resolve(global, byShip, 6).hidden).toEqual(PRESETS.trade.hidden);
    expect(resolve(global, null, null).hidden).toEqual(PRESETS.trade.hidden);
    expect(resolve(undefined, undefined, undefined)).toEqual(normalise(DEFAULT_LAYOUT));
  });
});
