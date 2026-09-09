<script>
  import { onMount, onDestroy } from "svelte";
  import { routing, showFromAi, start as startRouting, stop as stopRouting } from "./lib/route.svelte.js";
  import { start as startFollow, stop as stopFollow } from "./lib/follow.svelte.js";
  import { start as startShip, stop as stopShip } from "./lib/ship.svelte.js";
  import { KEYS, readKey, writeKey, removeKey } from "./lib/storage.svelte.js";
  import { useListeners } from "./lib/lifecycle.svelte.js";
  import StatusPanel from "./lib/StatusPanel.svelte";
  import CalloutFeed from "./lib/CalloutFeed.svelte";
  import TradePanel from "./lib/TradePanel.svelte";
  import MarketPanel from "./lib/MarketPanel.svelte";
  import MiningPanel from "./lib/MiningPanel.svelte";
  import CombatPanel from "./lib/CombatPanel.svelte";
  import GalaxyPanel from "./lib/GalaxyPanel.svelte";
  import PowerplayPanel from "./lib/PowerplayPanel.svelte";
  import InventoryPanel from "./lib/InventoryPanel.svelte";
  import VoiceSettings from "./lib/VoiceSettings.svelte";
  import SystemSettings from "./lib/SystemSettings.svelte";
  import ReportPanel from "./lib/ReportPanel.svelte";
  import MissionsPanel from "./lib/MissionsPanel.svelte";
  import RoutePanel from "./lib/RoutePanel.svelte";
  import AiChat from "./lib/AiChat.svelte";
  import HelpPanel from "./lib/HelpPanel.svelte";
  import { help as helpStore } from "./lib/help.svelte.js";
  import ShipsPanel from "./lib/ShipsPanel.svelte";
  import Onboarding from "./lib/Onboarding.svelte";
  import WhatsNew from "./lib/WhatsNew.svelte";
  import { releaseNotesGet } from "./lib/api.js";
  // The post-update splash: shown once per version, never over the
  // onboarding flow (a fresh install meets Setup first; the notes wait).
  let whatsNew = $state(null);
  import { onSyncProgress, onSyncComplete, onGameState, gameState, getStatus, say, onSpeechEngineProgress, onListenSetup, onListenReply, onKnowledgeProgress, onAppUpdate } from "./lib/api.js";
  import { trade, start as startTrade, stop as stopTrade } from "./lib/trade.svelte.js";
  import { start as startTradeFollowStore, stop as stopTradeFollowStore } from "./lib/tradeFollow.svelte.js";

  const tabs = [
    ["setup", "Setup"],
    ["route", "Route"],
    ["galaxy", "Galaxy"],
    ["market", "Market"],
    ["trade", "Trade"],
    ["mining", "Mining"],
    ["missions", "Missions"],
    ["inventory", "Inventory"],
    ["combat", "Combat"],
    ["powerplay", "Powerplay"],
    ["ships", "Ships"],
    ["voice", "Voice"],
    ["settings", "Settings"],
    ["report", "Report"],
    ["help", "Help"],
  ];
  let tab = $state("trade");
  let setupVisible = $state(true);
  const visibleTabs = $derived(tabs.filter(([k]) => k !== "setup" || setupVisible));
  // Other tabs can ask for the Route tab (e.g. "route to this trader").
  $effect(() => { if (routing.openTab) { pick("route"); routing.openTab = false; } });
  // A "?" anywhere asks for a Help topic.
  $effect(() => { if (helpStore.requested) pick("help"); });
  let sync = $state(null); // {file, done, total}
  let synced = $state(false);
  let game = $state(null); // {running}
  let speechJobTimer;
  let speechJob = $state(null);
  // Self-update lifecycle from the backend watch: available → (install
  // happens in Settings) → ready-to-restart. The pill only points there.
  let appUpdatePill = $state(null);
  let recognitionJobs = $state({});
  let knowledgeJob = $state(null);
  const listeners = useListeners();
  let logoClicks = 0;
  let friendshipDriveReadyAt = 0;
  let nameClicks = 0;
  let alternateName = $state(false);

  function logoEgg() {
    if (Date.now() < friendshipDriveReadyAt) return;
    if (++logoClicks >= 7) {
      logoClicks = 0;
      friendshipDriveReadyAt = Date.now() + 60_000;
      say("Friendship drive charging.").catch(() => {});
    }
  }

  function nameEgg() {
    if (++nameClicks >= 5) {
      nameClicks = 0;
      alternateName = true;
      setTimeout(() => (alternateName = false), 5000);
    }
  }

  {
    const onboardingComplete = readKey(KEYS.onboardingComplete, false);
    setupVisible = !onboardingComplete;
    const saved = readKey(KEYS.tab, "");
    if (!onboardingComplete) tab = "setup";
    else if (saved && saved !== "setup" && tabs.some(([k]) => k === saved)) tab = saved;
  }

  function pick(k) {
    tab = k;
    writeKey(KEYS.tab, k);
  }

  function finishOnboarding() {
    writeKey(KEYS.onboardingComplete, true); removeKey(KEYS.onboardingStep);
    setupVisible = false;
    pick("market");
  }

  function repeatOnboarding() {
    removeKey(KEYS.onboardingComplete); removeKey(KEYS.onboardingStep);
    setupVisible = true;
    pick("setup");
  }

  const pct = (value) => value == null ? "" : ` · ${Math.max(0, Math.min(100, value * 100)).toFixed(0)}%`;
  const mb = (bytes) => `${(Number(bytes || 0) / 1048576).toFixed(0)} MB`;

  onMount(async () => {
    // Shared stores subscribe to the backend here, not at import.
    startRouting(); startFollow(); startTrade(); startShip(); startTradeFollowStore();
    // .catch(() => {}) here meant a splash that never appeared was
    // indistinguishable from one correctly suppressed. Log it: the
    // console is the only witness a release build leaves us.
    releaseNotesGet()
      .then((n) => { if (n.unseen && tab !== "setup") whatsNew = n; })
      .catch((e) => console.error("release notes unavailable:", e));
    listeners.add(onSyncProgress((e) => (sync = e.payload)));
    listeners.add(onSyncComplete(() => {
      synced = true;
      sync = null;
    }));
    // The first sync often finishes before this window is listening: ask.
    try { const st = await getStatus(); if (st?.location || st?.commander) synced = true; } catch {}
    listeners.add(onGameState((e) => (game = e.payload)));
    try { game = await gameState(); } catch {}
    // "ready" is the terminal phase (not "installed", which "starting"
    // follows immediately): it lingers briefly and clears, and an error
    // lingers longer so it can actually be read — a pill must neither
    // vanish mid-thought nor stand at 100% forever (field, 2026-09-04).
    listeners.add(onAppUpdate((e) => {
      const p = e.payload;
      if (p.phase === "available") appUpdatePill = `Update ${p.version} available`;
      else if (p.phase === "ready") appUpdatePill = `Restart to update to ${p.version}`;
      else if (p.phase === "downloading") appUpdatePill = "Downloading update…";
    }));
    listeners.add(onSpeechEngineProgress((e) => {
      const p = e.payload;
      clearTimeout(speechJobTimer);
      if (p.phase === "installed") return;
      speechJob = p;
      if (p.phase === "ready" || p.phase === "error") {
        speechJobTimer = setTimeout(() => (speechJob = null), p.phase === "ready" ? 5_000 : 12_000);
      }
    }));
    listeners.add(onListenSetup((e) => {
      const p = e.payload;
      if (p.phase === "done") recognitionJobs = {};
      else recognitionJobs = { ...recognitionJobs, [p.phase]: p };
    }));
    listeners.add(onKnowledgeProgress((e) => (knowledgeJob = e.payload.swept >= e.payload.cells ? null : e.payload)));
    // A route or trade report the ship computer produced BY VOICE lands in
    // the tabs exactly like a chat answer would (AiChat does the same for
    // typed questions) — a plotted route the commander cannot see is not
    // plotted.
    listeners.add(onListenReply((e) => {
      if (e.payload.route) showFromAi(e.payload.route);
      if (e.payload.profit) { trade.report = e.payload.profit; trade.error = ""; }
    }));
  });
  onDestroy(() => {
    clearTimeout(speechJobTimer);
    stopRouting(); stopFollow(); stopTrade(); stopShip(); stopTradeFollowStore();
  });
</script>

<main>
  <header>
    <h1><button class="wordmark" onclick={logoEgg} aria-label="EDDA">EDDA</button> <button class="sub expansion" onclick={nameEgg} title="Elite Dangerous Desktop Aid">{alternateName ? "Extremely Dependable Digital Assistant" : "Elite Dangerous Desktop Aid"}</button></h1>
    <nav>
      {#each visibleTabs as [k, label]}
        <button class="tab {tab === k ? 'active' : ''}" onclick={() => pick(k)}>{label}</button>
      {/each}
    </nav>
    <div class="status-line">
      {#if sync}
        <span class="pill warn">Syncing journal {sync.done}/{sync.total}</span>
      {:else if synced}
        <span class="pill ok">Journal live</span>
      {:else}
        <span class="pill">Starting…</span>
      {/if}
      {#if trade.loading}
        <button class="pill accent" onclick={() => pick("trade")} title="Profit finder running">Searching trade… {(trade.elapsedMs / 1000).toFixed(0)}s</button>
      {/if}
      {#if routing.loading}
        <button class="pill accent" onclick={() => pick("route")} title="Route plot running">Plotting… {(routing.elapsedMs / 1000).toFixed(0)}s</button>
      {/if}
      {#if speechJob}
        <button class="pill accent" onclick={() => pick(setupVisible ? "setup" : "voice")} title={speechJob.detail}>Kokoro · {speechJob.detail}{pct(speechJob.fraction)}</button>
      {/if}
      {#each Object.values(recognitionJobs) as job}
        <button class="pill accent" onclick={() => pick(setupVisible ? "setup" : "voice")} title="Voice-recognition model setup">{job.phase} · {job.status}{job.total ? ` · ${mb(job.bytes)} / ${mb(job.total)}${pct(job.bytes / job.total)}` : ""}</button>
      {/each}
      {#if knowledgeJob}
        <button class="pill accent" onclick={() => pick("settings")} title="Galaxy knowledge sweep">Knowledge index · {knowledgeJob.swept.toLocaleString()} / {knowledgeJob.cells.toLocaleString()}</button>
      {/if}
      {#if appUpdatePill}
        <button class="pill cyan" onclick={() => pick("settings")} title="An EDDA update is ready in Settings — nothing installs without you">{appUpdatePill}</button>
      {/if}
      {#if game}
        <span class="pill {game.running ? 'ok' : ''}" title="Elite Dangerous process">{game.running ? "Game running" : "Game not running"}</span>
      {/if}
    </div>
  </header>

  <div class="grid">
    <aside class="left">
      <StatusPanel />
      <CalloutFeed />
    </aside>

    <section class="center">
      {#if tab === "setup"}<Onboarding open={pick} finish={finishOnboarding} />
      {:else if tab === "trade"}<TradePanel />
      {:else if tab === "market"}<MarketPanel />
      {:else if tab === "mining"}<MiningPanel />
      {:else if tab === "combat"}<CombatPanel />
      {:else if tab === "missions"}<MissionsPanel />
      {:else if tab === "route"}<RoutePanel />
      {:else if tab === "galaxy"}<GalaxyPanel />
      {:else if tab === "powerplay"}<PowerplayPanel />
      {:else if tab === "inventory"}<InventoryPanel />
      {:else if tab === "ships"}<ShipsPanel />
      {:else if tab === "voice"}<VoiceSettings />
      {:else if tab === "settings"}<SystemSettings onSetup={repeatOnboarding} />
      {:else if tab === "report"}<ReportPanel />
    {:else if tab === "help"}
      <HelpPanel open={pick} />
      {/if}
    </section>

    <aside class="right">
      <AiChat />
    </aside>
  </div>
{#if whatsNew}
  <WhatsNew notes={whatsNew} latestOnly onclose={() => (whatsNew = null)} />
{/if}
</main>

<style>
  main {
    height: 100vh;
    display: flex;
    flex-direction: column;
    padding: 0.75rem 0.9rem;
    gap: 0.7rem;
  }
  header {
    display: flex;
    align-items: center;
    gap: 1.2rem;
    flex-wrap: wrap;
  }
  header h1 {
    margin: 0;
    font-size: 1.05rem;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--accent);
    font-weight: 700;
  }
  .wordmark, .expansion { appearance: none; border: 0; background: transparent; padding: 0; font: inherit; color: inherit; letter-spacing: inherit; text-transform: inherit; cursor: default; }
  .expansion { color: var(--muted); }
  nav { display: flex; gap: 0.2rem; }
  .tab {
    background: transparent;
    color: var(--muted);
    border: 1px solid transparent;
    font-weight: 500;
    padding: 0.3rem 0.7rem;
  }
  .tab.active {
    color: var(--accent);
    border-color: var(--line);
    background: var(--panel);
  }
  .tab:hover { color: var(--text); filter: none; }
  .status-line { margin-left: auto; display: flex; gap: 0.4rem; }
  .grid {
    display: grid;
    grid-template-columns: 300px minmax(0, 1fr) 360px;
    gap: 0.7rem;
    flex: 1;
    min-height: 0;
  }
  .left, .center, .right {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    min-height: 0;
    overflow-y: auto;
  }
  @media (max-width: 1100px) {
    .grid { grid-template-columns: 260px minmax(0, 1fr); }
    .right { grid-column: 1 / -1; max-height: 40vh; }
  }
</style>
