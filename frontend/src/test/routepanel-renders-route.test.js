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

  // 2026-10-09: a rim plot that needs FSD injections (Jongou XM-W d1-0 ->
  // Byoi Fraae CQ-G d10-0 on a 72 ly Caspian) must say so above the route
  // and mark the hop. The server marks hops with `injection: "<grade>"`
  // and counts them in `injections`; here the production answer is given
  // two premium hops.
  it("flags a route that needs FSD injections and marks the hops", () => {
    const injected = structuredClone(route);
    injected.injections = 2;
    injected.hops[3].injection = "premium";
    injected.hops[9].injection = "premium";
    routing.lastQuery = { from: "HIP 90112", to: "Colonia", supercharge: true };
    setRoute(injected, "tab", "HIP 90112", "Colonia");
    const { body } = render(RoutePanel);
    expect(body).toContain("FSD injections required: 2 × premium");
    expect(body).toContain("you cannot synthesise any");
    expect(body).toContain("premium injection");
  });

  // The server is refining behind the early answer: the strip shows the
  // best so far with its numbers and a way to take it now.
  it("shows the refinement strip with the best route so far", () => {
    routing.lastQuery = { from: "HIP 90112", to: "Colonia", supercharge: true };
    setRoute(route, "tab", "HIP 90112", "Colonia");
    routing.refining = { earlyJumps: route.jumps, best: { ...route, jumps: 51, boosted_jumps: 40, refuel_stops: 2, injections: 0 }, done: false };
    const { body } = render(RoutePanel);
    routing.refining = null;
    expect(body).toContain("Refining on the server");
    expect(body).toContain("best so far 51 jumps");
    expect(body).toContain("40 boosted");
    expect(body).toContain("Use it");
  });

  it("offers the replot with injections on an island refusal made without them", () => {
    routing.route = null;
    routing.error = 'no known star within 37.1 ly of Byoi Fraae AE-T d3-0 (the origin): the nearest is 74.8 ly away; a route may be possible with FSD injections -- turn on "injections if required" and replot';
    const { body } = render(RoutePanel);
    routing.error = "";
    expect(body).toContain("Try with injections");
  });

  it("does not offer it on a refusal that already had injections", () => {
    routing.route = null;
    routing.error = "no known star within 37.1 ly of Byoi Fraae AE-T d3-0 (the origin): the nearest is 74.8 ly away; an FSD injection reaches 74.1 ly, still short";
    const { body } = render(RoutePanel);
    routing.error = "";
    expect(body).not.toContain("Try with injections");
  });

  it("says nothing about injections on a route that needs none", () => {
    routing.lastQuery = { from: "HIP 90112", to: "Colonia", supercharge: true };
    setRoute(route, "tab", "HIP 90112", "Colonia");
    const { body } = render(RoutePanel);
    expect(body).not.toContain("injections required");
    expect(body).not.toContain("injection required");
  });
});
