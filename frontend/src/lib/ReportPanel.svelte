<script>
  // Report a problem — top-bar citizen, not buried in Settings (maintainer,
  // 2026-09-05): the site promises it, so it gets a door of its own.
  import { feedbackSend } from "./api.js";
  let text = $state("");
  let includeLog = $state(false);
  let busy = $state(false);
  let msg = $state("");
  async function send() {
    busy = true;
    msg = "";
    try {
      msg = await feedbackSend(text, includeLog);
      text = "";
      includeLog = false;
    } catch (e) {
      msg = String(e);
    } finally {
      busy = false;
    }
  }
</script>

<section class="panel">
  <h2>Report a problem <span class="sub">anonymous</span></h2>
  <p class="muted small" style="margin:0 0 0.6rem">What went wrong, what you expected, what you were doing. Sent with your app version and OS — nothing that identifies you. <a href="https://edda-app.com/privacy/" target="_blank" rel="noreferrer">Exactly what EDDA collects.</a></p>
  <textarea rows="8" style="width:100%" bind:value={text} placeholder="It said 'throttle down' after I had already become one with the star…"></textarea>
  <div class="row" style="margin-top:0.6rem">
    <label><input type="checkbox" bind:checked={includeLog} /> include today's log <span class="muted small">(may contain system names you visited)</span></label>
    <button onclick={send} disabled={busy || !text.trim()} style="margin-left:auto">{busy ? "Sending…" : "Send report"}</button>
  </div>
  {#if msg}<p class="muted small" style="margin:0.5rem 0 0">{msg}</p>{/if}
</section>
