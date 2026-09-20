import { describe, it, expect } from "vitest";
import { planRows, withSwap, swapsFrom, sameForAll, groupCounts, planRequest, proposedFor, savedFrom, isPlanned, hasWork, compactSlots, itinerary, blocked, applyImport } from "../lib/buildplan.js";

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

describe("the report reads as groups, not one line per slot", () => {
  it("folds slot names into ranges", () => {
    expect(compactSlots(["Large hardpoint 1", "Large hardpoint 2", "Large hardpoint 3", "Large hardpoint 4", "Medium hardpoint 1", "Medium hardpoint 2", "Medium hardpoint 3", "Small hardpoint 1", "Small hardpoint 2"]))
      .toBe("Large hardpoints 1–4; Medium hardpoints 1–3; Small hardpoints 1–2");
    expect(compactSlots(["Utility 1", "Utility 2", "Utility 3", "Utility 5", "Utility 6", "Utility 8"])).toBe("Utility 1–3, 5–6, 8");
    expect(compactSlots(["Power Plant", "Utility 4"])).toBe("Power Plant; Utility 4");
  });

  // The maintainer's Type-10 plan, 2026-09-19: Long Range lasers to The
  // Dweller, six shield boosters and a power plant nobody unlocked takes
  // to G5. Twenty-two lines became four.
  const report = {
    engineers: [{ engineer: "The Dweller", rank: 5, jobs: [] }],
    items: [
      ...[1, 2, 3, 4].map((n) => ({ slot_name: `Large hardpoint ${n}`, module_type: "Pulse Laser", blueprint: "Long Range Weapon", target_grade: 4, reachable: true, assigned_to: "The Dweller", engineers: [] })),
      ...[1, 2, 3].map((n) => ({ slot_name: `Medium hardpoint ${n}`, module_type: "Pulse Laser", blueprint: "Long Range Weapon", target_grade: 4, reachable: true, assigned_to: "The Dweller", engineers: [] })),
      { slot_name: "Power Plant", module_type: "Power Plant", blueprint: "Overcharged", target_grade: 5, reachable: false, max_reachable_grade: 4, assigned_to: null,
        engineers: [{ engineer: "Etienne Dorn", max_grade: 5, unlocked: false, status: "Not known" }, { engineer: "Felicity Farseer", max_grade: 1, unlocked: true, status: "Unlocked" }, { engineer: "Marco Qwent", max_grade: 4, unlocked: true, status: "Unlocked" }] },
      ...[1, 2, 3, 5, 6, 8].map((n) => ({ slot_name: `Utility ${n}`, module_type: "Shield Booster", blueprint: "Resistance Augmented", target_grade: 5, reachable: false, max_reachable_grade: 3, assigned_to: null,
        engineers: [{ engineer: "Didi Vatermann", max_grade: 5, unlocked: false, status: "Not known" }, { engineer: "Lei Cheung", max_grade: 3, unlocked: true, status: "Unlocked" }, { engineer: "Mel Brandon", max_grade: 5, unlocked: false, status: "Known" }] })),
      { slot_name: "Power Distributor", module_type: "Power Distributor", blueprint: null, target_grade: 5, reachable: false, assigned_to: null, engineers: [], experimental: "Super Conduits" },
    ],
  };

  it("one line per engineer, jobs grouped by blueprint and grade with the slots folded", () => {
    expect(itinerary(report)).toEqual([
      { engineer: "The Dweller", rank: 5, jobs: [{ what: "Long Range Weapon G4", module_type: "Pulse Laser", count: 7, slots: "Large hardpoints 1–4; Medium hardpoints 1–3" }] },
    ]);
  });

  it("what is blocked is grouped, with who takes it part-way today and who to unlock", () => {
    const b = blocked(report);
    expect(b.map((g) => [g.what, g.count, g.slots])).toEqual([
      ["Overcharged G5", 1, "Power Plant"],
      ["Resistance Augmented G5", 6, "Utility 1–3, 5–6, 8"],
    ]);
    expect(b[1]).toMatchObject({ max_reachable_grade: 3, today: ["Lei Cheung"], unlock: [{ engineer: "Didi Vatermann", status: "Not known" }, { engineer: "Mel Brandon", status: "Known" }] });
    expect(b[0]).toMatchObject({ today: ["Marco Qwent"], unlock: [{ engineer: "Etienne Dorn", status: "Not known" }] });
  });
});

describe("applyImport", () => {
  it("lays an imported build over the rows: continue, start over, done, new slot, untouched", () => {
    const rows = planRows(modules);
    const imported = {
      items: [
        // Same blueprint on the fitted laser: continue from grade 2 to 5.
        { slot: "LargeHardpoint1", slot_name: "Large Hardpoint 1", item_name: "Pulse Laser 3C/G", module_type: "Pulse Laser", blueprint: "Focused Weapon", from_grade: 2, target_grade: 5, experimental: "Oversized", done: false },
        // A swapped module: beam laser where a pulse laser sits, from grade 0.
        { slot: "LargeHardpoint2", slot_name: "Large Hardpoint 2", item_name: "Beam Laser 3C/G", module_type: "Beam Laser", blueprint: "Long Range Weapon", from_grade: 0, target_grade: 5, experimental: null, done: false },
        // Exactly what the ship has: nothing to do.
        { slot: "MediumHardpoint1", slot_name: "Medium Hardpoint 1", item_name: "Multi-cannon 2D/G", module_type: "Multi-cannon", blueprint: "Overcharged Weapon", from_grade: 5, target_grade: 5, experimental: null, done: true },
        // A slot the ship has empty.
        { slot: "Slot01_Size6", slot_name: "Optional 1 (size 6)", item_name: "Shield Generator 6A", module_type: "Shield Generator", blueprint: "Thermal Resistant Shields", from_grade: 0, target_grade: 5, experimental: "Fast Charge", done: false },
      ],
    };
    const out = applyImport(rows, imported);
    expect(out.find((r) => r.slot === "LargeHardpoint1")).toMatchObject({ blueprint: "Focused Weapon", from_grade: 2, target_grade: 5, experimental: "Oversized", include: true });
    expect(out.find((r) => r.slot === "LargeHardpoint2")).toMatchObject({ item_name: "Beam Laser 3C/G", module_type: "Beam Laser", blueprint: "Long Range Weapon", from_grade: 0, include: true });
    expect(out.find((r) => r.slot === "MediumHardpoint1")).toMatchObject({ include: false });
    expect(out.find((r) => r.slot === "PowerDistributor")).toMatchObject({ include: false });
    expect(out.find((r) => r.slot === "Slot01_Size6")).toMatchObject({ slot_name: "Optional 1 (size 6)", blueprint: "Thermal Resistant Shields", experimental: "Fast Charge", include: true });
    expect(planRequest(out).map((r) => r.slot)).toEqual(["LargeHardpoint1", "LargeHardpoint2", "Slot01_Size6"]);
  });
});

// The slot table for a stub hull: two hardpoints (one empty), a cargo slot
// nobody engineers. Candidates are what the slot takes.
const slots = [
  { slot: "LargeHardpoint1", slot_name: "Large hardpoint 1", group: "hardpoint", size: 3, fitted: "hpt_pulselaser_gimbal_large", fitted_name: "Pulse Laser 3C/G",
    candidates: [
      { item: "hpt_pulselaser_gimbal_large", item_name: "Pulse Laser 3C/G", kind: "Pulse Laser", module_type: "Pulse Laser" },
      { item: "hpt_beamlaser_gimbal_large", item_name: "Beam Laser 3C/G", kind: "Beam Laser", module_type: "Beam Laser" },
      { item: "hpt_guardian_gausscannon_fixed_medium", item_name: "Guardian Gauss Cannon (fixed, medium)", kind: "Guardian Gauss Cannon", module_type: null },
    ] },
  { slot: "LargeHardpoint2", slot_name: "Large hardpoint 2", group: "hardpoint", size: 3, fitted: null, fitted_name: null,
    candidates: [{ item: "hpt_pulselaser_gimbal_large", item_name: "Pulse Laser 3C/G", kind: "Pulse Laser", module_type: "Pulse Laser" }] },
  { slot: "Slot01_Size8", slot_name: "Optional 1 (size 8)", group: "internal", size: 8, fitted: "int_cargorack_size8_class1", fitted_name: "Cargo Rack 8E",
    candidates: [{ item: "int_cargorack_size7_class1", item_name: "Cargo Rack 7E", kind: "Cargo Rack", module_type: null }] },
];
const fitted = [{ slot: "LargeHardpoint1", slot_name: "Large hardpoint 1", item: "hpt_pulselaser_gimbal_large", item_name: "Pulse Laser 3C/G", module_type: "Pulse Laser", blueprint: "Focused Weapon", grade: 2 }];

describe("swaps: only what the slot takes, starting over", () => {
  it("one row per slot with the slot table, empty slots included, non-engineerable ones without a plan", () => {
    const rows = planRows(fitted, {}, slots);
    expect(rows.map((r) => r.slot)).toEqual(["LargeHardpoint1", "LargeHardpoint2", "Slot01_Size8"]);
    expect(rows[0]).toMatchObject({ item_name: "Pulse Laser 3C/G", module_type: "Pulse Laser", blueprint: "Focused Weapon", from_grade: 2, include: true, swap: null });
    expect(rows[1]).toMatchObject({ item: null, item_name: null, module_type: null, include: false });
    expect(rows[2]).toMatchObject({ item_name: "Cargo Rack 8E", module_type: null, blueprint: "", include: false });
    expect(hasWork(rows[2])).toBe(false);
    expect(planRequest(rows)).toHaveLength(1);
  });

  it("a swap takes the candidate's type and starts at grade 0; back to fitted restores the fitted plan", () => {
    const rows = planRows(fitted, {}, slots);
    const beam = slots[0].candidates[1];
    const swapped = withSwap(rows[0], beam);
    expect(swapped).toMatchObject({ swap: "hpt_beamlaser_gimbal_large", item_name: "Beam Laser 3C/G", module_type: "Beam Laser", from_grade: 0, blueprint: "", include: false });
    expect(swapsFrom([swapped, rows[1], rows[2]])).toEqual([{ slot: "LargeHardpoint1", item: "hpt_beamlaser_gimbal_large" }]);
    const back = withSwap(swapped, null);
    expect(back).toMatchObject({ swap: null, item_name: "Pulse Laser 3C/G", module_type: "Pulse Laser", from_grade: 2, blueprint: "Focused Weapon", include: true });
    // Picking the fitted module itself is no swap.
    expect(withSwap(swapped, slots[0].candidates[0]).swap).toBeNull();
  });

  it("a swap to a module no engineer works has no plan controls but still counts as a swap", () => {
    const rows = planRows(fitted, {}, slots);
    const gauss = withSwap(rows[0], slots[0].candidates[2]);
    expect(gauss.module_type).toBeNull();
    expect(planRequest([gauss])).toEqual([]);
    expect(swapsFrom([gauss])).toEqual([{ slot: "LargeHardpoint1", item: "hpt_guardian_gausscannon_fixed_medium" }]);
  });

  it("the saved plan carries the swap and restores it, module type included", () => {
    const rows = planRows(fitted, {}, slots);
    const swapped = [withSwap(rows[0], slots[0].candidates[1]), rows[1], rows[2]];
    const saved = savedFrom(swapped);
    expect(saved.LargeHardpoint1.swap).toBe("hpt_beamlaser_gimbal_large");
    expect(saved.Slot01_Size8.swap).toBeUndefined();
    const again = planRows(fitted, saved, slots);
    expect(again[0]).toMatchObject({ swap: "hpt_beamlaser_gimbal_large", module_type: "Beam Laser", from_grade: 0 });
    // A saved swap the slot no longer takes is dropped, not fitted blind.
    const stale = planRows(fitted, { LargeHardpoint1: { swap: "hpt_railgun_fixed_huge" } }, slots);
    expect(stale[0].swap).toBeNull();
  });

  it("an imported build's swaps land in the rows' Swap to column", () => {
    const rows = planRows(fitted, {}, slots);
    const out = applyImport(rows, {
      swaps: [{ slot: "LargeHardpoint2", slot_name: "Large hardpoint 2", have: null, want: "Pulse Laser 3C/G", want_item: "hpt_pulselaser_gimbal_large" }],
      items: [{ slot: "LargeHardpoint2", slot_name: "Large hardpoint 2", item_name: "Pulse Laser 3C/G", module_type: "Pulse Laser", blueprint: "Long Range Weapon", from_grade: 0, target_grade: 5, experimental: null, done: false }],
    });
    expect(out[1]).toMatchObject({ swap: "hpt_pulselaser_gimbal_large", module_type: "Pulse Laser", blueprint: "Long Range Weapon", include: true });
    expect(swapsFrom(out)).toEqual([{ slot: "LargeHardpoint2", item: "hpt_pulselaser_gimbal_large" }]);
  });
});
