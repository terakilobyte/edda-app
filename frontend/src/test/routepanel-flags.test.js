// Carrier routing is hidden on the route page until 0.3.0 (maintainer,
// 2026-09-07). The flag is a build-time constant, so a server-side render
// of the page proves what a release actually shows: no carrier section,
// no carrier inputs, while the flag is off.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import RoutePanel from "../lib/RoutePanel.svelte";
import { CARRIER_ROUTING } from "../lib/flags.js";

// The route page embeds GalaxyView, whose teardown (run by the server
// renderer too) cancels an animation frame — a browser global Node lacks.
globalThis.cancelAnimationFrame ??= () => {};

describe("RoutePanel and the CARRIER_ROUTING flag", () => {
  it("the flag is off for this release", () => {
    expect(CARRIER_ROUTING).toBe(false);
  });

  it("renders no carrier route section while the flag is off", () => {
    const { body } = render(RoutePanel);
    expect(body).not.toContain("Carrier route");
    expect(body).not.toContain("Plot carrier route");
    expect(body).not.toContain("Destination system");
    // The rest of the page is still there.
    expect(body).toContain("<section");
  });
});
