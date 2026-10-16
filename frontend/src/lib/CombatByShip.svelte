<script>
  // Combat by ship: every ship that fought, ranked by kills, and the
  // sessions of the one picked. Pure presentation over the store's
  // `combat_by_ship` / `combat_sessions` rows (2026-10-16 spec); the
  // numbers come from the journal, not the build.
  import { fmtCrShort, fmtDur, fmtInt, fmtTs } from "./format.js";

  /** @type {{ ships: any[], sessions: any[], selected: number|null, onPick: (shipId: number|null) => void }} */
  let { ships = [], sessions = [], selected = null, onPick = () => {} } = $props();

  const shipLabel = (s) => (s.ship_name ? `${s.ship_name} · ${s.hull}` : s.hull);
  const perHour = (n, secs) => (secs > 0 ? (n / (secs / 3600)).toFixed(1) : "—");
  const crPerHour = (cr, secs) => (secs > 0 ? fmtCrShort(cr / (secs / 3600)) : "—");
  const ttk = (s) => (s == null ? "—" : `${s} s`);
  const closer = (c) => c == null ? "open" : ({ supercruiseentry: "to supercruise", fsdjump: "jumped", docked: "docked", shutdown: "quit", loadgame: "relaunched", died: "destroyed", loadout: "ship swap" }[c] ?? c);
</script>

{#if ships.length}
  <h3>By ship</h3>
  <p class="muted small">One session is one normal-space instance with a fight in it. Time to kill runs from your first lock on the pilot to the kill; kills with no lock (the fighter's, the turrets') count for the rate and the credits, never for time to kill.</p>
  <div class="table-wrap">
    <table class="byship">
      <thead>
        <tr>
          <th>Ship</th><th class="r">Sessions</th><th class="r">Time</th><th class="r">Kills</th><th class="r">Kills/h</th>
          <th class="r" title="median time to kill, timed kills only">TTK</th><th class="r" title="90th percentile time to kill">p90</th>
          <th class="r" title="kills with a time to kill / kills with the fighter out">Timed / fighter</th>
          <th class="r">Cr/h</th><th class="r">Hull hits</th><th class="r">Deaths</th>
        </tr>
      </thead>
      <tbody>
        {#each ships as s (s.ship_id ?? s.ship)}
          <tr class={selected === s.ship_id ? "on" : ""} onclick={() => onPick(selected === s.ship_id ? null : s.ship_id)}>
            <td>{shipLabel(s)}</td>
            <td class="r num">{fmtInt(s.sessions)}</td>
            <td class="r num">{fmtDur(s.secs)}</td>
            <td class="r num">{fmtInt(s.kills)}</td>
            <td class="r num">{perHour(s.kills, s.secs)}</td>
            <td class="r num">{ttk(s.median_ttk_s)}</td>
            <td class="r num">{ttk(s.p90_ttk_s)}</td>
            <td class="r num">{fmtInt(s.kills_timed)} / {fmtInt(s.kills_fighter)}</td>
            <td class="r num">{crPerHour(s.credits, s.secs)}</td>
            <td class="r num">{fmtInt(s.hull_hits)}</td>
            <td class="r num {s.died ? 'warn' : ''}">{fmtInt(s.died)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

{#if sessions.length}
  <h3>Sessions{selected != null ? ` · ${shipLabel(sessions[0])}` : ""}</h3>
  <div class="table-wrap">
    <table>
      <thead><tr><th>Entered</th>{#if selected == null}<th>Ship</th>{/if}<th>System</th><th class="r">Length</th><th class="r">Fighting</th><th class="r">Kills</th><th class="r">TTK</th><th class="r">Credits</th><th class="r">Attacked</th><th>Ended</th></tr></thead>
      <tbody>
        {#each sessions as s (s.id)}
          <tr>
            <td class="num small">{fmtTs(s.entered_ts)}</td>
            {#if selected == null}<td class="small">{shipLabel(s)}</td>{/if}
            <td class="small">{s.system_name ?? "—"}</td>
            <td class="r num">{fmtDur(s.secs)}</td>
            <td class="r num">{fmtDur(s.fighting_secs)}</td>
            <td class="r num">{fmtInt(s.kills)}<span class="muted small">{s.kills_fighter ? ` (${s.kills_fighter} fighter)` : ""}</span></td>
            <td class="r num">{ttk(s.median_ttk_s)}</td>
            <td class="r num">{fmtCrShort(s.credits)}</td>
            <td class="r num">{fmtInt(s.under_attack)}</td>
            <td class="small {s.died ? 'warn' : ''}">{closer(s.closed_by)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  h3 { font-size: 0.8rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; margin: 0.8rem 0 0.3rem; }
  table.byship tbody tr { cursor: pointer; }
  table.byship tbody tr.on td { color: var(--accent); }
</style>
