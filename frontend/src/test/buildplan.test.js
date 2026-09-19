import { describe, it, expect } from "vitest";
import { planRows, sameForAll, groupCounts, planRequest, proposedFor, savedFrom, isPlanned, hasWork } from "../lib/buildplan.js";

// A Type-10's business end: nine hardpoints, one already engineered.
const modules = [
  { slot: "LargeHardpoint1", slot_name: "Large Hardpoint 1", item_name: "Pulse Laser 3C/G", module_type: "Pulse Laser", blueprint: "Focused Weapon", grade: 2 },
  { slot: "LargeHardpoint2", slot_name: "Large Hardpoint 2", item_name: "Pulse Laser 3C/G", module_type: "Pulse Laser", blueprint: null, grade: null },
  { slot: "MediumHardpoint1", slot_name: "Medium Hardpoint 1", item_name: "Multi-cannon 2D/G", module_type: "Multi-cannon", blueprint: "Overcharged Weapon", grade: 5 },
  { slot: "PowerDistributor", slot_name: "Power Distributor", item_name: "Power Distributor 7A", module_type: "Power Distributor", blueprint: null, grade: null },
  { slot: "PaintJob", slot_name: "Paint job", item_name: "Paint", module_type: null },
];

describe("planRows", () => {
  it("makes one row per engineerable module, continuing what is fitted", () => {
    const rows = planRows(modules);
    expect(rows.map((r) => r.slot)).toEqual(["LargeHardpoint1", "LargeHardpoint2", "MediumHardpoint1", "PowerDistributor"]);
    const [l1, l2, mc, pd] = rows;
    expect(l1).toMatchObject({ blueprint: "Focused Weapon", from_grade: 2, target_grade: 5, include: true });
    expect(l2).toMatchObject({ blueprint: "", from_grade: 0, include: false });
    // Already at the top grade: nothing to plan, so not included by default.
    expect(mc).toMatchObject({ from_grade: 5, include: false });
    expect(pd.include).toBe(false);
  });

  it("a saved plan wins over the defaults", () => {
    const rows = planRows(modules, { LargeHardpoint2: { blueprint: "Efficient Weapon", target_grade: 4, experimental: "Oversized", include: true } });
    expect(rows[1]).toMatchObject({ blueprint: "Efficient Weapon", target_grade: 4, experimental: "Oversized", include: true });
  });
});

describe("sameForAll", () => {
  it("copies one laser's choice onto every laser and nothing else", () => {
    const rows = planRows(modules);
    const pick = { ...rows[1], blueprint: "Focused Weapon", target_grade: 4, experimental: "Oversized", include: true };
    const out = sameForAll(rows, pick);
    expect(out[0]).toMatchObject({ blueprint: "Focused Weapon", target_grade: 4, experimental: "Oversized", include: true });
    expect(out[1]).toMatchObject({ blueprint: "Focused Weapon", target_grade: 4, include: true });
    expect(out[2].blueprint).toBe("Overcharged Weapon");
    expect(out[3].blueprint).toBe("");
  });

  it("does not include a module already at or above the copied grade", () => {
    const rows = planRows([
      { slot: "A", slot_name: "A", item_name: "x", module_type: "Pulse Laser", blueprint: "Focused Weapon", grade: 4 },
      { slot: "B", slot_name: "B", item_name: "x", module_type: "Pulse Laser", blueprint: null, grade: null },
    ]);
    const out = sameForAll(rows, { ...rows[1], blueprint: "Focused Weapon", target_grade: 4, include: true });
    expect(out[0].include).toBe(false);
    expect(out[1].include).toBe(true);
  });
});

describe("planRequest / proposedFor / savedFrom", () => {
  it("asks only for what is included and still to do, shaped for the backend", () => {
    let rows = planRows(modules);
    rows = sameForAll(rows, { ...rows[1], blueprint: "Focused Weapon", target_grade: 4, experimental: "", include: true });
    rows[3] = { ...rows[3], experimental: "Super Conduits", include: true };
    const req = planRequest(rows);
    expect(req).toEqual([
      { slot: "LargeHardpoint1", module_type: "Pulse Laser", blueprint: "Focused Weapon", from_grade: 2, target_grade: 4, experimental: null },
      { slot: "LargeHardpoint2", module_type: "Pulse Laser", blueprint: "Focused Weapon", from_grade: 0, target_grade: 4, experimental: null },
      { slot: "PowerDistributor", module_type: "Power Distributor", blueprint: null, from_grade: 0, target_grade: 5, experimental: "Super Conduits" },
    ]);
    // The SLEF export carries only blueprints (an experimental has no grade).
    expect(proposedFor(rows)).toEqual([
      { slot: "LargeHardpoint1", module_type: "Pulse Laser", blueprint: "Focused Weapon", grade: 4 },
      { slot: "LargeHardpoint2", module_type: "Pulse Laser", blueprint: "Focused Weapon", grade: 4 },
    ]);
    const saved = savedFrom(rows);
    expect(saved.PowerDistributor).toEqual({ blueprint: "", target_grade: 5, experimental: "Super Conduits", include: true });
    expect(planRows(modules, saved)).toEqual(rows);
  });

  it("a row with nothing chosen or nothing left is not planned", () => {
    expect(isPlanned({ include: true, blueprint: "", experimental: "", from_grade: 0, target_grade: 5 })).toBe(false);
    expect(isPlanned({ include: true, blueprint: "X", experimental: "", from_grade: 5, target_grade: 5 })).toBe(false);
    expect(isPlanned({ include: false, blueprint: "X", experimental: "", from_grade: 0, target_grade: 5 })).toBe(false);
    expect(isPlanned({ include: true, blueprint: "X", experimental: "", from_grade: 3, target_grade: "4" })).toBe(true);
  });

  /// The maintainer's Power Distributor 7A at G5 (2026-09-19): nothing to
  /// roll, so it cannot be included — until an experimental is chosen,
  /// which is work of its own and goes to the backend without a blueprint.
  it("a top-grade module has no work until an experimental is chosen", () => {
    const rows = planRows([{ slot: "PowerDistributor", slot_name: "Power Distributor", item_name: "Power Distributor 7A", module_type: "Power Distributor", blueprint: "Weapon Focused", grade: 5 }]);
    expect(hasWork(rows[0])).toBe(false);
    expect(isPlanned({ ...rows[0], include: true })).toBe(false);
    const withX = { ...rows[0], experimental: "Super Conduits", include: true };
    expect(hasWork(withX)).toBe(true);
    expect(planRequest([withX])).toEqual([{ slot: "PowerDistributor", module_type: "Power Distributor", blueprint: null, from_grade: 5, target_grade: 5, experimental: "Super Conduits" }]);
    expect(proposedFor([withX])).toEqual([]);
  });

  it("counts rows per module type", () => {
    expect([...groupCounts(planRows(modules))]).toEqual([["Pulse Laser", 2], ["Multi-cannon", 1], ["Power Distributor", 1]]);
  });
});
