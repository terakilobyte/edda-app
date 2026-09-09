<script>
  // System and station lookup — the Inara replacement. Community data,
  // with its age shown, and first-hand Powerplay readings preferred.
  import { findSystem, galaxyFind, edsmSystem, stationsInSystem, findStation, nearestService, stationMarket, nameComplete, activityHeatmap } from "./api.js";
  import { onDestroy } from "svelte";
  import { fmtInt, fmtLs, fmtLy, fmtTs } from "./format.js";
  import { mergeSystem } from "./galaxyMerge.js";
  import { KEYS, persisted } from "./storage.svelte.js";
  import Autocomplete from "./Autocomplete.svelte";
  import GalaxyView from "./GalaxyView.svelte";

  let q = $state("");
  let mode = $state("system"); // system | station | service
  let service = $state("outfitting");
  let minPad = $state("large");
  let radius = $state(50);
  let carriers = $state(false);
  let minor = $state(false);

  let system = $state(null);
  let stations = $state([]);
  let results = $state([]);
  let market = $state(null);
  let marketFor = $state(null);
  let error = $state("");
  let loading = $state(false);
  const reducedMotion = (() => { try { return matchMedia("(prefers-reduced-motion: reduce)").matches; } catch { return false; } })();
  const suspendAnimations = persisted(KEYS.galaxySuspendAnimations, reducedMotion);
  const mapFocus = $derived(system?.coords ? { name: system.name, pos: system.coords } : null);

  // Star-layer brightness, commander-controlled and universal across
  // modes (maintainer 2026-09-04: "a slider for controlling the brightness
  // might be the way to go" / "slider is universal for all modes").
  const starDim = persisted(KEYS.galaxyStarBrightness, 100);

  // Live activity mode: the EDDN pulse as an unlabeled heat layer.
  // Aggregates only — deliberately nothing here can name a system.
  let heat = $state(null);
  let heatStats = $state(null);
  let heatTimer = null;
  let lastPlaced = null;
  async function pollHeat() {
    try {
      const snap = await activityHeatmap();
      heat = snap.cells;
      const rate = lastPlaced == null ? null : Math.max(0, snap.placed - lastPlaced) * 30;
      lastPlaced = snap.placed;
      heatStats = {
        cells: snap.cells.length,
        rising: snap.cells.filter((c) => c.rising > 0.5).length,
        perMin: rate,
        unplaced: snap.unplaced,
      };
    } catch {}
  }
  $effect(() => {
    if (mode === "activity") {
      pollHeat();
      heatTimer = setInterval(pollHeat, 2000);
    } else {
      clearInterval(heatTimer);
      heatTimer = null;
      heat = null;
      heatStats = null;
      lastPlaced = null;
    }
  });
  onDestroy(() => clearInterval(heatTimer));

  async function run() {
    const name = q.trim();
    if (!name) return;
    loading = true;
    error = "";
    market = null;
    try {
      if (mode === "system") {
        const [local, indexed] = await Promise.all([findSystem(name), galaxyFind(name)]);
        let external = null;
        if (!local || !local.coords) {
          try { external = await edsmSystem(name); } catch {}
        }
        system = mergeSystem(local, indexed, external);
        stations = system ? await stationsInSystem(name, carriers, minor) : [];
        results = [];
        if (!system) error = `No system named “${name}” in the galaxy database.`;
      } else if (mode === "station") {
        results = await findStation(name);
        system = null;
        stations = [];
      } else {
        results = await nearestService(name, service, minPad || null, Number(radius), carriers);
        system = null;
        stations = [];
      }
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  let marketBoard = $state(null);
  async function showMarket(st) {
    marketFor = st;
    market = null;
    marketBoard = null;
    try {
      const board = await stationMarket(st.id);
      market = board.entries ?? [];
      marketBoard = board;
    } catch (e) {
      error = String(e);
    }
  }

  const padLabel = (p) => (p ? p[0].toUpperCase() : "?");
</script>

<section class="panel">
  <h2>Galaxy <span class="sub">community data</span></h2>

  <div class="row" style="margin-bottom:0.5rem">
    <select bind:value={mode}>
      <option value="system">System</option>
      <option value="station">Station by name</option>
      <option value="service">Nearest service from</option>
      <option value="activity">Live activity</option>
    </select>
    {#if mode !== "activity"}<Autocomplete bind:value={q} placeholder={mode === "station" ? "Station name" : "System name"} minWidth="14rem" fetch={(p) => nameComplete(mode === "station" ? "station" : "system", p)} onenter={run} onchoose={run} />{/if}
    {#if mode === "service"}
      <select bind:value={service}>
        <option value="outfitting">outfitting</option>
        <option value="shipyard">shipyard</option>
        <option value="market">market</option>
      </select>
      <select bind:value={minPad}>
        <option value="">any pad</option>
        <option value="medium">medium+</option>
        <option value="large">large</option>
      </select>
      <label>within <input type="number" bind:value={radius} /> ly</label>
    {/if}
    {#if mode !== "activity"}<label><input type="checkbox" bind:checked={carriers} /> carriers</label>{/if}
    {#if mode === "system"}<label><input type="checkbox" bind:checked={minor} /> settlements</label>{/if}
    {#if mode !== "activity"}<button onclick={run} disabled={loading}>{loading ? "…" : "Look up"}</button>{/if}
    <label title="Background star brightness"><span class="muted small">stars</span> <input type="range" min="0" max="100" bind:value={starDim.value} style="width:7rem;vertical-align:middle" /></label>
    <label style="margin-left:auto"><input type="checkbox" bind:checked={suspendAnimations.value} /> suspend animations</label>
  </div>

  <div style="margin-bottom:0.8rem"><GalaxyView focus={mode === "activity" ? null : mapFocus} height={440} idleRotate={!suspendAnimations.value} heat={mode === "activity" ? heat : null} identify={mode !== "activity"} starBrightness={starDim.value / 100} /></div>

  {#if mode === "activity"}
    <div class="row muted small" style="margin-bottom:0.8rem">
      <span>The galaxy's economic pulse, live from the community feed: <span style="color:var(--accent)">markets</span> · <span style="color:var(--cyan)">traffic</span> · white = heating up. Old routes fade on their own.</span>
      {#if heatStats}
        <span class="num" style="margin-left:auto">{heatStats.cells} warm cells · {heatStats.rising} rising{#if heatStats.perMin != null} · {heatStats.perMin}/min heard{/if}</span>
      {/if}
    </div>
  {/if}

  {#if error}<p class="error">{error}</p>{/if}

  {#if system}
    <div class="sysbox">
      <div class="sysname">{system.name}</div>
      <div class="row small">
        {#if system.controlling_power}
          <span class="pill accent">{system.controlling_power}</span>
          <span class="pill">{system.power_state ?? "?"}{system.control_progress != null ? ` · ${(system.control_progress * 100).toFixed(0)}%` : ""}</span>
          <span class="muted">{system.power_provenance === "first_hand" ? "seen by you" : "community"} {system.power_observed ? fmtTs(system.power_observed).slice(0, 10) : ""}</span>
        {:else}
          <span class="pill">no Powerplay control recorded</span>
        {/if}
      </div>
      <dl class="kv small" style="margin-top:0.4rem">
        <dt>Allegiance</dt><dd>{system.allegiance ?? "—"} · {system.government ?? "—"}</dd>
        <dt>Economy</dt><dd>{system.primary_economy ?? "—"}</dd>
        <dt>Security</dt><dd>{system.security ?? "—"} · pop {fmtInt(system.population)}</dd>
        <dt>Coords</dt><dd class="num">{system.coords ? system.coords.map((c) => c.toFixed(1)).join(", ") : "—"}</dd>
        {#if system.primary_star}<dt>Primary star</dt><dd>{system.primary_star}{system.scoopable != null ? ` · ${system.scoopable ? "scoopable" : "not scoopable"}` : ""}</dd>{/if}
        <dt>Stations</dt><dd>{system.station_count} total, {stations.length} shown</dd>
        <!-- Where the knowledge comes from is plumbing, not the commander's concern (maintainer, 2026-09-05). -->
        {#if system.external}<dt>Source</dt><dd>community data</dd>{/if}
      </dl>
    </div>
  {/if}

  {#if stations.length || results.length}
    <div class="table-wrap">
      <table>
        <thead><tr>
          <th>Station</th>{#if results.length}<th>System</th>{/if}<th>Type</th><th class="r">Arrival</th>
          {#if mode === "service"}<th class="r">Dist</th>{/if}
          <th>Pad</th><th>Services</th><th>Updated</th><th></th>
        </tr></thead>
        <tbody>
          {#each (stations.length ? stations : results) as row}
            {@const st = row.station ?? row}
            <tr>
              <td><strong>{st.name}</strong>{#if st.is_carrier}<span class="pill" style="margin-left:0.3rem">carrier</span>{/if}</td>
              {#if results.length}<td>{st.system_name ?? "—"}</td>{/if}
              <td class="small">{st.kind ?? st.class}</td>
              <td class="r num">{fmtLs(st.distance_to_arrival)}</td>
              {#if mode === "service"}<td class="r num">{fmtLy(row.distance_ly)}</td>{/if}
              <td><span class="pill {st.max_pad === 'large' ? 'ok' : st.max_pad ? '' : 'warn'}">{padLabel(st.max_pad)}</span></td>
              <td class="small">{[st.has_market && "market", st.has_outfitting && "outfitting", st.has_shipyard && "shipyard"].filter(Boolean).join(" · ") || "—"}</td>
              <td class="small muted">{st.updated ? fmtTs(st.updated).slice(0, 10) : "—"}</td>
              <td>{#if st.has_market}<button class="quiet tiny" onclick={() => showMarket(st)}>market</button>{/if}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}

  {#if marketFor}
    <h3>{marketFor.name} — market {#if market}<span class="muted">({market.length} commodities)</span>{/if}</h3>
    {#if !market}
      <p class="muted">Loading…</p>
    {:else if market.length === 0}
      <p class="muted">No prices on record for this station: its board was observed and it lists nothing.</p>
    {:else}
      <div class="table-wrap">
        <table>
          <thead><tr><th>Commodity</th><th>Category</th><th class="r">Buy</th><th class="r">Supply</th><th class="r">Sell</th><th class="r">Demand</th><th>Updated</th></tr></thead>
          <tbody>
            {#each market as m}
              <tr>
                <td>{m.name ?? m.symbol}</td>
                <td class="small muted">{m.category ?? ""}</td>
                <td class="r num">{m.buy_price ? fmtInt(m.buy_price) : "—"}</td>
                <td class="r num">{m.supply ? fmtInt(m.supply) : "—"}</td>
                <td class="r num">{m.sell_price ? fmtInt(m.sell_price) : "—"}</td>
                <td class="r num">{m.demand ? fmtInt(m.demand) : "—"}</td>
                <td class="small muted">{m.updated ? fmtTs(m.updated).slice(0, 16) : "—"}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</section>

<style>
  .sysbox { background: var(--bg-2); border: 1px solid var(--line); border-radius: 5px; padding: 0.6rem 0.8rem; margin-bottom: 0.6rem; }
  .sysname { font-size: 1.2rem; font-weight: 600; color: var(--accent-2); margin-bottom: 0.2rem; }
  h3 { font-size: 0.85rem; margin: 0.8rem 0 0.3rem; color: var(--accent); }
  .tiny { padding: 0.05rem 0.45rem; font-size: 0.72rem; }
</style>
