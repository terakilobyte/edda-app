import { describe, it, expect } from "vitest";
import { ownCarriers } from "../lib/carriers.js";

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
