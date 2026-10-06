<script>
  // The inventory, laid out like the game's material trader screen (boss,
  // 2026-10-04, with a screenshot of Reiter City's manufactured trader):
  // one row per trader group, grade 1 to 5 left to right, the count in
  // each cell. "Material trading mode" turns the same grid into the
  // trader: pick the material you want and every other cell shows what
  // you would pay of it, exactly as the game does (216 ⇄ 1 on a G1 for a
  // G4); and the planner's trades are drawn on the cells as −given and
  // +received. The plan is the one the boss described after doing it by
  // hand: fill the lower grades from the near-full G4/G5, then make room
  // in G4/G5 so the next mission's material reward fits.
  import { getInventory, materialGrid, materialTradePlan } from "./api.js";
  import { journalResource } from "./lifecycle.svelte.js";
  import Place from "./Place.svelte";
  import { ratio } from "./materials.js";

  let cells = $state([]);
  let cargo = $state([]);
  let error = $state("");
  let filter = $state("");
  let trading = $state(false);
  let selected = $state(null); // symbol of the material you want, in trading mode

  // Planner knobs (the defaults are the boss's own workflow).
  let kind = $state("");
  let order = $state("bottom_first");
  let minGrade = $state(4);
  let roomBelow = $state(85);
  let sourceMin = $state(90);
  let floor = $state(50);
  let cross = $state(false);
  let up = $state(false);
  let view = $state(null);
  let busy = $state(false);

  async function load() {
    try {
      const [g, inv] = await Promise.all([materialGrid(), getInventory()]);
      cells = g;
      cargo = inv.filter((i) => i.category === "Cargo");
      error = "";
    } catch (e) {
      error = String(e);
    }
    if (trading) await plan();
  }
  async function plan() {
    busy = true;
    try {
      view = await materialTradePlan({ kind: kind || null, sourceMin: sourceMin / 100, floor: floor / 100, cross, up, order, minSourceGrade: minGrade, roomBelow: roomBelow / 100 });
      error = "";
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
  journalResource(load);
  function toggleTrading() {
    trading = !trading;
    selected = null;
    if (trading) plan();
  }

  // The game's screen order. Manufactured: Chemical, Thermic, Heat from
  // the boss's screenshot; the rest, and raw and encoded, as the game
  // lists them (unconfirmed against the screen — ledger, 2026-10-04).
  const ORDER = {
    raw: ["1", "2", "3", "4", "5", "6", "7"],
    manufactured: ["Chemical", "Thermic", "Heat", "Conductive", "Mechanical Components", "Capacitors", "Shielding", "Composite", "Crystals", "Alloys"],
    encoded: ["Emission Data", "Wake Scans", "Shield Data", "Encryption Files", "Data Archives", "Encoded Firmware"],
  };
  const KIND_LABEL = { raw: "Raw", manufactured: "Manufactured", encoded: "Encoded" };
  const groupLabel = (k, g) => (k === "raw" ? `Raw ${g}` : g);

  const f = $derived(filter.trim().toLowerCase());
  const sections = $derived.by(() => {
    const out = [];
    for (const k of ["raw", "manufactured", "encoded"]) {
      const mine = cells.filter((c) => c.kind === k);
      const groups = new Map();
      for (const c of mine) {
        const key = c.group ?? "";
        if (!groups.has(key)) groups.set(key, []);
        groups.get(key).push(c);
      }
      const names = [...groups.keys()].filter((g) => g);
      names.sort((a, b) => {
        const ia = ORDER[k].indexOf(a), ib = ORDER[k].indexOf(b);
        return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b, undefined, { numeric: true });
      });
      const rows = names
        .map((g) => ({ group: g, cells: [1, 2, 3, 4, 5].map((grade) => groups.get(g).find((c) => c.grade === grade) ?? null) }))
        .filter((r) => !f || r.cells.some((c) => c && (c.name.toLowerCase().includes(f) || groupLabel(k, r.group).toLowerCase().includes(f))));
      const other = (groups.get("") ?? []).filter((c) => c.count > 0 && (!f || c.name.toLowerCase().includes(f)));
      if (rows.length || other.length) out.push({ kind: k, rows, other });
    }
    return out;
  });
  const selectedCell = $derived(selected ? cells.find((c) => c.symbol === selected) : null);
  // The plan's effect per cell: given and received totals.
  const delta = $derived.by(() => {
    const m = new Map();
    if (!view) return m;
    for (const t of view.plan.trades) {
      const g = m.get(t.give_symbol) ?? { give: 0, recv: 0 };
      g.give += t.give_qty;
      m.set(t.give_symbol, g);
      const r = m.get(t.recv_symbol) ?? { give: 0, recv: 0 };
      r.recv += t.recv_qty;
      m.set(t.recv_symbol, r);
    }
    return m;
  });
  const given = $derived(view ? view.plan.trades.reduce((n, t) => n + t.give_qty, 0) : 0);
  const received = $derived(view ? view.plan.trades.reduce((n, t) => n + t.recv_qty, 0) : 0);
  const shownCargo = $derived(cargo.filter((i) => !f || i.name.toLowerCase().includes(f)));
</script>

<section class="panel">
  <h2>Inventory <span class="sub">{trading ? "material trading mode · pick what you want, every cell shows what you'd pay" : "materials laid out as the trader shows them · and cargo"}</span></h2>
  <div class="row" style="margin-bottom:0.5rem; flex-wrap:wrap; gap:0.6rem">
    <input placeholder="Filter…" bind:value={filter} />
    <button class="small {trading ? 'primary' : ''}" onclick={toggleTrading}>{trading ? "Leave trading mode" : "Material trading mode"}</button>
    {#if trading && view}
      <span class="muted small">planning for a <strong>{view.kind}</strong> trader{view.kind_from !== "chosen" ? ` (${view.kind_from})` : ""} · {view.plan.trades.length} trades · give {given}, receive {received}</span>
    {/if}
  </div>
  {#if trading}
    <div class="row knobs" style="margin-bottom:0.6rem; flex-wrap:wrap; gap:0.6rem">
      <label>Trader
        <select bind:value={kind} onchange={plan}>
          <option value="">auto</option><option value="raw">Raw</option><option value="manufactured">Manufactured</option><option value="encoded">Encoded</option>
        </select>
      </label>
      <label title="Which grades are spent">Spend
        <select bind:value={minGrade} onchange={plan}><option value={4}>G4 and G5 only</option><option value={1}>any near-full grade</option></select>
      </label>
      <label title="Bottom first: one unit goes furthest at the bottom (1:81). Nearest first: keeps the most value.">Fill
        <select bind:value={order} onchange={plan}><option value="bottom_first">bottom grade first</option><option value="nearest_first">nearest grade first</option></select>
      </label>
      <label title="A material at this share of its cap or above is spent">Near full ≥ <input type="range" min="50" max="100" step="5" bind:value={sourceMin} onchange={plan} /> <span class="num">{sourceMin}%</span></label>
      <label title="Sources are never spent below this share of their cap">Keep ≥ <input type="range" min="0" max="100" step="5" bind:value={floor} onchange={plan} /> <span class="num">{floor}%</span></label>
      <label title="After the gaps are filled, anything still over this share of its cap moves into a G4/G5 with room, so a mission reward fits. 0 turns it off.">Room in G4/G5 below <input type="range" min="0" max="100" step="5" bind:value={roomBelow} onchange={plan} /> <span class="num">{roomBelow ? `${roomBelow}%` : "off"}</span></label>
      <label title="Fill lower grades of other groups too, at six times the price"><input type="checkbox" bind:checked={cross} onchange={plan} /> across groups</label>
      <label><input type="checkbox" bind:checked={up} onchange={plan} /> up a grade</label>
      <button class="small" onclick={plan} disabled={busy}>{busy ? "…" : "Re-plan"}</button>
    </div>
  {/if}
  {#if error}<p class="error small">{error}</p>{/if}

  {#each sections as sec (sec.kind)}
    <h3 class:planned={trading && view && view.kind === sec.kind}>{KIND_LABEL[sec.kind]} {#if trading && view && view.kind === sec.kind}<span class="muted">· the plan</span>{/if}</h3>
    {#each sec.rows as row (row.group)}
      <div class="grow">
        <div class="glabel">{groupLabel(sec.kind, row.group)}</div>
        {#each row.cells as c, i (c ? c.symbol : `${row.group}-${i}`)}
          {#if c}
            {@const d = delta.get(c.symbol)}
            {@const r = trading && selectedCell ? ratio(c, selectedCell) : null}
            <button
              class="cell"
              class:empty={c.count === 0}
              class:full={c.count >= c.cap * 0.9}
              class:sel={selected === c.symbol}
              class:give={d && d.give}
              class:recv={d && d.recv}
              disabled={!trading}
              title={trading ? (selected === c.symbol ? "selected" : "select: see what each cell pays for one of these") : `${c.name}: ${c.count} of ${c.cap}`}
              onclick={() => (selected = selected === c.symbol ? null : c.symbol)}
            >
              <span class="bar" style="width:{Math.min(100, (c.count / c.cap) * 100)}%"></span>
              {#if r}<span class="ratio" title="pay {r[0]} {c.name} for {r[1]} {selectedCell.name}">{r[0]} ⇄ {r[1]}</span>{/if}
              {#if d && (d.give || d.recv)}<span class="delta">{d.give ? `−${d.give}` : ""}{d.give && d.recv ? " " : ""}{d.recv ? `+${d.recv}` : ""}</span>{/if}
              <span class="count">{c.count}<span class="cap">/{c.cap}</span></span>
              <span class="name">{c.name}</span>
            </button>
          {:else}
            <span class="cell blank"></span>
          {/if}
        {/each}
      </div>
    {/each}
    {#if sec.other.length}
      <div class="grow"><div class="glabel">Other</div>
        {#each sec.other as c (c.symbol)}<span class="cell static"><span class="count">{c.count}</span><span class="name">{c.name}</span></span>{/each}
      </div>
    {/if}
  {/each}

  {#if trading && view}
    <h3>Trades <span class="muted">({view.plan.trades.length} · give {given}, receive {received})</span></h3>
    {#if view.plan.sources.length === 0}
      <p class="muted small">Nothing in {view.kind} is near full at {sourceMin}% of cap. Lower the threshold, or pick another trader.</p>
    {:else if view.plan.trades.length === 0}
      <p class="muted small">No trade to make: the gaps the sources could fill are full, the floor stops them, or nothing in G4/G5 has room.</p>
    {:else}
      <div class="table-wrap">
        <table>
          <thead><tr><th>#</th><th>Give</th><th></th><th>Receive</th><th>Ratio</th><th>Why</th><th>Left with</th></tr></thead>
          <tbody>
            {#each view.plan.trades as t, i}
              <tr>
                <td class="num muted">{i + 1}</td>
                <td><span class="num">{t.give_qty}</span> × {t.give_name} <span class="muted small">G{t.give_grade}</span></td>
                <td class="muted">→</td>
                <td><span class="num">{t.recv_qty}</span> × {t.recv_name} <span class="muted small">G{t.recv_grade}</span></td>
                <td class="num small">{t.ratio}</td>
                <td><span class="pill {t.direction === 'down' ? 'ok' : t.direction === 'across' ? 'warn' : t.direction === 'room' ? 'cyan' : ''}">{t.direction === "room" ? "make room" : t.direction}</span></td>
                <td class="small muted">{t.give_name} {t.give_after} · {t.recv_name} {t.recv_after}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
    <h3>Nearest {view.kind} traders {#if view.origin_system}<span class="muted">from {view.origin_system}</span>{/if}</h3>
    {#if !view.origin_system}
      <p class="muted small">No position yet: the journal has not said where you are.</p>
    {:else if !view.traders_known}
      <p class="muted small">Could not reach the API for traders.</p>
    {:else if view.traders.length === 0}
      <p class="muted small">No {view.kind} trader within 300 ly.</p>
    {:else}
      <table><tbody>
        {#each view.traders as t (t.station.id)}
          <tr>
            <td><Place system={t.station.system_name} station={t.station.name} /></td>
            <td class="small">{t.station.name}</td>
            <td class="num small">{t.distance_ly.toFixed(1)} ly</td>
            <td class="small muted">{t.station.primary_economy ?? ""}{t.station.distance_to_arrival != null ? ` · ${Math.round(t.station.distance_to_arrival)} ls` : ""}</td>
          </tr>
        {/each}
      </tbody></table>
    {/if}
  {/if}

  {#if shownCargo.length}
    <h3>Cargo <span class="muted">({shownCargo.length})</span></h3>
    <table style="max-width:28rem"><tbody>
      {#each shownCargo as i (i.symbol)}<tr><td>{i.name}</td><td class="r num">{i.count}</td></tr>{/each}
    </tbody></table>
  {/if}
</section>

<style>
  h3 { font-size: 0.8rem; color: var(--accent); text-transform: uppercase; letter-spacing: 0.05em; margin: 0.9rem 0 0.3rem; }
  h3.planned { color: var(--ok, #7c7); }
  .grow { display: grid; grid-template-columns: 7.5rem repeat(5, minmax(7.5rem, 1fr)); gap: 0.35rem; align-items: stretch; margin-bottom: 0.35rem; }
  .glabel { font-size: 0.72rem; text-transform: uppercase; letter-spacing: 0.05em; color: var(--muted, #9aa); align-self: center; }
  .cell { position: relative; overflow: hidden; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 0.1rem; min-height: 3.6rem; padding: 0.35rem 0.4rem; border: 1px solid var(--border, #345); border-radius: 4px; background: transparent; color: inherit; font: inherit; text-align: center; cursor: default; }
  .cell:not(:disabled) { cursor: pointer; }
  .cell:not(:disabled):hover { border-color: var(--accent); }
  .cell.blank { border-style: dashed; opacity: 0.25; }
  .cell.empty .count, .cell.empty .name { opacity: 0.4; }
  .cell.full { border-color: var(--warn, #c93); }
  .cell.sel { border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
  .cell.give { border-color: var(--warn, #c93); }
  .cell.recv { border-color: var(--ok, #7c7); }
  .bar { position: absolute; left: 0; bottom: 0; height: 3px; background: var(--accent); opacity: 0.6; }
  .count { font-variant-numeric: tabular-nums; font-size: 1.1rem; line-height: 1.1; }
  .cap { font-size: 0.65rem; opacity: 0.55; }
  .name { font-size: 0.68rem; line-height: 1.1; opacity: 0.85; }
  .ratio { position: absolute; top: 0.2rem; left: 0.35rem; font-size: 0.68rem; font-variant-numeric: tabular-nums; color: var(--accent); }
  .delta { position: absolute; top: 0.2rem; right: 0.35rem; font-size: 0.72rem; font-variant-numeric: tabular-nums; font-weight: 600; }
  .cell.give .delta { color: var(--warn, #c93); }
  .cell.recv .delta { color: var(--ok, #7c7); }
  .cell.give.recv .delta { color: inherit; }
  .cell.static { border-style: dotted; }
  .knobs input[type="range"] { width: 6rem; vertical-align: middle; }
</style>
