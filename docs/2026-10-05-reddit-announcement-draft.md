# Reddit announcement draft (2026-10-05)

Status: DRAFT for the maintainer to post. Nothing here is published by
the repo. Every number is from a bench CSV or a release note named in
the "Claims and their sources" section at the end; every "other tools
do this too" line is from the overlap research of the same day. Two
lines the maintainer asked for could not be supported and are
rewritten honestly: the memory comparison (see the footprint bench) and
"novel" features that other tools already have.

Suggested subreddit: r/EliteDangerous. Suggested flair: Tool/Resource.

---

## Title

EDDA 0.4: a companion app for Elite that is small, quiet, and does not look like a spreadsheet that has been to space

## Body

You have a problem, and the problem is tabs. You are flying a ship worth 400 million credits through a galaxy that is actively trying to kill you, and to do this safely you have opened Inara, Spansh, EDSM, a Discord, a second Spansh because the first one is the neutron plotter and this one is the trade planner, and a YouTube video from 2019 in which a man with a very calm voice explains core mining. You also have three desktop tools running, each of which was written in a different decade, each of which has a panel, and each panel has a grid, and each grid has 40 columns, and one of the columns is called "Flags".

I wrote EDDA because I wanted a tool that looks the way I want a tool to look in 2026, reads the journal, tells me the things I need to know *out loud* so I can keep my eyes on the thing trying to kill me, and otherwise shuts up. Nothing did, and the alternative was continuing to not have one. It is free, open source (MIT), and it is new: Rust, a native window, no runtime to install first. Download: https://edda-app.com

It runs on Linux. Not "runs on Linux if you have Mono and a free weekend": a .deb, an .rpm and an AppImage that update themselves, journals found under Proton, and the HUD works on Wayland. Windows too, obviously, with a signed installer. No Mac yet; I own one and I am ashamed.

**What it is**

A route planner that plots the whole galaxy, bubble to Colonia to Beagle Point, against its own 200-million-system index on a community server, so nothing large ever downloads to your machine. A 22,000 ly plot comes back in under a second. Every hop is planned against the ship you are actually flying and the fuel you actually have, it knows what an SCO drive does to a neutron star, and if your ship has no scoop it refuses the route instead of guiding you politely into an empty tank. Follow mode targets each jump in game and re-plans when you wander.

A trade finder for the ship in your hangar, ranked by credits per hour, that fences out carrier price games and drops the stations that would confiscate your cargo. Round trips, rings, and since 0.4.1 "sell only to this faction", for those of you grinding reputation the honest way.

A build planner that imports your EDSY or Coriolis build, works out what you still need as a *gap* from the ship you fly, pools the materials against your inventory, and hands you a shopping list and the shortest list of engineers to visit with what to bring to each. Its mass and jump figures are checked against EDSY to a tenth of a tonne, because I have been burned before.

Voice. Arrivals with faction states, valuable scans, kills with the bounty, interdictions, hull, fuel, the unscoopable star you are about to jump to, your pad number. From the journal alone, no model in the loop. It speaks when it matters; otherwise, silence. If you *want* a model in the loop there is a ship computer you can point at Claude, any OpenAI-compatible endpoint, or a local Ollama, with the key kept in your OS credential store and nothing you say to it passing through our server. Voice input is recognised entirely on your machine. No VoiceAttack required, though I have nothing against VoiceAttack and neither should you.

Missions, with the hand-in shown where you actually took the mission, and completions announced once, from the game's own signal. No invented kill counts unless you turn the estimate on, because the journal cannot see a ship that died before you scanned it, and I would rather tell you nothing than tell you a number.

Surface mining for the new Rhino goods: where the mining locations are, counted from detailed surface scans on the public EDDN feed, with the community's own survey of what each ground yields.

A HUD overlay you can lay out per ship. Your fleet carrier's real hold and balance through Frontier's own API, never guessed from the journal. Materials as a trader grid. Engineers directory. Markets with the age on every price.

**What it is not**

Novel, mostly. Spansh plots routes. Inara finds trade loops. EDDI and EDCoPilot talk to you. EDDiscovery has a panel for everything and ODEliteTracker's massacre board is, frankly, richer than mine, and I say so inside the app. If you are happy with your current seven windows, you have my blessing and my envy. EDDA's argument is not that it does something nobody has done. It is that it does the things you actually use, in one window, fast, without eating your machine, and without looking like the inside of a tax return.

Numbers, because I did measure, and I would like you to hold me to them:

- Idle, game closed: 1.2% of one core. On a modern desktop that rounds to nothing.
- Launch to "listening": 4.4 seconds, overlay up in under one.
- Installer: 17 MB. On disk: 46 MB. No galaxy sync, ever.
- Memory: the app itself is under 100 MB. Windows then starts seven Edge processes to draw the window, because that is how Windows draws windows now, and the whole affair comes to about 600 MB, which is still less than the four Chrome tabs you have open, by a margin I would describe as "measured". If you turn on the neural voice it is about 800 MB, because a neural voice is a neural network and it lives in your RAM now. That is the price of the nice voice and I will not pretend otherwise.

**What it does with your data**

EDDA is not a surveillance tool, and that sentence is written into the project's own rules. Your journal never leaves your machine. There is no account. The galaxy data comes from the public EDDN feed that every tool already shares, plus the Spansh dumps, digested on a server I run so your install stays small. Usage telemetry is on by default and is a closed list of timings and error counts with no names, no positions and no journal content; it is one checkbox to turn off, and the full list is on the privacy page. We currently only *listen* to EDDN; sending your market visits back to the commons is being built and will be opt-in.

Bugs go to the Report tab in the app or to GitHub. It is a fan project, not affiliated with Frontier, built with love and, I am told, an unhealthy number of benchmarks.

o7

---

## Claims and their sources (for the maintainer; do not post)

| Claim in the post | Source |
|---|---|
| 1.2% of one core at idle, game closed | `docs/benches/2026-10-05-app-footprint.csv` (1.17%, 60 s average, boss's box, 0.4.2) |
| 4.4 s launch to listening, overlay under 1 s | same CSV (overlay +0.84 s, galaxy index +1.69 s, listening +4.41 s) |
| 17 MB installer, 46 MB on disk | same CSV (17.3 MB, 46 MB) |
| app itself under 100 MB; whole app ~600 MB; less than four Chrome tabs | same CSV, SAPI voice, listening off, idle: edda.exe 138 MB WS / 89 MB private; with the 7 WebView2 children 605-619 MB WS / 331-366 MB private; four average Chrome renderers on the same box ~665 MB WS / 699 MB private. Holds clearly on private bytes, narrowly on working set. Task Manager files the WebView2 processes under Edge, which is why the maintainer sees 65-85 MB |
| ~800 MB with the neural voice | same CSV: 799 MB working set with Kokoro + Parakeet loaded |
| "less memory than 4 Chrome tabs" | holds with the standard voice (above); does NOT hold with the neural voice + speech model (799 MB WS / 1,846 MB private), which the post says. In-game memory is not measured |
| 22,000 ly in under a second | site/index.html "0.7 s to plot 22,000 ly"; `docs/benches/2026-09-09-galos-index-spike.csv` 141 jumps in 1.14 s (server-side figures) |
| 200-million-system index | README; site "199,000,000 charted systems" |
| mass/jump checked against EDSY to a tenth of a tonne | RELEASE-NOTES 0.3.5; `docs/benches/2026-09-19-ship-physics-phase1-vs-edsy.csv` |
| completions from the game's signal, estimate off by default | helpTopics "missions"; RELEASE-NOTES 0.3.5/0.4.2; `docs/benches/2026-09-19-mission-kill-credit-at-redirect.csv` |
| surface mining from EDDN DSS scans + survey | RELEASE-NOTES 0.3.5 |
| telemetry on by default, closed allowlist, one checkbox | site/privacy/index.html |
| listens to EDDN, does not upload (yet) | site/privacy; Waldorf's upload spec `docs/superpowers/specs/2026-09-30-eddn-upload-design.md` |
| ODEliteTracker's board is richer, credited in-app | helpTopics "missions"; `docs/2026-09-20-odelitetracker-mission-gap-review.md` |
| Linux native packages with updater, Proton journals, Wayland HUD | RELEASE-NOTES 0.3.1; site; README Linux notes. Overlap research: EDMC runs on Linux from source, EDDiscovery under Mono "with limitations", Observatory under Wine; the rest Windows-only or unstated |
| "no Mac yet" | the updater manifest serves windows-x86_64 and linux-x86_64 only (2026-10-05); the Mac build is the maintainer's local script, unpublished |

Things deliberately NOT claimed as novel (other tools have them): route plotting incl. neutron/SCO (Spansh, EDDiscovery, EDCoPilot), trade loops with profit/h and carrier filters (Inara, Spansh), voice callouts (EDDI, EDCoPilot, EDDiscovery, Observatory), LLM copilots (EDCoPilot + ChatGPT, COVAS:NEXT), overlays (EDMC overlay plugins, EDCoPilot incl. VR, ODET), cAPI carrier data (EDDiscovery, EDMC, Inara), engineering shopping lists (EDSY, Coriolis, EDO Materials Helper, EDDiscovery), massacre stacking (ODET, EDMC-Massacres), EDSY/Coriolis export (EDMC, EDDiscovery). Not claimed at all: a local-data mode (not in the public repo), VR overlay (EDCoPilot has it, we do not).
