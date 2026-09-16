import { describe, it, expect } from "vitest";
import { traderStatus } from "../lib/engineering.svelte.js";

// The 2026-09-15 stations outage hid for twenty minutes because a failed
// lookup and an empty galaxy rendered the same sentence. These are three
// different facts and must read as three.
describe("traderStatus", () => {
  it("says the API could not be asked, rather than that nothing is there", () => {
    const s = traderStatus({ asked: false, nearest: [] }, "Sol");
    expect(s.tone).toBe("warn");
    expect(s.text).toMatch(/could not reach/i);
    expect(s.text).not.toMatch(/none known/i);
  });

  it("says nothing is in range only when the lookup actually happened", () => {
    const s = traderStatus({ asked: true, nearest: [] }, "Sol");
    expect(s.tone).toBe("muted");
    expect(s.text).toMatch(/none known within 300 ly/);
  });

  it("flags an unclassified list when the kind could not be told", () => {
    const s = traderStatus({ asked: true, kind_known: false, nearest: [{}, {}] }, "Sol");
    expect(s.text).toMatch(/kind unknown/i);
    expect(s.title).toMatch(/economy/i);
  });

  // Caught in review: fill_traders returns before touching a stop when the
  // position is unknown, so `asked` stays false for a reason that has
  // nothing to do with the API. Saying "could not reach" there is the same
  // class of lie this function exists to stop.
  it("blames a missing position on the position, not on the API", () => {
    const s = traderStatus({ asked: false, nearest: [] }, null);
    expect(s.tone).toBe("muted");
    expect(s.text).toMatch(/position unknown/i);
    expect(s.text).not.toMatch(/could not reach/i);
  });

  it("prefers the missing position even when the lookup did run", () => {
    expect(traderStatus({ asked: true, nearest: [] }, "").text).toMatch(/position unknown/i);
  });

  it("says nothing extra when the answer is a plain typed list", () => {
    expect(traderStatus({ asked: true, kind_known: true, nearest: [{}] }, "Sol").text).toBe("");
  });

  // A stop from an older backend carries neither flag: treat it as asked,
  // so an upgrade never turns a working list into a false alarm.
  it("treats an absent flag as asked", () => {
    expect(traderStatus({ nearest: [{}] }, "Sol").text).toBe("");
    expect(traderStatus({ nearest: [] }, "Sol").tone).toBe("muted");
  });
});
