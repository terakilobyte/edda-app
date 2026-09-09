<script>
  import { openHelp } from "./help.svelte.js";
  // Long-distance route plotter over the galaxy star index, with a map.
  // Everything that must survive a tab switch lives in route.svelte.js;
  // this component is a view of that store plus the form.
  import { onMount } from "svelte";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { galaxyStatus, nameComplete, getStatus, injectionsAvailable, findSystem, gameRouteMaxGet, mapPointsGet, routePlotInGame, routeClearInGame, routeActiveGet, shipScoopInfo } from "./api.js";
  import { fmtInt } from "./format.js";
  import { KEYS, persisted } from "./storage.svelte.js";
  import { plotQueryFrom } from "./queries.js";
  import { etaPlan, fmtDuration, fsdIntegrityPlan } from "./ui.js";
  import Autocomplete from "./Autocomplete.svelte";
  import GalaxyView from "./GalaxyView.svelte";
  import { carrierRoute, start as startCarrierRoute, stop as stopCarrierRoute, plotCarrier, followCarrier, nextCarrierJump, clearCarrier } from "./carrierRoute.svelte.js";
  import { CARRIER_ROUTING } from "./flags.js";
  import { routing, setRoute, runPlot, stopPlot, tryHarder, plotStatusLine, plotDetailLine, importSpansh } from "./route.svelte.js";
  import { follow, followShownRoute, stopFollowing, targetNext } from "./follow.svelte.js";
  import { ship } from "./ship.svelte.js";

  // The ONE pipe (maintainer, 2026-09-05): ship identity comes from the shared
  // store, refreshed on every journal change. The dropdown holds only an
  // OVERRIDE — null means "the ship I'm flying", resolved live by the
  // backend at plot time, so a ship swap can never make the planner plan
  // for yesterday's hull. Choosing another ship is explicit and resets
  // itself the moment the flown ship changes.
  let shipOverride = $state(null);
  $effect(() => { void ship.currentId; shipOverride = null; });
  const shipId = $derived(shipOverride);
  const shipLabel = (s) => (s.ship_name ? `${s.ship_name} (${s.ship})` : s.ship) + (s.current ? " · flying" : "");
  async function replotForThisShip() {
    shipOverride = null;
    follow.error = "";
    await plot();
  }

  let status = $state(null);
  let currentSystem = $state("");
  let supercharge = $state(true);
  // FSD injections: used only when nothing else crosses a gap. Opt-in.
  const injections = persisted(KEYS.plotInjections, false);
  // White dwarfs boost too but take about twice as long to line up as a
  // neutron, so they are opt-in; off plots them as plain stars.
  const whiteDwarfs = persisted(KEYS.plotWhiteDwarfs, false);
  // Skip top-up scoops: refuel only when the tank requires it. Opt-in;
  // the tank rides lower, so eager scooping stays the default.
  const safeMargins = persisted(KEYS.plotSafeMargins, false);
  // Jumps-vs-refuels dial: scales what a fuel stop costs in the route
  // judge. 1 = the time model fitted from real journals; 0 = stops are
  // free (fewest jumps); 3 = strongly stop-averse.
  // Absolute fewest jumps: thorough search, stops priced at zero, the
  // planner digs for the whole budget. Slow and proud of it.
  let injGrades = $state([]);
  onMount(() => {
    if (!CARRIER_ROUTING) return;
    startCarrierRoute();
    return () => stopCarrierRoute();
  });
  onMount(async () => { try { injGrades = await injectionsAvailable(); } catch {} });
  const injSummary = $derived(injGrades.filter((g) => g.can_make > 0).map((g) => `${g.can_make} ${g.grade}`).join(", ") || "none");
  // How long the plotter may keep looking for a better route.
  // Swap origin and destination: the way back once you are there. A blank
  // origin means "here", so the swap puts the current system in the target.
  function swapEnds() {
    const from = routing.from.trim();
    const to = routing.to.trim();
    routing.from = to;
    routing.to = from || currentSystem;
  }
  const showMap = persisted(KEYS.routeMap, true);
  let spanshLink = $state("");
  let reserve = $state(0);
  let gamePlotMessage = $state("");

  // The store is the truth: routes plotted here, by the ship computer, or
  // re-planned by the follower all land in routing.route.
  const route = $derived(routing.route);
  const loading = $derived(routing.loading);
  const progress = $derived(routing.progress);
  // Integrity along the route: a projection for the reader, not a plan input.
  const integrity = $derived(route ? fsdIntegrityPlan(route.hops, route.fsd_integrity, route.integrity_loss_per_boost) : null);
  // Scoop rate + drive cap for the ETA projection, fetched per plan's ship.
  let scoopInfo = $state(null);
  $effect(() => {
    const id = route?.ship_id;
    if (route) shipScoopInfo(id ?? null).then((s) => (scoopInfo = s)).catch(() => (scoopInfo = null));
  });
  const eta = $derived(route ? etaPlan(route.hops, scoopInfo) : null);
  const error = $derived(routing.error);
  // Another tab asked for a plot ("route me to this trader").
  $effect(() => {
    const req = routing.request;
    if (!req) return;
    routing.request = null;
    routing.to = req.to; if (req.from) routing.from = req.from;
    plot();
  });

  onMount(async () => {
    try { status = await galaxyStatus(); } catch (e) { routing.error = String(e); }
    try { const s = await getStatus(); currentSystem = s.location?.system_name ?? ""; routing.from = currentSystem; } catch {}
    // A route being followed survives restarts; show it even when nothing was plotted this session.
    if (!routing.route) { try { const r = await routeActiveGet(); if (r) setRoute(r, "tab"); } catch {} }
  });

  async function plot() {
    const to = routing.to.trim();
    if (!to) return;
    routing.route = null;
    gamePlotMessage = "";
    const from = routing.from.trim();
    if (!from || from.toLowerCase() === currentSystem.toLowerCase()) {
      try {
        const [origin, system, max, points] = await Promise.all([findSystem(currentSystem), findSystem(to), gameRouteMaxGet(), mapPointsGet()]);
        const distance = origin?.coords && system?.coords ? Math.hypot(system.coords[0]-origin.coords[0], system.coords[1]-origin.coords[1], system.coords[2]-origin.coords[2]) : null;
        if (max > 0 && distance != null && distance <= max && points?.search && points?.result && points?.plot) {
          try { gamePlotMessage = await routePlotInGame(system.name ?? to); }
          catch (e) { routing.error = String(e); }
          return;
        }
      } catch { /* unavailable or not taught: use EDDA's local planner */ }
    }
    await runPlot(plotQueryFrom({ from, to, supercharge, reserve, shipId, injections: injections.value, whiteDwarfs: whiteDwarfs.value, safeMargins: safeMargins.value }));
  }

  async function copy(text) {
    try { await navigator.clipboard.writeText(text); } catch {}
  }
</script>

<section class="panel">
  <h2>Route plotter <button class="ghost" style="margin-left:auto" title="How plotting works" aria-label="Help" onclick={() => openHelp("route-plotting")}>?</button></h2>

  {#if status && !status.available}
    <p class="warn">No galaxy index yet — it installs with the community data download (Settings → System data).</p>
  {:else if status?.populated_only}
    <p class="muted small">Populated systems only ({fmtInt(status.systems)}){status.import_running ? " · import running…" : ""}</p>
  {:else if status}
    <p class="muted small">{fmtInt(status.systems)} systems indexed.</p>
  {/if}

  <div class="plot-form">
    <div class="row">
      <select bind:value={shipOverride} title="Plan for this ship. The default follows whatever you're flying; another ship is assumed to start with a full tank."><option value={null}>{ship.current ? shipLabel(ship.current) : "Current ship"}</option>{#each ship.list.filter((sh) => !sh.current) as sh}<option value={sh.ship_id}>{shipLabel(sh)}</option>{/each}</select>
    </div>
    <div class="row" style="position:relative">
      <Autocomplete bind:value={routing.from} placeholder="From (blank = here)" fetch={(p) => nameComplete("system", p)} />
      <button class="quiet swap" onclick={swapEnds} title="Swap origin and destination" aria-label="Swap origin and destination" disabled={loading || (!routing.to.trim() && !routing.from.trim())}>⇄</button>
      <Autocomplete bind:value={routing.to} placeholder="To" fetch={(p) => nameComplete("system", p)} onenter={plot} />
    </div>
    <div class="row">
      <label title="Supercharge the drive at neutron stars on the way."><input type="checkbox" bind:checked={supercharge} /> neutrons</label>
      <label title="Supercharge at white dwarfs too (×1.5, ×3 on an SCO Mk II). Off by default: a white-dwarf boost takes about twice as long to line up as a neutron's." style="opacity:{supercharge ? 1 : 0.5}"><input type="checkbox" bind:checked={whiteDwarfs.value} disabled={!supercharge} /> white dwarfs</label>
      <label title="Use the FSD injections you can synthesise (now: {injSummary}) when nothing else crosses a gap. A route that works without them never gets one."><input type="checkbox" bind:checked={injections.value} /> injections</label>
      <label title="Plan every jump with 2 tonnes of fuel in hand instead of flying the drive's true reach. A jump or two longer on big trips; turn on if you'd rather not manage the tank closely. Off, the flight monitor coaches the margins live."><input type="checkbox" bind:checked={safeMargins.value} /> safe margins</label>
    </div>
    <div class="row">
      <label>reserve <input type="number" min="0" step="1" bind:value={reserve} style="width:4rem" /> t <span class="help" title="Fuel to keep in hand: no jump on the route is planned to leave less than this in the tank, on top of the plotter's own safety margin. 0 = the safety margin only. Set a few tonnes if you want room for a detour or a missed scoop.">?</span></label>
    </div>
    <div class="row">
      <button onclick={plot} disabled={loading || !routing.to.trim()}>{loading ? "Plotting…" : "Plot"}</button>
      {#if loading}<button class="ghost" onclick={stopPlot} title={routing.best ? "Stop looking and keep the best route found so far" : "Stop plotting"}>{routing.best ? "Use best so far" : "Stop"}</button>{/if}
      <span class="muted small">or</span>
      <button class="quiet" onclick={() => openUrl("https://www.spansh.co.uk/plotter")} title="Open the Spansh galaxy plotter in your browser">Open Spansh ↗</button>
      <input placeholder="Spansh results link" bind:value={spanshLink} style="min-width:18rem" title="Paste a spansh.co.uk plotter results URL (or job id): the route lands here with our fuel column, HUD and callouts" />
      <button class="quiet" onclick={() => importSpansh(spanshLink)} disabled={!spanshLink.trim() || loading}>Import</button>
      <label style="margin-left:auto"><input type="checkbox" bind:checked={showMap.value} /> show map</label>
    </div>
  </div>

  {#if follow.error}<p class="error small">{follow.error} {#if follow.error.startsWith("wrong ship")}<button class="quiet" onclick={replotForThisShip}>Re-plot for this ship</button>{/if}</p>{/if}
  {#if loading}
    <p class="small"><span class="pill accent">plotting</span> {plotStatusLine()} <span class="muted">{plotDetailLine()}</span></p>
  {/if}
  {#if error}
    <p class="error">{error}</p>

  {/if}
  {#if route && route.ship_has_scoop === false && route.refuel_stops > 0}
    <p class="warn small bounded">No fuel scoop fitted: this route relies on {route.refuel_stops} scoop stop{route.refuel_stops === 1 ? "" : "s"}. It can be plotted, not followed.</p>
  {/if}
  {#if route && !loading && routing.origin === "tab" && routing.lastQuery}
    <p class="small muted try-harder">
      {#if routing.notice}{routing.notice}{/if}
      {#if !routing.triedHarder}Thorough plot may save a jump or two — takes a few seconds. <button class="quiet tiny" onclick={tryHarder}>Try harder</button>{/if}
    </p>
  {/if}
  {#if gamePlotMessage}<p class="notice good">{gamePlotMessage} EDDA will pick up Elite's route from the journal.</p>{/if}

  <!-- One map for both the plot in progress and the result: remounting it
       resets the camera and blanks the view for a frame. -->
  {#if showMap.value}<GalaxyView route={route ?? routing.best} candidates={loading ? routing.candidates : []} nextIndex={route && follow.active && follow.source !== null ? follow.next_index : 0} height={440} />{/if}

  {#if route}
    <div class="stat-grid" style="margin-bottom:0.6rem">
      <div class="stat"><div class="label">Jumps</div><div class="value">{route.jumps}<span class="muted small"> at {route.range_ly.toFixed(1)} ly{#if route.ship} · {route.ship}{/if}</span></div></div>
      <div class="stat"><div class="label">Flown</div><div class="value">{route.total_ly.toFixed(0)} ly</div></div>
      <div class="stat"><div class="label">Straight</div><div class="value">{route.straight_ly.toFixed(0)} ly</div></div>
      <div class="stat"><div class="label">Boosted</div><div class="value">{route.boosted_jumps}</div></div>
      {#if route.hops.some((h) => h.fuel_after != null)}<div class="stat"><div class="label">Scoop stops</div><div class="value">{route.refuel_stops}</div></div>{/if}
      {#if eta && eta.totalS > 0}<div class="stat"><div class="label">ETA</div><div class="value">~{fmtDuration(eta.totalS)}<span class="muted small" title="~50 s per jump, 35/60 s per neutron/white-dwarf line-up, scooping at the fitted scoop's rate. A projection for the reader, not a promise.">{#if eta.scoopS > 30} · {fmtDuration(eta.scoopS)} scooping{/if}</span></div></div>{/if}
      {#if route.injections > 0}<div class="stat"><div class="label">Injections</div><div class="value">{route.injections}<span class="muted small"> {route.hops.find((h) => h.injection)?.injection} · you can make {injSummary}</span></div></div>{/if}
      <div class="stat">
        <div class="label">Follow</div>
        <div class="value row">
          {#if follow.active && follow.destination === route.hops[route.hops.length - 1]?.name}
            <span class="pill ok">following · {follow.jumps_left} left</span>
            <button class="quiet" onclick={targetNext} disabled={follow.targeting} title="Say 'Guidance, target next', or use your captured Target Next System in Route control">Target next</button>
            <button class="ghost" onclick={stopFollowing}>Stop</button>
            <button class="ghost" onclick={async () => { follow.error = "Switch to Elite now — clearing the game's route in 3 seconds."; await new Promise((r) => setTimeout(r, 3000)); try { follow.error = await routeClearInGame(); } catch (e) { follow.error = String(e); } }} title="Gives you 3 seconds to switch to Elite, then opens the galaxy map and clears the game's plotted route">Clear in game</button>
          {:else}
            <button onclick={() => followShownRoute(route)} title="Track this route against your jumps with HUD countdown and callouts">Follow this route</button>
          {/if}
        </div>
      </div>
      {#if integrity && integrity.boosts > 0}
        <div class="stat">
          <div class="label">FSD integrity</div>
          <div class="value">{Math.round(route.fsd_integrity * 100)} → {Math.round(integrity.end * 100)} %<span class="muted small"> {route.integrity_loss_per_boost > 0 ? `1 % per boost · ${integrity.repairs.length === 0 ? "no repairs" : `${integrity.repairs.length} repair${integrity.repairs.length === 1 ? "" : "s"}`}` : "no boost damage"}</span></div>
        </div>
      {/if}
      <div class="stat"><div class="label">Search</div><div class="value">{fmtInt(route.expansions)}<span class="muted small"> nodes · {route.elapsed_ms} ms</span></div></div>
    </div>

    <!-- Warnings get their own row between the stat boxes and the route
         overview (maintainer, 2026-09-05) — wedged mid-grid they split the
         boxes into two ragged groups. -->
    {#if integrity && integrity.repairs.length > 0}
      <p class="warn small bounded" style="margin: 0 0 0.6rem 0">{route.ship_has_afmu === false ? "No AFMU fitted: " : ""}FSD repair{integrity.repairs.length === 1 ? "" : "s"} before jump{integrity.repairs.length === 1 ? "" : "s"} {integrity.repairs.join(", ")}{route.ship_has_afmu === false ? " — without an AFMU that means a station each time." : route.ship_has_afmu ? " with the AFMU." : "."}</p>
    {/if}

    <div class="table-wrap" style="margin-top:0.6rem">
      <table>
        <thead><tr><th>#</th><th>System</th><th>Star</th><th class="r">Jump</th><th class="r">Fuel</th>{#if integrity && integrity.boosts > 0}<th class="r" title="FSD integrity after this jump: 1 % per supercharge; the drive malfunctions below 80 %">FSD</th>{/if}<th class="r">Total</th><th></th></tr></thead>
        <tbody>
          {#each route.hops as h, i}
            <tr class="{h.boosted ? 'boost' : ''} {follow.active && follow.next?.name === h.name ? 'next' : ''}">
              <td class="num">{i}</td>
              <td>{h.name} <button class="copy" title="Copy system name" onclick={() => copy(h.name)}>⧉</button></td>
              <td><span class="pill {h.class === 'neutron' ? 'cyan' : h.scoopable ? 'ok' : 'warn'}">{h.class === "unknown" ? (h.scoopable ? "star unknown · scoopable" : "star unknown · no scoop") : h.class.replace("_", " ") + (h.scoopable ? (["neutron", "white_dwarf", "black_hole"].includes(h.class) ? " · scoopable companion" : "") : " · no scoop")}</span></td>
              <td class="r num">{h.distance_ly.toFixed(1)} ly{#if h.injection} <span class="pill warn" title="Synthesise this FSD injection before the jump">{h.injection} injection</span>{/if}</td>
              <td class="r num" title={h.fuel_after != null ? "tonnes in the tank on arrival" : ""}>{#if h.fuel_after != null}{h.fuel_after.toFixed(1)} t{#if h.refuel} <span class="pill ok" title="scoop here">scoop</span>{/if}{/if}</td>
              {#if integrity && integrity.boosts > 0}<td class="r num {h.boosted ? '' : 'muted'}" class:warn={integrity.after[i] < 0.81} title={h.boosted ? "FSD integrity after this supercharge" : "FSD integrity (unchanged: no boost on this jump)"}>{Math.round(integrity.after[i] * 100)} %{#if integrity.repairs.includes(i)} <span class="pill warn" title={route.ship_has_afmu === false ? "This boost would take the drive below 81 % (it malfunctions below 80 %). No AFMU aboard: repair at a station first; the projection assumes you do." : "This boost would take the drive below 81 % (it malfunctions below 80 %). Repair first (AFMU or station); the projection assumes you do."}>repair first</span>{/if}</td>{/if}
              <td class="r num">{h.total_ly.toFixed(0)} ly</td>
              <td class="small muted" title={route.hops[i + 1]?.boosted ? "The jump out of this system is supercharged: dip the jet cone before you leave." : ""}>{route.hops[i + 1]?.boosted ? (h.class === "white_dwarf" ? "⚡ 1.5× boost, careful" : "⚡ supercharge here") : ""}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>


{#if CARRIER_ROUTING}
<section class="panel">
  <h2>Carrier route <span class="sub">500 ly per jump · tritium by mass · long-lived</span></h2>
  {#if carrierRoute.follow.active}
    <div class="row" style="gap:1.4rem; flex-wrap:wrap">
      <div class="stat"><div class="label">Following</div><div class="value">{carrierRoute.follow.from} → {carrierRoute.follow.to}</div></div>
      <div class="stat"><div class="label">Jumps left</div><div class="value">{carrierRoute.follow.remaining} of {carrierRoute.follow.jumps}</div></div>
      <div class="stat"><div class="label">Next</div><div class="value">{carrierRoute.follow.next_system ?? "done"}{#if carrierRoute.follow.next_distance_ly != null}<span class="muted small"> · {carrierRoute.follow.next_distance_ly.toFixed(0)} ly · {carrierRoute.follow.next_fuel_t} t</span>{/if}</div></div>
      <div class="stat"><div class="label">ETA</div><div class="value">{carrierRoute.follow.eta_minutes} min<span class="muted small"> at {Math.round(carrierRoute.follow.minutes_per_jump)} min/jump{carrierRoute.follow.cadence_measured ? " (your measured pace)" : " (game minimum; queues add more)"}</span></div></div>
    </div>
    {#if carrierRoute.follow.scheduled}<p class="small warn">Jump to {carrierRoute.follow.scheduled.system} scheduled{carrierRoute.follow.scheduled.departure ? ` · departs ${carrierRoute.follow.scheduled.departure.replace("T", " ").replace("Z", " UTC")}` : ""}</p>{/if}
    {#if carrierRoute.follow.verdict?.status === "short_by"}<p class="small warn">Tritium runs {carrierRoute.follow.verdict.tons} t short at jump {carrierRoute.follow.verdict.at_hop}: refuel before it.</p>{/if}
    <div class="row" style="margin-top:0.4rem">
      <button onclick={nextCarrierJump} disabled={carrierRoute.follow.done}>Next system → clipboard</button>
      <button class="quiet" onclick={clearCarrier}>Stop following</button>
      {#if carrierRoute.copied}<span class="muted small">“{carrierRoute.copied}” copied — paste it into the carrier’s navigation panel.</span>{/if}
    </div>
  {:else}
    <div class="row">
      <input placeholder="Destination system" bind:value={carrierRoute.to} onkeydown={(e) => e.key === "Enter" && plotCarrier()} style="min-width:16rem" />
      <input placeholder="From (blank = carrier’s last position)" bind:value={carrierRoute.from} style="min-width:16rem" />
      <button onclick={plotCarrier} disabled={carrierRoute.loading}>{carrierRoute.loading ? "Plotting…" : "Plot carrier route"}</button>
    </div>
    {#if carrierRoute.error}<p class="error small">{carrierRoute.error}</p>{/if}
    {#if carrierRoute.plan}
      <div class="row" style="gap:1.4rem; flex-wrap:wrap; margin-top:0.4rem">
        <div class="stat"><div class="label">Jumps</div><div class="value">{carrierRoute.plan.jumps}<span class="muted small"> · {carrierRoute.plan.total_ly.toFixed(0)} ly</span></div></div>
        <div class="stat"><div class="label">Tritium</div><div class="value">{carrierRoute.plan.fuel_t} t<span class="muted small"> · {carrierRoute.plan.tank_end_t} t left</span></div></div>
        <div class="stat"><div class="label">ETA</div><div class="value">≥ {carrierRoute.plan.eta_minutes} min<span class="muted small"> at the game’s {Math.round(carrierRoute.plan.minutes_per_jump)} min/jump minimum</span></div></div>
      </div>
      {#if carrierRoute.plan.verdict?.status === "short_by"}<p class="small warn">Tritium runs {carrierRoute.plan.verdict.tons} t short at jump {carrierRoute.plan.verdict.at_hop}. The route is still shown; plan a refuel before it.</p>{/if}
      <div class="table-wrap"><table>
        <thead><tr><th>#</th><th>System</th><th>ly</th><th>Tritium</th><th>Tank after</th></tr></thead>
        <tbody>{#each carrierRoute.plan.hops as h, i}<tr><td>{i + 1}</td><td>{h.name}</td><td>{h.distance_ly.toFixed(0)}</td><td>{h.fuel_t}{h.topped_up_t ? ` (+${h.topped_up_t} from hold)` : ""}</td><td class={h.tank_after_t < 0 ? "warn" : ""}>{h.tank_after_t}</td></tr>{/each}</tbody>
      </table></div>
      <div class="row" style="margin-top:0.4rem"><button onclick={followCarrier}>Follow this carrier route</button></div>
    {/if}
  {/if}
</section>
{/if}

<style>
  .bounded { max-width: 72ch; overflow-wrap: anywhere; white-space: normal; }
  .plot-form { display: flex; flex-direction: column; gap: 0.5rem; margin-bottom: 0.6rem; }
  .plot-form .row { flex-wrap: wrap; align-items: center; gap: 1rem; }
  .plot-form label { display: inline-flex; align-items: center; gap: 0.4rem; margin: 0; white-space: nowrap; }
  .plot-form select, .plot-form input[type="number"] { height: 2rem; box-sizing: border-box; }
  .plot-form select { min-width: 11rem; }
  .plot-form input[type="checkbox"] { margin: 0; }
  .plot-form button { height: 2rem; }
  .plot-form .swap { width: 2rem; padding: 0; font-size: 1.1rem; line-height: 1; }
  .try-harder { margin: 0.2rem 0 0.6rem; }
  .help { display: inline-block; width: 1.1rem; height: 1.1rem; line-height: 1.1rem; text-align: center; border-radius: 50%; border: 1px solid var(--line); color: var(--muted); font-size: 0.72rem; cursor: help; margin-left: 0.2rem; }
  tr.boost td { background: #7ec8ff10; }
  button.copy { background: transparent; color: var(--muted); border: none; padding: 0 0.2rem; font-size: 0.85rem; cursor: pointer; }
  button.copy:hover { color: var(--accent); filter: none; }
  tr.next td { background: rgba(240, 123, 5, 0.12); }
</style>
