// 2026-10-24 dev-app run: the strip was read before the first sync had
// ingested a Commander line, so it showed bare FIDs, and nothing read it
// again when that sync finished (it refreshed only on journal-changed,
// which the first sync does not send). App.svelte is too wired to render
// under SSR, so this pins the wiring in its source.
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

const src = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");

describe("App commander strip", () => {
  it("re-reads the commanders when the first sync completes", () => {
    const handler = src.match(/onSyncComplete\(\(\) => \{([\s\S]*?)\}\)\);/);
    expect(handler, "an onSyncComplete handler").not.toBeNull();
    expect(handler[1]).toContain("refreshCommanders");
  });
});
