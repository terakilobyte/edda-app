<script>
  // The Build planner: pick a ship, give every engineerable module a
  // blueprint, grade and experimental — or import a build from EDSY or
  // Coriolis and plan the gap — and get one answer for the lot: what the
  // plan does to the ship, every material pooled, one shopping list, the
  // fewest engineers to visit. Its own page with a ship dropdown
  // (maintainer, 2026-09-20); the Ships tab's Plan build lands here.
  import { shipsList, shipModules, shipSlef, listBlueprintNames, buildPlanReport, importBuild, buildPerformance } from "./api.js";
  import { ship } from "./ship.svelte.js";
  import { planner } from "./planner.svelte.js";
  import { KEYS, readKey, writeKey, removeKey } from "./storage.svelte.js";
  import { planRows, sameForAll, groupCounts, planRequest, proposedFor, savedFrom, isPlanned, hasWork, itinerary, blocked, applyImport } from "./buildplan.js";
  import ShoppingReport from "./ShoppingReport.svelte";
  import { useTabActive } from "./lifecycle.svelte.js";

  let ships = $state([]);
  let selectedId = $state(null);
  const selected = $derived(ships.find((s) => s.ship_id === selectedId) ?? null);
  let build = $state(null);
  let msg = $state("");
  const label = (s) => (s.ship_name ? `${s.ship_name} (${s.ship})` : s.ship) + (s.ident ? ` · ${s.ident}` : "");

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
  // shouldn't show").
  const planKey = (id) => { const s = ships.find((x) => x.ship_id === id); return `${KEYS.buildPlan}.${id}.${(s?.ship ?? "").toLowerCase().replace(/[^a-z0-9]+/g, "-")}`; };
  const counts = $derived(groupCounts(rows));
  const plannedCount = $derived(rows.filter(isPlanned).length);
  const blueprintsFor = (type) => (bpOptions[type] ?? []).filter((b) => b.grades.length > 0);
  const experimentalsFor = (type) => (bpOptions[type] ?? []).filter((b) => b.grades.length === 0);
  const gradesFor = (row) => (blueprintsFor(row.module_type).find((b) => b.name === row.blueprint)?.grades ?? [1, 2, 3, 4, 5]).filter((g) => g > row.from_grade);

  const active = useTabActive();

  async function loadShips() {
    try {
      ships = await shipsList(false);
      const want = planner.shipId != null ? ships.find((s) => s.ship_id === planner.shipId) : null;
      const cur = want ?? ships.find((s) => s.ship_id === selectedId) ?? ships.find((s) => s.current) ?? ships[0] ?? null;
      planner.shipId = null;
      if (cur) await select(cur.ship_id);
      else { selectedId = null; build = null; rows = []; }
    } catch (e) { msg = String(e); }
  }
  // Mount, every ship swap, and every hand-off from the Ships tab.
  $effect(() => { void ship.currentId; void planner.shipId; loadShips(); });

  async function select(id) {
    selectedId = id; build = null; msg = ""; planReport = null; planMsg = ""; imported = null; perf = null; rows = [];
    try { build = await shipModules(id); } catch (e) { msg = String(e); return; }
    rows = planRows(build.modules, readKey(planKey(id), {}, { json: true }) ?? {});
    refreshPerformance();
    await loadOptions();
  }
  async function loadOptions() {
    const types = [...new Set(rows.map((r) => r.module_type))].filter((t) => !bpOptions[t]);
    if (!types.length) return;
    try {
      const got = await Promise.all(types.map((t) => listBlueprintNames(t)));
      const next = { ...bpOptions };
      types.forEach((t, i) => { next[t] = got[i]; });
      bpOptions = next;
    } catch (e) { planMsg = String(e); }
  }
  function savePlan() { if (selectedId != null) writeKey(planKey(selectedId), savedFrom(rows), { json: true }); refreshPerformance(); }

  // What the plan does to the ship: mass, jump, power, as flown and at a
  // full roll of every planned blueprint (EDSY's convention). Pinned
  // against EDSY in ed_ships; refreshed whenever the rows change.
  let perf = $state(null);
  let perfSeq = 0;
  let imported = $state(null);   // the last imported build (its swaps feed the figures)
  async function refreshPerformance() {
    if (selectedId == null) { perf = null; return; }
    const seq = ++perfSeq;
    try {
      const swaps = (imported?.swaps ?? []).map((s) => ({ slot: s.slot, item: s.want_item }));
      const r = await buildPerformance(selectedId, proposedFor(rows), swaps);
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
  function copyToAll(i) { rows = sameForAll(rows, rows[i]); planReport = null; savePlan(); }
  function includeAll(on) { rows = rows.map((r) => ({ ...r, include: on && hasWork(r) })); planReport = null; savePlan(); }
  function togglePlanPick(i) { const s = new Set(planPicked); s.has(i) ? s.delete(i) : s.add(i); planPicked = s; }

  async function runPlan() {
    if (selectedId == null) return;
    planBusy = true; planMsg = "";
    try {
      planReport = await buildPlanReport({ shipId: selectedId, items: planRequest(rows) });
      planPicked = new Set((planReport.shopping?.list?.trades ?? []).map((_, i) => i));
    } catch (e) { planReport = null; planMsg = String(e); } finally { planBusy = false; }
  }

  // A build from EDSY or Coriolis (their SLEF export) becomes the plan:
  // what to swap, then the engineering to reach it.
  let importText = $state("");
  let importBusy = $state(false);
  let importOpen = $state(false);
  async function runImport() {
    if (selectedId == null || !importText.trim()) return;
    importBusy = true; planMsg = ""; imported = null;
    try {
      const r = await importBuild(selectedId, importText);
      if (!r.ship_matches) {
        planMsg = `That build is a ${r.ship}${r.ship_name ? ` ("${r.ship_name}")` : ""}, not ${selected ? label(selected) : "this ship"} — pick that ship above, or paste a build for this one.`;
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
  // this ship, drop any imported build, back to the fitted defaults.
  function clearBuild() {
    if (selectedId == null || !build) return;
    removeKey(planKey(selectedId));
    imported = null; planReport = null; planMsg = "";
    rows = planRows(build.modules, {});
    refreshPerformance();
  }

  async function copyPlannedBuild() {
    if (selectedId == null) return;
    try {
      const slef = await shipSlef(selectedId, null, proposedFor(rows));
      await navigator.clipboard.writeText(slef);
      planMsg = "Planned build copied as SLEF — paste into EDSY or Coriolis (Import).";
    } catch (e) { planMsg = String(e); }
  }
</script>

<section class="panel">
  <h2>Build planner <span class="sub">every module at once · one material list · the fewest engineers</span></h2>
  <div class="row" style="gap:0.8rem; flex-wrap:wrap; align-items:center">
    <label>Ship
      <select value={selectedId} onchange={(e) => select(Number(e.currentTarget.value))} style="margin-left:0.4rem; min-width:18rem">
        {#each ships as s}<option value={s.ship_id}>{label(s)}{s.current ? " · flying" : ""}</option>{/each}
      </select>
    </label>
    <button class={importOpen ? "" : "quiet"} onclick={() => (importOpen = !importOpen)} title="Paste an EDSY or Coriolis SLEF export; the plan becomes the difference between this ship and that build">Import a build</button>
    <button class="quiet" onclick={copyPlannedBuild} disabled={plannedCount === 0} title="The build with every planned blueprint at its target grade, for EDSY or Coriolis">Copy planned build (SLEF)</button>
    <button class="quiet" onclick={clearBuild} disabled={!build} title="Forget the plan saved for this ship and start again from what is fitted">Clear this build</button>
    {#if msg}<span class="error small">{msg}</span>{/if}
  </div>

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
      {#if imported.swaps.length}
        <div class="table-wrap" style="margin-top:0.3rem">
          <table>
            <thead><tr><th>Slot</th><th>Fitted</th><th>Build wants</th></tr></thead>
            <tbody>
              {#each imported.swaps as sw}
                <tr><td class="muted">{sw.slot_name}</td><td>{sw.have ?? "empty"}</td><td>{sw.want}</td></tr>
              {/each}
            </tbody>
          </table>
        </div>
        <div class="muted">A swapped module is engineered from scratch: its rows below start at grade 0.</div>
      {/if}
      {#each imported.skipped as sk}<div class="warn">{sk}</div>{/each}
    </div>
  {/if}

  {#if perf && !perf.error}
    {@const b = perf.before}
    {@const a = perf.after}
    <div class="small perf" style="margin-top:0.6rem">
      <strong>As flown{a ? " → with this plan" : ""}</strong>
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
      <span class="muted small">{plannedCount} of {rows.length} modules planned · fitted engineering continues to the top grade unless you change it</span>
      <button class="quiet" onclick={() => includeAll(true)}>Include all chosen</button>
      <button class="quiet" onclick={() => includeAll(false)}>Include none</button>
    </div>
    <div class="table-wrap" style="margin-top:0.4rem">
      <table>
        <thead><tr><th></th><th>Slot</th><th>Module</th><th>Fitted</th><th>Blueprint</th><th>To grade</th><th>Experimental</th><th></th></tr></thead>
        <tbody>
          {#each rows as r, i (r.slot)}
            <tr class={isPlanned(r) ? "eng" : ""}>
              <td><input type="checkbox" checked={r.include && hasWork(r)} disabled={!hasWork(r)} onchange={(e) => update(i, { include: e.currentTarget.checked })} title={hasWork(r) ? "Include this module in the plan" : "Nothing to do: at the top grade and no experimental chosen"} /></td>
              <td class="small muted">{r.slot_name}</td>
              <td>{r.item_name}</td>
              <td class="small muted">{r.from_grade ? `G${r.from_grade}` : "—"}</td>
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
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <div class="row" style="margin-top:0.6rem">
      <button onclick={runPlan} disabled={planBusy || plannedCount === 0}>{planBusy ? "Working…" : "Materials for this build"}</button>
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
    <p class="muted" style="margin-top:0.6rem">{ships.length ? "Loading the build…" : "No ships in the journal yet."}</p>
  {/if}
</section>

<style>
  .perf { display: flex; flex-wrap: wrap; gap: 0.3rem 1.1rem; align-items: baseline; }
  tr.eng td { background: #7ec8ff10; }
  button.small { padding: 0.1rem 0.5rem; font-size: 0.78rem; }
  .import { border: 1px solid var(--line); border-radius: 6px; padding: 0.6rem 0.8rem; background: var(--panel-2); }
</style>
