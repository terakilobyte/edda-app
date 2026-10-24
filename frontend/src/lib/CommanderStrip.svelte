<script>
  // The player's own commanders, one pill each; click selects. Rendered
  // only with two or more: a single-account install looks as it always
  // did. Live dot: the journal wrote in the last minute. Nothing here is
  // another EDDA user — these are the accounts in this PC's journal folder.
  let { commanders = [], onSelect = () => {} } = $props();
  const label = (c) => c.name || c.fid;
  const detail = (c) => [c.ship, c.system].filter(Boolean).join(" · ");
</script>

{#if commanders.length > 1}
  <div class="commanders" role="group" aria-label="Commanders">
    {#each commanders as c (c.fid)}
      <button class="pill {c.selected ? 'ok' : ''}" aria-pressed={c.selected} onclick={() => onSelect(c.fid)} title={detail(c) || label(c)}>
        {#if c.wrote_s_ago != null && c.wrote_s_ago < 60}<span class="live" aria-label="in game now"></span>{/if}
        <strong>{label(c)}</strong>{#if detail(c)} <span class="muted">{detail(c)}</span>{/if}
      </button>
    {/each}
  </div>
{/if}

