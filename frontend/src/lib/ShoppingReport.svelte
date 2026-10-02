<script>
  // A shopping report as the Engineering tab and the Ships tab's build plan
  // both show it: trades from what you carry, nearest traders, farm plans.
  import Place from "./Place.svelte";
  import { traderStatus } from "./engineering.svelte.js";
  import { KEYS, persisted } from "./storage.svelte.js";

  /** @type {{ shopping: any, picked: Set<number>, onToggle: (i: number) => void, title?: string }} */
  let { shopping, picked, onToggle, title = "Shopping list" } = $props();

  // Tracked on the HUD: while the box is ticked, the picked trades and
  // what stays short follow every change here, so the trader screen and
  // the list are on one screen (maintainer, 2026-09-28: "Track trade list
  // in HUD" — "something to check in the build planner page").
  const hudShopping = persisted(KEYS.hudShopping, null, { json: true, sync: true });
  const tracked = $derived(hudShopping.value?.title === title);
  const snapshot = () => ({
    title,
    at: new Date().toISOString(),
    trades: (shopping?.list?.trades ?? []).filter((_, i) => picked.has(i)).map((t) => ({ kind: String(t.kind).toLowerCase(), give: t.give, give_material: t.give_material, get: t.get, get_material: t.get_material, rate: t.rate })),
    still_short: shopping?.list?.still_short ?? [],
  });
  const setTracked = (on) => { hudShopping.value = on ? snapshot() : null; };
  $effect(() => {
    // Re-read the picks and the list so a change re-runs this; write only
    // while this list is the one tracked.
    const next = snapshot();
    if (tracked && hudShopping.value && JSON.stringify({ ...hudShopping.value, at: "" }) !== JSON.stringify({ ...next, at: "" })) hudShopping.value = next;
  });

  // Trader kinds needed by the selected trades plus any farm-then-trade plan.
  const neededKinds = $derived(new Set([
    ...(shopping?.list?.trades ?? []).filter((_, i) => picked.has(i)).map((t) => t.kind),
    ...(shopping?.farm ?? []).flatMap((p) => p.options.filter((o) => o.rate).map((o) => o.kind)),
  ]));

  // One place, printed once: a body's full name already carries its system
  // ("Synuefe GV-T b50-4 B 1"), so the system is not said again before it.
  const place = (system, body) => {
    if (!body) return system ?? "";
    if (!system || body.toLowerCase().startsWith(system.toLowerCase())) return body;
    return `${system} · ${body}`;
  };
  // A farm option is a place the commander picked the material up (its
  // note starts "picked up before") or a community site with a name.
  const witnessed = (o) => (o.site ?? "").startsWith("picked up before");

  // Every still-short material has one "where to get it" entry; a material
  // with a farm table shows that table and folds the entry's notes under
  // it (the sites with no fixed system, and the ways its kind is found).
  // Only a material with no farm table keeps the entry as its own block.
  const sourceFor = (material) => (shopping?.sources ?? []).find((s) => s.material.toLowerCase() === material.toLowerCase());
  const farmPlans = $derived((shopping?.farm ?? []).filter((p) => p.options.length));
  const farmed = $derived(new Set(farmPlans.map((p) => p.material.toLowerCase())));
  const ownSources = $derived((shopping?.sources ?? []).filter((s) => !farmed.has(s.material.toLowerCase())));
</script>

{#if shopping}
  <h3 style="margin-top:0.8rem">Shopping list <span class="muted">material traders · 6:1 per grade up, 3:1 per grade down, ×6 across groups</span>
    {#if !shopping.error && (shopping.list?.trades?.length || shopping.list?.still_short?.length)}
      <label class="small" style="margin-left:0.6rem; font-weight:normal" title="The picked trades and what stays short, on the HUD, following every change here"><input type="checkbox" checked={tracked} onchange={(e) => setTracked(e.currentTarget.checked)} /> Track trade list in HUD</label>
      {#if hudShopping.value && !tracked}<span class="muted small">(the HUD is tracking “{hudShopping.value.title}”)</span>{/if}
    {/if}
  </h3>
  {#if shopping.error}
    <p class="error">{shopping.error}</p>
  {:else}
    {#if shopping.list.trades.length}
      <div class="table-wrap">
        <table>
          <thead><tr><th></th><th>At</th><th class="r">Give</th><th></th><th class="r">Get</th><th></th><th>Rate</th></tr></thead>
          <tbody>
            {#each shopping.list.trades as t, i}
              <tr class={picked.has(i) ? "" : "dim"}>
                <td><input type="checkbox" checked={picked.has(i)} onchange={() => onToggle(i)} /></td>
                <td class="muted">{t.kind} trader</td>
                <td class="r num">{t.give}</td><td>{t.give_material}</td>
                <td class="r num">{t.get}</td><td>{t.get_material}</td>
                <td class="muted">{t.rate}{t.same_group ? "" : " (cross-group)"}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    {#if shopping.list.still_short.length}
      <p class="warn small">Still short after trading: {shopping.list.still_short.map(([m, n]) => `${n} ${m}`).join(", ")}.</p>
    {:else if shopping.list.trades.length}
      <p class="ok small">Everything covered by trading what you carry.</p>
    {/if}
    {#each shopping.traders.filter((t) => neededKinds.has(t.kind)) as t}
      {@const status = traderStatus(t, shopping.origin_system)}
      <div class="small" style="margin:0.3rem 0">
        <strong>Nearest {t.kind_known === false ? "material" : t.kind} traders</strong>{shopping.origin_system ? ` from ${shopping.origin_system}` : ""}:
        {#if status.text && t.nearest.length}<span class={status.tone} title={status.title ?? ""}>{status.text}</span>{/if}
        {#if t.nearest.length}
          <div class="row small" style="margin-top:0.2rem">
            {#each t.nearest as n}
              <span class="pill">{n.station.name} · {n.station.system_name} · {n.distance_ly.toFixed(1)} ly{n.station.distance_to_arrival != null ? ` · ${Math.round(n.station.distance_to_arrival)} ls` : ""} <Place system={n.station.system_name} /></span>
            {/each}
          </div>
        {:else}
          <span class={status.tone} title={status.title ?? ""}>{status.text}</span>
        {/if}
      </div>
    {/each}
    {#each farmPlans as plan}
      {@const src = sourceFor(plan.material)}
      <div class="small" style="margin:0.4rem 0">
        <strong>{plan.needed} {plan.material}</strong> <span class="muted">{src?.kind ? src.kind.toLowerCase() : ""}</span> — collect, or farm something and trade it in:
        <div class="table-wrap">
          <table>
            <thead><tr><th>Site</th><th class="r">Collect</th><th></th><th>Then</th><th class="r">ly</th><th></th></tr></thead>
            <tbody>
              {#each plan.options as o}
                <tr>
                  {#if witnessed(o)}
                    <td>{place(o.system, o.body)} <span class="muted">· {o.site}</span></td>
                  {:else}
                    <td>{o.site}{place(o.system, o.body) ? ` · ${place(o.system, o.body)}` : ""}</td>
                  {/if}
                  <td class="r num">{o.collect}</td><td>{o.farm_material}</td>
                  <td class="muted">{o.rate ? `trade ${o.rate} → ${o.get} ${plan.material} at a ${o.kind} trader` : "use directly"}</td>
                  <td class="r num">{o.distance_ly != null ? o.distance_ly.toFixed(0) : "?"}</td>
                  <td><Place system={o.system} /></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if src}
          {#each src.known.filter((k) => !k.system) as k}
            <div class="muted" style="margin:0.15rem 0 0 1rem"><strong>{k.site}</strong>: {k.method}</div>
          {/each}
          {#each src.methods as m}
            <div class="muted" style="margin:0.15rem 0 0 1rem">{m}</div>
          {/each}
        {/if}
      </div>
    {/each}
    {#each ownSources as src}
      <div class="small source" style="margin:0.5rem 0">
        <strong>{src.needed} {src.material}</strong> <span class="muted">{src.kind ? src.kind.toLowerCase() : ""}</span> — where to get it:
        {#if src.witnessed.length}
          <div style="margin:0.2rem 0 0.2rem 1rem"><span class="ok">You picked it up before</span>
            {#each src.witnessed as w}
              <span class="pill">{place(w.system, w.body)} · {w.count} unit{w.count === 1 ? "" : "s"} over {w.pickups} pickup{w.pickups === 1 ? "" : "s"}{w.distance_ly != null ? ` · ${w.distance_ly.toFixed(0)} ly` : ""} <Place system={w.system} /></span>
            {/each}
          </div>
        {/if}
        {#each src.known as k}
          <div style="margin:0.2rem 0 0.2rem 1rem"><strong>{k.site}</strong>{place(k.system, k.body) ? ` · ${place(k.system, k.body)}` : ""}{k.distance_ly != null ? ` · ${k.distance_ly.toFixed(0)} ly` : ""}: <span class="muted">{k.method}</span> <Place system={k.system} /></div>
        {/each}
        {#each src.methods as m}
          <div class="muted" style="margin:0.15rem 0 0 1rem">{m}</div>
        {/each}
        {#if !src.witnessed.length && !src.known.length && !src.methods.length}
          <div class="muted" style="margin:0.15rem 0 0 1rem">No source for it in EDDA's data yet: not in your journal, no community site listed, and its kind has no method written. Worth a Feedback note.</div>
        {/if}
      </div>
    {/each}
  {/if}
{/if}

<style>
  .source { border-top: 1px solid var(--line); padding-top: 0.4rem; }
  tr.dim { opacity: 0.5; }
  .mini { font-size: 0.7rem; padding: 0 0.4rem; }
</style>
