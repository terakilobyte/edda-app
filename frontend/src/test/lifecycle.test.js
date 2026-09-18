import { describe, it, expect } from "vitest";
import { fakeTransport } from "./fakeTransport.js";
import { setTransport } from "../lib/transport.js";
import { createJournalResource } from "../lib/lifecycle.svelte.js";

// Tabs stay mounted between visits (2026-09-18). A hidden panel must not
// re-query on every journal change -- on a fight that is one per kill --
// but it must not show stale data when it comes back either: it remembers
// it is stale and catches up exactly once.
describe("journalResource gated by tab visibility", () => {
  const tick = () => new Promise((r) => setTimeout(r, 0));

  async function harness(active) {
    const t = fakeTransport();
    setTransport(t);
    let loads = 0;
    const res = createJournalResource(async () => { loads += 1; }, () => active.value);
    res.start();
    await tick(); await tick();
    return { t, res, loads: () => loads };
  }

  it("refreshes on journal-changed while showing", async () => {
    const active = { value: true };
    const { t, res, loads } = await harness(active);
    expect(loads()).toBe(1);
    await t.emit("journal-changed", 3);
    await tick();
    expect(loads()).toBe(2);
    expect(res.stale).toBe(false);
    res.stop();
  });

  it("while hidden it only remembers, then catches up once when shown", async () => {
    const active = { value: false };
    const { t, res, loads } = await harness(active);
    expect(loads()).toBe(1, "the first load happens regardless");
    await t.emit("journal-changed", 1);
    await t.emit("journal-changed", 2);
    await tick();
    expect(loads()).toBe(1, "hidden: no re-query per change");
    expect(res.stale).toBe(true);
    active.value = true;
    await res.catchUp();
    expect(loads()).toBe(2, "one catch-up for two changes");
    expect(res.stale).toBe(false);
    await res.catchUp();
    expect(loads()).toBe(2, "nothing new: no refresh");
    res.stop();
  });
});
