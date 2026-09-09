<script>
  // Combat record: totals for a window, a timeline chart, and the last kills.
  import { combatSummary, combatTimeline, recentKills } from "./api.js";
  import { fmtCr, fmtCrShort, fmtInt, fmtTs } from "./format.js";
  import { journalResource } from "./lifecycle.svelte.js";

  const windows = [
    ["24h", 1],
    ["7d", 7],
    ["30d", 30],
    ["All", null],
  ];
  let win = $state(7);
  let bucket = $state("day");
  let summary = $state(null);
  let timeline = $state([]);
  let kills = $state([]);

  function sinceIso() {
    if (win == null) return null;
    return new Date(Date.now() - win * 86400e3).toISOString().replace(/\.\d+Z$/, "Z");
  }

  const res = journalResource(async () => {
    const since = sinceIso();
    const b = win === 1 ? "hour" : win === null ? "week" : bucket;
    [summary, timeline, kills] = await Promise.all([
      combatSummary(since),
      combatTimeline(since, b),
      recentKills(25),
    ]);
  });
  const refresh = res.refresh;
  const error = $derived(res.error);

  // Simple SVG bars: kills as bars, credits as a line, no library.
  const W = 640, H = 140, PAD = 24;
  function bars() {
    if (!timeline.length) return [];
    const maxK = Math.max(1, ...timeline.map((t) => t.kills));
    const bw = (W - PAD * 2) / timeline.length;
    return timeline.map((t, i) => ({
      x: PAD + i * bw + 1,
      w: Math.max(1, bw - 2),
      h: ((H - PAD) * t.kills) / maxK,
      t,
    }));
  }
  function creditLine() {
    if (!timeline.length) return "";
    const maxC = Math.max(1, ...timeline.map((t) => t.credits));
    const bw = (W - PAD * 2) / timeline.length;
    return timeline
      .map((t, i) => `${PAD + i * bw + bw / 2},${H - ((H - PAD) * t.credits) / maxC}`)
      .join(" ");
  }
</script>

<section class="panel">
  <h2>Combat record</h2>

  <div class="row" style="margin-bottom:0.6rem">
    {#each windows as [label, days]}
      <button class="quiet {win === days ? 'on' : ''}" onclick={() => { win = days; refresh(); }}>{label}</button>
    {/each}
  </div>

  {#if error}<p class="error">{error}</p>{/if}

  {#if summary}
    <div class="stat-grid" style="margin-bottom:0.7rem">
      <div class="stat"><div class="label">Kills</div><div class="value">{fmtInt(summary.kills)}</div></div>
      <div class="stat"><div class="label">Bounties</div><div class="value">{fmtCrShort(summary.bounty_credits)}</div></div>
      <div class="stat"><div class="label">Bonds</div><div class="value">{fmtCrShort(summary.bond_credits)}</div></div>
      <div class="stat"><div class="label">Deaths</div><div class="value {summary.deaths ? 'warn' : ''}">{summary.deaths}</div></div>
      <div class="stat"><div class="label">Interdicted</div><div class="value">{summary.interdicted}<span class="muted small"> / {summary.escaped} escaped</span></div></div>
      <div class="stat"><div class="label">Cr per kill</div><div class="value">{summary.kills ? fmtCrShort((summary.bounty_credits + summary.bond_credits) / summary.kills) : "—"}</div></div>
    </div>
  {/if}

  {#if timeline.length}
    <div class="chart-wrap">
      <svg viewBox="0 0 {W} {H + 18}" preserveAspectRatio="none" role="img" aria-label="kills per period">
        {#each bars() as b}
          <rect x={b.x} y={H - b.h} width={b.w} height={b.h} fill="var(--accent)" opacity="0.8">
            <title>{b.t.bucket}: {b.t.kills} kills, {fmtCr(b.t.credits)}{b.t.deaths ? `, ${b.t.deaths} deaths` : ""}</title>
          </rect>
          {#if b.t.deaths}
            <circle cx={b.x + b.w / 2} cy={H + 8} r="3" fill="var(--bad)"><title>{b.t.deaths} death(s)</title></circle>
          {/if}
        {/each}
        <polyline points={creditLine()} fill="none" stroke="var(--cyan)" stroke-width="1.5" />
        <text x={PAD} y={H + 15} class="axis">{timeline[0].bucket}</text>
        <text x={W - PAD} y={H + 15} class="axis" text-anchor="end">{timeline[timeline.length - 1].bucket}</text>
      </svg>
      <div class="legend muted small"><span class="sw acc"></span> kills <span class="sw cy"></span> credits <span class="sw bad"></span> deaths</div>
    </div>
  {:else if summary}
    <p class="muted">No combat in this window.</p>
  {/if}

  {#if summary && (summary.by_target.length || summary.by_faction.length)}
    <div class="two">
      <div>
        <h3>Most killed</h3>
        <table><tbody>
          {#each summary.by_target.slice(0, 8) as [ship, n]}
            <tr><td>{ship}</td><td class="r num">{n}</td></tr>
          {/each}
        </tbody></table>
      </div>
      <div>
        <h3>Paid by</h3>
        <table><tbody>
          {#each summary.by_faction.slice(0, 8) as [f, cr]}
            <tr><td>{f}</td><td class="r num">{fmtCrShort(cr)}</td></tr>
          {/each}
        </tbody></table>
      </div>
    </div>
  {/if}

  {#if kills.length}
    <h3>Latest kills</h3>
    <div class="table-wrap">
      <table>
        <thead><tr><th>When</th><th>Target</th><th>Pilot</th><th>Faction</th><th class="r">Reward</th><th>System</th></tr></thead>
        <tbody>
          {#each kills as k}
            <tr>
              <td class="num small">{fmtTs(k.ts)}</td>
              <td>{k.target_ship ?? "—"}<span class="muted small"> {k.kind === "bounty" ? "" : `· ${k.kind.replace(/_/g, " ")}`}</span></td>
              <td class="small">{k.pilot_name ?? "—"}</td>
              <td class="small">{k.faction ?? "—"}</td>
              <td class="r num">{fmtCr(k.reward)}</td>
              <td class="small">{k.system_name ?? "—"}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<style>
  button.on { border-color: var(--accent); color: var(--accent); }
  h3 { font-size: 0.8rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.05em; margin: 0.8rem 0 0.3rem; }
  .chart-wrap { background: var(--bg-2); border: 1px solid var(--line); border-radius: 5px; padding: 0.4rem; }
  svg { width: 100%; height: 160px; display: block; }
  .axis { fill: var(--muted); font-size: 11px; font-family: var(--mono); }
  .legend { display: flex; gap: 0.4rem; align-items: center; margin-top: 0.2rem; }
  .sw { display: inline-block; width: 10px; height: 10px; border-radius: 2px; }
  .sw.acc { background: var(--accent); } .sw.cy { background: var(--cyan); } .sw.bad { background: var(--bad); border-radius: 50%; }
  .two { display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; }
</style>
