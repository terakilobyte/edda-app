<script>
  // Missions from the journal, including game-reported cargo-depot progress.
  import { onDestroy } from "svelte";
  import { missions, missionStack, missionsHere, speculativeMissions, speculativeMissionsSet } from "./api.js";
  import { fmtCr, fmtTs } from "./format.js";
  import { journalResource } from "./lifecycle.svelte.js";
  import { KEYS, persisted } from "./storage.svelte.js";
  import { giverLabel, giverTitle, stackSummary, stackEconomics, handInsLabel } from "./stacking.js";

  let list = $state([]);
  let showAll = $state(false);
  // Stacking mode (maintainer, 2026-09-16): the HUD lists every giver
  // already holding a massacre against the target. Storage is the bus to
  // the HUD window; the board is shown here too, where the box is ticked.
  const stackingMode = persisted(KEYS.stackingMode, false, { sync: true });
  let stack = $state(null);
  // What is ready to hand in where you are docked (None otherwise).
  let here = $state(null);
  onDestroy(() => stackingMode.dispose());

  const res = journalResource(async () => { list = await missions(!showAll); stack = await missionStack(); here = await missionsHere(); });
  // The opt-in kill estimate (boss, 2026-10-03: "let's make it a checkbox"
  // after users asked again). Off by default: the journal cannot see every
  // kill the game credits, so what it shows is a floor, labelled as such.
  let estimate = $state(false);
  speculativeMissions().then((v) => (estimate = v)).catch(() => {});
  async function setEstimate(on) {
    estimate = on;
    try { await speculativeMissionsSet(on); } catch {}
    res.refresh?.();
    list = await missions(!showAll);
  }
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
    <label class="check small" style="margin-left:auto; font-weight:normal" title="Count the kills your journal saw towards each massacre and assassination. A floor, not the game's tally: a target that dies before your scan completes writes no journal entry, so the game's own count can run ahead. The mission still completes only when the game says so."><input type="checkbox" checked={estimate} onchange={(e) => setEstimate(e.currentTarget.checked)} /> estimate kill progress <span class="pill warn">speculative</span></label>
    <label class="tog" title="On the HUD, list every faction you already hold a massacre from against this target, so you never accept a second from the same giver: those progress one after another, not together."><input type="checkbox" checked={stackingMode.value} onchange={(e) => (stackingMode.value = e.currentTarget.checked)} /> stacking mode</label>
    <label class="tog"><input type="checkbox" bind:checked={showAll} onchange={refresh} /> history</label>
  </h2>
  {#if error}<p class="error">{error}</p>{/if}

  {#if !showAll}
    <div class="stat-grid" style="margin-bottom:0.7rem">
      <div class="stat"><div class="label">In play</div><div class="value">{list.length}</div></div>
      <div class="stat"><div class="label">Ready to turn in</div><div class="value ok">{list.filter((m) => m.status === "ready_to_turn_in").length}</div></div>
      <div class="stat"><div class="label">Rewards pending</div><div class="value">{fmtCr(rewardTotal)}</div></div>
      {#if here}
        <div class="stat" title={here.missions.map((m) => m.title).join("\n")}><div class="label">Ready here · {here.station}</div><div class="value ok">{here.missions.length} · {fmtCr(here.credits)}</div></div>
      {/if}
    </div>
  {/if}

  {#if stackingMode.value}
    <div class="board">
      {#if stack}
        <span class="pill accent">{stack.target_faction}</span>
        <span class="muted small">{stackSummary(stack)}</span>
        {#each stack.givers as g (g.faction)}
          <span class="pill {g.duplicate ? 'warn' : g.ready === g.missions ? 'ok' : ''}" title={giverTitle(g)}>{giverLabel(g)}</span>
        {/each}
        {#if stackEconomics(stack)}<div class="small muted" style="flex-basis:100%" title="Every figure is a stated field summed: kill counts and rewards as the game gave them. Kills that clear the stack are the largest per-giver sum; credited is every kill count added, because one kill counts for every giver at once.">{stackEconomics(stack)}</div>{/if}
      {:else}
        <span class="muted small">No massacre missions in play — the board fills as you accept them.</span>
      {/if}
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
              <td><strong>{m.title}</strong>{#if m.wing}<span class="pill wing" title="Wing mission: kills by any wing member count">wing</span>{/if}
                {#if m.target_faction}<div class="muted small">target: {m.target ?? m.target_faction}{m.target && m.target_faction ? ` (${m.target_faction})` : ""}</div>{/if}
                {#if m.commodity}<div class="muted small">{m.count ?? ""} {m.commodity}</div>{/if}
              </td>
              <td class="small">{m.faction}</td>
              <td>
                {#if m.kill_count}
                  {#if m.kills_seen != null}
                    <div class="num" title={m.kills_exact ? "The game's own count, worked out exactly: a mission that shared this target just completed, and the kills it was credited since this one joined are known." : "Kills your journal saw, credited the way the game does (in the mission's system, one mission per giver at a time). A floor: the game's own tally can run ahead. The row turns ready only when the game says so."}>{m.kills_exact ? "=" : "≥"} {m.kills_seen} / {m.kill_count} <span class="muted small">{m.kills_exact ? "exact" : "seen"}</span></div>
                    <div class="bar" style="width:90px"><div class="bar-fill" style="width:{Math.min(100, (100 * m.kills_seen) / m.kill_count)}%"></div></div>
                  {:else}
                    <div class="num" title="The game's mission panel is the only kill tally there is: kills your ship never scanned leave no journal entry. The row turns ready when the game says so. Tick 'estimate kill progress' above for a floor from your own kill events.">{m.kill_count} kills</div>
                  {/if}
                  {#if m.destination_system}<div class="muted small">in {m.destination_system}</div>{/if}
                {:else if m.total_items_to_deliver}
                  <div class="num">{m.items_delivered} / {m.total_items_to_deliver} delivered</div>
                  <div class="bar" style="width:110px"><div class="bar-fill" style="width:{Math.min(100, (100 * m.items_delivered) / m.total_items_to_deliver)}%"></div></div>
                  <div class="muted small">{Math.max(0, m.items_collected - m.items_delivered)} aboard · {m.items_collected} collected</div>
                {:else if m.kind === "assassinate"}
                  {m.status === "ready_to_turn_in" ? "target down" : m.kills_seen ? "target down (seen)" : "target alive"}
                {:else}
                  <span class="muted">—</span>
                {/if}
              </td>
              <td class="small">{m.hand_in_station ?? "—"}<div class="muted">{m.hand_in_system ?? ""}</div></td>
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
  /* The game's blue wing triangle, as a word. */
  .pill.wing { margin-left: 0.35rem; color: #7ec8ff; border-color: #7ec8ff66; }
  .board { display: flex; flex-wrap: wrap; align-items: center; gap: 0.3rem 0.4rem; margin-bottom: 0.7rem; }
  .tog { margin-left: auto; font-weight: 400; text-transform: none; letter-spacing: 0; color: var(--muted); display: inline-flex; gap: 0.3rem; align-items: center; }
</style>
