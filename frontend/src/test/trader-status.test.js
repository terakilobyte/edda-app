import { describe, it, expect } from "vitest";
import { traderStatus } from "../lib/engineering.svelte.js";

// The 2026-09-15 stations outage hid for twenty minutes because a failed
// lookup and an empty galaxy rendered the same sentence. These are three
// different facts and must read as three.
describe("traderStatus", () => {
  it("says the API could not be asked, rather than that nothing is there", () => {
    const s = traderStatus({ asked: false, nearest: [] });
    expect(s.tone).toBe("warn");
    expect(s.text).toMatch(/could not reach/i);
    expect(s.text).not.toMatch(/none known/i);
  });

  it("says nothing is in range only when the lookup actually happened", () => {
    const s = traderStatus({ asked: true, nearest: [] });
    expect(s.tone).toBe("muted");
    expect(s.text).toMatch(/none known within 300 ly/);
  });

  it("flags an unclassified list when the kind could not be told", () => {
    const s = traderStatus({ asked: true, kind_known: false, nearest: [{}, {}] });
    expect(s.tone).toBe("ok");
    expect(s.text).toMatch(/kind unknown/i);
  });

  it("says nothing extra when the answer is a plain typed list", () => {
    expect(traderStatus({ asked: true, kind_known: true, nearest: [{}] }).text).toBe("");
  });

  // A stop from an older backend carries neither flag: treat it as asked,
  // so an upgrade never turns a working list into a false alarm.
  it("treats an absent flag as asked", () => {
    expect(traderStatus({ nearest: [{}] }).text).toBe("");
    expect(traderStatus({ nearest: [] }).tone).toBe("muted");
  });
});
