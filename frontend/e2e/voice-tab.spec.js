// The Voice tab, in a real browser, with Kokoro removed. 2026-10-17: the
// maintainer removed Kokoro to test the flow, had to go back to setup to
// install it, and the Voice tab then probed http://localhost:8880 — an
// address EDDA's managed Kokoro never listens on — and reported "no speech
// server". The tab now reads the engine's live status: not installed is
// said in words, with the install offered right there.
import { test, expect } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.setItem("edda.onboarding.complete", "true");
  });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  page.errors = errors;
});

test("picking the speech server with Kokoro removed offers the install, not a dead address", async ({ page }) => {
  await page.goto("/?transport=fake");
  await page.getByRole("button", { name: "Voice", exact: true }).click();
  await expect(page.getByText("no voice", { exact: true }).first()).toBeVisible();

  await page.getByLabel("speech server (Kokoro or compatible)").check();
  await expect(page.getByText("Kokoro is not installed on this computer.", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Download and install (about 2.8 GB)" })).toBeVisible();
  await expect(page.getByText("no speech server at")).toHaveCount(0);
  await expect(page.getByText("localhost:8880")).toHaveCount(0);

  await page.screenshot({ path: "test-results/voice-tab-kokoro-removed.png", fullPage: false });
  expect(page.errors, "no uncaught error in the webview").toEqual([]);
});
