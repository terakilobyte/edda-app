# EDDA frontend

The desktop app's UI: Svelte 5 + Vite, rendered by Tauri (`../src-tauri`).
`index.html` is the main window; `overlay.html` is the in-game HUD
overlay. Panels live in `src/lib/` (`RoutePanel`, `TradePanel`,
`GalaxyView` for the 3D map, `SystemSettings`, …); Tauri commands are
called through `src/lib/api.js`.

```
npm ci            # once
npm run dev       # Vite dev server (cargo tauri dev runs this for you)
npm run build     # production bundle into dist/ (cargo tauri build runs it)
npx vitest run    # unit tests in src/test/
```

`public/galaxy-stars.bin` and `public/galaxy-populated.bin` are the map's
star clouds, derived from a Spansh galaxy dump (see
`../THIRD-PARTY-NOTICES.md`); `public/icons.svg` is the icon sprite.
