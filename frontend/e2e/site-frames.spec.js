// The website's screenshot frames: the four accent brackets live in the
// bezel, never over the picture (maintainer, 2026-10-07: "the 4 square
// orange corner edges show on top of the images and cover things").
// The bezel is the frame's padding; the brackets sit 6 px in with a
// 14 px edge, so an image inset by at least 18 px on every side cannot
// be touched by them.
import { test, expect } from "@playwright/test";

const SITE = "http://127.0.0.1:8099/index.html";

test("every screenshot sits inside its bezel, clear of the corner brackets", async ({ page }) => {
  await page.goto(SITE);
  const frames = page.locator(".shot");
  const n = await frames.count();
  expect(n).toBeGreaterThan(0);
  for (let i = 0; i < n; i++) {
    const frame = frames.nth(i);
    const img = frame.locator("img");
    await expect(img).toBeVisible();
    const f = await frame.boundingBox();
    const m = await img.boundingBox();
    const inset = { left: m.x - f.x, top: m.y - f.y, right: f.x + f.width - (m.x + m.width), bottom: f.y + f.height - (m.y + m.height) };
    for (const [side, px] of Object.entries(inset)) {
      expect(px, `frame ${i}: image is ${px.toFixed(1)} px from the ${side} edge; the bracket band is 18 px`).toBeGreaterThanOrEqual(18);
    }
  }
  await frames.first().screenshot({ path: "test-results/site-hero-frame.png" });
});

test("the receipts say what the benches say", async ({ page }) => {
  await page.goto(SITE);
  const band = page.locator(".band");
  await expect(band).toContainText("~1 s");
  await expect(band).toContainText("17 MB");
  await expect(band).not.toContainText("0.7 s");
  await expect(page.locator("body")).not.toContainText("live on your disk");
});
