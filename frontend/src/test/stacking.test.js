import { describe, it, expect } from "vitest";
import { giverLabel, giverTitle, stackSummary } from "../lib/stacking.js";

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
