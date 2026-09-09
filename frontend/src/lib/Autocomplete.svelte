<script>
  // Text input with name suggestions from the backend. `fetch(prefix)` must
  // return [{name, detail?}]; the chosen name is written back via bind:value.
  let { value = $bindable(""), fetch, placeholder = "", minWidth = "14rem", onenter = () => {}, onchoose = () => {}, onchange = () => {} } = $props();

  let items = $state([]);
  let open = $state(false);
  let active = $state(-1);
  let timer;

  function onInput(e) {
    value = e.target.value;
    onchange(value);
    clearTimeout(timer);
    const v = value.trim();
    if (v.length < 2) { items = []; open = false; return; }
    timer = setTimeout(async () => {
      try { items = (await fetch(v)) ?? []; open = items.length > 0; active = -1; } catch { items = []; open = false; }
    }, 120);
  }

  function choose(i) {
    if (i < 0 || i >= items.length) return;
    value = items[i].name;
    onchoose(items[i]);
    open = false;
  }

  function onKey(e) {
    if (open && e.key === "ArrowDown") { e.preventDefault(); active = Math.min(active + 1, items.length - 1); }
    else if (open && e.key === "ArrowUp") { e.preventDefault(); active = Math.max(active - 1, 0); }
    else if (e.key === "Enter") { if (open && active >= 0) { e.preventDefault(); choose(active); } else { open = false; onenter(); } }
    else if (e.key === "Escape") { open = false; }
  }
</script>

<span class="ac" style="min-width:{minWidth}">
  <input {placeholder} {value} oninput={onInput} onkeydown={onKey} onblur={() => setTimeout(() => (open = false), 150)} onfocus={() => (open = items.length > 0)} autocomplete="off" spellcheck="false" />
  {#if open}
    <ul>
      {#each items as it, i}
        <li class={i === active ? "on" : ""}><button type="button" onmousedown={(e) => { e.preventDefault(); choose(i); }}>{it.name}{#if it.detail}<span class="muted small"> · {it.detail}</span>{/if}</button></li>
      {/each}
    </ul>
  {/if}
</span>

<style>
  .ac { position: relative; display: inline-block; }
  .ac input { width: 100%; }
  ul { position: absolute; top: 100%; left: 0; z-index: 20; list-style: none; margin: 0.15rem 0 0; padding: 0.25rem; min-width: 100%; max-height: 14rem; overflow-y: auto; background: var(--panel-2); border: 1px solid var(--line); border-radius: 5px; display: flex; flex-direction: column; gap: 0.1rem; box-shadow: 0 6px 18px #0009; }
  li button { width: 100%; text-align: left; background: transparent; color: var(--text); border: none; font-weight: 400; padding: 0.25rem 0.5rem; border-radius: 3px; white-space: nowrap; }
  li.on button, li button:hover { background: #ff8c1a22; filter: none; }
</style>
