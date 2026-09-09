<script>
  import { getInventory } from "./api.js";
  import { journalResource } from "./lifecycle.svelte.js";

  let items = $state([]);
  let filter = $state("");

  const res = journalResource(async () => { items = await getInventory(); });
  const error = $derived(res.error);

  const shown = $derived(
    items.filter((i) => !filter || `${i.name} ${i.category}`.toLowerCase().includes(filter.toLowerCase()))
  );
  const groups = $derived(
    Object.entries(
      shown.reduce((acc, i) => ((acc[i.category] ??= []).push(i), acc), {})
    ).sort(([a], [b]) => a.localeCompare(b))
  );
</script>

<section class="panel">
  <h2>Inventory <span class="sub">materials and cargo, live</span></h2>
  <div class="row" style="margin-bottom:0.5rem">
    <input placeholder="Filter…" bind:value={filter} />
    <span class="muted small">{shown.length} items</span>
  </div>
  {#if error}<p class="error">{error}</p>{/if}
  <div class="groups">
    {#each groups as [category, list]}
      <div class="group">
        <h3>{category} <span class="muted">({list.length})</span></h3>
        <table><tbody>
          {#each list as i}
            <tr><td>{i.name}</td><td class="r num">{i.count}</td></tr>
          {/each}
        </tbody></table>
      </div>
    {/each}
  </div>
</section>

<style>
  .groups { display: grid; grid-template-columns: repeat(auto-fill, minmax(240px, 1fr)); gap: 0.8rem; }
  h3 { font-size: 0.8rem; color: var(--accent); text-transform: uppercase; letter-spacing: 0.05em; margin: 0 0 0.2rem; }
</style>
