<script>
  // Powerplay: your own merit history, the per-station K model, and every
  // system you have seen control recorded in.
  import { meritModel, powerplaySeen, meritTimeline } from "./api.js";
  import { openHelp } from "./help.svelte.js";
  import { fmtInt, fmtTs } from "./format.js";
  import { journalResource } from "./lifecycle.svelte.js";

  let model = $state(null);
  let seen = $state([]);
  let timeline = $state([]);

  const res = journalResource(async () => {
    const since = new Date(Date.now() - 30 * 86400e3).toISOString().replace(/\.\d+Z$/, "Z");
    [model, seen, timeline] = await Promise.all([meritModel(), powerplaySeen(), meritTimeline(since, "day")]);
  });
  const error = $derived(res.error);

  const total = $derived(timeline.length ? timeline[timeline.length - 1].total_at_end : null);
  const last30 = $derived(timeline.reduce((a, b) => a + b.merits, 0));
  const maxDay = $derived(Math.max(1, ...timeline.map((t) => t.merits)));
</script>

<section class="panel">
  <h2>Powerplay <span class="sub">first-hand merits · community control state</span> <button class="ghost" style="margin-left:auto" title="How Powerplay tracking works" aria-label="Help" onclick={() => openHelp("powerplay")}>?</button></h2>
  {#if error}<p class="error">{error}</p>{/if}

  <div class="stat-grid" style="margin-bottom:0.7rem">
    <div class="stat"><div class="label">Total merits</div><div class="value">{fmtInt(total)}</div></div>
    <div class="stat"><div class="label">Last 30 days</div><div class="value">{fmtInt(last30)}</div></div>
    <div class="stat"><div class="label">Awards</div><div class="value">{fmtInt(timeline.reduce((a, b) => a + b.awards, 0))}</div></div>
    <div class="stat"><div class="label">Stations calibrated</div><div class="value">{model?.stations?.length ?? 0}</div></div>
  </div>

  {#if timeline.length}
    <div class="spark">
      {#each timeline as t}
        <div class="col" title="{t.bucket}: {t.merits} merits in {t.awards} awards">
          <div class="fill" style="height:{(100 * t.merits) / maxDay}%"></div>
        </div>
      {/each}
    </div>
    <div class="row small muted" style="justify-content:space-between"><span>{timeline[0].bucket}</span><span>merits per day</span><span>{timeline[timeline.length - 1].bucket}</span></div>
  {/if}

  <h3>Merit rate <span class="muted">profit needed per merit</span></h3>
  <p class="muted small">Learned per station from your own sales: sell 100,000 Cr of profit where the rate is 4,100 Cr/merit and you earn 24 merits. Stations you have not sold at have no estimate.</p>
  {#if model?.stations?.length}
    <div class="table-wrap">
      <table>
        <thead><tr><th>Station</th><th>System</th><th>State</th><th class="r">Sales</th><th class="r">Cr / merit</th><th></th></tr></thead>
        <tbody>
          {#each model.stations as s}
            <tr>
              <td>{s.station ?? s.market_id}</td>
              <td>{s.system ?? "—"}</td>
              <td class="small">{s.controlling_power ?? ""} {s.powerplay_state ?? ""}</td>
              <td class="r num">{s.samples}</td>
              <td class="r num">{s.k_lo.toFixed(0) === s.k_hi.toFixed(0) ? s.k_lo.toFixed(0) : `${s.k_lo.toFixed(0)} – ${s.k_hi.toFixed(0)}`}</td>
              <td>{#if !s.consistent}<span class="pill bad">inconsistent</span>{:else}<span class="pill ok">fits</span>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else}
    <p class="muted">No calibrated stations yet — sell something for a Power and the model starts learning.</p>
  {/if}

  <h3>Control state you have witnessed</h3>
  {#if seen.length}
    <div class="table-wrap">
      <table>
        <thead><tr><th>System</th><th>Power</th><th>State</th><th class="r">Progress</th><th class="r">Reinf.</th><th class="r">Underm.</th><th>Seen</th></tr></thead>
        <tbody>
          {#each seen as p}
            <tr>
              <td>{p.system_name}</td>
              <td>{p.controlling_power ?? "—"}</td>
              <td>{p.powerplay_state ?? "—"}</td>
              <td class="r num">{p.control_progress != null ? `${(p.control_progress * 100).toFixed(1)}%` : "—"}</td>
              <td class="r num">{fmtInt(p.reinforcement)}</td>
              <td class="r num">{fmtInt(p.undermining)}</td>
              <td class="small muted">{fmtTs(p.ts).slice(0, 16)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {:else}
    <p class="muted">No Powerplay readings yet.</p>
  {/if}
</section>

<style>
  h3 { font-size: 0.8rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; margin: 0.9rem 0 0.3rem; }
  .spark { display: flex; align-items: flex-end; gap: 2px; height: 70px; background: var(--bg-2); border: 1px solid var(--line); border-radius: 5px; padding: 4px; }
  .col { flex: 1; height: 100%; display: flex; align-items: flex-end; }
  .fill { width: 100%; background: var(--accent); opacity: 0.85; border-radius: 1px 1px 0 0; }
</style>
