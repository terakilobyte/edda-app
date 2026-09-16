<script>
  import { ownCarriers, holdSummary } from "./carriers.js";
  import { SvelteSet } from "svelte/reactivity";
  // Every ship from the journal, its build, and a one-click SLEF export for
  // EDSY / Coriolis.
  import { shipsList, shipModules, shipSlef, shipLinks, carrierStatus } from "./api.js";
  import { ship } from "./ship.svelte.js";
  import { fmtInt, fmtTs, fmtAge } from "./format.js";
  import { requestPlan } from "./engineering.svelte.js";
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
  // Strangers' carriers are in the store because docking at one creates a
  // row; they do not belong on the commander's Ships tab.
  const mine = $derived(ownCarriers(carriers));
  // Which carriers' holds are open. Panel-local and per carrier: the
  // maintainer asked to click a carrier open, not to have it remembered
  // (2026-09-16, "clicking a carrier should it expand it so I can see
  // the full inventory").
  let expanded = $state(new SvelteSet());
  function toggleHold(id) {
    if (expanded.has(id)) expanded.delete(id);
    else expanded.add(id);
  }
  async function loadCarriers() {
    try { carriers = (await carrierStatus()).carriers ?? []; } catch { carriers = []; }
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
      {#if c.hold_moved.length}
        {@const open = expanded.has(c.carrier_id)}
        {@const hold = holdSummary(c.hold_moved, open ? Infinity : 8)}
        <button
          class="holdToggle muted small"
          aria-expanded={open}
          onclick={() => toggleHold(c.carrier_id)}
          title={open ? "Collapse the hold" : "Show every commodity aboard"}
        >{open ? "▾" : "▸"} Moved aboard by you · {c.hold_moved.length} commodit{c.hold_moved.length === 1 ? "y" : "ies"}</button>
        <div class="row small hold">
          {#each hold.shown as h}
            <span class="pill"><b>{fmtInt(h.tons)} t</b> {h.label}</span>
          {/each}
          {#if hold.more}
            <button class="pill asButton" onclick={() => toggleHold(c.carrier_id)}
              title="Show every commodity aboard">Show all {c.hold_moved.length} · +{fmtInt(hold.moreTons)} t more</button>
          {:else if open && c.hold_moved.length > 8}
            <button class="pill asButton" onclick={() => toggleHold(c.carrier_id)}
              title="Show only the largest lots">Show less</button>
          {/if}
        </div>
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
              <td>{#if m.module_type}<button class="mini" onclick={() => requestPlan({ ...m, ship_id: selected.ship_id })} title={m.grade ? "Plan the next grades in the Engineering tab" : "Plan engineering for this module in the Engineering tab"}>Plan</button>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
{/if}

<style>
  /* The hold's expand/collapse: a control, not a paragraph that happens
     to be clickable (maintainer, 2026-09-16: "expand/collapse button or
     something"). */
  .holdToggle {
    display: block;
    margin: 0.3rem 0 0.1rem;
    padding: 0;
    background: none;
    border: 0;
    font: inherit;
    color: var(--muted);
    cursor: pointer;
    text-align: left;
  }
  .holdToggle:hover { color: var(--fg); }
  .pill.asButton { font: inherit; cursor: pointer; border: 1px solid var(--muted); background: none; color: inherit; }
  .pill.asButton:hover { border-color: var(--fg); }

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
