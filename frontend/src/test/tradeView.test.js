// The opening tab of a profit report: loops first when they out-earn legs.
import { describe, it, expect } from "vitest";
import { boardLine } from "../lib/tradeView.js";
import { pickView } from "../lib/tradeView.js";

const report = ({ leg = 0, trip = 0, ring = 0 } = {}) => ({
  legs: leg ? [{ profit_per_hour_repeat: leg }] : [],
  round_trips: trip ? [{ profit_per_hour: trip }] : [],
  rings: ring ? [{ profit_per_hour: ring }] : [],
});

describe("pickView", () => {
  it("opens on legs when there is nothing else", () => {
    expect(pickView(report({ leg: 1_000_000 }))).toBe("legs");
    expect(pickView(report())).toBe("legs");
    expect(pickView(null)).toBe("legs");
  });
  it("opens on round trips when the best loop beats the best leg", () => {
    expect(pickView(report({ leg: 1_000_000, trip: 1_200_000 }))).toBe("trips");
  });
  it("stays on legs when the loop does not beat the leg", () => {
    expect(pickView(report({ leg: 1_000_000, trip: 900_000 }))).toBe("legs");
    expect(pickView(report({ leg: 1_000_000, trip: 1_000_000 }))).toBe("legs");
  });
  it("opens on rings when the best ring beats both", () => {
    expect(pickView(report({ leg: 1_000_000, trip: 1_200_000, ring: 1_500_000 }))).toBe("rings");
    expect(pickView(report({ leg: 1_000_000, trip: 1_200_000, ring: 1_100_000 }))).toBe("trips");
  });
});

describe("boardLine", () => {
  it("names which board the server priced against, or nothing when none was sent", () => {
    expect(boardLine(null)).toBeNull();
    expect(boardLine(undefined)).toBeNull();
    expect(boardLine({ used: true, reason: "newer", rows: 212 })).toContain("your board from the dock (212 rows");
    expect(boardLine({ used: false, reason: "older", rows: 212 })).toContain("yours from the dock was older");
    expect(boardLine({ used: false, reason: "mismatch", rows: 0 })).toContain("another station");
  });
});
