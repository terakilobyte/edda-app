// How heavy is the 3D galaxy map? (2026-10-16, the boss: "I'm not sure how
// heavyweight our 3d map is".) Drives the app in Playwright's Chromium on
// the fake transport and sums the browser's child processes (renderer, GPU)
// from the OS in four states: before the Galaxy tab was ever opened, with
// its map shown, with the tab hidden (mounted, not rendered), and with the
// map unmounted by the "show map" checkbox. Chromium stands in for WebView2
// (same engine); the app's own figure comes from the same recipe by hand.
//
//   cd frontend && npx vite --port 5173 &   (or let playwright.config start it)
//   node ../docs/benches/knobs/galaxy_map_memory.mjs > ../docs/benches/<date>-galaxy-map-memory.csv
import { execFileSync } from "node:child_process";
// The knob lives outside frontend/, so Playwright is loaded from the app's
// own node_modules by path (ESM resolves from the script, not the cwd).
const { chromium } = await import(new URL("../../../frontend/node_modules/playwright/index.mjs", import.meta.url).href);

const BASE = process.env.EDDA_BASE ?? "http://localhost:5173";
const SETTLE_MS = Number(process.env.SETTLE_MS ?? 6000);

async function processes(browser) {
  // Chromium names its own processes over CDP (browser, renderer, GPU...);
  // the OS supplies their memory.
  const cdp = await browser.newBrowserCDPSession();
  const { processInfo } = await cdp.send("SystemInfo.getProcessInfo");
  await cdp.detach();
  const ids = processInfo.map((p) => p.id);
  const ps = execFileSync("powershell", ["-NoProfile", "-Command",
    `Get-CimInstance Win32_Process -Filter "${ids.map((id) => `ProcessId=${id}`).join(" OR ")}" | Select-Object ProcessId,@{n='Private';e={$_.PrivatePageCount}},@{n='WS';e={$_.WorkingSetSize}} | ConvertTo-Json -Compress`],
    { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
  const parsed = JSON.parse(ps);
  const rows = Array.isArray(parsed) ? parsed : [parsed];
  return processInfo.map((p) => {
    const os = rows.find((r) => r.ProcessId === p.id) ?? {};
    return { pid: p.id, role: p.type, privateMb: Math.round((os.Private ?? 0) / 1048576), wsMb: Math.round((os.WS ?? 0) / 1048576) };
  });
}

function summarize(procs) {
  const sum = (role) => procs.filter((p) => role === "all" || p.role === role).reduce((n, p) => n + p.privateMb, 0);
  return { renderer: sum("renderer"), gpu: sum("GPU"), all: sum("all") };
}

const browser = await chromium.launch({ headless: false, args: ["--force-device-scale-factor=1"] });
const page = await browser.newPage({ viewport: { width: 1232, height: 900 } });
await page.addInitScript(() => { localStorage.setItem("edda.onboarding.complete", "true"); localStorage.setItem("edda.galaxy.map", "1"); });
await page.goto(`${BASE}/?transport=fake`);
await page.waitForTimeout(SETTLE_MS);

console.log("# Galaxy 3D map memory: Playwright Chromium on the fake transport; Chromium's processes (CDP SystemInfo.getProcessInfo) measured by the OS (MB private); settle " + SETTLE_MS + " ms per state.");
console.log("state,renderer_mb,gpu_mb,all_mb");
const row = (state, s) => console.log(`${state},${s.renderer},${s.gpu},${s.all}`);

row("trade tab; galaxy never opened", summarize(await processes(browser)));

await page.getByRole("button", { name: "Galaxy", exact: true }).click();
await page.waitForTimeout(SETTLE_MS);
row("galaxy tab shown; map mounted and rendering", summarize(await processes(browser)));

await page.getByRole("button", { name: "Trade", exact: true }).click();
await page.waitForTimeout(SETTLE_MS);
row("trade tab; galaxy map mounted, hidden (not rendering)", summarize(await processes(browser)));

await page.getByRole("button", { name: "Galaxy", exact: true }).click();
await page.getByLabel("show map").uncheck();
await page.waitForTimeout(SETTLE_MS);
row("galaxy tab; show map unchecked (map unmounted, disposed)", summarize(await processes(browser)));
// Freed memory comes back only when the JS heap is collected; ask for it
// so the figure is the floor, not the moment.
const heap = await page.context().newCDPSession(page);
await heap.send("HeapProfiler.collectGarbage");
await page.waitForTimeout(SETTLE_MS);
row("galaxy tab; unmounted, after a forced garbage collection", summarize(await processes(browser)));

await page.getByLabel("show map").check();
await page.waitForTimeout(SETTLE_MS);
row("galaxy tab; show map checked again (remounted)", summarize(await processes(browser)));

await browser.close();
