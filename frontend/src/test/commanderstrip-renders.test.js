// 2026-10-24: one EDDA, several of the player's own commanders, one HUD.
// The strip renders only when there is something to choose between.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import CommanderStrip from "../lib/CommanderStrip.svelte";

const two = [
  { fid: "F0000001", name: "Alpha", ship: "Mandalay", system: "Sol", selected: true, wrote_s_ago: 12, store_bytes: 1_200_000 },
  { fid: "F0000002", name: "Beta", ship: "Type-9 Heavy", system: "Sol", selected: false, wrote_s_ago: 4000, store_bytes: 300_000 },
];

describe("CommanderStrip", () => {
  it("renders one pill per commander with name, ship and system, the selected one marked", () => {
    const { body } = render(CommanderStrip, { props: { commanders: two, onSelect: () => {} } });
    expect(body).toContain("Alpha");
    expect(body).toContain("Beta");
    expect(body).toContain("Mandalay");
    expect(body).toContain("Type-9 Heavy");
    expect(body).toContain('aria-pressed="true"');
    expect(body).toContain('aria-pressed="false"');
    expect(body).toContain('class="live"');
    expect(body).not.toMatch(/\bsir\b|\bma'am\b/i);
  });

  it("renders nothing for a single commander", () => {
    const { body } = render(CommanderStrip, { props: { commanders: [two[0]], onSelect: () => {} } });
    expect(body.replace(/<!--.*?-->/g, "").trim()).toBe("");
  });

  it("names a commander without a name by FID and skips the live dot when the journal is quiet", () => {
    const { body } = render(CommanderStrip, { props: { commanders: [{ ...two[0], name: null }, two[1]], onSelect: () => {} } });
    expect(body).toContain("F0000001");
    expect(body.match(/class="live"/g)?.length ?? 0).toBe(1);
  });
});
