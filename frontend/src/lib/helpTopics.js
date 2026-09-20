// The Help tab's content. Data, not markup, so tests can hold it to
// account: topic ids must be unique (they are deep-link targets for the
// "?" buttons around the app) and `tab` chips must name real tabs.
//
// body entries are paragraphs. Optional per topic:
//   tip:  one highlighted line (rendered as a callout)
//   keys: [["Ctrl+Shift+H", "what it does"], ...]
//   tabs: [["route", "Open the Route tab"], ...] — buttons that jump
//         there; "settings:<section>" opens Settings on that section
//         (sections: computer, data, index, follow, hud, database, setup)
//   legend: true — the route-icon legend (Plotting only)

export const topics = [
  {
    id: "ship-computer",
    title: "Ship computer",
    body: [
      "Ask anything in plain language, by typing or by voice. It answers from your own journal and the community API, tells you where each fact came from, and can act: set pips, drop the gear, open the scoop, target the next system, plot and follow routes.",
      "Provider: Claude, or any OpenAI-compatible service or local server (Mistral, Ollama, LM Studio, OpenRouter, Groq, OpenAI, Grok). Keys are kept in your operating system's credential store, never in a file. Web research (looking up game mechanics and citing sources) is available with Claude only.",
      "Check ship computer runs a set of cockpit questions against the current provider and scores the answers. Nothing is pressed in the game and nothing is spoken while it runs.",
    ],
    tabs: [["settings:computer", "Ship computer settings"]],
  },
  {
    id: "connecting-a-model",
    title: "Connecting a model",
    body: [
      "EDDA works without a model. Connect one and the ship computer answers from your journal and the community data, cites where each fact came from, and can act: routes, pips, bindings, engineering plans, the profit finder, your fleet and carrier. The model runs on your own account and key, or on your own machine; nothing you say to it passes through EDDA's server. The full walkthrough with links is at edda-app.com/ship-computer.",
      "Claude: a key from console.anthropic.com; leave the model blank for the default. The only provider with web research.",
      "OpenAI: a key from platform.openai.com; endpoint https://api.openai.com/v1, default model gpt-4.1-mini. Mistral: console.mistral.ai; https://api.mistral.ai/v1, mistral-small-latest. Groq: console.groq.com; https://api.groq.com/openai/v1, llama-3.3-70b-versatile, very fast. OpenRouter: openrouter.ai; https://openrouter.ai/api/v1, default mistralai/mistral-small-3.2-24b-instruct:free (no cost) - pick models whose page lists tools. Grok: console.x.ai; https://api.x.ai/v1, grok-4.6.",
      "Ollama (local): install it, run `ollama pull qwen3:8b`; endpoint http://localhost:11434/v1, no key. Choose models tagged for tools; qwen3:8b is the smallest that calls them reliably. LM Studio (local): load a model marked for tool use, start the server; http://localhost:1234/v1, the model identifier as LM Studio shows it, no key.",
      "Anything else (Other service / Other local server): it must serve the OpenAI chat-completions API with tool calling - the ship computer answers by calling tools, so a model or server without tool_calls will make things up or fail. Allow at least 8k tokens of context (the tool catalogue alone is about 5k) and, locally, a model of 8B parameters or more.",
      "After connecting, press Check ship computer: a pass means the model used the tools instead of guessing.",
    ],
    tip: "Tool calling is the one thing that is not optional. Check the model, not just the server.",
    tabs: [["settings:computer", "Ship computer settings"]],
  },
  {
    id: "galaxy-data",
    title: "Where the data comes from",
    body: [
      "EDDA runs its searches on the official EDDA community API: route plotting beyond the bubble, trade finding (single legs, round trips and rings), market prices, station and system lookups, and the star field the fuel guard reads. Nothing large downloads and nothing needs syncing; what you see is the server's live picture, with every price carrying its age.",
      "A compact index of the populated bubble ships inside EDDA, so bubble-scale plotting - following a trade loop, for instance - happens on your own machine without a server call. Your journal, ships, marks and materials never leave this computer; the ship computer is where they meet the server's answers.",
    ],
    tip: "Elite is an online game: if the network is down, so is the game. EDDA does not keep a copy of the galaxy for a case that never happens.",
  },
  {
    id: "route-plotting",
    title: "Plotting a route",
    body: [
      "The Route tab plots for the ship you pick — yours by default, with the fuel it has now; another ship is assumed full. A plot answers in seconds. Try harder runs a deeper search and replaces the route only if it finds a strictly better one.",
      "Neutrons, white dwarfs and FSD injections are each opt-in. A neutron star multiplies the jump ×4 (×6 with the Mk II SCO drive); a white dwarf ×1.5 (×3 Mk II SCO) — smaller, but far more common, useful in stretches with no neutron nearby. The plotter reads which drive you have from your loadout. Injections are synthesised from materials and are only planned when nothing else can cross a gap — a route that works without them never gets one.",
      "Each hop shows the fuel it leaves, whether to scoop, and your FSD's integrity: every supercharge costs 1% (the Mk II SCO drive is immune), and the plan assumes you repair when it reaches 81%. A ship without an AFMU gets a warning with the plan.",
      "A ship without a fuel scoop may still plot — but Follow will refuse the route rather than guide you into an empty tank.",
    ],
    legend: true,
    tabs: [["route", "Open the Route tab"]],
  },
  {
    id: "route-following",
    title: "Following a route",
    body: [
      "Plot a route in the Route tab, import one from Spansh, or ask the ship computer. Press Follow and the HUD counts down the jumps; every jump you make advances it.",
      "Say \"Guidance, target next\" to target the next system in the route you are following. For one-button use, capture the same key, stick, or throttle control you use in Elite for Target Next System in Route; one press then keeps Elite and EDDA in step. Other routing orders include \"Guidance, how many jumps left\", \"Guidance, skip\", and \"Guidance, clear the route\".",
      "If you prefer the app to drive the galaxy map for you, switch that on under Settings → Route control and teach it four places once — the map's search box, the first search result, the Target button, and the Plot Route button — by clicking each in the game (setup has a guided version). From then on target next opens the map, pastes the name into the search, clicks the first result, then Target for the next hop on an EDDA route — or Plot Route when Elite should calculate the whole journey — and closes the map. Each step has a wait next to it; raise them if the map is slow on your machine, lower them if it is quick.",
      "The plan is re-checked against your real tank on every jump: a hop that only needs a lighter ship gets a burn-down instruction, anything worse is re-planned, extra fuel clears stops you no longer need, and jumping off the route re-plans from where you are.",
    ],
    tabs: [["route", "Open the Route tab"], ["settings:follow", "Route control settings"], ["voice", "Voice input settings"]],
  },
  {
    id: "trade",
    title: "Trade & market",
    body: [
      "Trade finds profitable runs from wherever you are, on the community API, which hears every price the moment it is broadcast, plus every price you have seen yourself. Market shows the station you are docked at and lets you search any commodity across the galaxy.",
      "Every result can hand its destination straight to the route plotter.",
    ],
    tabs: [["trade", "Open the Trade tab"], ["market", "Open the Market tab"]],
  },
  {
    id: "powerplay",
    title: "Powerplay",
    body: [
      "The Powerplay tab tracks your merits first-hand from the journal — totals, the last 30 days, and a day-by-day chart.",
      "The merit rate table shows how many credits of profit earn one merit at each station you have sold at, learned from your own sales. Control state shows what you have personally witnessed in each system: progress, reinforcement, undermining.",
    ],
    tabs: [["powerplay", "Open the Powerplay tab"]],
  },
  {
    id: "missions",
    title: "Missions",
    body: [
      "Every mission from the journal with its status, hand-in, reward and expiry. The status comes from the game: a massacre, assassination or salvage mission becomes ready to turn in when the game redirects you to the hand-in, and done when you hand it in. A delivery or courier the game redirects has a new drop-off, not a completion, and is announced as a reroute.",
      "Kill counts are not shown, on purpose. The journal only records a kill when your ship finished scanning the target before it died, and never says which missions it counted for, so any count EDDA kept ran behind the game (13 kills in game against 9 in the journal in one afternoon's flying). A massacre shows its target count and its status instead, and the completion is spoken once, from the game's own signal.",
      "The hand-in is the station you accepted the mission at until the game moves it: the destination a kill mission names is where the kills happen, not where it pays. Wing missions carry a wing marker on the tab and the HUD. Stacking mode (a tick on the tab) lists on the HUD every faction you already hold a massacre from against the current target, so you never take a second from the same giver — those progress one after another, not together.",
    ],
    tabs: [["missions", "Open the Missions tab"]],
  },
  {
    id: "engineering",
    title: "Engineering",
    body: [
      "Blueprint costs for every engineerable module, your live material inventory from the journal, and a shopping list that knows material-trader exchange rates (6:1 per grade up, 3:1 per grade down, 6:1 across groups). A plan an unlocked engineer cannot apply yet says so in red. Everything here is local: the bundled blueprint data and your own journal, no server.",
      "Every engineer who works a blueprint is listed with the grade they reach (\"The Dweller · Unlocked · rank 5 · to G4\"), dimmed when that is below the grade you asked for, so an engineer who stops one grade short is never hidden. The engineer table is checked against Inara and the wiki and held by a test.",
      "To plan a whole ship at once — nine hardpoints, every utility — use Plan build on the Ships tab; it uses the same materials, traders and engineers as this tab.",
    ],
    tabs: [["engineering", "Open the Engineering tab"]],
  },
  {
    id: "ships",
    title: "Ships",
    body: [
      "Every ship you have flown, from the journal, with its current build: modules, engineering grades and experimental effects. Plan on any module hands it to the Engineering tab. Open in EDSY / Open in Coriolis opens the site with the build already loaded; Copy build puts it on the clipboard in SLEF for anything else that imports it.",
      "Plan build turns the module table into a plan: a blueprint, target grade and experimental for every engineerable module, \"same for all N\" to give every module of one type the same plan, saved per ship. Fitted engineering continues to the top grade unless you change it; a module at the top grade with no experimental has nothing to do and cannot be ticked. Materials for this build gives one answer for the lot: every material pooled against what you carry, one shopping list for the shortfall with the nearest traders and farm sites, and the fewest engineers to visit with what to bring each. What no unlocked engineer can do at the asked grade is grouped, with who takes it part-way today and who to unlock for the rest.",
      "Import a build: paste an EDSY or Coriolis SLEF export (Export → SLEF on either site) and the plan becomes the difference between your ship and that build — the modules to swap, then every engineering job to reach it, continuing from what is already rolled. Coriolis's JSON export names modules its own way and is refused with the way out.",
      "As flown → with this plan: unladen mass, jump range (full tank, laden, and the single-jump maximum) and power drawn against the plant, retracted and deployed, red when the plan would overdraw. Planned blueprints count at a full roll, as EDSY shows them; the figures are checked against EDSY on real ships to a tenth of a tonne and a hundredth of a light-year. Copy planned build puts the build with every planned blueprint at its target grade on the clipboard as SLEF.",
      "Your fleet carrier has a card above the fleet. What Frontier reports — the hold per commodity, tank, balance, reserved upkeep, orders — needs the Frontier link (Settings → Frontier account, or the setup step); Update now asks Frontier again on the spot. Without the link the card says so: EDDA never guesses a hold from the journal, which cannot see the carrier's own sales or other commanders' transfers.",
    ],
    tabs: [["ships", "Open the Ships tab"], ["engineering", "Open the Engineering tab"], ["settings:frontier", "Frontier account"]],
  },
  {
    id: "voice",
    title: "Voice",
    body: [
      "No neural voice model is bundled. Windows voice works immediately; if you choose a local neural voice, EDDA downloads Piper or Kokoro into your selected data folder. You can also connect another OpenAI-compatible speech server on the Voice tab.",
    ],
    tabs: [["voice", "Voice settings"]],
  },
  {
    id: "voice-input",
    title: "Voice input",
    body: [
      "Say the activation word (\"hey EDDA\") and then your order, or hold the push-to-talk key or joystick button while you speak. Everything is recognised on this machine; no audio leaves it.",
      "Voice-input models are optional and not bundled. The small model is enough for orders; Parakeet is best for free speech. EDDA downloads only the model you explicitly choose, and it works offline afterward.",
      "Short orders are carried out instantly without the ship computer: \"target next\", \"skip\", \"stop following\", \"clear the route\", \"how many jumps left\", \"what's next\", \"repeat\", and cockpit orders like \"full pips to systems\", \"gear down\", \"lights on\". Anything else is asked of the ship computer and the answer is spoken.",
    ],
    tip: "Push-to-talk hotkeys cannot use Shift: the game uses Shift as its UI focus key.",
    tabs: [["voice", "Voice settings"]],
  },
  {
    id: "callouts",
    title: "Callouts",
    body: [
      "Everything the ship computer speaks up about on its own has a kind — hazards, fuel, arrivals, scans, materials, missions, routes, signals — and each can be switched off under Voice → Callouts. A switched-off kind is neither spoken nor shown.",
      "While following a route: the arrival line says whether to supercharge here and whether this is a fuel stop; at a neutron fuel stop it names the companion star to scoop at and how far it is.",
    ],
    tabs: [["voice", "Callout settings"]],
  },
  {
    id: "signal-watch",
    title: "Signal watch",
    body: [
      "Tick the signal sources you care about under Voice → Signal watch — high grade emissions, Power convoy distress signals, non-human signals and so on — and the ship computer announces them when the game reports them on your sensors (once per system) and again, with the threat level, when you drop into one. The game only writes a signal to the journal when it resolves it — often not until you arrive — so the drop is the one moment it can always be called.",
      "By voice: \"I'm looking for high grade emissions\", \"stop looking for pirates\", \"what are we watching for\".",
    ],
    tabs: [["voice", "Signal watch settings"]],
  },
  {
    id: "hud",
    title: "HUD overlay",
    body: [
      "A transparent, always-on-top overlay for the game window: callouts, the route countdown, missions, and the voice conversation. Unlock it from Settings → HUD to move and resize it. The game must run in borderless window mode.",
    ],
    keys: [["Ctrl+Shift+H", "hide or show the HUD"]],
    tabs: [["settings:hud", "HUD settings"]],
  },
  {
    id: "frontier-account",
    title: "Frontier account",
    body: [
      "Some things never reach the journal — your fleet carrier's real hold, tank and balance among them. Link your Frontier account (Settings → Frontier account, or the step in first-run setup) and EDDA reads them from Frontier's own API. The login is Frontier's, in your browser, and comes back to the app; if the browser does not return, paste the address it landed on into the card.",
      "EDDA keeps only a token, in your operating system's credential store, and asks Frontier only when the journal shows carrier activity or you press Update now — never on a timer. Nothing from Frontier is sent to EDDA's servers. The login must be the account this journal belongs to; another account is refused and nothing is kept. Frontier logins last about 25 days, and linking on another PC unlinks this one; the card says when to link again.",
    ],
    tabs: [["settings:frontier", "Frontier account"], ["ships", "Open the Ships tab"]],
  },
  {
    id: "privacy",
    title: "Privacy",
    body: [
      "Your journal never leaves this machine except as questions you choose to send to the ship computer's provider. Voice audio is processed locally. Galaxy data comes from the EDDA community API. Your own EDDN connection keeps it live: market prices, outfitting, shipyards, and system sightings from commanders everywhere.",
      "The Frontier link runs entirely in the app: the token stays on this PC and nothing Frontier reports is sent to EDDA's servers.",
    ],
  },
];

/** The route-table icon legend, shared by Help and (one day) the Route tab. */
export const routeLegend = [
  ["N", "neutron star — supercharge here"],
  ["WD", "white dwarf — smaller supercharge"],
  ["⛽", "fuel stop — scoop before jumping on"],
  ["⚡", "this jump is supercharged"],
  ["⚓", "dock available"],
  ["?", "star class unknown"],
];
