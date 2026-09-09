<script>
  // Missions from the journal, including game-reported cargo-depot progress.
  import { missions } from "./api.js";
  import { fmtCr, fmtTs } from "./format.js";
  import { journalResource } from "./lifecycle.svelte.js";

  let list = $state([]);
  let showAll = $state(false);

  const res = journalResource(async () => { list = await missions(!showAll); });
  const refresh = res.refresh;
  const error = $derived(res.error);

  const statusClass = (s) =>
    s === "ready_to_turn_in" ? "ok" : s === "active" ? "accent" : s === "completed" ? "" : "bad";
  const statusLabel = (s) => s.replace(/_/g, " ");

  function hoursLeft(expiry) {
    if (!expiry) return null;
    return (new Date(expiry) - Date.now()) / 3600e3;
  }
  const rewardTotal = $derived(
    list.filter((m) => m.status === "active" || m.status === "ready_to_turn_in").reduce((a, m) => a + (m.reward ?? 0), 0)
  );
</script>

<section class="panel">
  <h2>Missions <span class="sub">from the journal · delivery progress direct from the game</span>
    <label class="tog"><input type="checkbox" bind:checked={showAll} onchange={refresh} /> history</label>
  </h2>
  {#if error}<p class="error">{error}</p>{/if}

  {#if !showAll}
    <div class="stat-grid" style="margin-bottom:0.7rem">
      <div class="stat"><div class="label">In play</div><div class="value">{list.length}</div></div>
      <div class="stat"><div class="label">Ready to turn in</div><div class="value ok">{list.filter((m) => m.status === "ready_to_turn_in").length}</div></div>
      <div class="stat"><div class="label">Rewards pending</div><div class="value">{fmtCr(rewardTotal)}</div></div>
    </div>
  {/if}

  {#if list.length === 0}
    <p class="muted">No missions{showAll ? " recorded" : " in play"}.</p>
  {:else}
    <div class="table-wrap">
      <table>
        <thead><tr><th>Mission</th><th>For</th><th>Progress</th><th>Hand in</th><th class="r">Reward</th><th>Expires</th><th>Status</th></tr></thead>
        <tbody>
          {#each list as m}
            {@const hl = hoursLeft(m.expiry)}
            <tr>
              <td><strong>{m.title}</strong>
                {#if m.target_faction}<div class="muted small">target: {m.target ?? m.target_faction}{m.target && m.target_faction ? ` (${m.target_faction})` : ""}</div>{/if}
                {#if m.commodity}<div class="muted small">{m.count ?? ""} {m.commodity}</div>{/if}
              </td>
              <td class="small">{m.faction}</td>
              <td>
                {#if m.kill_count}
                  <div class="num">{m.kills_done} / {m.kill_count}</div>
                  <div class="bar" style="width:90px"><div class="bar-fill" style="width:{Math.min(100, (100 * m.kills_done) / m.kill_count)}%"></div></div>
                {:else if m.total_items_to_deliver}
                  <div class="num">{m.items_delivered} / {m.total_items_to_deliver} delivered</div>
                  <div class="bar" style="width:110px"><div class="bar-fill" style="width:{Math.min(100, (100 * m.items_delivered) / m.total_items_to_deliver)}%"></div></div>
                  <div class="muted small">{Math.max(0, m.items_collected - m.items_delivered)} aboard · {m.items_collected} collected</div>
                {:else if m.kind === "assassinate"}
                  {m.kills_done ? "target down" : "target alive"}
                {:else}
                  <span class="muted">—</span>
                {/if}
              </td>
              <td class="small">{m.destination_station ?? "—"}<div class="muted">{m.destination_system ?? ""}</div></td>
              <td class="r num">{fmtCr(m.reward)}</td>
              <td class="small {hl != null && hl < 3 ? 'warn' : ''}">{hl == null ? "—" : hl < 0 ? "expired" : hl < 48 ? `${hl.toFixed(1)} h` : `${(hl / 24).toFixed(1)} d`}<div class="muted">{fmtTs(m.expiry).slice(5, 16)}</div></td>
              <td><span class="pill {statusClass(m.status)}">{statusLabel(m.status)}</span></td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<style>
  .tog { margin-left: auto; font-weight: 400; text-transform: none; letter-spacing: 0; color: var(--muted); display: inline-flex; gap: 0.3rem; align-items: center; }
</style>
