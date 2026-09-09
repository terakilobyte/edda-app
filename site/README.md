# EDDA site (edda-app.com apex)

Static, no build step. Deploy: Cloudflare Pages project rooted at
`site/` (the domain is already on Cloudflare; Pages on the apex, the
API stays on `api.`). The download buttons fetch
`https://api.edda-app.com/v1/app/latest.json` at page load — the
button can never offer a version the API isn't serving. That fetch is
cross-origin: the app route needs a CORS header for
`https://edda-app.com` (server lane).

`blog/` is GENERATED — never edit those files by hand. Source of
truth is `docs/blog/blazingly-fast.html` (the frozen long-read);
regenerate with `python3 site/build-blog.py` after any change to it.

Screenshots wanted (the `.shot` placeholders, 16:10, in-app captures):
1. hero — route plotted across the 3D galaxy map
2. route panel — a long plot (57 jumps to Colonia reads well)
3. in-game HUD overlay with the callout feed visible
4. trade routes panel with results
(held for now: a galaxy activity heatmap shot — section removed from the
page until the feature renders everywhere it should)
Drop into `site/assets/` and replace the matching `.ph` placeholder
with `<img src="assets/<name>.png" alt="...">`.

`notes/` is GENERATED — source of truth is `src-tauri/RELEASE-NOTES.md`
(the same file the app compiles into its what's-new splash). Regenerate
with `python3 site/build-notes.py` after editing the notes; the release
runbook order is: write the notes section, regenerate, commit, release.
The "what's new →" link sits beside the download buttons.
