// 2026-10-07: the boss plotted HIP 90112 -> Colonia against prod; the store
// received the route and logged "plot shown: 58 jumps", and the Route tab
// showed nothing. A template exception leaves a blank tab, so this renders
// the tab server-side with a real production answer in the store (the
// fields the client adds after the API are filled in as routing.rs does)
// and asserts the route's figures are in the markup.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import RoutePanel from "../lib/RoutePanel.svelte";
import { routing, setRoute } from "../lib/route.svelte.js";
import route from "./fixtures/colonia-route-2026-10-07.json";

globalThis.cancelAnimationFrame ??= () => {};

describe("RoutePanel with a plotted route in the store", () => {
  it("renders the production Colonia answer", () => {
    routing.lastQuery = { from: "HIP 90112", to: "Colonia", supercharge: true };
    routing.from = "HIP 90112"; routing.to = "Colonia";
    setRoute(route, "tab", "HIP 90112", "Colonia");
    const { body } = render(RoutePanel);
    expect(body).toContain("Jumps");
    expect(body).toContain(String(route.jumps));
    expect(body).toContain("Colonia");
    expect(body).toContain("Try harder");
  });
});
