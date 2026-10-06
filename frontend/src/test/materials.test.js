import { describe, it, expect } from "vitest";
import { ratio, cap } from "../lib/materials.js";

const m = (symbol, kind, group, grade) => ({ symbol, kind, group, grade });
const mga = m("militarygradealloys", "manufactured", "Thermic", 5);
const tempered = m("temperedalloys", "manufactured", "Thermic", 1);
const thermic = m("thermicalloys", "manufactured", "Thermic", 4);
const yttrium = m("yttrium", "raw", "1", 4);
const technetium = m("technetium", "raw", "2", 4);
const molybdenum = m("molybdenum", "raw", "2", 3);
const phosphorus = m("phosphorus", "raw", "2", 1);
const antimony = m("antimony", "raw", "7", 4);
const arsenic = m("arsenic", "raw", "6", 2);

describe("material trader ratios (the journal's nine)", () => {
  it("same group, down and up", () => {
    expect(ratio(mga, tempered)).toEqual([1, 81]);
    expect(ratio(mga, thermic)).toEqual([1, 3]);
    expect(ratio(tempered, thermic)).toEqual([216, 1]);
    expect(ratio(tempered, mga)).toEqual([1296, 1]);
  });
  it("across groups", () => {
    expect(ratio(yttrium, technetium)).toEqual([6, 1]);
    expect(ratio(yttrium, molybdenum)).toEqual([2, 1]);
    expect(ratio(antimony, arsenic)).toEqual([2, 3]);
    expect(ratio(yttrium, phosphorus)).toEqual([2, 9]);
    expect(ratio(phosphorus, yttrium)).toEqual([1296, 1]);
  });
  it("refuses what the trader refuses", () => {
    expect(ratio(yttrium, mga)).toBeNull();
    expect(ratio(mga, mga)).toBeNull();
    expect(ratio(m("guardian_powerconduit", "manufactured", null, 1), tempered)).toBeNull();
  });
  it("caps by grade", () => {
    expect([1, 2, 3, 4, 5].map(cap)).toEqual([300, 250, 200, 150, 100]);
  });
});
