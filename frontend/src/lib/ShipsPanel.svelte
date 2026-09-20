<script>
  import { ownCarriers } from "./carriers.js";
  // Every ship from the journal, its build, and a one-click SLEF export for
  // EDSY / Coriolis.
  import { shipsList, shipModules, shipSlef, shipLinks, carrierStatus, capiRefreshCarrier } from "./api.js";
  import { ship } from "./ship.svelte.js";
  import { fmtInt, fmtTs, fmtAge } from "./format.js";
  import { requestPlanner } from "./planner.svelte.js";
  import { openUrl } from "@tauri-apps/plugin-opener";

  let ships = $state([]);
  let selected = $state(null);
  let build = $state(null);
  let msg = $state("");
  let busy = $state(false);
  let showHistorical = $state(false);
  // Item 52 A: the commander's carrier from the journal — a card, not a
  // ship (a carrier has no ShipID and never appears in StoredShips).
  let carriers = $state([]);
  // The Frontier link's last /fleetcarrier answer (null until linked).
  let live = $state(null);
  // Strangers' carriers are in the store because docking at one creates a
  // row; they do not belong on the commander's Ships tab.
  const mine = $derived(ownCarriers(carriers));
  async function loadCarriers() {
    try { const r = await carrierStatus(); carriers = r.carriers ?? []; live = r.live ?? null; } catch { carriers = []; live = null; }
  }
  // "Update now" (maintainer, 2026-09-19): Frontier is never polled, so
  // the hold is as old as the last carrier event or press. This press
  // asks Frontier again, cooldown or not, and re-reads the card.
  let updating = $state(false);
  let updateMsg = $state("");
  async function updateCarrier() {
    updating = true; updateMsg = "";
    try { await capiRefreshCarrier(); await loadCarriers(); updateMsg = "Updated from Frontier."; }
    catch (e) { updateMsg = String(e); }
    finally { updating = false; }
  }
  // Item 53: one line per stored ship saying where it is and how fresh that is.
  const whereabouts = (l) => {
    const at = [l.station, l.system].filter(Boolean).join(", ");
    if (l.status === "aboard_carrier") return `aboard your carrier ${l.carrier}${l.system ? ` in ${l.system}` : ""} · as of ${fmtAge(l.age_hours)}`;
    if (l.status === "in_transit") return `in transit to ${at || "unknown"}${l.minutes_to_arrival != null ? ` · arrives in ${l.minutes_to_arrival} min` : ""}`;
    if (l.status === "arrived") return `transferred to ${at || "unknown"} · not seen since`;
    return `stored at ${at || "unknown"} · as of ${fmtAge(l.age_hours)}`;
  };
  const aged = (a, unit = "") => a ? `${typeof a.value === "number" ? fmtInt(a.value) : a.value}${unit} · as of ${fmtAge(a.age_hours)}` : "unknown";

  async function loadShips() {
    try {
      ships = await shipsList(showHistorical);
      const cur = ships.find((s) => s.current) ?? ships[0];
      if (cur) await pick(cur);
      else { selected = null; build = null; }
    } catch (e) { msg = String(e); }
  }
  // Loads on mount and RE-loads on every ship swap (the shared pipe's
  // currentId is the signal), so the "flying" pill is never yesterday's.
  // The historical toggle re-calls loadShips itself; this list stays
  // panel-local because the shared store has no historical variant.
  $effect(() => { void ship.currentId; loadShips(); loadCarriers(); });

  async function pick(s) {
    selected = s; build = null; msg = "";
    try { build = await shipModules(s.ship_id); } catch (e) { msg = String(e); }
  }

  async function copyBuild() {
    if (!selected) return;
    busy = true;
    try {
      const slef = await shipSlef(selected.ship_id);
      await navigator.clipboard.writeText(slef);
      msg = `${label(selected)} copied as SLEF — paste into EDSY or Coriolis (Import).`;
    } catch (e) { msg = String(e); } finally { busy = false; }
  }

  async function openIn(site) {
    if (!selected) return;
    try { const l = await shipLinks(selected.ship_id); await openUrl(site === "edsy" ? l.edsy : l.coriolis); } catch (e) { msg = String(e); }
  }
  const label = (s) => s.ship_name ? `${s.ship_name} (${s.ship})` : s.ship;
  const cr = (n) => `${fmtInt(n)} cr`;
</script>

{#if mine.length}
<section class="panel">
  <h2>Carrier{mine.length > 1 ? "s" : ""} <span class="sub">from your journal — every figure carries its age</span></h2>
  {#each mine as c}
    <div class="carrier">
      <div class="name">{c.name ?? c.callsign} <span class="muted small">{c.callsign}</span>
        {#if c.owned}<span class="pill ok">yours</span>{:else}<span class="pill">squadron</span>{/if}
        {#if c.decommissioned}<span class="pill warn">decommissioning</span>{/if}
      </div>
      <div class="muted small">Location: {aged(c.location)}{c.body ? ` (${c.body})` : ""}</div>
      <div class="muted small">Tritium tank: {aged(c.tank_tritium_t, " t")}</div>
      {#if c.capacity}<div class="muted small">Capacity: {fmtInt(c.capacity.value.used_t ?? 0)} t used, {fmtInt(c.capacity.value.free_t ?? 0)} t free of {fmtInt(c.capacity.value.total_t)} t · as of {fmtAge(c.capacity.age_hours)}</div>{/if}
      {#if c.balance_cr != null}<div class="muted small">Balance: {fmtInt(c.balance_cr)} cr · services: {c.services.join(", ") || "none"}</div>{/if}
      {#if c.pending_jump}<div class="small warn">Jump scheduled to {c.pending_jump.system}{c.pending_jump.minutes_to_departure != null ? ` · departs in ${c.pending_jump.minutes_to_departure} min` : ""}</div>{/if}
      <!-- The hold comes ONLY from Frontier's own report (the Frontier
           link, Settings → Frontier account). The journal's running total
           of the commander's own transfers is not the inventory — it
           cannot see the carrier's market sales, other commanders'
           transfers or services consuming cargo (maintainer, 2026-09-16:
           "if we can't show a carrier's *current* inventory we shouldn't
           show the inventory at all"), so without the link nothing is
           shown. -->
      {#if live && c.callsign && live.callsign === c.callsign}
        <div class="small" style="margin-top:0.4rem"><strong>Frontier reports</strong> <span class="muted">fetched {fmtAge((Date.now() - new Date(live.fetched_at)) / 3600e3)} ago</span> <button class="mini" onclick={updateCarrier} disabled={updating} title="Ask Frontier for the carrier's hold, tank and balance right now">{updating ? "Updating…" : "Update now"}</button>{#if updateMsg}<span class="muted"> · {updateMsg}</span>{/if}
          · tank {fmtInt(live.fuel_t)} t · balance {fmtInt(live.balance_cr)} cr{live.reserved_cr ? ` (${fmtInt(live.reserved_cr)} reserved for upkeep)` : ""}{live.state && live.state !== "normalOperation" ? ` · ${live.state}` : ""}{live.current_jump ? ` · jump plotted to ${live.current_jump}` : ""}</div>
        {#if live.hold.length}
          <div class="table-wrap" style="margin-top:0.3rem"><table>
            <thead><tr><th>Hold · {fmtInt(live.hold_t)} t</th><th class="num">t</th></tr></thead>
            <tbody>
              {#each live.hold as h}
                <tr><td>{h.name}{#if h.stolen_t}<span class="pill warn" style="margin-left:0.3rem">stolen {h.stolen_t} t</span>{/if}{#if h.mission_t}<span class="pill" style="margin-left:0.3rem">mission {h.mission_t} t</span>{/if}</td><td class="num">{fmtInt(h.tonnes)}</td></tr>
              {/each}
            </tbody>
          </table></div>
        {:else}
          <div class="muted small">Hold empty.</div>
        {/if}
        {#if live.sales.length || live.purchases.length}
          <div class="muted small">Orders: {live.sales.length} selling, {live.purchases.length} buying.</div>
        {/if}
      {:else if c.owned}
        <div class="muted small">Hold: link your Frontier account (Settings → Frontier account) to see what is really aboard.</div>
      {/if}
    </div>
  {/each}
</section>
{/if}

<section class="panel">
  <h2>Ships <span class="sub">{ships.filter(s=>!s.historical).length} in your fleet</span></h2>
  <label class="row small" style="margin-bottom:.6rem"><input type="checkbox" bind:checked={showHistorical} onchange={loadShips} /> Show historical ships</label>
  <div class="ships">
    {#each ships as s}
      <button class="card {selected?.ship_id === s.ship_id ? 'on' : ''} {s.historical?'historical':''}" onclick={() => pick(s)}>
        <div class="name">{label(s)}{#if s.current} <span class="pill ok">flying</span>{/if}</div>
        <div class="muted small">{s.ident ?? ""} · {s.max_jump_range.toFixed(1)} ly · {s.fuel_main} t tank · {s.cargo_capacity} t cargo</div>
        <div class="muted small">last seen {fmtTs(s.seen)}{s.historical?" · no longer owned":""}</div>
        {#if s.location && !s.current}<div class="muted small">{whereabouts(s.location)}</div>{/if}
      </button>
    {/each}
  </div>
</section>

{#if selected}
<section class="panel">
  <h2>{label(selected)} <span class="sub">{selected.ident ?? ""}</span></h2>
  <div class="row" style="gap:1.4rem; flex-wrap:wrap">
    <div class="stat"><div class="label">Jump range</div><div class="value">{selected.max_jump_range.toFixed(2)} ly</div></div>
    <div class="stat"><div class="label">Unladen</div><div class="value">{selected.unladen_mass.toFixed(1)} t</div></div>
    <div class="stat"><div class="label">Tank</div><div class="value">{selected.fuel_main} t</div></div>
    <div class="stat"><div class="label">Cargo</div><div class="value">{selected.cargo_capacity} t</div></div>
    <div class="stat"><div class="label">Hull</div><div class="value">{cr(selected.hull_value)}</div></div>
    <div class="stat"><div class="label">Modules</div><div class="value">{cr(selected.modules_value)}</div></div>
    <div class="stat"><div class="label">Rebuy</div><div class="value">{cr(selected.rebuy)}</div></div>
  </div>
  <div class="row" style="margin-top:0.6rem">
    <button onclick={copyBuild} disabled={busy}>Copy build (SLEF)</button>
    <button class="quiet" onclick={() => openIn("edsy")}>Open in EDSY</button>
    <button class="quiet" onclick={() => openIn("coriolis")}>Open in Coriolis</button>
    {#if build}<button onclick={() => requestPlanner(selected.ship_id)} title="Open the Build planner on this ship: a blueprint, grade and experimental for every module, one material list for the whole build">Plan build</button>{/if}
    {#if msg}<span class="muted small">{msg}</span>{/if}
  </div>
  {#if build}
    <div class="table-wrap" style="margin-top:0.6rem">
      <table>
        <thead><tr><th>Slot</th><th>Module</th><th>Engineering</th><th class="r">Grade</th><th>Engineer</th><th>Experimental</th><th></th></tr></thead>
        <tbody>
          {#each build.modules as m}
            <tr class={m.grade ? "eng" : ""}>
              <td class="small muted">{m.slot_name}</td>
              <td title={m.item}>{m.item_name}</td>
              <td>{m.blueprint ?? m.blueprint_symbol ?? (m.module_type ? "" : "—")}</td>
              <td class="r num">{#if m.grade}Grade {m.grade}{#if m.quality != null} <span class="muted small">· {(m.quality * 100).toFixed(0)}%</span>{/if}{:else if m.module_type}<span class="muted">unengineered</span>{/if}</td>
              <td class="small">{m.engineer ?? ""}</td>
              <td class="small">{m.experimental ?? ""}</td>
              <td>{#if m.module_type}<button class="mini" onclick={() => requestPlanner(selected.ship_id)} title="Plan this ship in the Build planner">Plan</button>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

</section>
{/if}

<style>
  .ships { display: grid; grid-template-columns: repeat(auto-fit, minmax(17rem, 1fr)); gap: 0.5rem; }
  .card { width: 100%; text-align: left; padding: 0.5rem 0.7rem; background: var(--panel-2); border: 1px solid var(--line); border-radius: 6px; color: var(--text); }
  .card.on { border-color: var(--accent); }
  .card.historical { opacity: .62; }
  .card .name { color: var(--text); font-weight: 600; }
  tr.eng td { background: #7ec8ff10; }
  .mini { font-size: 0.7rem; padding: 0 0.4rem; }
  .carrier { border: 1px solid var(--edge, #444); border-radius: 6px; padding: 0.6rem 0.8rem; margin-bottom: 0.5rem; }
  .carrier .name { font-weight: 600; margin-bottom: 0.2rem; }
</style>
