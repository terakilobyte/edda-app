<script>
  // EDDA's managed Kokoro, as the Voice tab sees it (2026-10-17). Until now
  // Kokoro could only be installed from setup; the Voice tab probed a fixed
  // address for it, and the page kept the address it read when it was first
  // shown (maintainer, after removing and reinstalling Kokoro: "it said
  // installed but wouldn't connect to it"). This shows the engine's live
  // state and the one action it calls for. Pure presentation; the panel
  // owns the calls.
  /** @type {{ status: {installed:boolean,running:boolean,ready:boolean,url:string|null,approximate_mb:number,available:boolean,note:string}|null, progress: {engine:string,phase:string,detail:string,fraction:number}|null, busy?: boolean, onInstall: () => void, onStart: () => void }} */
  let { status, progress = null, busy = false, onInstall = () => {}, onStart = () => {} } = $props();
  const mb = (n) => (n >= 1000 ? `${(n / 1000).toFixed(1)} GB` : `${n} MB`);
</script>

<div class="row managed">
  {#if !status}
    <span class="muted small">Checking Kokoro…</span>
  {:else if status.running && status.ready}
    <span class="pill ok" title={status.url ? `Answering at ${status.url}` : "Answering"}>Kokoro running</span>
  {:else if status.running}
    <span class="pill warn" title="The process is up and loading its model; it answers in about a minute">Kokoro starting…</span>
  {:else if status.installed}
    <span class="pill warn">Kokoro installed, not running</span>
    <button class="quiet" onclick={onStart} disabled={busy}>{busy ? "Starting…" : "Start and use"}</button>
  {:else}
    <span class="muted small">Kokoro is not installed on this computer.</span>
    <button class="quiet" onclick={onInstall} disabled={busy || !status.available} title={status.note}>{busy ? "Installing…" : status.available ? `Download and install (about ${mb(status.approximate_mb)})` : "Not available yet"}</button>
  {/if}
</div>
{#if progress && progress.engine === "kokoro" && progress.phase !== "ready"}
  <div class="bar" style="margin-top:0.3rem"><div class="bar-fill" style="width:{Math.round(Math.max(0, Math.min(1, progress.fraction)) * 100)}%"></div></div>
  <p class="muted small" style="margin:0.2rem 0 0">{progress.detail}</p>
{/if}

<style>
  .managed { flex-wrap: wrap; }
</style>
