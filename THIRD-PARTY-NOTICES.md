# Third-party notices

EDDA itself is MIT licensed (see `LICENSE`). It ports logic and data from
community projects, vendors one JavaScript library, downloads speech
engines on demand, and talks to community data services. Attribution and
licence obligations are recorded here; each ported or vendored site also
carries an in-source header naming its origin. Dependency counts come from
a 2026-09-09 audit of `cargo metadata` and `npm ls` (`frontend/`).

## 1. Ported code

### EDDI (github.com/EDCD/EDDI) — Apache License 2.0

Portions of EDDA are ported from EDDI, Copyright the EDDI contributors,
licensed under the Apache License, Version 2.0
(https://www.apache.org/licenses/LICENSE-2.0). Ported material is marked
in-source and modified for EDDA (Rust, plain-text speech instead of
SSML/IPA markup). Current ports:

- `src-tauri/src/phonetics.rs` — procgen system-name detection, NATO
  spell-out rules and named fix-ups, from
  `SpeechService/SpeechConversions/PhoneticStarSystem.cs` and
  `SpeechConversions.cs`.
- `src-tauri/src/status_flags.rs` — the Status.json Flags/Flags2 bit
  tables, from `DataDefinitions/Status.cs`.

`src-tauri/src/mission_route.rs` (mission hand-in ordering) follows the
shape of EDDI's mission routing — nearest-neighbour construction, 2-opt
improvement — and credits EDDI in its header; it is an independent Rust
implementation of that textbook algorithm, not a port.

## 2. Vendored data

### EDEngineer (github.com/msarilar/EDEngineer) — MIT

`crates/ed-engineering/data/blueprints.json` is the engineering blueprint
database (module types, grades, ingredients, effects) vendored from the
EDEngineer project, not hand-authored. The MIT licence requires the
copyright and permission notice to accompany the material; the upstream
copyright line is not reproduced in this tree, so the notice reads:

> Copyright (c) the EDEngineer authors (https://github.com/msarilar/EDEngineer).
> Licensed under the MIT License. Permission is hereby granted, free of
> charge, to any person obtaining a copy of this software and associated
> documentation files, to deal in the Software without restriction, subject
> to the condition that the above copyright notice and this permission
> notice are included in all copies or substantial portions of the Software.
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND.

### Elite Dangerous Fandom wiki, "Synthesis" page — CC BY-SA 3.0

`crates/ed-engineering/data/synthesis.json` carries synthesis recipe
numbers machine-extracted from the recipe tables of
https://elite-dangerous.fandom.com/wiki/Synthesis (MediaWiki API,
`action=parse`, wikitext), fetched 2026-09-07 (source and date are in the
file's own header). Recipe names, grades, ingredient counts and the
per-grade bonus figures are game facts. The page's descriptive prose
(each recipe's `refills` sentence) was removed from the file on 2026-09-09
so that nothing under the wiki's CC BY-SA 3.0 licence
(https://creativecommons.org/licenses/by-sa/3.0/) is reproduced here;
attribution for the extraction: Elite Dangerous Wiki contributors, via
the page above.

### Community engineering guides — used with attribution

- `crates/ed-engineering/data/engineer_unlocks.json` — engineer invite,
  unlock and rank-up conditions from the Wanderer's Toolbox guide
  https://wanderer-toolbox.com/guides/engineering-unlock/ (Fox's
  step-by-step), fetched 2026-08-26, with engineer base names and systems
  cross-checked against Inara's Engineers directory in August 2026 (per the
  file header). The condition strings are game facts; the free-text
  `notes` fields paraphrase the guide. The guide carries no licence
  statement; the maintainer's reading is that it is freely shared, and
  the notes are reproduced with attribution and will be rewritten or
  removed if the author asks.
- `crates/ed-engineering/data/material_sources.json` — well-known material
  farming sites (system, body, method, materials). Each entry names its
  source in-file as "community (Wanderer's Toolbox / Inara)". Site names,
  locations and yields are game facts; the `method` sentences are EDDA's
  own summaries. Used with attribution.

## 3. EDCD/FDevIDs (github.com/EDCD/FDevIDs) — no license file

Frontier's factual ID assignments (commodity and material symbols, display
names, categories) as collected and maintained by the Elite Dangerous
Community Developers. Reproduced in this tree as:

- `crates/ed-journal/data/commodity.csv` and `material.csv`, embedded into
  `ed-journal` at compile time (`crates/ed-journal/src/catalog.rs`);
- `crates/ed-api/src/fdev_data.rs`, a generated table from `commodity.csv`
  and `rare_commodity.csv`, regenerated as the game adds items.

The upstream repository carries no license file. The data is factual
identifiers consumed universally by community tooling; we reproduce the
tables with attribution to the EDCD/FDevIDs contributors, in-source and
here, and will replace or remove them if the maintainers ask.

## 4. Vendored library

### three.js (github.com/mrdoob/three.js) — MIT

- `site/assets/three/` — three.js r170 (`three.module.min.js`,
  `controls/OrbitControls.js`, `lines/*`), used by the site's 3D route
  map. The upstream `LICENSE` file ("Copyright © 2010-2024 three.js
  authors") sits alongside the files.
- The desktop app bundles the same library: `frontend/` depends on
  `three@0.170.0` and Vite builds it into the app's web assets, so the
  three.js MIT notice applies to the installers as well.

## 5. Runtime downloads

The app fetches these on demand, only after the user asks for the feature,
into its own data directory. Each is the upstream project's own release
artefact, unmodified; their licences are the upstream projects':

| Component | Source | Version | Licence |
|---|---|---|---|
| Piper TTS engine | github.com/rhasspy/piper | release `2023.11.14-2` | MIT |
| Piper voices (below) | huggingface.co/rhasspy/piper-voices | `v1.0.0` | The voices repository is MIT; each voice's training data carries its own terms (verified from the MODEL_CARDs, 2026-09-09). EDDA does not redistribute the models — the app downloads a voice from the repository on the commander's request — and EDDA is free, non-commercial software. |
| ↳ `en_US-lessac-high` | Lessac Blizzard 2013 corpus (CSTR, University of Edinburgh) | | Dataset licence: research purposes only, no commercial use, no redistribution of the materials (cstr.ed.ac.uk/projects/blizzard/2013/lessac_blizzard2013/license.html). The model card states no licence for the model itself. |
| ↳ `en_GB-alan-medium`, `en_US-amy-medium` | fine-tuned from the lessac medium voice; alan's base voice is Mycroft AI's `apope` (MycroftAI/mimic3-voices), whose LICENSE file is a copyright notice, "All Rights Reserved" | | As above for the lessac lineage; the apope base carries no open licence. |
| ↳ `en_US-ryan-high` | RyanSpeech (kaggle.com/datasets/roholazandie/ryanspeech) | | Dataset licence CC BY-NC-SA 4.0 (non-commercial, share-alike); the model card states no licence for the model itself. |
| Vosk speech library | github.com/alphacep/vosk-api | `v0.3.45` (Windows, Linux) | Apache-2.0 |
| Vosk model `vosk-model-small-en-us-0.15` | alphacephei.com/vosk/models | 0.15 | Apache-2.0 |
| sherpa-onnx runtime | github.com/k2-fsa/sherpa-onnx | `v1.13.6` (Windows, Linux) | Apache-2.0 |
| NVIDIA Parakeet TDT 0.6B v2 (int8, `sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8`) | sherpa-onnx `asr-models` release | v2 | CC BY 4.0 (NVIDIA) |
| Kokoro-FastAPI (managed "Kokoro" voice) | github.com/remsky/Kokoro-FastAPI | commit `5fb71ea` | Apache-2.0 per the upstream repository; the Kokoro model and the Python packages it installs carry their own licences |
| uv (Python runtime manager for Kokoro) | github.com/astral-sh/uv | `0.11.33` | MIT OR Apache-2.0 |

## 6. Data services

- **Spansh** (https://spansh.co.uk) — the galaxy dumps the server hydrates
  from, and the plotter the app links to. The star clouds bundled with the
  app and the site (`frontend/public/galaxy-*.bin`,
  `site/assets/galaxy-*.bin`) and the populated-bubble routing index
  (`src-tauri/assets/bubble/*.bin.zst`) are derived from a Spansh galaxy
  dump (`crates/ed-galaxy/src/import.rs`). Data from spansh.co.uk.
- **EDSM** (https://www.edsm.net) — system, body and sphere lookups the
  server proxies. Data from EDSM, edsm.net.
- **EDDN** (https://github.com/EDCD/EDDN, Elite Dangerous Community
  Developers) — the live feed of market, outfitting, shipyard and journal
  observations.
- **Inara** (https://inara.cz) — links only; no Inara data is stored.
  Likewise Coriolis and EDSY receive ship builds by link only.

## 7. Rust crate licences

545 crates from crates.io resolved with source available on the audit host
(macOS). Grouped by declared licence expression; a crate in an `OR` group
may be used under any listed licence and EDDA takes MIT or Apache-2.0
where offered:

| Declared licence | Crates |
|---|---:|
| MIT OR Apache-2.0 | 339 |
| MIT | 97 |
| Unicode-3.0 (ICU4X crates) | 18 |
| MPL-2.0 | 17 |
| Zlib OR Apache-2.0 OR MIT | 17 |
| Apache-2.0 | 11 |
| Unlicense OR MIT | 10 |
| BSD-3-Clause | 4 |
| BSD-2-Clause OR MIT OR Apache-2.0 | 3 |
| ISC | 3 |
| Zlib | 2 |
| CC0-1.0 (`notify`) | 2 |
| CDLA-Permissive-2.0 (`webpki-roots`) | 2 |
| BSD-3-Clause OR Apache-2.0 | 2 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 2 |
| Apache-2.0 OR ISC OR MIT | 2 |
| Apache-2.0 OR BSL-1.0 OR MIT | 2 |
| Apache-2.0 OR BSL-1.0 (`ryu`) | 1 |
| MIT OR Apache-2.0 OR LGPL-2.1-or-later (`r-efi`; used under MIT) | 1 |
| CC0-1.0 OR MIT-0 OR Apache-2.0 (`dunce`) | 1 |
| 0BSD OR MIT OR Apache-2.0 (`adler2`) | 1 |
| BSD-3-Clause OR MIT (`brotli-decompressor`) | 1 |
| Permissive `AND` combinations (MIT/Apache-2.0/BSD-3-Clause/ISC/Unicode-3.0) | 7 |

The MPL-2.0 crates are `symphonia`, `symphonia-core`, `symphonia-metadata`,
`symphonia-utils-xiph`, `symphonia-bundle-flac`, `symphonia-bundle-mp3`,
`symphonia-codec-aac`, `symphonia-codec-pcm`, `symphonia-codec-vorbis`,
`symphonia-format-isomp4`, `symphonia-format-ogg`, `symphonia-format-riff`
(audio decoding, via `rodio`), `cssparser`, `cssparser-macros`,
`dtoa-short`, `selectors` (HTML parsing, via `dom_query`) and `option-ext`
(via `dirs`). They are used unmodified as published on crates.io, where
their source is available; MPL-2.0's file-level copyleft is satisfied by
that unmodified use.

A further 237 platform-specific crates (Linux GTK/WebKit/D-Bus/Wayland,
Windows, Android and wasm targets) were not present in the audit host's
registry and were not read; `deny.toml` covers them when `cargo deny check`
runs on each target. The full per-crate list can be generated at any time
with `cargo license` or `cargo about generate`.

## 8. npm packages (`frontend/`)

69 packages, from `npm ls --all`:

| Declared licence | Packages |
|---|---:|
| MIT | 58 |
| Apache-2.0 | 4 |
| MIT OR Apache-2.0 (`@tauri-apps/api`, `@tauri-apps/plugin-opener`) | 2 |
| ISC | 2 |
| MPL-2.0 (`lightningcss`, `lightningcss-darwin-arm64`) | 2 |
| BSD-3-Clause (`source-map-js`) | 1 |

What ships in the app bundle is `three`, `@tauri-apps/api`,
`@tauri-apps/plugin-opener` and the Svelte runtime (all MIT or
MIT/Apache-2.0). Everything else is build or test tooling; `lightningcss`
(MPL-2.0) is Vite's build-time CSS minifier and nothing of it is
distributed.
