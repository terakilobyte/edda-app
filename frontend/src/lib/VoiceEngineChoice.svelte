<script>
  // The voice engine picker (2026-10-16): four choices — Windows voice,
  // Piper, a speech server, and "no voice" — a pill naming what actually
  // speaks, and the one place every installed voice can be removed
  // (maintainer: "if someone doesn't want kokoro anymore they have to
  // figure out where it installed and delete it"). Pure presentation;
  // the panel owns the calls.
  /** @type {{ engine: "windows"|"piper"|"server"|"none", backend: string, serverConfigured: boolean, installed: {engine:string,label:string,bytes:number}[], removing?: boolean, onPick: (which: string) => void, onRemoveAll: () => void }} */
  let { engine, backend, serverConfigured = false, installed = [], removing = false, onPick = () => {}, onRemoveAll = () => {} } = $props();

  const speaking = (b) => (b === "server" ? "speech server" : b === "piper" ? "Piper neural" : b === "sapi" ? "Windows voice" : "no voice");
  const pillClass = (b) => (b === "server" ? "cyan" : b === "piper" ? "ok" : b === "sapi" ? "warn" : "bad");
  const mb = (bytes) => (bytes >= 1024 * 1024 * 1024 ? `${(bytes / 1024 ** 3).toFixed(1)} GB` : `${Math.max(1, Math.round(bytes / 1048576))} MB`);
  const total = $derived(installed.reduce((n, v) => n + v.bytes, 0));
</script>

<div class="row">
  <label><input type="radio" name="voice-engine" value="windows" checked={engine === "windows"} onchange={() => onPick("windows")} /> Windows voice</label>
  <label><input type="radio" name="voice-engine" value="piper" checked={engine === "piper"} onchange={() => onPick("piper")} /> Piper (neural, inside EDDA)</label>
  <label><input type="radio" name="voice-engine" value="server" checked={engine === "server"} onchange={() => onPick("server")} /> speech server (Kokoro or compatible)</label>
  <label><input type="radio" name="voice-engine" value="none" checked={engine === "none"} onchange={() => onPick("none")} /> no voice</label>
  <span class="pill {pillClass(backend)}">{speaking(backend)}</span>
  {#if serverConfigured && backend !== "server" && engine !== "none"}<span class="pill warn" title="The configured speech server is not answering; EDDA fell back to the next voice. Retry the server or pick another engine.">server configured, not running — using {speaking(backend)}</span>{/if}
</div>
{#if engine === "none"}
  <p class="muted small" style="margin:0.3rem 0 0">Nothing is spoken and all callouts are muted. Callouts still show on the HUD. Pick another engine to hear EDDA again.</p>
{/if}
{#if installed.length}
  <div class="row installed" style="margin-top:0.4rem">
    <span class="muted small">Installed voices: {installed.map((v) => `${v.label} ${mb(v.bytes)}`).join(" · ")}</span>
    <button class="ghost" onclick={onRemoveAll} disabled={removing} title="Deletes Kokoro, every Piper voice and the Piper engine from EDDA's data folder. Voice input models stay.">{removing ? "Removing…" : `Remove installed voices (${mb(total)})`}</button>
  </div>
{/if}

<style>
  .installed { flex-wrap: wrap; }
</style>
