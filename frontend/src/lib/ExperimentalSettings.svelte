<script>
  // Settings > Experimental: features that ship marked, not hidden (the
  // maintainer, 2026-10-24: a release may carry them once single-account
  // play is known to be untouched). First entry: multiple commanders.
  // There is no master switch for it: one database per commander is how
  // EDDA keeps two accounts apart, and switching that off would mean
  // blending them again, which was the bug.
  import { onMount } from "svelte";
  import { otherCommanderAlerts, otherCommanderAlertsSet } from "./api.js";
  let { commanders = [] } = $props();
  const selected = $derived(commanders.find((c) => c.selected));
  const mb = (b) => `${(b / 1e6).toFixed(1)} MB`;
  // Danger-only alerts from the commanders NOT selected; off by default.
  let otherAlerts = $state(false);
  onMount(() => { otherCommanderAlerts().then((v) => (otherAlerts = v)).catch(() => {}); });
  async function setOtherAlerts(on) {
    otherAlerts = on;
    try { await otherCommanderAlertsSet(on); } catch {}
  }
</script>

<section class="panel">
  <h2>Experimental <span class="sub">new in this build and flown by few · what breaks, tell us</span></h2>

  <h3>Multiple commanders <span class="pill warn">experimental</span></h3>
  <p class="muted small">
    Two or more of your own accounts writing to one journal folder, as a second instance of the game on this PC does.
    EDDA keeps one database per commander, never mixes them, and follows one at a time: the tabs, the voice and the HUD are the selected commander's.
    When a second commander appears, a selector shows above the tabs. With one account nothing changes.
  </p>
  <dl class="kv">
    <dt>Commanders</dt>
    <dd>
      {#if commanders.length > 1}
        {#each commanders as c (c.fid)}<div><strong>{c.name || c.fid}</strong> <span class="muted">{mb(c.store_bytes)}{c.selected ? " · selected" : ""}</span></div>{/each}
      {:else if commanders.length === 1}
        One commander in this journal folder: <strong>{commanders[0].name || commanders[0].fid}</strong> <span class="muted">({mb(commanders[0].store_bytes)})</span>. The selector appears when a second one logs in.
      {:else}
        <span class="muted">No commander found yet; EDDA reads the folder on its first sync.</span>
      {/if}
    </dd>
  </dl>
  <p class="muted small" style="margin:0.5rem 0 0">
    <label><input type="checkbox" checked={otherAlerts} onchange={(e) => setOtherAlerts(e.target.checked)} />
    Alerts from the unselected commander</label> — danger only: under attack, interdicted, overheating, destroyed, carrier jump scheduled. Everything else from that account stays silent. Off by default.
  </p>
  <p class="muted small" style="margin:0.5rem 0 0">
    Known limits: the Frontier account link is the selected commander's and is dropped on a switch; when a database that held both accounts is split, each commander's copy keeps the bookmarks and the journal is read again; a commander who sat at the menu for an hour is picked up again on the next journal write.
    {#if selected}Following <strong>{selected.name || selected.fid}</strong> now.{/if}
  </p>
</section>
