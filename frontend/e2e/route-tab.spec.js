// The Route tab, in a real browser, draws a real production answer.
// 2026-10-07: HIP 90112 -> Colonia was answered by the API in 4 s, the
// store logged "plot shown: 58 jumps", and the tab showed nothing — a
// component used without an import, compiled to a runtime reference,
// green in every build since 0.4.2. This is the test that fails for that.
import { test, expect } from "@playwright/test";
import route from "../src/test/fixtures/colonia-route-2026-10-07.json" with { type: "json" };

test.beforeEach(async ({ page }) => {
  // Past the setup wizard; no telemetry consent prompt.
  await page.addInitScript(() => {
    localStorage.setItem("edda.onboarding.complete", "true");
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  page.errors = errors;
});

test("a plotted route renders on the Route tab", async ({ page }) => {
  await page.goto("/?transport=fake");
  await page.getByRole("button", { name: "Route", exact: true }).click();
  await page.getByPlaceholder("From (blank = here)").fill("HIP 90112");
  const to = page.getByPlaceholder("To");
  await to.fill("Colonia");
  await to.press("Enter");

  // The route's figures are in the DOM, not a blank panel.
  await expect(page.getByText(String(route.jumps), { exact: true }).first()).toBeVisible();
  await expect(page.getByText("Try harder")).toBeVisible();
  await expect(page.getByText("Colonia").first()).toBeVisible();

  await page.screenshot({ path: "test-results/route-tab-colonia.png", fullPage: false });
  expect(page.errors, "no uncaught error in the webview").toEqual([]);
});
