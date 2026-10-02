<script>
  // One renderer for "a place you can go": the system name, a route arrow
  // that asks the Route tab to plot there, and a copy button for the galaxy
  // map search box. Every result list uses it (maintainer, 2026-10-02:
  // "we should also have the ability to click an icon to navigate there,
  // same as we do on every other system result page … wire this into a
  // unified flow/renderer, these inconsistencies make us look unpolished").
  // Before: Market and Galaxy had an arrow, the build planner and the
  // shopping list a text button, Trade a copy button, Mining, Engineers and
  // Powerplay nothing.
  import { requestRoute } from "./route.svelte.js";
  import { copyText, copied } from "./clipboard.svelte.js";
  /** @type {{ system: string|null|undefined, station?: string|null, muted?: boolean, route?: boolean }} */
  let { system, station = null, muted = false, route = true } = $props();
  const label = $derived(station ? `${station}, ${system}` : system);
</script>

{#if system}<span class="place {muted ? 'muted' : ''}"><span class="name">{system}</span>{#if route}<button class="icon route" onclick={() => requestRoute(system)} title="Plot a route to {label}" aria-label="Plot a route to {label}">➤</button>{/if}<button class="icon copy" onclick={() => copyText(system)} title="Copy system name for the galaxy map" aria-label="Copy {system}">{copied.text === system ? "✓" : "⧉"}</button></span>{:else}<span class="muted">—</span>{/if}

<style>
  .place { white-space: nowrap; }
  .icon { appearance: none; border: 0; background: transparent; padding: 0 0.2rem; cursor: pointer; line-height: 1; font-size: 0.9em; vertical-align: baseline; }
  .icon.route { color: var(--accent); }
  .icon.route:hover { color: var(--accent-2); filter: none; transform: translateX(1px); }
  .icon.copy { color: var(--muted); font-size: 0.85em; }
  .icon.copy:hover { color: var(--accent); filter: none; }
</style>
