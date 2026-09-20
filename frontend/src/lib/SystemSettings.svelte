<script>
  // The Settings tab: ship computer (with EDDA's own updates), route
  // control, HUD, database, setup. Route control is its own component;
  // the rest is small enough to live here.
  let { onSetup = () => {} } = $props();
  import { onMount } from "svelte";
  import FrontierLink from "./FrontierLink.svelte";
  import { ship } from "./ship.svelte.js";
  import { SECTIONS as HUD_SECTIONS, PRESETS, normalise as normaliseLayout, move as moveSection, toggleHidden, toggleCompact, preset as hudPreset } from "./hudLayout.js";
  // One section at a time: seven panels stacked was a wall.
  const SECTIONS = [
    ["computer", "Ship computer"],
    ["follow", "Route control"],
    ["hud", "HUD"],
    ["database", "Database"],
    ["frontier", "Frontier account"],
    ["setup", "Setup"],
  ];
  import { capiStatus, capiLinkStart, capiLinkCode, capiUnlink, capiRefreshCarrier, onCapiState } from "./api.js";
  import { getAiConfig, setAiConfig, aiEval, dbStats, vacuum, syncNow, overlayVisible, setOverlayInteractive, telemetryPrefs, telemetryPrefsSet } from "./api.js";

  // Anonymous usage data — the maintainer's opt-out consent (2026-09-05).
  let telemetry = $state(true);
  onMount(async () => { try { telemetry = await telemetryPrefs(); } catch {} });
  async function setTelemetry(enabled) {
    telemetry = enabled;
    try { await telemetryPrefsSet(enabled); } catch {}
  }
  import { fmtInt } from "./format.js";
  import { KEYS, readKey, writeKey, persisted } from "./storage.svelte.js";
  // HUD look: shared with the overlay window over the storage bus.
  const hudAlpha = persisted(KEYS.hudAlpha, 1, { json: true, sync: true });
  const hudScale = persisted(KEYS.hudScale, 1, { json: true, sync: true });
  // Customisable HUD (maintainer, 2026-09-20): sections shown, order,
  // compact; presets; a layout remembered per ship. Same bus as the sliders.
  const hudLayout = persisted(KEYS.hudLayout, null, { json: true, sync: true });
  const hudLayoutByShip = persisted(KEYS.hudLayoutByShip, {}, { json: true, sync: true });
  const shipLayout = $derived(ship.currentId != null ? (hudLayoutByShip.value ?? {})[String(ship.currentId)] ?? null : null);
  // What the editor edits: the ship's own layout while one is remembered, else the global one.
  const editing = $derived(normaliseLayout(shipLayout ?? hudLayout.value));
  function setLayout(next) {
    if (shipLayout && ship.currentId != null) hudLayoutByShip.value = { ...(hudLayoutByShip.value ?? {}), [String(ship.currentId)]: next };
    else hudLayout.value = next;
  }
  const shipLabel = $derived(ship.current ? (ship.current.name ? `${ship.current.name} (${ship.current.ship})` : ship.current.ship) : null);
  function rememberForShip() { if (ship.currentId != null) hudLayoutByShip.value = { ...(hudLayoutByShip.value ?? {}), [String(ship.currentId)]: editing }; }
  function forgetForShip() { if (ship.currentId == null) return; const m = { ...(hudLayoutByShip.value ?? {}) }; delete m[String(ship.currentId)]; hudLayoutByShip.value = m; }
  // "index" and "data" were retired (API-only client, 2026-09-07); a stored
  // pick of either lands on the ship computer, where app updates now live.
  const storedSec = readKey(KEYS.settingsSection, "computer");
  let sec = $state(storedSec === "index" || storedSec === "data" ? "computer" : storedSec);
  $effect(() => writeKey(KEYS.settingsSection, SECTIONS.some(([k]) => k === sec) ? sec : "computer"));
  import AppUpdate from "./AppUpdate.svelte";
  import RouteControlSettings from "./RouteControlSettings.svelte";

  let db = $state(null);
  let ai = $state(null);
  // Where the ship computer's model runs. Claude uses its own API; everything else speaks the OpenAI chat API.
  const SERVICES = [
    { id: "lmstudio", kind: "local", label: "LM Studio", base: "http://localhost:1234/v1", model: "", key: false },
    { id: "ollama", kind: "local", label: "Ollama", base: "http://localhost:11434/v1", model: "qwen3:8b", key: false },
    { id: "customlocal", kind: "local", label: "Other local server", base: "", model: "", key: false },
    { id: "anthropic", kind: "remote", label: "Claude", base: "", model: "", key: true },
    { id: "mistral", kind: "remote", label: "Mistral", base: "https://api.mistral.ai/v1", model: "mistral-small-latest", key: true },
    { id: "openrouter", kind: "remote", label: "OpenRouter", base: "https://openrouter.ai/api/v1", model: "mistralai/mistral-small-3.2-24b-instruct:free", key: true },
    { id: "groq", kind: "remote", label: "Groq", base: "https://api.groq.com/openai/v1", model: "llama-3.3-70b-versatile", key: true },
    { id: "openai", kind: "remote", label: "OpenAI", base: "https://api.openai.com/v1", model: "gpt-4.1-mini", key: true },
    // xAI serves the OpenAI chat API (with tool calling) beside its own Responses API; the ship computer speaks the former.
    { id: "xai", kind: "remote", label: "Grok (xAI)", base: "https://api.x.ai/v1", model: "grok-4.6", key: true },
    { id: "custom", kind: "remote", label: "Other service", base: "", model: "", key: true },
  ];
  let aiKind = $state("remote"); let service = $state("anthropic"); let oaBase = $state(""); let oaModel = $state(""); let oaKey = $state("");
  const svc = () => SERVICES.find((x) => x.id === service);
  function kindChanged() { service = aiKind === "local" ? "lmstudio" : "anthropic"; applyService(); }
  function applyService() { const p = svc(); if (p && !p.id.startsWith("custom")) { oaBase = p.base; oaModel = p.id === "anthropic" ? (ai?.model ?? "") : p.model; } }
  function loadProvider() {
    if (ai.provider === "anthropic") { aiKind = "remote"; service = "anthropic"; oaModel = ai.model ?? ""; return; }
    oaBase = ai.openai_base_url ?? ""; oaModel = ai.openai_model ?? "";
    const hit = SERVICES.find((p) => p.base && oaBase.startsWith(p.base));
    service = hit?.id ?? (/localhost|127\.0\.0\.1/.test(oaBase) ? "customlocal" : "custom");
    aiKind = svc()?.kind ?? "remote";
  }
  async function saveProvider() {
    try {
      if (service === "anthropic") {
        ai = await setAiConfig(oaKey ? oaKey : null, oaModel, null, { provider: "anthropic" });
        aiMsg = "Ship computer now uses Claude.";
      } else {
        ai = await setAiConfig(null, null, null, { provider: "openai", openaiBaseUrl: oaBase, openaiModel: oaModel, openaiKey: oaKey ? oaKey : null });
        aiMsg = `Ship computer now uses ${oaModel || "(no model)"} at ${oaBase}.`;
      }
      oaKey = "";
    } catch (e) { aiMsg = String(e); }
  }
  async function clearProviderKey() {
    try { ai = service === "anthropic" ? await setAiConfig("", null) : await setAiConfig(null, null, null, { openaiKey: "" }); aiMsg = "Key removed."; } catch (e) { aiMsg = String(e); }
  }

  let msg = $state("");
  let aiMsg = $state("");
  let busy = $state(false);

  onMount(async () => {
    try { db = await dbStats(); } catch (e) { msg = String(e); }
    try { ai = await getAiConfig(); loadProvider(); } catch (e) { aiMsg = String(e); }
  });

  async function toggleResearch(e) {
    try {
      ai = await setAiConfig(null, null, e.target.checked);
      aiMsg = ai.research ? "Research on: mechanics questions are searched and cited." : "Research off: no web tools are sent to the model.";
    } catch (err) { aiMsg = String(err); }
  }

  let evalReport = $state(null); let evalBusy = $state(false);
  async function runEval() {
    evalBusy = true; evalReport = null;
    try { evalReport = await aiEval(); } catch (e) { aiMsg = String(e); } finally { evalBusy = false; }
  }

  let confirmVacuum = $state(false);
  async function runVacuum() {
    if (!confirmVacuum) { confirmVacuum = true; return; }
    confirmVacuum = false;
    busy = true;
    msg = "Compacting…";
    try { msg = await vacuum(); db = await dbStats(); } catch (e) { msg = String(e); } finally { busy = false; }
  }

  async function resync() {
    try { msg = await syncNow(); } catch (e) { msg = String(e); }
  }
</script>

<nav class="subnav">
  {#each SECTIONS as [k, label]}
    <button class:active={sec === k} onclick={() => (sec = k)}>{label}</button>
  {/each}
</nav>

{#if sec === "setup"}
<section class="panel">
  <h2>Setup</h2>
  <p class="muted small">Review data, voice, controls, routing, and Galaxy Map setup again.</p>
  <button class="quiet" onclick={onSetup}>Perform setup steps again</button>
</section>
{/if}

{#if sec === "computer"}
<section class="panel">
  <h2>Ship computer</h2>
  {#if ai}
    <dl class="kv">
      <dt>Runs</dt>
      <dd>
        <div class="row">
          <label><input type="radio" bind:group={aiKind} value="local" onchange={kindChanged} /> locally, on this machine</label>
          <label><input type="radio" bind:group={aiKind} value="remote" onchange={kindChanged} /> remotely, as a service</label>
        </div>
        <div class="row" style="margin-top:0.4rem">
          <select bind:value={service} onchange={applyService}>{#each SERVICES.filter((p) => p.kind === aiKind) as p}<option value={p.id}>{p.label}</option>{/each}</select>
          {#if service !== "anthropic"}<input placeholder="address, e.g. http://localhost:1234/v1" bind:value={oaBase} style="min-width:20rem" />{/if}
          <input placeholder={service === "anthropic" ? "claude-sonnet-5 (default)" : "model"} bind:value={oaModel} style="min-width:14rem" />
        </div>
        {#if svc()?.key}
          {@const has = service === "anthropic" ? ai.has_key : ai.openai_has_key}
          {@const hint = service === "anthropic" ? ai.key_hint : ai.openai_key_hint}
          <div class="row" style="margin-top:0.4rem">
            {#if has}<span class="pill ok">key ends {hint}</span>{:else}<span class="pill warn">no key</span>{/if}
            <input type="password" placeholder={has ? "replace key" : "API key"} bind:value={oaKey} style="min-width:22rem" autocomplete="off" />
            {#if has && !(service === "anthropic" && ai.env_override)}<button class="quiet" onclick={clearProviderKey}>Remove key</button>{/if}
          </div>
        {/if}
        <div class="row" style="margin-top:0.4rem"><button onclick={saveProvider}>Use this</button>
          <span class="muted small">now: {ai.provider === "anthropic" ? `Claude${ai.model ? ` · ${ai.model}` : ""}` : `${ai.openai_model ?? ""} at ${ai.openai_base_url ?? ""}`}</span></div>
      </dd>
      <dt>Research</dt>
      <dd>
        <label><input type="checkbox" checked={ai.research} onchange={toggleResearch} /> look up game mechanics on the web and cite sources (Claude only)</label>
      </dd>
    </dl>
    <div class="row" style="margin-top:0.6rem">
      <button class="quiet" onclick={runEval} disabled={evalBusy}>{evalBusy ? "Checking…" : "Check ship computer"}</button>
      <span class="muted small">a few minutes</span>
    </div>
    {#if evalReport}
      <p class="small" style="margin:0.4rem 0 0"><b>{evalReport.passed}/{evalReport.total}</b> passed on {evalReport.model} ({evalReport.provider})</p>
      <ul class="small" style="margin:0.2rem 0 0; padding-left:1.2rem">
        {#each evalReport.results as r}<li><span class={r.pass ? "ok" : "warn"}>{r.pass ? "✓" : "✗"}</span> {r.id}: {r.answer.slice(0, 160)}{#if r.failures.length} <span class="muted">— {r.failures.join("; ")}</span>{/if}</li>{/each}
      </ul>
    {/if}
    {#if aiMsg}<p class="muted small" style="margin:0.3rem 0 0">{aiMsg}</p>{/if}
  {/if}
</section>
<AppUpdate />
{/if}

{#if sec === "hud"}
<section class="panel">
  <h2>HUD overlay</h2>
  <p class="muted small">Unlock here, then drag to move and pull the ◢ corner to resize · <strong>Ctrl+Shift+H</strong> hide/show · game in borderless window mode</p>
  <div class="row">
    <button class="quiet" onclick={() => overlayVisible(true)}>Show</button>
    <button class="quiet" onclick={() => overlayVisible(false)}>Hide</button>
    <button class="quiet" onclick={() => setOverlayInteractive(true)}>Unlock for moving</button>
    <button class="quiet" onclick={() => writeKey(KEYS.pinnedLoop, null, { json: true })}>Clear pinned loop</button>
  </div>
  <!-- Live over the storage bus: the HUD window applies these as the
       sliders move, no restart, no backend round trip. -->
  <div class="row" style="margin-top:0.8rem; gap:1.4rem; align-items:center">
    <label>Background
      <span class="inline">
        <input type="range" min="0" max="100" step="5" value={Math.round(hudAlpha.value * 100)}
          oninput={(e) => (hudAlpha.value = Number(e.target.value) / 100)} style="width:9rem" aria-label="HUD background opacity" />
        <span class="muted">{Math.round(hudAlpha.value * 100)}%</span>
      </span>
    </label>
    <label>Text size
      <span class="inline">
        <input type="range" min="70" max="150" step="5" value={Math.round(hudScale.value * 100)}
          oninput={(e) => (hudScale.value = Number(e.target.value) / 100)} style="width:9rem" aria-label="HUD content scale" />
        <span class="muted">{Math.round(hudScale.value * 100)}%</span>
      </span>
    </label>
    <button class="ghost" onclick={() => { hudAlpha.value = 1; hudScale.value = 1; }}>Reset</button>
  </div>
  <p class="muted small" style="margin:0.3rem 0 0">Background fades the panel, never the text — callouts keep their shadow and stay readable over the game.</p>

  <h3 style="margin-top:1rem">Layout <span class="muted small">what the HUD shows, in what order · live, no restart</span></h3>
  <div class="row small" style="gap:0.4rem; flex-wrap:wrap; margin-bottom:0.5rem">
    <span class="muted">Presets</span>
    {#each Object.keys(PRESETS) as name}
      <button class="quiet" onclick={() => setLayout(hudPreset(name))}>{name === "default" ? "Default" : name[0].toUpperCase() + name.slice(1)}</button>
    {/each}
  </div>
  <div class="table-wrap">
    <table class="layout">
      <thead><tr><th></th><th>Section</th><th>Show</th><th>Compact</th><th></th></tr></thead>
      <tbody>
        {#each editing.order as id, i (id)}
          {@const s = HUD_SECTIONS.find((x) => x.id === id)}
          <tr class={editing.hidden.includes(id) ? "dim" : ""}>
            <td class="muted num">{i + 1}</td>
            <td>{s?.label ?? id}</td>
            <td><input type="checkbox" checked={!editing.hidden.includes(id)} onchange={() => setLayout(toggleHidden(editing, id))} aria-label={`Show ${s?.label ?? id}`} /></td>
            <td><input type="checkbox" checked={editing.compact.includes(id)} onchange={() => setLayout(toggleCompact(editing, id))} aria-label={`Compact ${s?.label ?? id}`} /></td>
            <td class="row" style="gap:0.2rem">
              <button class="quiet small" disabled={i === 0} onclick={() => setLayout(moveSection(editing, id, -1))} title="Move up">▲</button>
              <button class="quiet small" disabled={i === editing.order.length - 1} onclick={() => setLayout(moveSection(editing, id, 1))} title="Move down">▼</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <div class="row small" style="gap:0.6rem; flex-wrap:wrap; margin-top:0.5rem">
    {#if shipLabel}
      {#if shipLayout}
        <span class="pill ok">Layout remembered for {shipLabel}</span>
        <button class="quiet" onclick={forgetForShip}>Forget it (use the shared layout)</button>
      {:else}
        <span class="muted">Editing the shared layout, used by every ship.</span>
        <button class="quiet" onclick={rememberForShip}>Remember this layout for {shipLabel}</button>
      {/if}
    {:else}
      <span class="muted">Editing the shared layout. A ship-specific one can be remembered once EDDA knows which ship you are flying.</span>
    {/if}
  </div>
</section>
{/if}

{#if sec === "follow"}<RouteControlSettings />{/if}

{#if sec === "frontier"}
<section class="panel">
  <h2>Frontier account <span class="sub">optional · your carrier's real hold, tank and balance</span></h2>
  <p class="muted small">Linking signs you in to Frontier in your browser and lets EDDA read what Frontier reports about your fleet carrier — the hold per commodity, the tank, the balance, buy and sell orders — which the journal cannot see. Everything fetched stays on this PC; EDDA's own server never holds a token or sees a byte of it. Frontier logins last about 25 days, and linking on another PC unlinks this one; the card says when to link again.</p>
  <FrontierLink />
</section>
{/if}

{#if sec === "database"}
<section class="panel">
  <h2>Database</h2>
  {#if db}
    <dl class="kv">
      <dt>Journal DB</dt><dd class="small">{db.path} · <span class="num">{(db.journal_bytes / 1073741824).toFixed(2)} GiB</span></dd>
      <dt>Total on disk</dt><dd class="num">{(db.total_bytes / 1073741824).toFixed(2)} GiB</dd>
      <dt>Journal events</dt><dd class="num">{fmtInt(db.events)}</dd>
      <dt>Combat</dt><dd class="num">{fmtInt(db.kills)} kills · {fmtInt(db.merit_awards)} merit awards</dd>
    </dl>
  {/if}
  <div class="row" style="margin-top:0.6rem">
    <button class="quiet" onclick={resync}>Re-sync journal now</button>
    <button class="quiet" onclick={runVacuum} disabled={busy}>Compact journal DB</button>
    {#if confirmVacuum}
      <span class="small warn">Compacts the journal database. <button onclick={runVacuum}>Compact now</button> <button class="ghost" onclick={() => (confirmVacuum = false)}>Cancel</button></span>
    {/if}
  </div>
  {#if msg}<p class="muted small" style="margin:0.5rem 0 0">{msg}</p>{/if}
</section>
{/if}

<p class="muted small" style="margin:1rem 0 0">
  EDDA, with your consent, will collect anonymous data to improve the app.
  You can see exactly what data EDDA collects
  <a href="https://edda-app.com/privacy/" target="_blank" rel="noreferrer">here</a>.
  <label style="margin-left:0.6rem"><input type="checkbox" checked={telemetry} onchange={(e) => setTelemetry(e.currentTarget.checked)} /> Send anonymous usage data</label>
</p>

<style>
  table.layout td, table.layout th { padding: 0.15rem 0.5rem; }
  table.layout tr.dim td { opacity: 0.55; }
  .subnav { display: flex; flex-wrap: wrap; gap: 0.3rem; margin-bottom: 0.7rem; }
  .subnav button { font-size: 0.82rem; padding: 0.25rem 0.7rem; border: 1px solid var(--line); border-radius: 999px; background: transparent; color: var(--muted); cursor: pointer; }
  .subnav button:hover { color: var(--fg); }
  .subnav button.active { color: var(--accent); border-color: var(--accent); }
</style>
