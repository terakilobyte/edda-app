<script>
  // A shopping report as the Engineering tab and the Ships tab's build plan
  // both show it: trades from what you carry, nearest traders, farm plans.
  import { requestRoute } from "./route.svelte.js";
  import { traderStatus } from "./engineering.svelte.js";

  /** @type {{ shopping: any, picked: Set<number>, onToggle: (i: number) => void }} */
  let { shopping, picked, onToggle } = $props();

  // Trader kinds needed by the selected trades plus any farm-then-trade plan.
  const neededKinds = $derived(new Set([
    ...(shopping?.list?.trades ?? []).filter((_, i) => picked.has(i)).map((t) => t.kind),
    ...(shopping?.farm ?? []).flatMap((p) => p.options.filter((o) => o.rate).map((o) => o.kind)),
  ]));
</script>

{#if shopping}
  <h3 style="margin-top:0.8rem">Shopping list <span class="muted">material traders · 6:1 per grade up, 3:1 per grade down, ×6 across groups</span></h3>
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
              <span class="pill">{n.station.name} · {n.station.system_name} · {n.distance_ly.toFixed(1)} ly{n.station.distance_to_arrival != null ? ` · ${Math.round(n.station.distance_to_arrival)} ls` : ""}
                <button class="mini" onclick={() => requestRoute(n.station.system_name)} title="Plot a route there in the Route tab">route</button></span>
            {/each}
          </div>
        {:else}
          <span class={status.tone} title={status.title ?? ""}>{status.text}</span>
        {/if}
      </div>
    {/each}
    {#each shopping.farm as plan}
      <div class="small" style="margin:0.4rem 0">
        <strong>{plan.needed} {plan.material}</strong> — collect, or farm something and trade it in:
        <div class="table-wrap">
          <table>
            <thead><tr><th>Site</th><th class="r">Collect</th><th></th><th>Then</th><th class="r">ly</th><th></th></tr></thead>
            <tbody>
              {#each plan.options as o}
                <tr>
                  <td>{o.site}{o.body ? ` · ${o.body}` : ""}</td>
                  <td class="r num">{o.collect}</td><td>{o.farm_material}</td>
                  <td class="muted">{o.rate ? `trade ${o.rate} → ${o.get} ${plan.material} at a ${o.kind} trader` : "use directly"}</td>
                  <td class="r num">{o.distance_ly != null ? o.distance_ly.toFixed(0) : "?"}</td>
                  <td>{#if o.system}<button class="mini" onclick={() => requestRoute(o.system)}>route</button>{/if}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </div>
    {/each}
    {#each shopping.sources ?? [] as src}
      <div class="small source" style="margin:0.5rem 0">
        <strong>{src.needed} {src.material}</strong> <span class="muted">{src.kind ? src.kind.toLowerCase() : ""}</span> — where to get it:
        {#if src.witnessed.length}
          <div style="margin:0.2rem 0 0.2rem 1rem"><span class="ok">You picked it up before</span>
            {#each src.witnessed as w}
              <span class="pill">{w.system}{w.body ? ` · ${w.body}` : ""} · {w.count} unit{w.count === 1 ? "" : "s"} over {w.pickups} pickup{w.pickups === 1 ? "" : "s"}{w.distance_ly != null ? ` · ${w.distance_ly.toFixed(0)} ly` : ""}
                <button class="mini" onclick={() => requestRoute(w.system)} title="Plot a route there in the Route tab">route</button></span>
            {/each}
          </div>
        {/if}
        {#each src.known as k}
          <div style="margin:0.2rem 0 0.2rem 1rem"><strong>{k.site}</strong>{k.system ? ` · ${k.system}${k.body ? ` ${k.body}` : ""}` : ""}{k.distance_ly != null ? ` · ${k.distance_ly.toFixed(0)} ly` : ""}: <span class="muted">{k.method}</span>{#if k.system}<button class="mini" onclick={() => requestRoute(k.system)} style="margin-left:0.3rem">route</button>{/if}</div>
        {/each}
        {#each src.methods as m}
          <div class="muted" style="margin:0.15rem 0 0 1rem">{m}</div>
        {/each}
      </div>
    {/each}
  {/if}
{/if}

<style>
  .source { border-top: 1px solid var(--line); padding-top: 0.4rem; }
  tr.dim { opacity: 0.5; }
  .mini { font-size: 0.7rem; padding: 0 0.4rem; }
</style>
