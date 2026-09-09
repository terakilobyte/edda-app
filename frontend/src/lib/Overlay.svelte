<script>
  // The HUD. Transparent, click-through by default; Ctrl+Shift+O makes it
  // interactive (a frame appears and it can be dragged), Ctrl+Shift+H hides it.
  import { onMount, onDestroy } from "svelte";
  import { follow, targetNext, start as startFollow, stop as stopFollow } from "./follow.svelte.js";
  import { tradeFollow, stopTrade, start as startTradeFollow, stop as stopTradeFollow } from "./tradeFollow.svelte.js";
  import {
    getStatus,
    recentCallouts,
    onCallout,
    onSupercharge,
    onListenState, onListenHeard, onListenPartial, onListenReply,
    onOverlayInteractive,
    setOverlayInteractive,
    missions,
    currentRoute,
  } from "./api.js";
  import { fmtInt } from "./format.js";
  import { prioClass, fuelPct, fuelLabel } from "./ui.js";
  import { KEYS, persisted } from "./storage.svelte.js";
  import { journalResource, useListeners } from "./lifecycle.svelte.js";
  // Item 43: the next-target box lights while the drive is actually
  // supercharged (JetConeBoost in the journal), and cools after 10 s.
  let supercharged = $state(false);
  let chargeTimer;
  import { getCurrentWindow } from "@tauri-apps/api/window";

  let status = $state(null);
  let callouts = $state([]);
  let interactive = $state(false);
  let activeMissions = $state([]);
  let route = $state(null);
  // Item 39: game-route fuel marks — the burn model's "you need fuel by
  // here, and here has it". Icons come from these, never from mere
  // scoopability.
  let fuelMarks = $state([]);
  const fuelMark = (h) => fuelMarks.find((m) => m.index === h.index);
  // Pinned by the Trade tab in the other window; localStorage is the bus.
  const pinnedLoop = persisted(KEYS.pinnedLoop, null, { json: true, sync: true });
  const pinned = $derived(pinnedLoop.value);
  // Settings → HUD writes these; storage sync makes the sliders live.
  const hudAlpha = persisted(KEYS.hudAlpha, 1, { json: true, sync: true });
  const hudScale = persisted(KEYS.hudScale, 1, { json: true, sync: true });
  // NOT `|| 1`: zero is a legitimate alpha (maintainer wants a fully
  // transparent panel); only non-numbers fall back.
  const num = (v, def) => (Number.isFinite(Number(v)) ? Number(v) : def);
  const alpha = $derived(Math.min(1, Math.max(0, num(hudAlpha.value, 1))));
  const scale = $derived(Math.min(1.5, Math.max(0.7, num(hudScale.value, 1))));
  const listeners = useListeners();

  const MAX = 6;
  // Callouts age out of the HUD; the main window keeps the full feed.
  const ttlMs = (c) => (c.priority >= 3 ? 120_000 : c.priority === 2 ? 90_000 : c.kind === "greeting" || c.kind === "route" || c.kind === "session" ? 120_000 : c.kind === "you" || c.kind === "ship" ? 60_000 : 45_000);
  const prune = () => { const now = Date.now(); callouts = callouts.filter((c) => now - (c.at ?? 0) < ttlMs(c)); };
  let pruneTimer;

  // The store may not be ready yet; the next journal change tries again.
  journalResource(async () => {
    status = await getStatus();
    activeMissions = await missions(true);
    const rv = await currentRoute();
    route = rv.route;
    fuelMarks = rv.fuel_marks ?? [];
  });

  onMount(async () => {
    startFollow();
    startTradeFollow();
    try {
      // History gets a short life on the HUD -- it is context, not news.
      callouts = (await recentCallouts()).slice(-MAX).reverse().map((c) => ({ ...c, at: Date.now() - 30_000 }));
    } catch {}
    // Voice, as a conversation in the callout stream: a live "you" line
    // while listening, replaced by what was heard, then the ship's reply.
    const setLive = (text) => {
      const rest = callouts.filter((c) => c.kind !== "you…");
      callouts = text == null ? rest : [{ kind: "you…", text, priority: 0, ts: "live", at: Date.now() }, ...rest].slice(0, MAX);
    };
    listeners.add(onListenState((e) => { if (e.payload.phase === "listening") setLive("listening…"); else if (e.payload.phase !== "thinking") setLive(null); }));
    listeners.add(onListenPartial((e) => setLive(e.payload.text || "listening…")));
    listeners.add(onListenHeard((e) => { setLive(null); const t = e.payload.text || (e.payload.note ? `(${e.payload.note})` : ""); if (t) callouts = [{ kind: "you", text: t, priority: 0, ts: String(Date.now()), at: Date.now() }, ...callouts].slice(0, MAX); }));
    listeners.add(onListenReply((e) => { callouts = [{ kind: "ship", text: e.payload.text, priority: 1, ts: String(Date.now()), at: Date.now() }, ...callouts].slice(0, MAX); }));
    listeners.add(onCallout((e) => { callouts = [{ ...e.payload, at: Date.now() }, ...callouts].slice(0, MAX); }));
    listeners.add(onSupercharge(() => { supercharged = true; clearTimeout(chargeTimer); chargeTimer = setTimeout(() => (supercharged = false), 10000); }));
    listeners.add(onOverlayInteractive((e) => { interactive = !!e.payload; }));
    pruneTimer = setInterval(prune, 5000);
  });
  onDestroy(() => { clearInterval(pruneTimer); pinnedLoop.dispose(); stopFollow(); stopTradeFollow(); });

  function startDrag(e) {
    if (!interactive) return;
    if (e.button !== 0) return;
    getCurrentWindow().startDragging().catch(() => {});
  }

  const fuel = $derived(fuelPct(status));

  // Hops still ahead of the current system.
  const ahead = $derived.by(() => {
    if (!route) return [];
    const here = status?.location?.system_name?.toLowerCase();
    const i = route.hops.findIndex((h) => h.system.toLowerCase() === here);
    return route.hops.slice(i >= 0 ? i + 1 : 1);
  });
  const clearPinned = () => { pinnedLoop.value = null; };
</script>

<div class="hud {interactive ? 'interactive' : ''}" style="--hud-alpha:{alpha}; zoom:{scale}" data-tauri-drag-region onmousedown={startDrag} role="presentation">
  {#if interactive}
    <div class="grip" data-tauri-drag-region>
      HUD — drag to move · corner to resize
      <button class="quiet small" onmousedown={(e) => e.stopPropagation()} onclick={(e) => { e.stopPropagation(); setOverlayInteractive(false); }}>Lock</button>
    </div>
    <!-- Undecorated windows have no OS resize borders; this corner is
         the handle. Size persists via the window-state plugin. -->
    <div class="resize-grip" role="presentation" title="Drag to resize"
      onmousedown={(e) => { e.stopPropagation(); getCurrentWindow().startResizeDragging("SouthEast").catch((err) => console.error("resize drag refused:", err)); }}>◢</div>
  {/if}

  <div class="line big">
    <span class="sys">{status?.location?.system_name ?? "—"}</span>
    {#if status?.location?.docked}
      <span class="pill cyan">Docked · {status.location.station_name}</span>
    {/if}
  </div>

  <div class="line">
    {#if status?.controlling_power}
      <span class="pill accent">{status.controlling_power}</span>
      <span class="pill">{status.power_state ?? "?"}</span>
    {:else}
      <span class="pill">No Powerplay control</span>
    {/if}
  </div>

  {#if status?.nav?.target_system}
    <div class="line">
      <span class="lbl">Next</span>
      <span>{status.nav.target_system}</span>
      <span class="pill {status.nav.scoopable === false ? 'warn' : status.nav.scoopable ? 'ok' : ''}">
        {status.nav.star_class ?? "?"}{status.nav.scoopable === false ? " · no scoop" : ""}
      </span>
      {#if status.nav.remaining_jumps != null}
        <span class="num">{status.nav.remaining_jumps} jump{status.nav.remaining_jumps === 1 ? "" : "s"}</span>
      {/if}
    </div>
  {/if}

  {#if follow.active}
    <div class="line route follow">
      <span class="lbl">{follow.jumps_left} left</span>
      {#each follow.ahead.slice(0, 5) as h, i}
        <span class="hop {i === 0 ? 'now' : ''} {h.class === 'neutron' ? 'cyan' : ''} {h.refuel && !h.fuel_optional ? 'stop' : ''} {i === 0 && supercharged ? 'charged' : ''}" title="{h.name} · {h.class}{follow.ahead[i + 1]?.boosted ? ' · supercharge out of here' : ''}{h.refuel && !h.fuel_optional ? ' · fuel here' : h.scoopable ? ' · fuel available, not needed' : ', no scoop'}{h.fuel_after != null ? ' · ' + h.fuel_after.toFixed(1) + ' t after' : ''}">
          {h.class === "neutron" ? "N" : h.class === "white_dwarf" ? "WD" : h.class === "unknown" ? "?" : h.class.toUpperCase()}{h.refuel && !h.fuel_optional ? "⛽" : ""}{follow.ahead[i + 1]?.boosted ? "⚡" : ""}
        </span>
      {/each}
      {#if follow.ahead.length > 5}<span class="muted small">…{follow.destination}</span>{/if}
      {#if interactive}<button class="quiet small" onclick={targetNext} title="Target the next system">▶</button>{/if}
    </div>
  {:else if ahead.length}
    <div class="line route">
      <span class="lbl">Route</span>
      {#each ahead.slice(0, 5) as h}
        <span class="hop {h.hazard ? 'bad' : h.opposing ? 'warn' : ''} {fuelMark(h) ? 'stop' : ''}" title="{h.system} · class {h.star_class}{fuelMark(h) ? ' · fuel here (' + fuelMark(h).via + ')' : ''}{h.hazard ? ' · ' + h.hazard : ''}{h.controlling_power ? ' · ' + h.controlling_power + (h.power_state ? ' ' + h.power_state : '') : ''}{h.dock ? ' · dock: ' + h.dock.station : ''}">
          {h.star_class}{fuelMark(h) ? "⛽" : ""}{h.dock ? "⚓" : ""}
        </span>
      {/each}
      {#if ahead.length > 5}<span class="muted small">+{ahead.length - 5}</span>{/if}
    </div>
  {/if}

  <div class="line gauges">
    <div class="gauge">
      <span class="lbl">Fuel</span>
      <div class="bar"><div class="bar-fill {fuel != null && fuel < 25 ? 'bad' : ''}" style="width:{fuel ?? 0}%"></div></div>
      <span class="num">{fuelLabel(status)}</span>
    </div>
    <div class="gauge">
      <span class="lbl">Cargo</span>
      <div class="bar"><div class="bar-fill" style="width:{status?.cargo_capacity ? Math.round((status.cargo_count / status.cargo_capacity) * 100) : 0}%"></div></div>
      <span class="num">{fmtInt(status?.cargo_count)} / {fmtInt(status?.cargo_capacity)}</span>
    </div>
  </div>

  {#if tradeFollow.active}
    <!-- The FOLLOWED trade route (maintainer spec 2026-09-05): each stop with
         its goods, the current one highlighted. Supersedes the passive
         pinned loop while active. -->
    <div class="line loop trade">
      <span class="lbl">TRADE ROUTE{tradeFollow.lap > 1 ? ` · lap ${tradeFollow.lap}` : ""}</span>
      {#if interactive}<button class="quiet small" title="Stop following" onclick={stopTrade}>✕</button>{/if}
      <span>
        {#each tradeFollow.stops as s, i}
          {#if i > 0} → {/if}<strong class={i === tradeFollow.at ? "now" : "muted"}>{s.station}</strong>
          <span class="muted small">({s.system}){#each s.sell as g} sell {g.commodity} ×{g.tons}{/each}{#each s.buy as g} buy {g.commodity} ×{g.tons}{/each}</span>
        {/each}
      </span>
    </div>
  {:else if pinned?.stops}
    <div class="line loop">
      <span class="lbl">Ring</span>{#if interactive}<button class="quiet small" title="Clear" onclick={clearPinned}>✕</button>{/if}
      <span>{#each pinned.stops as s, i}{#if i > 0} → {/if}<strong>{s.station}</strong> <span class="muted">({s.system}) {s.buy}</span>{/each} → back</span>
    </div>
  {:else if pinned}
    <div class="line loop">
      <span class="lbl">Loop</span>{#if interactive}<button class="quiet small" title="Clear" onclick={clearPinned}>✕</button>{/if}
      <span><strong>{pinned.a.station}</strong> <span class="muted">({pinned.a.system})</span> buy {pinned.a.buy} →
        <strong>{pinned.b.station}</strong> <span class="muted">({pinned.b.system})</span> buy {pinned.b.buy} → back</span>
    </div>
  {/if}

  {#if activeMissions.length}
    <div class="line missions">
      <span class="lbl">Missions</span>
      {#each activeMissions.slice(0, 3) as m}
        <span class="pill {m.status === 'ready_to_turn_in' ? 'ok' : ''}" title={m.title}>
          {m.kill_count ? `${m.kills_done}/${m.kill_count} ${m.target_faction ?? ""}` : m.kind === "assassinate" ? `${m.target}${m.kills_done ? " ✓" : ""}` : m.title.slice(0, 28)}
        </span>
      {/each}
      {#if activeMissions.length > 3}<span class="muted small">+{activeMissions.length - 3}</span>{/if}
    </div>
  {/if}

  <ul class="callouts">
    {#each callouts as c, i (c.ts + c.text + i)}
      <li class="{prioClass(c.priority)} {i === 0 ? 'latest' : ''} {c.kind === 'you' || c.kind === 'you…' ? 'you' : ''} {c.kind === 'you…' ? 'live' : ''}">
        <span class="kind">{c.kind === "you…" ? "🎙" : c.kind}</span>{c.text}
      </li>
    {/each}
  </ul>
</div>

<style>
  .hud {
    position: fixed;
    inset: 0;
    padding: 0.55rem 0.7rem;
    color: var(--text);
    font-size: 0.85rem;
    text-shadow: 0 1px 2px #000, 0 0 6px #000a;
    /* Settings → HUD scales the panel alpha; text keeps its shadow so
       a near-transparent HUD stays readable over the game. */
    background: linear-gradient(
      180deg,
      rgba(10, 12, 16, calc(0.8 * var(--hud-alpha, 1))),
      rgba(10, 12, 16, calc(0.53 * var(--hud-alpha, 1)))
    );
    border: 1px solid transparent;
    border-radius: 8px;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    user-select: none;
  }
  .hud.interactive {
    border-color: var(--accent);
    background: #0a0c10ee;
    cursor: move;
  }
  .resize-grip {
    position: fixed;
    right: 2px;
    bottom: 0;
    color: var(--accent);
    cursor: nwse-resize;
    font-size: 0.9rem;
    line-height: 1;
    padding: 0.15rem;
    user-select: none;
  }
  .grip {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    color: var(--accent);
    font-size: 0.72rem;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .line { display: flex; align-items: center; gap: 0.45rem; flex-wrap: wrap; }
  .big .sys { font-size: 1.25rem; font-weight: 600; color: var(--accent-2); }
  .lbl { color: var(--muted); font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.05em; }
  .gauges { gap: 0.9rem; }
  .loop { font-size: 0.8rem; }
  .hop { display: inline-flex; align-items: center; gap: 0.2rem; padding: 0.05rem 0.4rem; border: 1px solid #ffffff22; border-radius: 4px; font-family: var(--mono); font-size: 0.75rem; }
  .hop.warn { border-color: #ffb30077; color: var(--warn); }
  /* Item 40a: fuel stops read at a glance; the spoken negative case is gone. */
  .hop.stop { border-color: #ffb300aa; background: #ffb30018; }
  /* Item 43: the box lights while the drive is actually supercharged. */
  .hop.charged { border-color: #7fd4ff; box-shadow: 0 0 8px #7fd4ff88; color: #bfe9ff; }
  .hop.bad { border-color: #ff5c5c77; color: var(--bad); }
  .fuel { width: 7px; height: 7px; border-radius: 50%; display: inline-block; }
  .fuel.ok { background: var(--ok); }
  .fuel.no { background: transparent; border: 1px solid var(--muted); }
  .gauge { display: grid; grid-template-columns: auto 1fr auto; align-items: center; gap: 0.4rem; flex: 1; min-width: 150px; }
  .gauge .bar { min-width: 60px; }
  .callouts {
    list-style: none;
    margin: 0.2rem 0 0;
    padding: 0.35rem 0 0;
    border-top: 1px solid #ffffff18;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    overflow: hidden;
  }
  .callouts li { color: var(--muted); font-size: 0.8rem; line-height: 1.3; }
  .callouts li.latest { color: var(--text); }
  .callouts li.accent { color: var(--accent-2); }
  .callouts li.warn { color: var(--warn); }
  .callouts li.bad { color: var(--bad); }
  .kind {
    display: inline-block;
    min-width: 4.2rem;
    color: #ffffff66;
    font-size: 0.68rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  button.small { padding: 0.1rem 0.5rem; font-size: 0.72rem; }
  .route.follow .hop.now { outline: 1px solid var(--accent, #f07b05); border-radius: 3px; padding: 0 0.2rem; }
  .route.follow .hop.cyan { color: #7ec8ff; }
  .callouts li.you { color: #57c66d; }
  .callouts li.you.live { font-style: italic; opacity: 0.85; }
</style>
