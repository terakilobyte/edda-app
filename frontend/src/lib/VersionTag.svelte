<script>
  // The running EDDA's version beside the wordmark; "-dev" on the dev app.
  let { version = "" } = $props();
</script>

{#if version}<span class="version" title="EDDA version">v{version}</span>{/if}

<style>
  .version { font-family: var(--mono); font-size: 0.7rem; font-weight: normal; letter-spacing: 0; text-transform: none; color: var(--muted); }
</style>
