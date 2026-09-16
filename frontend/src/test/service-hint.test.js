import { describe, it, expect } from "vitest";
import { emptyServiceHint } from "../lib/serviceHint.js";

// The redemption-office case (2026-09-16): every one in the data sits on a
// fleet carrier, the default search excludes carriers, and the commander
// saw a blank table. An empty result must say which kind of empty it is.
describe("emptyServiceHint", () => {
  it("points at the carriers box when the matches were all carriers", () => {
    const s = emptyServiceHint({ label: "redemption offices", radius: 200, carriersIncluded: false, onCarriers: 10618 });
    expect(s).toMatch(/No redemption offices at a station within 200 ly/);
    expect(s).toMatch(/10618 are on fleet carriers/);
    expect(s).toMatch(/carriers/);
  });

  it("uses the singular for one", () => {
    expect(emptyServiceHint({ label: "shipyards", radius: 20, carriersIncluded: false, onCarriers: 1 })).toMatch(/1 is on a fleet carrier/);
  });

  it("says nothing about carriers when there were none there either", () => {
    const s = emptyServiceHint({ label: "material traders", radius: 50, carriersIncluded: false, onCarriers: 0 });
    expect(s).toMatch(/No material traders within 50 ly/);
    expect(s).toMatch(/none on those either/);
    expect(s).not.toMatch(/tick/);
  });

  it("is plainly empty when carriers were already included", () => {
    const s = emptyServiceHint({ label: "shipyards", radius: 30, carriersIncluded: true, onCarriers: 0 });
    expect(s).toBe("No shipyards within 30 ly, on stations or fleet carriers.");
  });
});
