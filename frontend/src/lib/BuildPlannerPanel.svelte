<script>
  // The Build planner: pick a ship — one of yours, or any hull you do not
  // own yet — swap modules slot by slot from what each slot takes, give
  // every engineerable module a blueprint, grade and experimental (or
  // import a build from EDSY or Coriolis and plan the gap), and get one
  // answer for the lot: what the plan does to the ship, every material
  // pooled, one shopping list, the fewest engineers to visit. Its own page
  // with a ship dropdown (maintainer, 2026-09-20); the Ships tab's Plan
  // build lands here.
  import { shipsList, hullsList, shipModules, slotOptions, shipSlef, listBlueprintNames, buildPlanReport, importBuild, buildPerformance } from "./api.js";
  import { ship } from "./ship.svelte.js";
  import { planner } from "./planner.svelte.js";
  import { KEYS, readKey, writeKey, removeKey } from "./storage.svelte.js";
  import { planRows, withSwap, swapsFrom, findCandidate, swapKey, EMPTY, sameForAll, groupCounts, planRequest, proposedFor, savedFrom, isPlanned, hasWork, itinerary, blocked, applyImport } from "./buildplan.js";
  import ShoppingReport from "./ShoppingReport.svelte";
  import { useTabActive } from "./lifecycle.svelte.js";

  let ships = $state([]);
  let hulls = $state([]);
  // What is selected: one of the commander's ships by ShipID, or a hull
  // the commander does not own (planned from its stock fit).
  let selectedId = $state(null);
  let selectedHull = $state(null);
  const selectKey = $derived(selectedHull ? `hull:${selectedHull}` : selectedId == null ? "" : String(selectedId));
  const selected = $derived(selectedHull ? null : ships.find((s) => s.ship_id === selectedId) ?? null);
  const hull = $derived(selectedHull ? hulls.find((h) => h.symbol === selectedHull) ?? null : null);
  const target = $derived({ shipId: selectedHull ? null : selectedId, hull: selectedHull });
  let build = $state(null);
  let slots = $state([]);          // every slot of the hull with its candidates
  let msg = $state("");
  const label = (s) => (s.ship_name ? `${s.ship_name} (${s.ship})` : s.ship) + (s.ident ? ` · ${s.ident}` : "");
  const title = $derived(selected ? label(selected) : hull ? `${hull.name} (stock hull)` : "this ship");

  let rows = $state([]);
  let bpOptions = $state({});      // module type → [{name, grades}]
  let planReport = $state(null);
  let planBusy = $state(false);
  let planMsg = $state("");
  let planPicked = $state(new Set());
  // The saved plan is keyed by ShipID AND hull: the game hands a sold
  // ship's ID to the next one bought, and a plan for a Type-10 must never
  // surface on whatever ship inherits its number (maintainer, 2026-09-20:
  // "if the build in memory is associated with a different ship it
  // shouldn't show"). A hull the commander does not own is keyed by hull.
  const slug = (s) => (s ?? "").toLowerCase().replace(/[^a-z0-9]+/g, "-");
  const planKey = () => (selectedHull ? `${KEYS.buildPlan}.hull.${slug(selectedHull)}` : `${KEYS.buildPlan}.${selectedId}.${slug(ships.find((x) => x.ship_id === selectedId)?.ship)}`);
  const counts = $derived(groupCounts(rows));
  const engineerable = $derived(rows.filter((r) => r.module_type).length);
  const plannedCount = $derived(rows.filter(isPlanned).length);
  const swapCount = $derived(rows.filter((r) => r.swap).length);
  const blueprintsFor = (type) => (bpOptions[type] ?? []).filter((b) => b.grades.length > 0);
  const experimentalsFor = (type) => (bpOptions[type] ?? []).filter((b) => b.grades.length === 0);
  const gradesFor = (row) => (blueprintsFor(row.module_type).find((b) => b.name === row.blueprint)?.grades ?? [1, 2, 3, 4, 5]).filter((g) => g > row.from_grade);
  const candidatesFor = (slot) => slots.find((s) => s.slot === slot)?.candidates ?? [];
  const canEmpty = (slot) => slots.find((s) => s.slot === slot)?.can_empty ?? false;
  // Candidates grouped by kind for the swap dropdown ("Pulse Laser" → its sizes, ratings and mounts).
  const kindsFor = (slot) => {
    const groups = new Map();
    for (const c of candidatesFor(slot)) {
      if (!groups.has(c.kind)) groups.set(c.kind, []);
      groups.get(c.kind).push(c);
    }
    return [...groups.entries()];
  };

  const active = useTabActive();

  async function loadShips() {
    try {
      const [own, all] = await Promise.all([shipsList(false), hulls.length ? Promise.resolve(hulls) : hullsList()]);
      ships = own;
      hulls = all;
      const want = planner.shipId != null ? ships.find((s) => s.ship_id === planner.shipId) : null;
      planner.shipId = null;
      if (want) await select(String(want.ship_id));
      else if (selectedHull) await select(`hull:${selectedHull}`);
      else {
        const cur = ships.find((s) => s.ship_id === selectedId) ?? ships.find((s) => s.current) ?? ships[0] ?? null;
        if (cur) await select(String(cur.ship_id));
        else { selectedId = null; build = null; rows = []; slots = []; }
      }
    } catch (e) { msg = String(e); }
  }
  // Mount, every ship swap, and every hand-off from the Ships tab.
  $effect(() => { void ship.currentId; void planner.shipId; loadShips(); });

  async function select(key) {
    if (key.startsWith("hull:")) { selectedHull = key.slice(5); selectedId = null; }
    else { selectedHull = null; selectedId = Number(key); }
    build = null; slots = []; msg = ""; planReport = null; planMsg = ""; imported = null; perf = null; rows = [];
    try {
      const [b, s] = await Promise.all([shipModules(target.shipId, target.hull), slotOptions(target.shipId, target.hull)]);
      build = b; slots = s;
    } catch (e) { msg = String(e); return; }
    rows = planRows(build.modules, readKey(planKey(), {}, { json: true }) ?? {}, slots);
    refreshPerformance();
    await loadOptions();
  }
  async function loadOptions() {
    const types = [...new Set(rows.map((r) => r.module_type).filter(Boolean))].filter((t) => !bpOptions[t]);
    if (!types.length) return;
    try {
      const got = await Promise.all(types.map((t) => listBlueprintNames(t)));
      const next = { ...bpOptions };
      types.forEach((t, i) => { next[t] = got[i]; });
      bpOptions = next;
    } catch (e) { planMsg = String(e); }
  }
  function savePlan() { if (selectedId != null || selectedHull) writeKey(planKey(), savedFrom(rows), { json: true }); refreshPerformance(); }

  // What the plan does to the ship: mass, jump, power, as flown and with
  // the swaps at their base figures plus a full roll of every planned
  // blueprint (EDSY's convention). Pinned against EDSY in ed_ships;
  // refreshed whenever the rows change.
  let perf = $state(null);
  let perfSeq = 0;
  let imported = $state(null);   // the last imported build (what it said)
  async function refreshPerformance() {
    if (selectedId == null && !selectedHull) { perf = null; return; }
    const seq = ++perfSeq;
    try {
      const r = await buildPerformance(target.shipId, proposedFor(rows), swapsFrom(rows), target.hull);
      if (seq === perfSeq) perf = r;
    } catch (e) { if (seq === perfSeq) perf = { error: String(e) }; }
  }
  const t1 = (n) => (n == null ? "—" : n.toFixed(1));
  const t2 = (n) => (n == null ? "—" : n.toFixed(2));
  const pct = (draw, cap) => (cap ? `${Math.round((100 * draw) / cap)}%` : "—");
  const arrow = (a, b) => (b == null || a === b ? a : `${a} → ${b}`);

  function update(i, patch) {
    const r = { ...rows[i], ...patch };
    if ("blueprint" in patch) {
      const gs = gradesFor(r);
      if (!gs.includes(Number(r.target_grade))) r.target_grade = gs[gs.length - 1] ?? r.from_grade;
    }
    if ("blueprint" in patch || "experimental" in patch) r.include = hasWork(r);
    if (!hasWork(r)) r.include = false;
    rows = rows.map((x, j) => (j === i ? r : x));
    planReport = null;
    savePlan();
  }
  // Another module in the slot, from what the slot takes; "" is the fitted
  // module (or empty) again; EMPTY clears the slot; "item|preset" is a
  // pre-engineered variant.
  async function swapTo(i, key) {
    const [item, preset] = key.split("|");
    const candidate = key === EMPTY ? { item: EMPTY } : item ? findCandidate(candidatesFor(rows[i].slot), item, preset || null) : null;
    rows = rows.map((x, j) => (j === i ? withSwap(x, candidate) : x));
    planReport = null;
    savePlan();
    await loadOptions();
  }
  function copyToAll(i) { rows = sameForAll(rows, rows[i]); planReport = null; savePlan(); }
  function includeAll(on) { rows = rows.map((r) => ({ ...r, include: on && hasWork(r) })); planReport = null; savePlan(); }
  function togglePlanPick(i) { const s = new Set(planPicked); s.has(i) ? s.delete(i) : s.add(i); planPicked = s; }

  async function runPlan() {
    if (selectedId == null && !selectedHull) return;
    planBusy = true; planMsg = "";
    try {
      planReport = await buildPlanReport({ shipId: target.shipId, hull: target.hull, items: planRequest(rows), swaps: swapsFrom(rows) });
      planPicked = new Set((planReport.shopping?.list?.trades ?? []).map((_, i) => i));
    } catch (e) { planReport = null; planMsg = String(e); } finally { planBusy = false; }
  }

  // A build from EDSY or Coriolis (their SLEF export) becomes the plan:
  // what to swap, then the engineering to reach it. On a hull you do not
  // own, the gap from the stock fit: every module a swap, every
  // engineered one a job from grade 0.
  let importText = $state("");
  let importBusy = $state(false);
  let importOpen = $state(false);
  async function runImport() {
    if ((selectedId == null && !selectedHull) || !importText.trim()) return;
    importBusy = true; planMsg = ""; imported = null;
    try {
      const r = await importBuild(target.shipId, importText, target.hull);
      if (!r.ship_matches) {
        const hullFor = hulls.find((h) => h.name.toLowerCase() === (r.ship ?? "").toLowerCase());
        planMsg = `That build is a ${r.ship}${r.ship_name ? ` ("${r.ship_name}")` : ""}, not ${title} — pick that ship above${hullFor ? ` (it is under "Any ship" if you do not own one)` : ""}, or paste a build for this one.`;
        return;
      }
      imported = r;
      rows = applyImport(rows, r);
      planReport = null;
      savePlan();
      await loadOptions();
      importText = "";
      importOpen = false;
    } catch (e) { planMsg = String(e); } finally { importBusy = false; }
  }

  // "Clear this build" (maintainer, 2026-09-20): forget the saved plan for
  // this ship, drop any imported build and every swap, back to the fitted
  // defaults (the stock fit, for a hull).
  function clearBuild() {
    if ((selectedId == null && !selectedHull) || !build) return;
    removeKey(planKey());
    imported = null; planReport = null; planMsg = "";
    rows = planRows(build.modules, {}, slots);
    refreshPerformance();
  }

  async function copyPlannedBuild() {
    if (selectedId == null && !selectedHull) return;
    try {
      const slef = await shipSlef(target.shipId, null, proposedFor(rows), swapsFrom(rows), target.hull);
      await navigator.clipboard.writeText(slef);
      planMsg = "Planned build copied as SLEF — paste into EDSY or Coriolis (Import).";
    } catch (e) { planMsg = String(e); }
  }
</script>

<section class="panel">
  <h2>Build planner <span class="sub">every module at once · swap what a slot takes · one material list · the fewest engineers</span></h2>
  <div class="row" style="gap:0.8rem; flex-wrap:wrap; align-items:center">
    <label>Ship
      <select value={selectKey} onchange={(e) => select(e.currentTarget.value)} style="margin-left:0.4rem; min-width:18rem">
        {#if ships.length}
          <optgroup label="Your ships">
            {#each ships as s}<option value={String(s.ship_id)}>{label(s)}{s.current ? " · flying" : ""}</option>{/each}
          </optgroup>
        {/if}
        <optgroup label="Any ship (as sold)">
          {#each hulls as h}<option value={`hull:${h.symbol}`}>{h.name}</option>{/each}
        </optgroup>
      </select>
    </label>
    <button class={importOpen ? "" : "quiet"} onclick={() => (importOpen = !importOpen)} title="Paste an EDSY or Coriolis SLEF export; the plan becomes the difference between this ship and that build">Import a build</button>
    <button class="quiet" onclick={copyPlannedBuild} disabled={plannedCount === 0 && swapCount === 0} title="The build with every swap and every planned blueprint at its target grade, for EDSY or Coriolis">Copy planned build (SLEF)</button>
    <button class="quiet" onclick={clearBuild} disabled={!build} title="Forget the plan saved for this ship, every swap included, and start again from what is fitted">Clear this build</button>
    {#if msg}<span class="error small">{msg}</span>{/if}
  </div>
  {#if hull}
    <div class="muted small" style="margin-top:0.4rem">A {hull.name} as sold, not one of yours: every slot starts at the stock fit. Swap modules in, plan the engineering, or import a build for it.</div>
  {/if}

  {#if importOpen}
    <div class="import" style="margin-top:0.6rem">
      <div class="muted small" style="margin-bottom:0.3rem">Paste the SLEF export (EDSY: Export → SLEF; Coriolis: Export → SLEF). The plan becomes the difference: modules to swap, then the engineering to reach the build.</div>
      <textarea rows="4" style="width:100%; font-family: monospace" bind:value={importText} placeholder={'[{"header": {"appName": "EDSY", ...}, "data": {"event": "Loadout", ...}}]'}></textarea>
      <div class="row" style="margin-top:0.4rem">
        <button onclick={runImport} disabled={importBusy || !importText.trim()}>{importBusy ? "Importing…" : "Import build"}</button>
        <button class="quiet" onclick={() => { importOpen = false; importText = ""; }}>Cancel</button>
      </div>
    </div>
  {/if}

  {#if imported}
    <div class="small" style="margin-top:0.5rem">
      <strong>Imported{imported.app ? ` from ${imported.app}` : ""}{imported.ship_name ? `: "${imported.ship_name}"` : ""}</strong>
      · {imported.swaps.length} module{imported.swaps.length === 1 ? "" : "s"} to swap
      · {imported.items.filter((it) => !it.done).length} engineering job{imported.items.filter((it) => !it.done).length === 1 ? "" : "s"}
      · {imported.items.filter((it) => it.done).length} already there
      {#if imported.swaps.length}<div class="muted">The swaps are in the table below, in the Swap to column; a swapped module is engineered from scratch, so its row starts at grade 0.</div>{/if}
      {#each imported.skipped as sk}<div class="warn">{sk}</div>{/each}
    </div>
  {/if}

  {#if perf && !perf.error}
    {@const b = perf.before}
    {@const a = perf.after}
    <div class="small perf" style="margin-top:0.6rem">
      <strong>{hull ? "As sold" : "As flown"}{a ? " → with this plan" : ""}</strong>
      <span title="Hull and modules, no fuel or cargo">unladen {arrow(t1(b.unladen_mass), a && t1(a.unladen_mass))} t</span>
      <span title="Full tank, no cargo / full cargo / one jump's fuel only">jump {arrow(t2(b.jump_unladen), a && t2(a.jump_unladen))} ly <span class="muted">(laden {arrow(t2(b.jump_laden), a && t2(a.jump_laden))}, max {arrow(t2(b.jump_max), a && t2(a.jump_max))})</span></span>
      <span class={(a ?? b).power_deployed > (a ?? b).power_capacity ? "bad" : ""} title="Draw with hardpoints retracted / deployed, against the power plant">power {arrow(pct(b.power_retracted, b.power_capacity), a && pct(a.power_retracted, a.power_capacity))} / {arrow(pct(b.power_deployed, b.power_capacity), a && pct(a.power_deployed, a.power_capacity))} of {arrow(t1(b.power_capacity), a && t1(a.power_capacity))} MW</span>
      {#if perf.unknown_items.length}<span class="muted" title={perf.unknown_items.join(", ")}>{perf.unknown_items.length} module{perf.unknown_items.length === 1 ? "" : "s"} unknown to the figures</span>{/if}
      {#each perf.notes as n}<span class="warn">{n}</span>{/each}
    </div>
  {:else if perf?.error}
    <div class="small warn" style="margin-top:0.6rem">{perf.error}</div>
  {/if}

  {#if build}
    <div class="row" style="margin-top:0.7rem; gap:0.6rem; flex-wrap:wrap; align-items:center">
      <span class="muted small">{plannedCount} of {engineerable} engineerable modules planned{swapCount ? ` · ${swapCount} swap${swapCount === 1 ? "" : "s"}` : ""} · fitted engineering continues to the top grade unless you change it · Swap to offers only what the slot takes, "remove" on every slot but the core, and the brokers' pre-engineered modules with their engineering already on</span>
      <button class="quiet" onclick={() => includeAll(true)}>Include all chosen</button>
      <button class="quiet" onclick={() => includeAll(false)}>Include none</button>
    </div>
    <div class="table-wrap" style="margin-top:0.4rem">
      <table>
        <thead><tr><th></th><th>Slot</th><th>Module</th><th>Swap to</th><th>Fitted</th><th>Blueprint</th><th>To grade</th><th>Experimental</th><th></th></tr></thead>
        <tbody>
          {#each rows as r, i (r.slot)}
            <tr class={isPlanned(r) ? "eng" : r.swap ? "swap" : ""}>
              <td>{#if r.module_type}<input type="checkbox" checked={r.include && hasWork(r)} disabled={!hasWork(r)} onchange={(e) => update(i, { include: e.currentTarget.checked })} title={hasWork(r) ? "Include this module in the plan" : "Nothing to do: at the top grade and no experimental chosen"} />{/if}</td>
              <td class="small muted" title={r.size != null ? `${r.slot_name} · size ${r.size}` : r.slot_name}>{r.slot_name}</td>
              <td>{#if r.swap}<span class="muted">{r.fitted_name ?? "empty"}</span> → <strong>{r.item_name ?? "empty"}</strong>{:else}{r.item_name ?? "empty"}{/if}</td>
              <td>
                {#if candidatesFor(r.slot).length}
                  <select value={r.swap === EMPTY ? EMPTY : swapKey(r.swap, r.preset)} onchange={(e) => swapTo(i, e.currentTarget.value)} title="Every module this slot takes">
                    <option value="">{r.fitted_name ? `keep: ${r.fitted_name}` : "leave empty"}</option>
                    {#if r.fitted_name && canEmpty(r.slot)}<option value={EMPTY}>remove — leave empty</option>{/if}
                    {#each kindsFor(r.slot) as [kind, cs]}
                      <optgroup label={kind}>
                        {#each cs as c}<option value={swapKey(c.item, c.preset)}>{c.item_name}</option>{/each}
                      </optgroup>
                    {/each}
                  </select>
                {:else}<span class="muted">—</span>{/if}
              </td>
              <td class="small muted">{r.swap === EMPTY ? "—" : r.swap && r.preset ? `bought G${r.from_grade}` : r.swap ? "new" : r.from_grade ? `G${r.from_grade}` : "—"}</td>
              {#if r.module_type}
                <td>
                  <select value={r.blueprint} onchange={(e) => update(i, { blueprint: e.currentTarget.value })}>
                    <option value="">none</option>
                    {#each blueprintsFor(r.module_type) as b}<option value={b.name}>{b.name}</option>{/each}
                  </select>
                </td>
                <td>
                  {#if r.blueprint && gradesFor(r).length}
                    <select value={String(r.target_grade)} onchange={(e) => update(i, { target_grade: Number(e.currentTarget.value) })}>
                      {#each gradesFor(r) as g}<option value={String(g)}>G{g}</option>{/each}
                    </select>
                  {:else if r.blueprint}<span class="muted small">at the top</span>
                  {:else}<span class="muted">—</span>{/if}
                </td>
                <td>
                  <select value={r.experimental} onchange={(e) => update(i, { experimental: e.currentTarget.value })}>
                    <option value="">none</option>
                    {#each experimentalsFor(r.module_type) as x}<option value={x.name}>{x.name}</option>{/each}
                  </select>
                </td>
                <td>{#if (counts.get(r.module_type) ?? 0) > 1}<button class="quiet small" onclick={() => copyToAll(i)} title="Give every {r.module_type} on this ship the same plan">same for all {counts.get(r.module_type)}</button>{/if}</td>
              {:else}
                <td colspan="4" class="muted small">{r.swap === EMPTY ? "slot cleared" : r.item ? "no engineer works this" : ""}</td>
              {/if}
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <div class="row" style="margin-top:0.6rem">
      <button onclick={runPlan} disabled={planBusy || (plannedCount === 0 && swapCount === 0)}>{planBusy ? "Working…" : "Materials for this build"}</button>
      {#if planMsg}<span class="muted small">{planMsg}</span>{/if}
    </div>

    {#if planReport}
      <h3 style="margin-top:0.8rem">
        {planReport.items.length} module{planReport.items.length === 1 ? "" : "s"} · {planReport.materials.length} materials
        <span class={planReport.fully_met ? "ok" : "warn"}>{planReport.fully_met ? "all materials in hand" : `${planReport.materials.filter((l) => l.have < l.need).length} short`}</span>
        {#if planReport.unassigned.length}<span class="bad">{planReport.unassigned.length} no unlocked engineer can do</span>{/if}
      </h3>
      <div class="table-wrap">
        <table>
          <thead><tr><th>Material</th><th class="r">Need</th><th class="r">Have</th><th>Short</th></tr></thead>
          <tbody>
            {#each planReport.materials as line}
              <tr>
                <td>{line.material}</td>
                <td class="r num">{line.need}</td>
                <td class="r num">{line.have}</td>
                <td class={line.have >= line.need ? "ok" : "warn"}>{line.have >= line.need ? "✓" : `${line.need - line.have} more`}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>

      {#if planReport.unlocks?.length}
        <h3 style="margin-top:0.8rem">Technology broker unlocks <span class="muted">for the modules the build swaps in · materials counted above</span></h3>
        {#each planReport.unlocks as u}
          <div class="small" style="margin:0.3rem 0"><strong>{u.item_name}</strong> <span class="muted">({u.slot_name}) · {u.broker} technology broker</span>
            <div style="margin-left:1rem">{u.materials.map((l) => `${l.need} ${l.material}${l.have >= l.need ? " ✓" : ` (have ${l.have})`}`).join(", ")}{#if u.commodities.length}<span class="muted"> · commodities to buy: {u.commodities.map(([c, n]) => `${n} ${c}`).join(", ")}</span>{/if}</div>
          </div>
        {/each}
      {/if}

      <h3 style="margin-top:0.8rem">Engineers to visit <span class="muted">fewest stops that cover the plan</span></h3>
      {#each itinerary(planReport) as stop}
        <div class="small" style="margin:0.3rem 0"><strong>{stop.engineer}</strong>{stop.rank ? ` · rank ${stop.rank}` : ""}
          {#each stop.jobs as j}<div style="margin-left:1rem">{j.count > 1 ? `${j.count} × ` : ""}{j.module_type} {j.what} <span class="muted">({j.slots})</span></div>{/each}
        </div>
      {/each}
      {#if blocked(planReport).length}
        <h3 style="margin-top:0.6rem">Not reachable yet <span class="muted">no unlocked engineer offers the asked grade</span></h3>
        {#each blocked(planReport) as b}
          <div class="small" style="margin:0.3rem 0"><span class="bad">{b.count > 1 ? `${b.count} × ` : ""}{b.module_type} {b.what}</span> <span class="muted">({b.slots})</span>
            <div style="margin-left:1rem">
              {#if b.max_reachable_grade}Today: to G{b.max_reachable_grade} with {b.today.join(" or ")}.{:else}Nobody unlocked works this blueprint.{/if}
              {#if b.unlock.length}For G{b.target_grade}: unlock {b.unlock.map((u) => `${u.engineer} (${u.status.toLowerCase()})`).join(" or ")}.{/if}
            </div>
          </div>
        {/each}
      {/if}

      <ShoppingReport shopping={planReport.shopping} picked={planPicked} onToggle={togglePlanPick} />
    {/if}
  {:else if !msg}
    <p class="muted" style="margin-top:0.6rem">{ships.length || hulls.length ? "Loading the build…" : "No ships in the journal yet — pick any hull above to plan one."}</p>
  {/if}
</section>

<style>
  .perf { display: flex; flex-wrap: wrap; gap: 0.3rem 1.1rem; align-items: baseline; }
  tr.eng td { background: #7ec8ff10; }
  tr.swap td { background: #ffd47e10; }
  button.small { padding: 0.1rem 0.5rem; font-size: 0.78rem; }
  .import { border: 1px solid var(--line); border-radius: 6px; padding: 0.6rem 0.8rem; background: var(--panel-2); }
</style>
