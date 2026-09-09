<script>
  // The profit finder. Ranks by credits per hour, shows why stations were
  // excluded, and labels every time estimate as estimated. State lives in
  // trade.svelte.js so a running search survives switching tabs.
  import { trade, runSearch, stopSearch, sortBy, sortedLegs, LEG_SORTS } from "./trade.svelte.js";
  import { boardLine } from "./tradeView.js";
  import { tradeFollow, followTrade, stopTrade } from "./tradeFollow.svelte.js";
  import { ship } from "./ship.svelte.js";
  // Copy a system name for the galaxy map search box.
  let copied = $state("");
  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = text; document.body.appendChild(ta); ta.select();
      try { document.execCommand("copy"); } catch {}
      ta.remove();
    }
    copied = text;
    setTimeout(() => (copied === text) && (copied = ""), 1200);
  }
  const arrow = (k) => (trade.sortKey === k ? (trade.sortDir < 0 ? " ▼" : " ▲") : "");
  import { powerplayOptions, nameComplete } from "./api.js";
  import { fmtCr, fmtCrShort, fmtLy, fmtLs, fmtMin, fmtAge, fmtInt } from "./format.js";
  import { KEYS, persisted } from "./storage.svelte.js";
  import Autocomplete from "./Autocomplete.svelte";
  import { onMount } from "svelte";

  // Powerplay filter options come from the galaxy tables.
  let pp = $state({ powers: [], states: [], pledged: null });
  onMount(async () => { try { pp = await powerplayOptions(); } catch {} });

  const ageClass = (h) => (h > 48 ? "warn" : "");
  const secs = (ms) => (ms / 1000).toFixed(1);

  // Pin a loop to the HUD. Both windows share localStorage, and the
  // overlay listens for the storage event, so no backend round trip.
  const pinnedLoop = persisted(KEYS.pinnedLoop, null, { json: true });
  const pinned = $derived(pinnedLoop.value);
  function pinLoop(t) {
    const p = t ? {
      a: { station: t.out.from.station, system: t.out.from.system, buy: t.out.commodity, tons: t.out.tons },
      b: { station: t.out.to.station, system: t.out.to.system, buy: t.back.commodity, tons: t.back.tons },
      cr_h: t.profit_per_hour, profit: t.profit,
    } : null;
    pinnedLoop.value = p;
  }
  function pinRing(r) {
    const p = r ? { stops: r.legs.map((l) => ({ station: l.from.station, system: l.from.system, buy: l.commodity, tons: l.tons })), cr_h: r.profit_per_hour, profit: r.profit } : null;
    pinnedLoop.value = p;
  }
  const isPinnedRing = (r) => !!pinned?.stops && pinned.stops.length === r.legs.length && pinned.stops.every((s, i) => s.station === r.legs[i].from.station);
  const isPinned = (t) => !!pinned?.a && pinned.a.station === t.out.from.station && pinned.b.station === t.out.to.station;
</script>

<section class="panel">
  <h2>Profit finder <span class="sub">community prices · ranked by credits per hour</span></h2>

  {#if tradeFollow.active}
    <p class="pinbar">Following {tradeFollow.kind === "ring" ? "ring" : "trade route"} · lap {tradeFollow.lap} ·
      {#each tradeFollow.stops as s, i}{i > 0 ? " → " : ""}<strong class:muted={i !== tradeFollow.at}>{s.station}</strong>{/each}
      <button class="ghost" onclick={stopTrade}>Stop</button>
      {#if tradeFollow.error}<span class="warn small"> {tradeFollow.error}</span>{/if}
    </p>
  {/if}

  <!-- The Profit Finder form, laid out as the maintainer drew it (2026-09-07):
       ship | radius | max price age | carriers | prohibited goods
       min supply | min demand | max arrival distance
       buy in power | controls | state  ·  sell in power | controls | state
       [ ] search for trade rings  ·  how many stops?
       Origin, cargo and jump range come from the selected ship: the one
       being flown is where you are; a stored ship is where it is stored.
       "Other ship…" is the freeform case (a ship you are about to buy). -->
  <div class="row" style="margin-bottom:0.35rem">
    <label>ship <select bind:value={trade.query.shipId}>
      <option value={null}>{ship.current ? `${ship.current.ship_name ?? ship.current.ship} (flying)` : "the one I'm flying"}</option>
      {#each ship.list.filter((s) => !s.current) as s (s.ship_id)}<option value={s.ship_id}>{s.ship_name ?? s.ship}{s.ident ? ` (${s.ident})` : ""} · {s.ship} · {s.cargo_capacity} t{s.location?.system ? ` · at ${s.location.system}` : ""}</option>{/each}
      <option value="custom">Other ship…</option>
    </select></label>
    {#if trade.query.shipId === "custom"}
      <label title="The hold to plan for">cargo <input type="number" min="1" placeholder="t" bind:value={trade.query.cargo} /> t</label>
      <label title="Unladen jump range to plan with">jump <input type="number" min="1" step="0.1" placeholder="ly" bind:value={trade.query.jumpRange} /> ly</label>
      <label>pad <select bind:value={trade.query.minPad}>
        <option value="any">Any</option>
        <option value="medium">Medium+</option>
        <option value="large">Large only</option>
      </select></label>
    {/if}
    <label>radius <input type="number" min="1" bind:value={trade.query.radius} /> ly</label>
    <label>max price age <input type="number" min="1" bind:value={trade.query.maxAge} /> h</label>
    <label><input type="checkbox" bind:checked={trade.query.carriers} /> carriers</label>
    <label title="Sell confiscated goods where a black-market contact exists"><input type="checkbox" bind:checked={trade.query.prohibited} /> prohibited goods</label>
  </div>
  <div class="row" style="margin-bottom:0.35rem">
    <label title="Buy boards below this supply are ignored — primaries and hold-fillers alike">min supply <select bind:value={trade.query.minSupply}>
      <option value={1}>Any</option>
      <option value={100}>≥ 100 t</option>
      <option value={500}>≥ 500 t</option>
      <option value={1000}>≥ 1,000 t</option>
      <option value={5000}>≥ 5,000 t</option>
    </select></label>
    <label title="Sell boards below this demand are ignored">min demand <select bind:value={trade.query.minDemand}>
      <option value={1}>Any</option>
      <option value={100}>≥ 100 t</option>
      <option value={500}>≥ 500 t</option>
      <option value={1000}>≥ 1,000 t</option>
      <option value={5000}>≥ 5,000 t</option>
    </select></label>
    <label>max arrival distance <select bind:value={trade.query.maxArrivalLs}>
      <option value={0}>Any</option>
      <option value={2000}>≤ 2,000 ls</option>
      <option value={5000}>≤ 5,000 ls</option>
      <option value={20000}>≤ 20,000 ls</option>
    </select></label>
  </div>
  <div class="row" style="margin-bottom:0.35rem">
    <span class="pp">
      <span class="lbl">buy in</span>
      <select bind:value={trade.query.buyPower}><option value="any">Any power</option>{#if pp.pledged}<option value="mine">my power ({pp.pledged})</option>{/if}{#each pp.powers as p}<option value={p}>{p}</option>{/each}</select>
      <select bind:value={trade.query.buyPowerMode} disabled={trade.query.buyPower === "any"}><option value="controls">Controls</option><option value="present">Present</option><option value="undermining">Undermining</option></select>
      <select bind:value={trade.query.buyState}><option value="any">Any state</option><option value="none">Uncontrolled</option>{#each pp.states as s}<option value={s}>{s}</option>{/each}</select>
      <span class="lbl">sell in</span>
      <select bind:value={trade.query.sellPower}><option value="any">Any power</option>{#if pp.pledged}<option value="mine">my power ({pp.pledged})</option>{/if}{#each pp.powers as p}<option value={p}>{p}</option>{/each}</select>
      <select bind:value={trade.query.sellPowerMode} disabled={trade.query.sellPower === "any"}><option value="controls">Controls</option><option value="present">Present</option><option value="undermining">Undermining</option></select>
      <select bind:value={trade.query.sellState}><option value="any">Any state</option><option value="none">Uncontrolled</option>{#each pp.states as s}<option value={s}>{s}</option>{/each}</select>
    </span>
  </div>
  <div class="row" style="margin-bottom:0.5rem">
    <label title="Closed loops of three or more stops, every leg loaded (round trips are always searched)"><input type="checkbox" checked={trade.query.maxStops > 0} onchange={(e) => (trade.query.maxStops = e.currentTarget.checked ? 3 : 0)} /> search for trade rings</label>
    {#if trade.query.maxStops > 0}
      <label>how many stops? <select bind:value={trade.query.maxStops}>
        <option value={3}>3</option><option value={4}>4</option><option value={5}>5</option>
      </select></label>
    {/if}
    <button onclick={() => runSearch()} disabled={trade.loading}>{trade.loading ? "Searching…" : "Find"}</button>
    {#if trade.loading}<button class="ghost" onclick={stopSearch}>Stop</button>{/if}
  </div>

  {#if pinned}
    <div class="row small pinbar"><span class="lbl">Pinned to HUD:</span> {pinned.stops ? pinned.stops.map((s) => s.station).join(" → ") + " → back" : ` ⇄ `} <button class="quiet tiny" onclick={() => pinLoop(null)}>Unpin</button></div>
  {/if}

  {#if trade.loading}
    <div class="progress">
      <div class="spinner" aria-hidden="true"></div>
      <div>
        <div>Searching {trade.query.radius} ly around {trade.query.system.trim() || "your position"} — <span class="num">{secs(trade.elapsedMs)} s</span></div>
        <div class="muted small">You can switch tabs; the result will be here.</div>
      </div>
    </div>
  {:else if trade.report}
    <p class="muted small" style="margin:0 0 0.4rem">Finished in {secs(trade.elapsedMs)} s.</p>
  {/if}

  {#if trade.error}<p class="error">{trade.error}</p>{/if}

  {#if trade.report}
    {@const report = trade.report}
    <div class="row small muted" style="margin-bottom:0.5rem">
      <span>From <strong>{report.origin}</strong> · {report.constraints.radius_ly} ly{Number(trade.query.radius) > report.constraints.radius_ly ? " (max)" : ""} · {report.stations_considered.toLocaleString()} stations ·
        ship {fmtInt(report.ship.cargo_capacity)} t, {report.ship.jump_range_ly.toFixed(1)} ly empty / {report.ship.laden_range_ly.toFixed(1)} ly laden
        {#if trade.query.shipId == null && !trade.query.cargo && ship.current && report.ship.cargo_capacity !== ship.current.cargo_capacity}<span class="warn" title="You switched ships since this search ran — search again for your current ship">· stale: you're flying {ship.current.ship_name ?? ship.current.ship} now</span>{/if}
        {#if report.constraints.min_pad}· pad ≥ {report.constraints.min_pad}{:else}· <span class="warn">pad unknown — outposts included</span>{/if}
        {#if boardLine(report.board)}· priced against {boardLine(report.board)}{/if}
      </span>
      <span>
        excluded: {report.excluded.carriers} carriers, {report.excluded.pad_too_small} pads too small,
        {report.excluded.pad_unknown} unknown pads, {report.excluded.too_far_from_star} too far from star,
        {report.excluded.stale_price_rows} stale prices{report.excluded.no_market_data ? `, ${report.excluded.no_market_data} with no market data` : ""}{report.excluded.beyond_station_cap ? `, ${report.excluded.beyond_station_cap} beyond the station cap` : ""}
      </span>
    </div>

    <div class="row" style="margin-bottom:0.4rem">
      <button class="quiet {trade.view === 'legs' ? 'on' : ''}" onclick={() => (trade.view = "legs")}>Single legs ({report.legs.length})</button>
      <button class="quiet {trade.view === 'trips' ? 'on' : ''}" onclick={() => (trade.view = "trips")}>Round trips ({report.round_trips.length})</button>
      <button class="quiet {trade.view === 'rings' ? 'on' : ''}" onclick={() => (trade.view = "rings")}>Rings ({report.rings?.length ?? 0})</button>
    </div>

    {#if trade.view === "rings"}
      {#if !report.rings?.length}
        <p class="muted">No closed rings found inside these constraints.</p>
      {:else}
        <div class="table-wrap">
          <table>
            <thead><tr><th></th><th>Ring</th><th class="r">cr/h</th><th class="r">Profit / ring</th><th class="r">Time*</th></tr></thead>
            <tbody>
              {#each report.rings as r}
                <tr class={isPinnedRing(r) ? "pinned" : ""}>
                  <td><button class="quiet tiny" title={isPinnedRing(r) ? "Unpin from HUD" : "Pin this ring to the HUD"} onclick={() => pinRing(isPinnedRing(r) ? null : r)}>{isPinnedRing(r) ? "★" : "☆"}</button>
                    <button class="quiet tiny" title="Follow this ring: EDDA targets each stop's system and briefs every arrival" onclick={() => followTrade(r.legs, "ring")}>▶</button></td>
                  <td>
                    <ol class="ring">
                      {#each r.legs as l}
                        <li><strong>{l.from.station}</strong> <span class="muted small">({l.from.system} <button class="copy" title="Copy system name" onclick={() => copy(l.from.system)}>{copied === l.from.system ? "✓" : "⧉"}</button>)</span> — buy {l.commodity}{#each l.extra ?? [] as x} + {x.tons} t {x.commodity}{/each} ({l.tons} t · s {fmtInt(l.supply)} · d {fmtInt(l.demand)}), {fmtCrShort(l.profit)} at the next stop</li>
                      {/each}
                    </ol>
                  </td>
                  <td class="r num"><strong>{fmtCrShort(r.profit_per_hour)}</strong></td>
                  <td class="r num">{fmtCrShort(r.profit)}</td>
                  <td class="r num">{fmtMin(r.duration.seconds)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {:else if trade.view === "legs"}
      {#if report.legs.length === 0}
        <p class="muted">No profitable legs inside these constraints. Widen the radius or price age.</p>
      {:else}
        <div class="table-wrap">
          <table>
            <thead><tr>
              {#each ["commodity", "from", "to"] as k}<th class="sortable" onclick={() => sortBy(k)}>{LEG_SORTS[k].label}{arrow(k)}</th>{/each}
              {#each ["rate", "profit", "perton", "tons", "distance", "time", "arrival", "age"] as k}<th class="r sortable" onclick={() => sortBy(k)}>{LEG_SORTS[k].label}{arrow(k)}</th>{/each}
            </tr></thead>
            <tbody>
              {#each sortedLegs(report.legs) as l}
                <tr>
                  <td><button class="quiet tiny" title="Follow this leg: EDDA targets each end's system and briefs both pads (fly back empty)" onclick={() => followTrade([l], "leg")}>▶</button>
                    <strong>{l.commodity}</strong>{#each l.extra ?? [] as x}<div class="muted small">+ {x.tons} t {x.commodity}</div>{/each}</td>
                  <td>{l.from.station}<div class="muted small">{l.from.system} <button class="copy" title="Copy system name" onclick={() => copy(l.from.system)}>{copied === l.from.system ? "✓" : "⧉"}</button></div></td>
                  <td>{l.to.station}<div class="muted small">{l.to.system} <button class="copy" title="Copy system name" onclick={() => copy(l.to.system)}>{copied === l.to.system ? "✓" : "⧉"}</button> · {l.to.class}</div></td>
                  <!-- Maintainer ruling 2026-09-09: "repeat rate only on single legs" -
                       the one-way figure (profit ÷ loaded flight, as if you
                       never flew back) is nonsense for a delivery you repeat;
                       it stays only inside rings, where legs continue onward. -->
                  <td class="r num"><strong>{fmtCrShort(l.profit_per_hour_repeat)}</strong></td>
                  <td class="r num">{fmtCrShort(l.profit)}</td>
                  <td class="r num">{fmtInt(l.profit_per_ton)}</td>
                  <td class="r num">{l.tons}<div class="muted small">s {fmtInt(l.supply)} · d {fmtInt(l.demand)}</div></td>
                  <td class="r num">{fmtLy(l.distance_ly)}<div class="muted small">{l.jumps} jump{l.jumps === 1 ? "" : "s"}</div></td>
                  <!-- Each half of the loop with the supercruise that
                       belongs to it: Out ends at the SELL station's pad,
                       Back ends at the BUY station's (maintainer, 2026-09-05). -->
                  <td class="r num" title="loaded run: jumps + supercruise to the sell station">{fmtMin(l.duration.seconds)}<div class="muted small">{fmtLs(l.to.arrival_ls)}</div></td>
                  <td class="r num" title="empty return: jumps + supercruise back to the buy station">{fmtMin(l.return_duration.seconds)}<div class="muted small">{fmtLs(l.from.arrival_ls)}</div></td>
                  <td class="r num {ageClass(Math.max(l.buy_age_hours, l.sell_age_hours))}">{fmtAge(l.buy_age_hours)} / {fmtAge(l.sell_age_hours)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {:else}
      {#if report.round_trips.length === 0}
        <p class="muted">No round trips found: nothing sells back the other way inside these constraints.</p>
      {:else}
        <div class="table-wrap">
          <table>
            <thead><tr>
              <th></th><th>Loop</th><th>Out</th><th>Back</th><th class="r">cr/h</th><th class="r">Profit / loop</th><th class="r">Time*</th>
            </tr></thead>
            <tbody>
              {#each report.round_trips as t}
                <tr class={isPinned(t) ? "pinned" : ""}>
                  <td><button class="quiet tiny" title={isPinned(t) ? "Unpin from HUD" : "Pin this loop to the HUD"} onclick={() => pinLoop(isPinned(t) ? null : t)}>{isPinned(t) ? "★" : "☆"}</button>
                    <button class="quiet tiny" title="Follow this loop: EDDA targets each stop's system and briefs every arrival" onclick={() => followTrade([t.out, t.back], "round_trip")}>▶</button></td>
                  <td><strong>{t.out.from.station}</strong> ⇄ <strong>{t.out.to.station}</strong>
                    <div class="muted small">{t.out.from.system} <button class="copy" title="Copy system name" onclick={() => copy(t.out.from.system)}>{copied === t.out.from.system ? "✓" : "⧉"}</button> ⇄ {t.out.to.system} <button class="copy" title="Copy system name" onclick={() => copy(t.out.to.system)}>{copied === t.out.to.system ? "✓" : "⧉"}</button> · {fmtLy(t.out.distance_ly)}</div></td>
                  <td>{t.out.commodity}{#each t.out.extra ?? [] as x}<span class="muted small"> + {x.tons} t {x.commodity}</span>{/each}<div class="muted small">{fmtCr(t.out.profit)} · {t.out.tons} t · s {fmtInt(t.out.supply)} · d {fmtInt(t.out.demand)}</div></td>
                  <td>{t.back.commodity}{#each t.back.extra ?? [] as x}<span class="muted small"> + {x.tons} t {x.commodity}</span>{/each}<div class="muted small">{fmtCr(t.back.profit)} · {t.back.tons} t · s {fmtInt(t.back.supply)} · d {fmtInt(t.back.demand)}</div></td>
                  <td class="r num"><strong>{fmtCrShort(t.profit_per_hour)}</strong></td>
                  <td class="r num">{fmtCrShort(t.profit)}</td>
                  <td class="r num">{fmtMin(t.duration.seconds)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {/if}
    <p class="muted small" style="margin:0.5rem 0 0">* {report.note}</p>
  {/if}
</section>

<style>
  button.on { border-color: var(--accent); color: var(--accent); }
  th.sortable { cursor: pointer; user-select: none; white-space: nowrap; }
  .tiny { padding: 0 0.4rem; font-size: 0.9rem; }
  tr.pinned td { background: #ff8c1a12; }
  .pinbar { margin-bottom: 0.5rem; color: var(--accent-2); }
  .pinbar .lbl { color: var(--muted); }
  .pp { display: inline-flex; gap: 0.35rem; align-items: center; flex-wrap: wrap; }
  .pp .lbl { color: var(--muted); font-size: 0.8rem; }
  ol.ring { margin: 0; padding-left: 1.2rem; }
  ol.ring li { margin: 0.1rem 0; }
  button.copy { background: transparent; color: var(--muted); border: none; padding: 0 0.2rem; font-size: 0.85rem; cursor: pointer; line-height: 1; }
  button.copy:hover { color: var(--accent); filter: none; }
  th.sortable:hover { color: var(--accent); }
  .progress { display: flex; gap: 0.8rem; align-items: center; background: var(--bg-2); border: 1px solid var(--line); border-radius: 5px; padding: 0.6rem 0.8rem; margin-bottom: 0.6rem; }
  .spinner { width: 18px; height: 18px; border-radius: 50%; border: 2px solid var(--line); border-top-color: var(--accent); animation: spin 0.8s linear infinite; flex: none; }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
