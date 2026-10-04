<script>
  // The material trader (boss, 2026-10-04: "I need to go trade down
  // materials maximally... it'd be awesome if edda could do that"). What to
  // give and what to take at one trader so the materials nearing their cap
  // fill the gaps below them: nearest grade first (keeps the most value),
  // own group before across, down before up, never below the floor. The
  // ratios are the game's, measured against every trade in the journal
  // (docs/benches/2026-10-04-material-trade-ratios.csv). The plan re-reads
  // the journal as you trade, so the list shrinks at the counter.
  import { materialTradePlan } from "./api.js";
  import { journalResource } from "./lifecycle.svelte.js";
  import Place from "./Place.svelte";

  let view = $state(null);
  let error = $state("");
  let busy = $state(false);
  // "" = let EDDA pick: the docked trader's type, else the busiest.
  let kind = $state("");
  let sourceMin = $state(90);
  let floor = $state(50);
  let cross = $state(true);
  let up = $state(true);
  // bottom_first: most units per unit spent (1:81 at the bottom). nearest_first: keeps the most value.
  let order = $state("bottom_first");

  async function load() {
    busy = true;
    try {
      view = await materialTradePlan({ kind: kind || null, sourceMin: sourceMin / 100, floor: floor / 100, cross, up, order });
      error = "";
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
  journalResource(load);

  const kinds = [["", "auto"], ["raw", "Raw"], ["manufactured", "Manufactured"], ["encoded", "Encoded"]];
  const title = (s) => s.charAt(0).toUpperCase() + s.slice(1);
  const given = $derived(view ? view.plan.trades.reduce((n, t) => n + t.give_qty, 0) : 0);
  const received = $derived(view ? view.plan.trades.reduce((n, t) => n + t.recv_qty, 0) : 0);
  const byGroup = $derived.by(() => {
    if (!view) return [];
    const m = new Map();
    for (const l of view.plan.still_short) (m.get(l.group) ?? m.set(l.group, []).get(l.group)).push(l);
    return [...m.entries()].sort(([a], [b]) => a.localeCompare(b, undefined, { numeric: true }));
  });
</script>

<section class="panel">
  <h2>Material trader <span class="sub">give the near-full, take the gaps · every line a direct trade · own group before across · down before up</span></h2>
  <div class="row" style="margin:0.4rem 0; flex-wrap:wrap; gap:0.6rem">
    <label>Trader
      <select bind:value={kind} onchange={load}>
        {#each kinds as [v, label]}<option value={v}>{label}</option>{/each}
      </select>
    </label>
    <label title="A material at this share of its cap or above is spent">Near full ≥
      <input type="range" min="50" max="100" step="5" bind:value={sourceMin} onchange={load} /> <span class="num">{sourceMin}%</span>
    </label>
    <label title="Sources are never spent below this share of their cap">Keep ≥
      <input type="range" min="0" max="100" step="5" bind:value={floor} onchange={load} /> <span class="num">{floor}%</span>
    </label>
    <label title="Bottom first: one unit goes furthest at the bottom (1:81). Nearest first: keeps the most value in the hold.">Fill
      <select bind:value={order} onchange={load}>
        <option value="bottom_first">bottom grade first (most units)</option>
        <option value="nearest_first">nearest grade first (keeps value)</option>
      </select>
    </label>
    <label><input type="checkbox" bind:checked={cross} onchange={load} /> across groups (6× dearer)</label>
    <label><input type="checkbox" bind:checked={up} onchange={load} /> up a grade (6:1)</label>
    <button class="small" onclick={load} disabled={busy}>{busy ? "…" : "Re-plan"}</button>
  </div>
  {#if error}<p class="error small">{error}</p>{/if}
  {#if view}
    <p class="small muted">
      Planning for a <strong>{view.kind}</strong> trader
      {#if view.kind_from !== "chosen"}({view.kind_from}){/if}.
      {#if view.docked_kind && view.docked_kind !== view.kind}You are docked at a {view.docked_kind} trader.{/if}
    </p>

    <h3>Sources <span class="muted">({view.plan.sources.length} at or above {Math.round(view.plan.policy_source_min * 100)}% of cap)</span></h3>
    {#if view.plan.sources.length === 0}
      <p class="muted small">Nothing is near full in {view.kind}. Lower the threshold, or pick another trader.</p>
    {:else}
      <div class="row" style="flex-wrap:wrap; gap:0.3rem">
        {#each view.plan.sources as s (s.symbol)}
          <span class="pill" title="group {s.group}">{s.name} <span class="muted">G{s.grade}</span> <span class="num">{s.count}/{s.cap}</span></span>
        {/each}
      </div>
    {/if}

    <h3>Trades <span class="muted">({view.plan.trades.length} · give {given}, receive {received})</span></h3>
    {#if view.plan.trades.length === 0}
      <p class="muted small">No trade to make: every gap the sources could fill is already full, or the floor stops them.</p>
    {:else}
      <div class="table-wrap">
        <table>
          <thead><tr><th>#</th><th>Give</th><th></th><th>Receive</th><th>Ratio</th><th>Direction</th><th>Left with</th></tr></thead>
          <tbody>
            {#each view.plan.trades as t, i}
              <tr>
                <td class="num muted">{i + 1}</td>
                <td><span class="num">{t.give_qty}</span> × {t.give_name} <span class="muted small">G{t.give_grade}</span></td>
                <td class="muted">→</td>
                <td><span class="num">{t.recv_qty}</span> × {t.recv_name} <span class="muted small">G{t.recv_grade}</span></td>
                <td class="num small">{t.ratio}</td>
                <td><span class="pill {t.direction === 'down' ? 'ok' : t.direction === 'across' ? 'warn' : ''}">{t.direction}</span></td>
                <td class="small muted">{t.give_name} {t.give_after} · {t.recv_name} {t.recv_after}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}

    {#if view.plan.still_short.length}
      <h3>Still short after the plan <span class="muted">({view.plan.still_short.length} below {Math.round(view.plan.policy_source_min * 100)}% of cap)</span></h3>
      <div class="groups">
        {#each byGroup as [group, list] (group)}
          <div class="group">
            <h4>{title(view.kind)} {group}</h4>
            {#each list as l (l.symbol)}
              <div class="small"><span class="muted">G{l.grade}</span> {l.name} <span class="num">{l.count}/{l.cap}</span></div>
            {/each}
          </div>
        {/each}
      </div>
    {/if}

    <h3>Nearest {view.kind} traders {#if view.origin_system}<span class="muted">from {view.origin_system}</span>{/if}</h3>
    {#if !view.origin_system}
      <p class="muted small">No position yet: the journal has not said where you are.</p>
    {:else if !view.traders_known}
      <p class="muted small">Could not reach the API for traders.</p>
    {:else if view.traders.length === 0}
      <p class="muted small">No {view.kind} trader within 300 ly.</p>
    {:else}
      <table>
        <tbody>
          {#each view.traders as t (t.station.id)}
            <tr>
              <td><Place system={t.station.system_name} station={t.station.name} /></td>
              <td class="small">{t.station.name}</td>
              <td class="num small">{t.distance_ly.toFixed(1)} ly</td>
              <td class="small muted">{t.station.primary_economy ?? ""}{t.station.distance_to_arrival != null ? ` · ${Math.round(t.station.distance_to_arrival)} ls` : ""}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    {/if}
  {/if}
</section>

<style>
  h3 { font-size: 0.8rem; color: var(--accent); text-transform: uppercase; letter-spacing: 0.05em; margin: 0.8rem 0 0.3rem; }
  h4 { font-size: 0.75rem; margin: 0 0 0.2rem; color: var(--muted, #9aa); }
  .groups { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 0.6rem; }
  input[type="range"] { width: 7rem; vertical-align: middle; }
</style>
