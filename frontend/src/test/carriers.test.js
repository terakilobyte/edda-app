import { describe, it, expect } from "vitest";
import { ownCarriers, carrierBadge, jumpText } from "../lib/carriers.js";

const aged = (h) => ({ value: 1, as_of: "x", age_hours: h });

describe("ownCarriers", () => {
  // The donated seven-year journal had five strangers' carriers listed
  // above the commander's own, each with a year-old location.
  it("shows the commander's own and their squadron's, and nothing else", () => {
    const rows = [
      { callsign: "MINE", name: "Mine", owned: true, carrier_type: "FleetCarrier" },
      { callsign: "SQUAD", name: "Squad", owned: false, carrier_type: "SquadronCarrier" },
      { callsign: "SEEN", owned: false, carrier_type: "FleetCarrier" },
      { callsign: "ALSO-SEEN", owned: false, carrier_type: null },
    ];
    expect(ownCarriers(rows).map((c) => c.callsign)).toEqual(["MINE", "SQUAD"]);
  });

  // 2026-10-24 dev-app run on a two-account folder: the same squadron's
  // carrier twice (two carrier ids, one callsign and name: the squadron
  // replaced it), and squadron carriers seen only by a location heartbeat
  // listed as blank cards.
  it("lists a squadron's carrier once, the most recently heard of", () => {
    const rows = [
      { carrier_id: 1, callsign: "FRHT", name: "Freight", owned: false, carrier_type: "SquadronCarrier", location: aged(7400), capacity: aged(8200) },
      { carrier_id: 2, callsign: "FRHT", name: "Freight", owned: false, carrier_type: "SquadronCarrier", location: aged(4000), capacity: aged(6500) },
    ];
    expect(ownCarriers(rows).map((c) => c.carrier_id)).toEqual([2]);
  });

  it("drops a squadron carrier with neither name nor callsign", () => {
    const rows = [
      { carrier_id: 1, callsign: null, name: null, owned: false, carrier_type: "SquadronCarrier", location: aged(3000) },
      { carrier_id: 2, callsign: "P3RC", name: "11", owned: false, carrier_type: "SquadronCarrier", location: aged(200) },
    ];
    expect(ownCarriers(rows).map((c) => c.carrier_id)).toEqual([2]);
  });

  it("survives no carriers at all", () => {
    expect(ownCarriers(undefined)).toEqual([]);
    expect(ownCarriers([])).toEqual([]);
  });
});

describe("carrierBadge", () => {
  it("says a squadron carrier the commander bought is the squadron's, bought by them", () => {
    expect(carrierBadge({ owned: true, carrier_type: "FleetCarrier" })).toEqual({ text: "yours", cls: "ok" });
    expect(carrierBadge({ owned: true, carrier_type: "SquadronCarrier" })).toEqual({ text: "squadron · yours", cls: "ok" });
    expect(carrierBadge({ owned: false, carrier_type: "SquadronCarrier" })).toEqual({ text: "squadron", cls: "" });
  });
});

describe("jumpText", () => {
  it("never prints a negative departure", () => {
    expect(jumpText({ system: "Beta", minutes_to_departure: 17 })).toBe("Jump scheduled to Beta · departs in 17 min");
    expect(jumpText({ system: "Beta", minutes_to_departure: -33 })).toBe("Jump to Beta · departed 33 min ago");
    expect(jumpText({ system: "Beta", minutes_to_departure: null })).toBe("Jump scheduled to Beta");
  });
});
