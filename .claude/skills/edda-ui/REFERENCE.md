# EDDA UI reference

## Tokens

App (`frontend/src/app.css`) and site (`site/index.html` `<style>`) share
one palette. Use the variable, never the value.

| token | value | use |
|---|---|---|
| `--bg` | #0a0c10 | page ground |
| `--bg-2` | #10141b | recessed areas, image backgrounds |
| `--panel` | #131820 | cards, panels, frames |
| `--panel-2` | #1a2029 | raised rows (app only) |
| `--line` | #262e3a | borders, dividers |
| `--line-2` | #33405060 | faint separators (app only) |
| `--text` | #e4e6ea | body |
| `--muted` | #8b95a5 | secondary text, eyebrows |
| `--accent` | #ff8c1a | THE orange: one per view, for the thing to act on |
| `--accent-2` | #ffb35c | accent on dark ground, hover |
| `--cyan` | #5ec8e5 | routes, links, the neutron line |
| `--ok` / `--warn` / `--bad` | #57c66d / #ffb300 / #ff5c5c | state, never decoration |

Type: site display `Bricolage Grotesque`, site mono `IBM Plex Mono`; app
body `Segoe UI` stack, app mono `Cascadia Mono`/`Consolas`. Numbers that
line up in columns get `font-variant-numeric: tabular-nums` and the
`.num` class.

Semantic colour is not the accent: a red pill means a state; the orange
means "here is the control". Two accents on one view is the spreadsheet
look coming back.

## Where things live

- `frontend/src/App.svelte`: tabs, onboarding gate, the visited-tab
  mount (tabs stay mounted; a panel refreshes only while showing).
- `frontend/src/lib/*Panel.svelte`: one file per tab. `Overlay.svelte`
  is the HUD window. `Place.svelte` renders a system/station with the
  route hand-off control (`requestRoute` in `route.svelte.js`).
- `frontend/src/lib/api.js`: the command table (`name: ["tauri_cmd",
  [args], defaults]`) AND an export list further down. Both, or the
  contract test (`src/test/contracts.test.js`) fails the build.
- `*.svelte.js` modules hold runes state shared across panels.
- `frontend/src/lib/helpTopics.js`: the Help tab's text; every panel
  gets a topic, written for a commander, no internals.
- `src-tauri/src/overlay.rs`: the HUD window flags; `hudLayout.js`:
  sections, order, compact, presets, per-ship layouts.
- Site: `site/index.html` (one file, inline CSS), `site/route/` (the web
  planner), `site/ship-computer/`, `site/linux/`, `site/privacy/`,
  `site/notes/` (release notes rendered by `build-notes.py`), `site/blog/`
  (frozen; verdicts go to the roadmap, not the blog).

## Svelte 5 and Tauri 2 conventions

- Runes (`$state`, `$derived`, `$effect`, `$props`), no stores for new
  code. `{@const}` must be the immediate child of a block.
- A component used in markup must be imported; Svelte compiles an
  unknown tag as a runtime reference and the build stays green (the
  0.4.2 Route tab blanked for five days this way). The render test
  (`RoutePanel` against the real Colonia answer) is the guard; give any
  new panel one.
- Every webview error is forwarded to the app log; read
  `%LOCALAPPDATA%\edda\logs` (Windows) when a tab misbehaves.
- WebView2 spawns ~7 helper processes that Task Manager files under
  Edge; "the app uses 65 MB" is edda.exe alone. Quote the whole-app
  figure from `docs/benches/2026-10-05-app-footprint.csv`.
- Wayland: the overlay's always-on-top and click-through are compositor
  dependent; Proton journals are found under the compatdata path (see
  `site/linux/`). Test overlay changes on both window systems or say
  which one was not.
- Voice is muted under test (`test_state`); audible tests only with
  `--features test-voice`.

## The HUD overlay

Read in a cockpit at a glance, in a window the game does not know
exists. One fact per line, numbers first, no scrolling, no tooltips. A
section is a card the commander can hide, move or compact from Settings;
presets exist; the layout is remembered per ship. Callouts that are
spoken also appear here; the mute set and the HUD sections are
independent. When a value is an estimate, say so in the line ("≥",
"about"); when it is from the game's own signal, do not hedge it.

## Rendering before and after

`scripts/render.sh` runs headless Chrome at 1232 px width and writes a
PNG; given a local file it serves the file's directory itself, because
the Chrome extension cannot open file:// or localhost pages and headless
Chrome can. Crop the region that changed and attach both images to the
PR. A UI change without the pair is not done, the way a planner change
without a bench is not done.

For the dev app, the `run` skill launches it and takes screenshots; the
Mac dev tree can show every tab, the Windows box is where the overlay is
captured over the game.

## Site copy

The maintainer's voice: exasperated, specific, funny only when a fact
earns it. Every number traces to `docs/benches/` or a release note, and
the band of receipts on the front page is re-checked at every release:
in 2026-10 it still said "0.7 s to plot 22,000 ly" and "199 M systems on
your disk" a month after the local index left the product. When a claim
cannot be re-measured, it comes off the page.
