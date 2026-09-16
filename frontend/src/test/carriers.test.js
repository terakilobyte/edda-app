import { describe, it, expect } from "vitest";
import { ownCarriers, holdSummary } from "../lib/carriers.js";

describe("ownCarriers", () => {
  // The donated seven-year journal had five strangers' carriers listed
  // above the commander's own, each with a year-old location.
  it("shows the commander's own and their squadron's, and nothing else", () => {
    const rows = [
      { callsign: "MINE", owned: true, carrier_type: "FleetCarrier" },
      { callsign: "SQUAD", owned: false, carrier_type: "SquadronCarrier" },
      { callsign: "SEEN", owned: false, carrier_type: "FleetCarrier" },
      { callsign: "ALSO-SEEN", owned: false, carrier_type: null },
    ];
    expect(ownCarriers(rows).map((c) => c.callsign)).toEqual(["MINE", "SQUAD"]);
  });

  it("survives no carriers at all", () => {
    expect(ownCarriers(undefined)).toEqual([]);
    expect(ownCarriers([])).toEqual([]);
  });
});

describe("holdSummary", () => {
  const lines = [
    { commodity: "cmmcomposite", name: "CMM Composite", tons: 12753 },
    { commodity: "liquidoxygen", name: "Liquid Oxygen", tons: 24773 },
    { commodity: "aluminium", name: "Aluminium", tons: 5718 },
  ];

  it("puts the biggest first and uses the display name", () => {
    const { shown } = holdSummary(lines);
    expect(shown.map((s) => s.label)).toEqual(["Liquid Oxygen", "CMM Composite", "Aluminium"]);
    expect(shown[0].tons).toBe(24773);
  });

  it("falls back to the symbol rather than showing nothing", () => {
    const { shown } = holdSummary([{ commodity: "tritium", tons: 10 }]);
    expect(shown[0].label).toBe("tritium");
  });

  // 48 entries in one sentence is what the maintainer was looking at.
  it("caps the list and counts what it left out, in tons as well as rows", () => {
    const many = Array.from({ length: 48 }, (_, i) => ({ commodity: `c${i}`, tons: 100 - i }));
    const { shown, more, moreTons } = holdSummary(many, 8);
    expect(shown).toHaveLength(8);
    expect(more).toBe(40);
    expect(moreTons).toBe(many.slice(8).reduce((n, h) => n + h.tons, 0));
  });

  it("says nothing extra when everything fits", () => {
    expect(holdSummary(lines).more).toBe(0);
  });
});
