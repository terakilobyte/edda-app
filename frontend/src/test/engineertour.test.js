// The engineer tour's panel helpers (2026-10-14): what the tour command
// is asked for, and the lines the planner shows per leg.
import { describe, expect, it } from "vitest";
import { tourAsks, tourLines } from "../lib/buildplan.js";

const report = {
  engineers: [{ engineer: "Felicity Farseer", rank: 5 }, { engineer: "The Dweller", rank: 5 }],
  items: [
    { assigned_to: "Felicity Farseer", module_type: "Frame Shift Drive", blueprint: "Increased FSD Range", target_grade: 5, experimental: null, slot_name: "Frame Shift Drive" },
    { assigned_to: "The Dweller", module_type: "Power Distributor", blueprint: "Charge Enhanced", target_grade: 5, experimental: "Super Conduits", slot_name: "Power Distributor" },
    { assigned_to: "The Dweller", module_type: "Pulse Laser", blueprint: "Efficient Weapon", target_grade: 3, experimental: null, slot_name: "Medium Hardpoint 1" },
    { assigned_to: "The Dweller", module_type: "Pulse Laser", blueprint: "Efficient Weapon", target_grade: 3, experimental: null, slot_name: "Medium Hardpoint 2" },
  ],
};

describe("engineer tour helpers", () => {
  it("asks for each itinerary stop with its jobs as short lines", () => {
    const asks = tourAsks(report);
    expect(asks.map((a) => a.engineer)).toEqual(["Felicity Farseer", "The Dweller"]);
    expect(asks[0].jobs).toEqual(["Frame Shift Drive Increased FSD Range G5"]);
    expect(asks[1].jobs[0]).toMatch(/^2 × Pulse Laser Efficient Weapon G3$/);
    expect(asks[1].jobs.length).toBe(2);
  });

  it("renders one line per leg with the base, the system and the leg distance", () => {
    const tour = {
      from_system: "Sol", total_ly: 186.4, unplaced: [],
      stops: [
        { engineer: "Felicity Farseer", system: "Deciat", station: "Farseer Inc", leg_ly: 12.4, jobs: ["x"] },
        { engineer: "Elvira Martuuk", system: "Khun", station: "Long Sight Base", leg_ly: 173.9, jobs: [] },
      ],
    };
    expect(tourLines(tour)).toEqual([
      { engineer: "Felicity Farseer", where: "Farseer Inc (Deciat)", leg: "12 ly", jobs: ["x"] },
      { engineer: "Elvira Martuuk", where: "Long Sight Base (Khun)", leg: "174 ly", jobs: [] },
    ]);
    expect(tourLines(null)).toEqual([]);
    expect(tourAsks(null)).toEqual([]);
  });
});
