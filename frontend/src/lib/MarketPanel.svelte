<script>
  import { commoditySearch, outfittingSearch, shipyardSearch, sellHoldSearch, getStatus, listCommodities, nameComplete } from "./api.js";
  import { fmtInt, fmtLs, fmtLy, fmtTs } from "./format.js";
  import Autocomplete from "./Autocomplete.svelte";
  import { requestRoute } from "./route.svelte.js";

  let kind = $state("commodity");
  let text = $state("");
  let system = $state("");
  let side = $state("buy");
  let radius = $state(100);
  let maxAge = $state(48);
  let pad = $state("");
  let carriers = $state(false);
  let prohibited = $state(false);
  let minQty = $state(0);
  let loading = $state(false);
  let report = $state(null);
  let error = $state("");
  let elapsed = $state(null);
  let commodities = $state([]);
  let sortKey = $state("distance_ly");
  let sortDir = $state(1);
  // "Sell my hold": every cargo item searched sell-side at once, ranked
  // by what the whole hold earns per station (demand-capped).
  let holdReport = $state(null);
  let holdLoading = $state(false);
  async function runHold() {
    holdLoading = true; error = "";
    try {
      holdReport = await sellHoldSearch(Number(radius), pad || null, carriers, Number(maxAge));
    } catch (e) { error = String(e); } finally { holdLoading = false; }
  }
  const holdLineTitle = (s) => (s.lines ?? [])
    .map((l) => `${l.commodity}: ${fmtInt(l.takes)}${l.takes < l.held ? ` of ${fmtInt(l.held)}` : ""} t @ ${fmtInt(l.price)} cr = ${fmtInt(l.revenue)} cr`)
    .join("\n");

  getStatus().then((s) => { if (!system) system = s?.location?.system_name ?? ""; }).catch(() => {});
  listCommodities().then((items) => { commodities = items ?? []; }).catch(() => {});

  function commodityComplete(value) {
    const needle = value.trim().toLocaleLowerCase();
    if (!needle) return [];
    return commodities
      .filter((item) => item.name.toLocaleLowerCase().includes(needle))
      .sort((a, b) => {
        const an = a.name.toLocaleLowerCase();
        const bn = b.name.toLocaleLowerCase();
        return Number(!an.startsWith(needle)) - Number(!bn.startsWith(needle)) || an.localeCompare(bn);
      })
      .slice(0, 20);
  }

  //
  // GUARD, because this cost the maintainer an evening: `onclick={run}` hands
  // the handler a MouseEvent, which landed here as `source` and went to
  // the backend inside the request, where Rust's Option<String> reported
  // "invalid args `query` for command `commodity_search`: invalid type:
  // map, expected a string". Enter in the search box worked (onenter
  // calls run() with no argument) and only the Search BUTTON failed, so
  // it read as "market search is broken". The call site is fixed to
  // `() => run()`; this coerces anything that is not a string back to
  // null so a future handler wiring cannot resurrect it.
  async function run() {
    if (!text.trim()) return;
    loading = true;
    error = "";
    report = null;
    const started = performance.now();
    // The fetch order IS the selection under the limit: a distance-led
    // view asks the backend for the nearest matches, anything else for
    // the best-priced (F3 — the old fetch always took the cheapest 75
    // and re-sorting them by distance silently hid nearer stations).
    const query = {
      text: text.trim(), system: system.trim() || null, radius_ly: Number(radius),
      min_pad: pad || null, include_carriers: carriers, include_prohibited: prohibited, max_age_hours: Number(maxAge),
      side, limit: 75, sort: sortKey === "distance_ly" ? "distance" : "price",
      min_quantity: Number(minQty) || 0,
    };
    try {
      report = kind === "commodity" ? await commoditySearch(query)
        : kind === "outfitting" ? await outfittingSearch(query) : await shipyardSearch(query);
    } catch (e) {
      error = String(e);
    } finally {
      elapsed = performance.now() - started;
      loading = false;
    }
  }

  const padLabel = (p) => p ? p[0].toUpperCase() : "?";
  const defaultDirection = (key) => key === "price" ? (side === "buy" ? 1 : -1)
    : ["quantity", "updated"].includes(key) ? -1 : 1;

  function sortBy(key) {
    const wasDistanceLed = sortKey === "distance_ly";
    if (sortKey === key) sortDir *= -1;
    else { sortKey = key; sortDir = defaultDirection(key); }
    // Crossing between distance-led and price-led selection changes
    // WHICH rows the backend returns, not just their order — refetch.
    if (report && !loading && wasDistanceLed !== (sortKey === "distance_ly")) run();
  }

  const arrow = (key) => sortKey === key ? (sortDir > 0 ? " ▲" : " ▼") : "";
  const results = $derived.by(() => {
    const rows = [...(report?.results ?? [])];
    const key = sortKey;
    const dir = sortDir;
    rows.sort((a, b) => {
      let av = a[key];
      let bv = b[key];
      if (av == null && bv == null) return 0;
      if (av == null) return 1;
      if (bv == null) return -1;
      if (typeof av === "string") return av.localeCompare(bv, undefined, { sensitivity: "base" }) * dir;
      return (av - bv) * dir;
    });
    return rows;
  });
</script>

<section class="panel">
  <h2>Market search <span class="sub">prices · outfitting · shipyards</span></h2>

  <div class="search-grid">
    <label>Looking for
      <select bind:value={kind} onchange={() => { report = null; error = ""; text = ""; }}>
        <option value="commodity">Commodity</option>
        <option value="outfitting">Ship module</option>
        <option value="shipyard">Ship</option>
      </select>
    </label>
    <label class="item">Item
      {#if kind === "commodity"}
        <Autocomplete bind:value={text} minWidth="100%" fetch={commodityComplete} onenter={run}
          placeholder="Gold, tritium, meta-alloys…" />
      {:else}
        <input bind:value={text} style="width:100%" onkeydown={(e) => e.key === "Enter" && run()}
          placeholder={kind === "outfitting" ? "5A fuel scoop, beam laser…" : "Mandalay, Anaconda…"} />
      {/if}
    </label>
    {#if kind === "commodity"}
      <label>Action
        <select bind:value={side}>
          <option value="buy">Buy from station</option>
          <option value="sell">Sell to station</option>
        </select>
      </label>
    {/if}
    <label>From system
      <Autocomplete bind:value={system} placeholder="Current system" minWidth="12rem" fetch={(p) => nameComplete("system", p)} onenter={run} />
    </label>
    <label>Within <span class="inline"><input class="short" type="number" min="1" max="500" bind:value={radius} /> ly</span></label>
    <label>Landing pad
      <select bind:value={pad}>
        <option value="">current ship</option>
        <option value="any">any / unknown</option>
        <option value="medium">medium+</option>
        <option value="large">large</option>
      </select>
    </label>
    {#if kind === "commodity"}
      <label>Prices newer than <span class="inline"><input class="short" type="number" min="1" bind:value={maxAge} /> h</span></label>
      <label title="Skip stations with less depth than your hold needs">Min {side === "buy" ? "supply" : "demand"} <input class="short" type="number" min="0" bind:value={minQty} /></label>
    {/if}
    <label class="check"><input type="checkbox" bind:checked={carriers} /> Include carriers</label>
    <label class="check" title="Show sales of confiscated goods at stations with a black-market contact"><input type="checkbox" bind:checked={prohibited} /> Include prohibited goods and black markets</label>
    <button class="go" onclick={() => run()} disabled={loading}>{loading ? "Searching…" : "Search"}</button>
    <button class="quiet" title="Search every commodity in the cargo hold sell-side and rank stations by the combined price" onclick={runHold} disabled={holdLoading}>{holdLoading ? "Valuing hold…" : "Sell my hold"}</button>
  </div>

  <p class="muted small">Community reports can be stale. Unknown landing pads fail closed when a pad is required.</p>
  {#if error}<p class="error">{error}</p>{/if}

  {#if holdReport}
    <div class="summary row">
      <strong>Selling the hold</strong>
      {#if holdReport.hold?.length}
        <span>{holdReport.hold.map((h) => `${fmtInt(h.count)} t ${h.commodity}`).join(" · ")}</span>
      {/if}
      {#if holdReport.skipped?.length}<span class="pill warn" title="No price data found for these">unpriced: {holdReport.skipped.join(", ")}</span>{/if}
      <button class="ghost" style="margin-left:auto" onclick={() => (holdReport = null)}>Dismiss</button>
    </div>
    {#if holdReport.note}
      <p class="muted">{holdReport.note}</p>
    {:else if !holdReport.stations?.length}
      <p class="muted">No station in range buys any of it. Try a wider radius or a longer price age.</p>
    {:else}
      <div class="table-wrap">
        <table>
          <thead><tr><th>Station</th><th>System</th><th class="num">Distance</th><th class="num">Buys</th><th class="num">Hold value</th></tr></thead>
          <tbody>
            {#each holdReport.stations as s, i}
              <tr class={i === 0 ? "accent" : ""} title={holdLineTitle(s)}>
                <td>{s.station}{s.is_carrier ? " ⛴" : ""}{i === 0 ? " ★" : ""}</td>
                <td>{s.system}</td>
                <td class="num">{fmtLy(s.distance_ly)}</td>
                <td class="num">{s.covered}/{s.of}</td>
                <td class="num">{fmtInt(s.total)} cr</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      <p class="muted small">Ranked by what the whole hold earns there — each line pays price × what the station's demand actually takes (hover a row for the breakdown). ★ best combined offer.</p>
    {/if}
  {/if}

  {#if report}
    <div class="summary row">
      <strong>{kind === "commodity" ? report.commodity : report.query}</strong>
      <span>{results.length} result{results.length === 1 ? "" : "s"} from {report.origin}</span>
      {#if results.length >= 75}<span class="pill warn" title="The search stops at 75 matches — tighten the radius, depth, or price age to see the rest">first 75 only</span>{/if}
      <span class="pill ok" title="Answered by the community API's live board{report.as_of ? ` (as of ${report.as_of.replace("T", " ").replace("Z", " UTC")})` : ""}">live</span>
      {#if elapsed != null}<span class="muted">{elapsed < 1000 ? `${elapsed.toFixed(0)} ms` : `${(elapsed / 1000).toFixed(1)} s`}</span>{/if}
    </div>
    {#if results.length === 0}
      <p class="muted">Nothing matched. Try a wider radius, “any” pad, carriers, a longer price age, or a broader item name.</p>
    {:else}
      <div class="table-wrap">
        <table>
          <thead><tr>
            {#if kind !== "commodity"}<th><button class="sort" onclick={() => sortBy("name")}>Item{arrow("name")}</button></th>{/if}
            <th><button class="sort" onclick={() => sortBy("station")}>Station{arrow("station")}</button></th>
            <th><button class="sort" onclick={() => sortBy("system")}>System{arrow("system")}</button></th>
            <th class="r"><button class="sort" onclick={() => sortBy("distance_ly")}>Distance{arrow("distance_ly")}</button></th>
            <th class="r"><button class="sort" onclick={() => sortBy("distance_to_arrival")}>Arrival{arrow("distance_to_arrival")}</button></th>
            <th><button class="sort" onclick={() => sortBy("max_pad")}>Pad{arrow("max_pad")}</button></th>
            {#if kind === "commodity"}
              <th class="r"><button class="sort" onclick={() => sortBy("price")}>Price{arrow("price")}</button></th>
              <th class="r"><button class="sort" onclick={() => sortBy("quantity")}>{side === "buy" ? "Supply" : "Demand"}{arrow("quantity")}</button></th>
              <th><button class="sort" onclick={() => sortBy("age_hours")}>Age{arrow("age_hours")}</button></th>
            {:else}<th><button class="sort" onclick={() => sortBy("updated")}>Updated{arrow("updated")}</button></th>{/if}
          </tr></thead>
          <tbody>
            {#each results as r}
              <tr>
                {#if kind !== "commodity"}
                  <td><strong>{r.name}</strong>{#if kind === "outfitting" && (r.class || r.rating)}<span class="pill">{r.class ?? "?"}{r.rating ?? ""}</span>{/if}<div class="symbol">{r.symbol}</div></td>
                {/if}
                <td><strong>{r.station}</strong>{#if r.is_carrier}<span class="pill">carrier</span>{/if}
                  <button class="route-icon" onclick={() => requestRoute(r.system)}
                    title="Plot a route to {r.station}, {r.system}" aria-label="Plot a route to {r.station}, {r.system}">➤</button>
                </td>
                <td>{r.system}</td><td class="r num">{fmtLy(r.distance_ly)}</td><td class="r num">{fmtLs(r.distance_to_arrival)}</td>
                <td><span class="pill {r.max_pad === 'large' ? 'ok' : r.max_pad ? '' : 'warn'}">{padLabel(r.max_pad)}</span></td>
                {#if kind === "commodity"}
                  <td class="r num">{fmtInt(r.price)} cr</td><td class="r num">{fmtInt(r.quantity)}</td>
                  <td class="small {r.age_hours != null && r.age_hours > 48 ? '' : 'muted'}">{#if r.age_hours != null && r.age_hours > 48}<span class="pill warn">{r.age_hours.toFixed(0)} h</span>{:else}{r.age_hours == null ? "?" : r.age_hours < 1 ? "<1 h" : `${r.age_hours.toFixed(0)} h`}{/if}</td>
                {:else}<td class="small muted">{r.updated ? fmtTs(r.updated).slice(0, 16) : "—"}</td>{/if}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</section>

<style>
  .search-grid { display: flex; flex-wrap: wrap; align-items: end; gap: 0.55rem 0.8rem; }
  .search-grid label { display: flex; flex-direction: column; gap: 0.2rem; font-size: 0.76rem; color: var(--muted); }
  .search-grid .item { flex: 1 1 16rem; }
  .search-grid .check { flex-direction: row; align-items: center; padding-bottom: 0.45rem; }
  .inline { display: inline-flex; align-items: center; gap: 0.25rem; }
  .short { width: 5rem; }
  .go { min-width: 6rem; height: 2rem; }
  .summary { margin: 0.7rem 0 0.35rem; gap: 0.7rem; align-items: baseline; }
  .pill { margin-left: 0.3rem; }
  .symbol { font-family: monospace; font-size: 0.65rem; color: var(--muted); margin-top: 0.12rem; }
  button.sort { appearance: none; border: 0; background: transparent; color: inherit; padding: 0; font: inherit; font-weight: inherit; cursor: pointer; white-space: nowrap; }
  button.sort:hover { color: var(--accent); filter: none; }
  button.route-icon { appearance: none; border: 0; background: transparent; color: var(--accent); padding: 0 0.2rem; margin-left: 0.2rem; cursor: pointer; font-size: 0.9rem; line-height: 1; }
  button.route-icon:hover { color: var(--accent-2); filter: none; transform: translateX(1px); }
</style>
