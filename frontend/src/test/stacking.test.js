import { describe, it, expect } from "vitest";
import { giverLabel, giverTitle, stackSummary, stackEconomics, handInsLabel, shortCr } from "../lib/stacking.js";

// Stacking mode (2026-09-16): the HUD lists every giver the commander
// already holds a massacre from, so a duplicate — same giver, same
// target, which progresses consecutively — is seen before it is accepted.
describe("giverLabel", () => {
  it("is just the name for one live mission", () => {
    expect(giverLabel({ faction: "Pilots Trade Network", missions: 1, ready: 0, duplicate: false })).toBe("Pilots Trade Network");
  });
  it("counts duplicates and marks the finished ones", () => {
    expect(giverLabel({ faction: "HIP 90112 Jet Central Corp.", missions: 2, ready: 1, duplicate: true })).toBe("HIP 90112 Jet Central Corp. ×2 1✓");
    expect(giverLabel({ faction: "Liberals of Ahayan", missions: 1, ready: 1, duplicate: false })).toBe("Liberals of Ahayan ✓");
    expect(giverLabel({ faction: "Crimson Armada", missions: 2, ready: 2, duplicate: true })).toBe("Crimson Armada ×2 ✓");
  });
});

describe("giverTitle", () => {
  it("says why a duplicate matters", () => {
    const t = giverTitle({ faction: "HIP 90112 Jet Central Corp.", missions: 2, ready: 1, duplicate: true });
    expect(t).toBe("2 missions from HIP 90112 Jet Central Corp., 1 ready to turn in — duplicate: same giver, same target, so these progress one after another");
  });
  it("keeps the singular and says nothing about duplicates otherwise", () => {
    expect(giverTitle({ faction: "Liberals of Ahayan", missions: 1, ready: 1, duplicate: false })).toBe("1 mission from Liberals of Ahayan, all ready to turn in");
  });
});

describe("stackSummary", () => {
  const stack = (givers, other_targets = 0) => ({ target_faction: "Anana Brotherhood", givers, other_targets });
  it("counts givers and duplicates and names what is against another target", () => {
    const s = stack([
      { faction: "A", missions: 2, ready: 0, duplicate: true },
      { faction: "B", missions: 1, ready: 0, duplicate: false },
      { faction: "C", missions: 3, ready: 1, duplicate: true },
    ], 2);
    expect(stackSummary(s)).toBe("3 givers · 2 duplicates · +2 against another target");
  });
  it("says only what is there", () => {
    expect(stackSummary(stack([{ faction: "A", missions: 1, ready: 0, duplicate: false }]))).toBe("1 giver");
  });
});

// The stack's figures (2026-09-20), every one a stated field summed. The
// numbers are the maintainer's twenty-mission stack as the store's fixture
// pins it: 120 kills clear it, 72 still to make, 824 credited.
describe("stackEconomics", () => {
  const stack = { target_faction: "Anana Brotherhood", target_system: "Anana", givers: [], other_targets: 0, kills_needed: 120, kills_remaining: 72, kills_credited: 824, value: 20_000_000, value_ready: 12_000_000, value_shareable: 20_000_000 };
  it("says what clears the stack, what it credits, and what it is worth", () => {
    expect(stackEconomics(stack)).toBe("72 kills to go in Anana (120 for the stack) · 824 credited · 6.9× per kill · 20.0M cr (12.0M ready to collect, all wing-shared)");
  });
  it("says when the kills are all made and skips what is not there", () => {
    expect(stackEconomics({ ...stack, kills_remaining: 0, value: 0, target_system: null })).toBe("all 120 kills made · 824 credited · 6.9× per kill");
    expect(stackEconomics(null)).toBe("");
    expect(stackEconomics({ ...stack, kills_needed: 0 })).toBe("");
  });
  it("names the wing share only when it differs from the whole", () => {
    expect(stackEconomics({ ...stack, value_shareable: 5_000_000 })).toContain("5.0M wing-shared");
    expect(stackEconomics({ ...stack, value_shareable: 0, value_ready: 0 })).toBe("72 kills to go in Anana (120 for the stack) · 824 credited · 6.9× per kill · 20.0M cr");
  });
});

describe("handInsLabel / shortCr", () => {
  it("counts the missions ready at this dock with their credits", () => {
    expect(handInsLabel({ station: "Wheelock Port", system: "Puneith", missions: [{}, {}, {}], credits: 2_400_000 })).toBe("3 missions ready to hand in here · 2.4M cr");
    expect(handInsLabel({ station: "X", system: "Y", missions: [{}], credits: 0 })).toBe("1 mission ready to hand in here");
    expect(handInsLabel(null)).toBe("");
  });
  it("shortens credits the way the HUD reads them", () => {
    expect([shortCr(900), shortCr(12_400), shortCr(2_450_000), shortCr(1_250_000_000)]).toEqual(["900", "12k", "2.5M", "1.25B"]);
  });
});
