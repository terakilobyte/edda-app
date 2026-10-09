<script>
  // The toast stack: bottom-right, newest last, click to dismiss. Lives
  // in App.svelte so a notice from any store reaches the commander on
  // whichever tab is showing.
  import { toasts, dismiss } from "./toast.svelte.js";
</script>

{#if toasts.items.length}
  <div class="toasts" aria-live="polite">
    {#each toasts.items as t (t.id)}
      <button class="toast {t.kind}" type="button" onclick={() => dismiss(t.id)} title="Click to dismiss">{t.text}</button>
    {/each}
  </div>
{/if}

<style>
  .toasts {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-width: min(32rem, calc(100vw - 2rem));
    z-index: 60;
  }
  .toast {
    text-align: left;
    font: inherit;
    color: var(--text);
    background: var(--panel-2);
    border: 1px solid var(--line);
    border-left: 4px solid var(--accent);
    font-size: 0.9rem;
    border-radius: 6px;
    padding: 0.6rem 0.8rem;
    box-shadow: 0 6px 20px #0008;
    cursor: pointer;
    line-height: 1.35;
  }
  .toast.warn { border-left-color: var(--warn); }
  .toast.error { border-left-color: var(--bad); }
  .toast.ok { border-left-color: var(--ok); }
</style>
