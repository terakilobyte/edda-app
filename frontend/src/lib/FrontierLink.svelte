<script>
  // The Frontier link card: Link / paste the code / Unlink, the state in
  // words, and what the last fetch said. Lives in Settings → Frontier account.
  import { onMount } from "svelte";
  import { capiStatus, capiLinkStart, capiLinkCode, capiUnlink, capiRefreshCarrier, onCapiState } from "./api.js";
  import { fmtInt, fmtAge } from "./format.js";
  import { useListeners } from "./lifecycle.svelte.js";

  let status = $state(null);
  let msg = $state("");
  let pasted = $state("");
  let busy = $state(false);
  const listeners = useListeners();

  async function load() { try { status = await capiStatus(); } catch (e) { msg = String(e); } }
  onMount(() => { load(); listeners.add(onCapiState((e) => { status = e.payload; })); });

  const run = async (fn, okMsg) => {
    busy = true; msg = "";
    try { const r = await fn(); if (r && typeof r === "object") status = r; if (typeof r === "string") msg = r; else if (okMsg) msg = okMsg; }
    catch (e) { msg = String(e); }
    finally { busy = false; }
  };
  const ageHours = (iso) => iso ? (Date.now() - new Date(iso)) / 3600e3 : null;
  const stateLine = (s) => ({
    unlinked: "Not linked.",
    linked: "Linked.",
    expired: "The link has expired — Frontier logins last about 25 days, and linking on another PC unlinks this one. Link again.",
    wrong_account: "That Frontier login is not the account this journal belongs to; nothing was kept. Link with the account that plays this commander.",
    unavailable: "This build carries no Frontier client id, so linking is not available.",
  })[s] ?? s;
</script>

{#if status}
  <p class="small"><strong>{stateLine(status.link)}</strong></p>
  <div class="row" style="gap:0.5rem; flex-wrap:wrap; margin:0.4rem 0">
    {#if status.link !== "linked"}
      <button disabled={busy || status.link === "unavailable"} onclick={() => run(capiLinkStart)}>Link Frontier account</button>
    {:else}
      <button class="quiet" disabled={busy} onclick={() => run(capiRefreshCarrier, "Carrier refreshed.")}>Refresh carrier now</button>
      <button class="quiet" disabled={busy} onclick={() => run(capiUnlink, "Unlinked. The token is gone from this PC.")}>Unlink</button>
    {/if}
  </div>
  {#if status.link !== "linked" && status.link !== "unavailable"}
    <details>
      <summary class="small">Browser didn't open EDDA? Paste the address it landed on</summary>
      <div class="row" style="gap:0.4rem; margin-top:0.3rem">
        <input type="text" placeholder="edda://auth?code=…&state=…" bind:value={pasted} style="flex:1; min-width:16rem" />
        <button class="quiet" disabled={busy || !pasted.trim()} onclick={() => run(() => capiLinkCode(pasted).then((r) => { pasted = ""; return r; }), "Linked.")}>Use it</button>
      </div>
    </details>
  {/if}
  {#if status.profile}
    <p class="muted small">Frontier knows you as <strong>{status.profile.commander ?? "?"}</strong>{status.profile.credits != null ? ` · ${fmtInt(status.profile.credits)} cr` : ""}{status.profile.ship_name ? ` · flying ${status.profile.ship_name}` : ""}{status.profile.docked_at ? ` · docked at ${status.profile.docked_at}` : ""} · as of {fmtAge(ageHours(status.profile.fetched_at))}</p>
  {/if}
  {#if status.carrier}
    <p class="muted small">Carrier <strong>{status.carrier.name ?? status.carrier.callsign}</strong> ({status.carrier.callsign}): {fmtInt(status.carrier.hold_t)} t in the hold across {status.carrier.hold.length} commodit{status.carrier.hold.length === 1 ? "y" : "ies"}, tank {fmtInt(status.carrier.fuel_t)} t, balance {fmtInt(status.carrier.balance_cr)} cr · fetched {fmtAge(ageHours(status.carrier.fetched_at))} ago. Full hold on the Ships tab.</p>
  {:else if status.link === "linked" && status.no_carrier}
    <p class="muted small">Frontier reports no fleet carrier on this account.</p>
  {/if}
  {#if status.last_error}<p class="error small">{status.last_error}</p>{/if}
  {#if msg}<p class="small">{msg}</p>{/if}
{:else}
  <p class="muted small">{msg || "…"}</p>
{/if}
