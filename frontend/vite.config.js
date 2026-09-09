import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { resolve } from "node:path";

// Standard Tauri + Vite pairing: fixed port matching tauri.conf.json's
// devUrl, and ignore src-tauri's own rebuilds so they don't trigger a
// frontend reload loop. Two pages: the main window and the HUD overlay.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
  build: {
    // Three.js powers the interactive galaxy view. Its production chunk is
    // intentionally shared rather than duplicated across the main UI pages.
    chunkSizeWarningLimit: 750,
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, "index.html"),
        overlay: resolve(import.meta.dirname, "overlay.html"),
      },
    },
  },
});
