// Playwright: the app in a real browser on the fake transport, and the
// website. `npx playwright test` here; CI runs it on Ubuntu after vitest.
// The Chrome extension an agent drives cannot open localhost; Playwright
// (and its MCP server, .mcp.json at the repo root) can.
import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: "http://localhost:5173",
    viewport: { width: 1232, height: 900 },
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: [
    {
      command: "npx vite --port 5173 --strictPort",
      url: "http://localhost:5173/",
      reuseExistingServer: !process.env.CI,
      timeout: 60_000,
    },
    {
      command: "python3 -m http.server 8099 --bind 127.0.0.1 --directory ../site",
      url: "http://127.0.0.1:8099/index.html",
      reuseExistingServer: !process.env.CI,
      timeout: 15_000,
    },
  ],
});
