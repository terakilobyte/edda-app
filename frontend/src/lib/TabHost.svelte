<script>
  // One tab's home. Tabs used to be `{#if tab === k}<Panel/>{/if}`, which
  // destroyed the panel on every switch and rebuilt it from nothing on the
  // way back -- every table re-fetched, every scroll and toggle lost
  // (maintainer, 2026-09-16: "why do tabs dynamically reload their info
  // every time I switch back and forth? why are we not persisting this
  // info and then just updating elements that need updating?"). A panel
  // now stays mounted once visited and is merely hidden; `journalResource`
  // asks this context whether its panel is showing, pauses its refreshes
  // while it is not, and catches up once when it is shown again.
  import { setContext } from "svelte";
  import { TAB_ACTIVE } from "./lifecycle.svelte.js";
  let { active = false, children } = $props();
  setContext(TAB_ACTIVE, () => active);
</script>

<div class="tab-host" hidden={!active}>{@render children()}</div>

<style>
  .tab-host { display: contents; }
  .tab-host[hidden] { display: none; }
</style>
