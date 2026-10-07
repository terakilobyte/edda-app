---
name: edda-ui
description: EDDA's visual language and frontend conventions for the desktop app (Svelte 5 + Tauri 2), the HUD overlay and the website, with the render-before-and-after loop every UI change ships with. Use when editing anything under frontend/, site/, or the overlay, when adding a tab or panel, when touching screenshots or copy on edda-app.com, or when asked to make something look right.
---

# EDDA UI

EDDA's look is a cockpit instrument, not a spreadsheet in space: dark
ground, one orange accent, monospace for numbers, prose for everything a
commander reads. The tokens are the law; the copy is specific; every
change ships with a picture. Full conventions: [REFERENCE.md](REFERENCE.md).

## Quick start

1. Read the tokens before writing a colour: `frontend/src/app.css` (app),
   the `<style>` block of `site/index.html` (site). Never introduce a hex
   value that is not a token.
2. Make the change in a worktree with its own `CARGO_TARGET_DIR` (edits to
   the main tree restart the maintainer's dev app).
3. See it in a real browser. `cd frontend && npx playwright test` runs
   the app on the fake transport (`/?transport=fake`, fixtures in
   `src/lib/browserTransport.js`) and the website, with screenshots in
   `test-results/`; the Playwright MCP server (`.mcp.json`) gives an
   agent the same browser to click through. `scripts/render.sh` is the
   quick static render when a test is too much.
4. Run the gates: `cd frontend && npx vitest run && npx vite build`. The
   contract test fails on any `api.js` command that is imported but not
   exported; the build fails on a Svelte a11y warning. Both are meant to.
5. Put the before/after images in the PR and say what moved and why.

## Rules that are not taste

- The player is "Commander", never sir/ma'am, in every string.
- Nothing on screen identifies or locates another commander (EDDA is not
  a surveillance tool). No names, positions or journal content in
  telemetry, screenshots for the site, or panels.
- A number on a page comes from a bench CSV or a release note, with the
  source noted in the PR. `docs/benches/` is the record; a figure that
  cannot be traced is not written.
- The HUD overlay is transparent, undecorated, always on top and
  click-through; it is read at a glance in a cockpit, so: one line per
  fact, no scrolling, no hover-only information, every section
  removable from Settings (the layout editor), per-ship layouts remembered.
- Interactive controls say what happens ("Plot", "Try harder", "Stop");
  an error says what went wrong and what to do; a quiet state says why
  it is quiet ("fills from scans since 2026-09-19").
- Site copy is the maintainer's voice: short, specific, a joke only when
  it is earned by a fact. Do not add marketing adjectives.

## Workflows

**New panel or tab.** Copy the shape of an existing one (MissionsPanel,
MiningPanel): `<section class="panel">`, a `.kicker` eyebrow, numbers in
`.num`, pills for state. Register commands in `frontend/src/lib/api.js`
(table entry AND the export list). Add the vitest contract. Render it
with the dev app (`run` skill) or a fixture.

**Website section.** One `.strip` per feature: copy on one side, a
`.shot` frame on the other. The frame is a bezel: the image sits inside
an 18 px band and the four accent brackets live in that band, never over
the picture. Screenshots: 1600×753 for app views, HUD crops at least
900 px wide, PNG, no commander names visible.

**Screenshot refresh.** Captures come from the maintainer's Windows
build (the Mac has no release). Put them in `site/assets/` under the
existing names, update the `alt` text to what the picture actually
shows (counts and systems included), render the page, compare.

## Advanced

Tokens, component inventory, overlay constraints, the Tauri/WebView2 and
Wayland quirks, and the render script: [REFERENCE.md](REFERENCE.md).
