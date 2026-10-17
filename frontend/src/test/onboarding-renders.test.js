// 2026-10-16: the galaxy-map step of setup gained "Only copy to clipboard"
// (the boss: "we need that in the setup steps too on step 6"). A template
// exception leaves the setup sheet blank, so this renders the sheet
// server-side on each step a change touched and asserts the controls are in
// the markup. The step comes from localStorage; node has none, so a stub
// answers for the one key.
import { afterEach, describe, expect, it } from "vitest";
import { render } from "svelte/server";
import Onboarding from "../lib/Onboarding.svelte";
import { KEYS } from "../lib/storage.svelte.js";

function onStep(step) {
  globalThis.localStorage = {
    getItem: (k) => (k === KEYS.onboardingStep ? String(step) : null),
    setItem() {},
    removeItem() {},
  };
}

afterEach(() => {
  delete globalThis.localStorage;
});

describe("Onboarding", () => {
  it("step 6 offers galaxy-map automation and the clipboard-only alternative", () => {
    onStep(5);
    const { body } = render(Onboarding);
    expect(body).toContain("step 6 of 8");
    expect(body).toContain("Enable EDDA routing control");
    expect(body).toContain("Only copy to clipboard");
    expect(body).toContain("paste it into the galaxy map yourself");
    expect(body).toContain("Begin guided Galaxy Map setup");
  });

  it("step 4 captures the routing control", () => {
    onStep(3);
    const { body } = render(Onboarding);
    expect(body).toContain("step 4 of 8");
    expect(body).toContain("Capture routing control");
  });
});
