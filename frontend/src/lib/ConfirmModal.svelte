<script>
  // The app's one confirmation dialog (2026-10-16, maintainer: "no window
  // alerts, ever. Actually buttons and modals plz"): a scrim and a sheet
  // like the release-notes splash, a title, a few lines, and two buttons.
  // Escape and a click on the scrim cancel. Pure presentation.
  /** @type {{ title: string, lines?: string[], confirmLabel?: string, cancelLabel?: string, danger?: boolean, busy?: boolean, onConfirm: () => void, onCancel: () => void }} */
  let { title, lines = [], confirmLabel = "Confirm", cancelLabel = "Cancel", danger = false, busy = false, onConfirm = () => {}, onCancel = () => {} } = $props();
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && !busy && onCancel()} />

<div class="scrim" role="presentation" onclick={(e) => { if (e.target === e.currentTarget && !busy) onCancel(); }}>
  <div class="sheet" role="dialog" aria-modal="true" aria-label={title} tabindex="-1">
    <header><h2>{title}</h2></header>
    <div class="body">
      {#each lines as line}<p>{line}</p>{/each}
    </div>
    <footer>
      <button class="ghost" onclick={onCancel} disabled={busy}>{cancelLabel}</button>
      <button class={danger ? "danger" : ""} onclick={onConfirm} disabled={busy}>{busy ? "Working…" : confirmLabel}</button>
    </footer>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: #000a; display: grid; place-items: center; z-index: 95; }
  .sheet { background: var(--panel, #14181f); border: 1px solid var(--line, #2a3340); border-radius: 8px; width: min(34rem, 92vw); display: flex; flex-direction: column; }
  header { padding: 0.9rem 1.2rem 0; }
  header h2 { margin: 0; font-size: 1.05rem; }
  .body { padding: 0.4rem 1.2rem; }
  .body p { margin: 0.5rem 0; line-height: 1.45; }
  footer { padding: 0.6rem 1.2rem 1rem; display: flex; justify-content: flex-end; gap: 0.5rem; }
  button.danger { border-color: var(--bad, #e5484d); color: var(--bad, #e5484d); }
</style>
