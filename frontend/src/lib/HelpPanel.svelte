<script>
  import { topics, routeLegend } from "./helpTopics.js";
  import { help } from "./help.svelte.js";
  import { KEYS, writeKey } from "./storage.svelte.js";

  // "settings:follow" opens the Settings tab on its Route control section:
  // the section nav reads this key when the tab mounts.
  function go(target) {
    const [tab, section] = target.split(":");
    if (tab === "settings" && section) writeKey(KEYS.settingsSection, section);
    open(tab);
  }

  // `open` is App's tab switcher; topic chips use it to jump elsewhere.
  let { open = () => {} } = $props();

  let active = $state(topics[0].id);
  let root;

  function scrollTo(id) {
    active = id;
    root?.querySelector(`#help-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" });
  }

  // A "?" somewhere in the app asked for a topic.
  $effect(() => {
    help.requested;
    if (help.topic) {
      const id = help.topic;
      help.topic = null;
      // Wait for the tab switch to render us.
      setTimeout(() => scrollTo(id), 0);
    }
  });
</script>

<section class="panel" bind:this={root}>
  <h2>Help</h2>
  <div class="license"><b>EDDA is free; enjoy.</b> No subscription, license key, activation, payment, or feature gating. EDDA is an unofficial fan-made companion and is not affiliated with or endorsed by Frontier Developments; Elite Dangerous and related names remain their owners' property.</div>

  <div class="layout">
    <nav class="toc" aria-label="Help topics">
      {#each topics as t}
        <button class:active={active === t.id} onclick={() => scrollTo(t.id)}>{t.title}</button>
      {/each}
    </nav>

    <div class="content">
      {#each topics as t}
        <article id="help-{t.id}">
          <h3>{t.title}</h3>
          {#each t.body as p}<p class="small">{p}</p>{/each}
          {#if t.tip}<p class="tip small"><b>Tip</b> {t.tip}</p>{/if}
          {#if t.keys}
            <p class="small keys">
              {#each t.keys as [key, what]}<span><kbd>{key}</kbd> {what}</span>{/each}
            </p>
          {/if}
          {#if t.legend}
            <div class="legend small">
              {#each routeLegend as [icon, what]}
                <span><b class="icon">{icon}</b> {what}</span>
              {/each}
            </div>
          {/if}
          {#if t.tabs}
            <p class="chips">
              {#each t.tabs as [tab, label]}
                <button class="chip" onclick={() => go(tab)}>{label} →</button>
              {/each}
            </p>
          {/if}
        </article>
      {/each}
    </div>
  </div>
</section>

<style>
  .license { padding: 0.7rem; margin: 0.4rem 0 1rem; border: 1px solid var(--line); border-radius: 6px; background: var(--panel-2); font-size: 0.85rem; }
  .layout { display: grid; grid-template-columns: 11rem 1fr; gap: 1.2rem; align-items: start; }
  .toc { position: sticky; top: 0.5rem; display: flex; flex-direction: column; gap: 0.15rem; }
  .toc button { text-align: left; padding: 0.25rem 0.5rem; border: none; background: transparent; color: var(--muted); border-left: 2px solid var(--line); border-radius: 0; font-size: 0.82rem; cursor: pointer; }
  .toc button:hover { color: var(--fg); }
  .toc button.active { color: var(--accent); border-left-color: var(--accent); }
  article { scroll-margin-top: 0.5rem; }
  h3 { margin: 1.1rem 0 0.3rem; font-size: 0.95rem; }
  article:first-child h3 { margin-top: 0; }
  p { margin: 0 0 0.4rem; max-width: 70ch; line-height: 1.45; }
  .tip { border-left: 2px solid var(--accent); padding: 0.35rem 0.6rem; background: var(--panel-2); border-radius: 0 4px 4px 0; max-width: 60ch; }
  .keys span { margin-right: 1rem; }
  kbd { border: 1px solid var(--line); border-bottom-width: 2px; border-radius: 4px; padding: 0 0.35rem; font-size: 0.8rem; background: var(--panel-2); }
  .legend { display: flex; flex-wrap: wrap; gap: 0.4rem 1.1rem; padding: 0.45rem 0.6rem; border: 1px solid var(--line); border-radius: 6px; background: var(--panel-2); max-width: 60ch; }
  .legend .icon { color: var(--accent); min-width: 1.6rem; display: inline-block; }
  .chips { display: flex; flex-wrap: wrap; gap: 0.4rem; }
  .chip { font-size: 0.78rem; padding: 0.15rem 0.55rem; border: 1px solid var(--line); border-radius: 999px; background: transparent; color: var(--muted); cursor: pointer; }
  .chip:hover { color: var(--accent); border-color: var(--accent); }
  @media (max-width: 760px) { .layout { grid-template-columns: 1fr; } .toc { position: static; flex-direction: row; flex-wrap: wrap; } .toc button { border-left: none; } }
</style>
