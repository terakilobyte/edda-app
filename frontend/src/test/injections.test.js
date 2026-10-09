// FSD injections against the materials aboard (2026-10-09: the boss's
// rim plot, Jongou XM-W d1-0 -> Byoi Fraae CQ-G d10-0, needs them at both
// ends; the Route tab must say so and say whether the commander can make
// them). The words are pinned because the tab shows them verbatim.
import { describe, expect, it } from "vitest";
import { injectionBanner, injectionShortfall, routeInjectionGrade } from "../lib/injections.js";

const route = (injections, grade) => ({
  injections,
  hops: [{ name: "A" }, { name: "B", injection: injections > 0 ? grade : null }, { name: "C" }],
});
const grades = (premium, basic, missing = []) => [
  { grade: "premium", mult: 2, can_make: premium, materials: ["carbon", "germanium", "arsenic", "niobium", "yttrium", "polonium"].map((name) => ({ name, have: missing.includes(name) ? 0 : 3 })) },
  { grade: "standard", mult: 1.5, can_make: 0, materials: [] },
  { grade: "basic", mult: 1.25, can_make: basic, materials: [] },
];

describe("injections on a plotted route", () => {
  it("a route without injections has nothing to say", () => {
    expect(routeInjectionGrade(route(0, null))).toBeNull();
    expect(injectionShortfall(route(0, null), grades(2, 2))).toBeNull();
    expect(injectionBanner(route(0, null), grades(2, 2))).toBeNull();
    expect(injectionBanner(null, [])).toBeNull();
  });

  it("names the grade the hops use and counts them", () => {
    expect(routeInjectionGrade(route(2, "premium"))).toBe("premium");
    expect(injectionShortfall(route(2, "premium"), grades(3, 0))).toEqual({ grade: "premium", needed: 2, canMake: 3, short: 0, missing: [] });
  });

  it("says the commander can make them when they can", () => {
    expect(injectionBanner(route(2, "premium"), grades(3, 0))).toBe("FSD injections required: 2 × premium — you can make 3");
    expect(injectionBanner(route(1, "basic"), grades(0, 1))).toBe("FSD injection required: 1 × basic — you can make 1");
  });

  it("says how short they are", () => {
    expect(injectionBanner(route(3, "premium"), grades(1, 0))).toBe("FSD injections required: 3 × premium — you can make 1, short 2");
  });

  it("names the materials missing when none can be made", () => {
    expect(injectionBanner(route(2, "premium"), grades(0, 0, ["arsenic", "polonium"]))).toBe(
      "FSD injections required: 2 × premium — you cannot synthesise any (no arsenic, polonium aboard)",
    );
    // Materials are all aboard but short of a full recipe: no list to blame.
    expect(injectionBanner(route(2, "premium"), grades(0, 0))).toBe("FSD injections required: 2 × premium — you cannot synthesise any");
    // The grade list has not loaded yet: still honest, never a crash.
    expect(injectionBanner(route(2, "premium"), [])).toBe("FSD injections required: 2 × premium — you cannot synthesise any");
  });
});
