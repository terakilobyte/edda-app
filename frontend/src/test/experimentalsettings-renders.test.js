// 2026-10-24: Settings gained an Experimental sub-tab (the maintainer:
// a feature flown by few ships marked, not hidden). Multiple commanders
// is its first entry. Rendered server-side with real rows, as every
// panel is (Route tab blank 0.4.2-0.4.3).
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import ExperimentalSettings from "../lib/ExperimentalSettings.svelte";

const two = [
  { fid: "F0000001", name: "Alpha", ship: "Mandalay", system: "Sol", selected: true, wrote_s_ago: 12, store_bytes: 1_351_200_000 },
  { fid: "F0000002", name: "Beta", ship: "Type-9 Heavy", system: "Sol", selected: false, wrote_s_ago: 4000, store_bytes: 143_100_000 },
];

describe("ExperimentalSettings", () => {
  it("marks multiple commanders experimental and lists the commanders with their databases", () => {
    const { body } = render(ExperimentalSettings, { props: { commanders: two } });
    expect(body).toContain("Experimental");
    expect(body).toContain("Multiple commanders");
    expect(body).toContain("Alpha");
    expect(body).toContain("Beta");
    expect(body).toContain("1351.2 MB");
    expect(body).toContain("143.1 MB");
    expect(body).toContain("selected");
    expect(body).toContain("Alerts from the unselected commander");
    expect(body).not.toMatch(/\bsir\b|\bma'am\b/i);
  });

  it("tells a single-account player the feature is dormant", () => {
    const { body } = render(ExperimentalSettings, { props: { commanders: [two[0]] } });
    expect(body).toContain("Multiple commanders");
    expect(body).toContain("One commander in this journal folder");
    expect(body).toContain("Alpha");
    expect(body).not.toContain("143.1 MB");
  });

  it("renders its shell before the first sync has found anyone", () => {
    const { body } = render(ExperimentalSettings, { props: { commanders: [] } });
    expect(body).toContain("Multiple commanders");
    expect(body).toContain("No commander found yet");
  });
});
