import { defineConfig } from "vitest/config";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Runs the pure logic and the stores under node. `.svelte.js` files are
// compiled by the Svelte plugin so runes work outside a component; the
// browser condition picks Svelte's client build for that.
export default defineConfig({
  plugins: [svelte({ hot: false })],
  resolve: { conditions: ["browser"] },
  test: {
    include: ["src/**/*.test.js"],
    environment: "node",
    setupFiles: ["src/test/setup.js"],
  },
});
