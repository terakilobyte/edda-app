<script>
  // Engineering planner: a graded blueprint (from → to) and, separately, an
  // eng.experimental effect (no grade). Each shows what you're short from your
  // live inventory and whether an unlocked engineer can apply it.
  import { onMount } from "svelte";
  // Per-module engineering lives on the Ships tab; "Plan" there lands here.
  import { requestRoute } from "./route.svelte.js";
  import { eng } from "./engineering.svelte.js";
  import { listModuleTypes, listBlueprintNames, checkBlueprint, blueprintAccess, checkExperimental, listEngineers, materialShopping, shipSlef } from "./api.js";

  let error = $state("");
  let loading = $state(false);
  $effect(() => { if (eng.shopping?.list) eng.picked = new Set(eng.shopping.list.trades.map((_, i) => i)); });
  function togglePick(i) { const s = new Set(eng.picked); s.has(i) ? s.delete(i) : s.add(i); eng.picked = s; }
  // Trader kinds needed by the selected trades plus any farm-then-trade plan.
  const neededKinds = $derived(new Set([
    ...(eng.shopping?.list?.trades ?? []).filter((_, i) => eng.picked.has(i)).map((t) => t.kind),
    ...(eng.shopping?.farm ?? []).flatMap((p) => p.options.filter((o) => o.rate).map((o) => o.kind)),
  ]));

  const blueprints = $derived(eng.options.filter((b) => b.grades.length > 0));
  const experimentals = $derived(eng.options.filter((b) => b.grades.length === 0));
  const grades = $derived(blueprints.find((b) => b.name === eng.blueprintName)?.grades ?? []);
  $effect(() => {
    if (grades.length) {
      if (!grades.includes(Number(eng.targetGrade))) eng.targetGrade = grades[grades.length - 1];
      if (Number(eng.fromGrade) >= Number(eng.targetGrade)) eng.fromGrade = 0;
    }
  });

  onMount(async () => {
    try {
      if (!eng.moduleTypes.length) {
        [eng.moduleTypes, eng.engineers] = await Promise.all([listModuleTypes(), listEngineers()]);
      }
      // A module handed over from the Ships tab.
      if (eng.planRequest) {
        const m = eng.planRequest;
        eng.planRequest = null;
        await planFrom(m);
      }
    } catch (e) {
      error = String(e);
    }
  });

  async function onModuleTypeChange() {
    eng.planSlot = null;
    eng.blueprintName = "";
    eng.experimentalName = "";
    eng.report = null;
    eng.access = null;
    eng.experimental = null;
    eng.options = eng.moduleType ? await listBlueprintNames(eng.moduleType) : [];
  }

  async function runCheck() {
    if (!eng.moduleType || (!eng.blueprintName && !eng.experimentalName)) return;
    loading = true;
    error = "";
    try {
      const jobs = [];
      jobs.push(eng.blueprintName
        ? Promise.all([checkBlueprint(eng.moduleType, eng.blueprintName, Number(eng.fromGrade), Number(eng.targetGrade), eng.minimumRolls, eng.completeTarget), blueprintAccess(eng.moduleType, eng.blueprintName, Number(eng.targetGrade))])
        : Promise.resolve([null, null]));
      jobs.push(eng.experimentalName ? checkExperimental(eng.moduleType, eng.experimentalName) : Promise.resolve(null));
      const [[r, a], x] = await Promise.all(jobs);
      eng.report = r; eng.access = a; eng.experimental = x;
      // Shortfalls become trades at material traders from what we carry.
      eng.shopping = null;
      const anyShort = (r && !r.fully_met) || (x && !x.gap.fully_met);
      if (anyShort) {
        try {
          eng.shopping = await materialShopping({
            moduleType: eng.moduleType, blueprint: eng.blueprintName || null, fromGrade: Number(eng.fromGrade), targetGrade: Number(eng.targetGrade),
            minimum: eng.minimumRolls, complete: eng.completeTarget, experimental: eng.experimentalName || null,
          });
        } catch (e) { eng.shopping = { error: String(e) }; }
      }
    } catch (e) {
      error = String(e);
      eng.report = null; eng.access = null; eng.experimental = null;
    } finally {
      loading = false;
    }
  }

  const unlocked = $derived(eng.engineers.filter((e) => e.progress === "Unlocked"));

  // Export the build with this plan applied at nominal full-grade values:
  // theorycraft in EDSY before any materials are spent.
  let slefMsg = $state("");
  async function copyPlannedBuild() {
    if (!eng.planSlot || !eng.blueprintName) return;
    try {
      const slef = await shipSlef(eng.planSlot.shipId, {
        slot: eng.planSlot.slot,
        module_type: eng.moduleType,
        blueprint: eng.blueprintName,
        grade: Number(eng.targetGrade),
      });
      await navigator.clipboard.writeText(slef);
      slefMsg = `Build with ${eng.blueprintName} G${eng.targetGrade} copied as SLEF — paste into EDSY or Coriolis (Import).`;
    } catch (e) { slefMsg = String(e); }
  }

  // Plan a ship module: an engineered one continues from its grade, an
  // unengineered one starts at 0 (the blueprint is the commander's pick).
  async function planFrom(m) {
    if (!m.module_type) return;
    eng.moduleType = m.module_type;
    await onModuleTypeChange();
    eng.planSlot = m.slot ? { slot: m.slot, shipId: m.ship_id ?? null } : null;
    if (m.blueprint) {
      eng.blueprintName = m.blueprint;
      eng.fromGrade = m.grade ?? 0;
      const gs = eng.options.find((b) => b.name === m.blueprint)?.grades ?? [];
      eng.targetGrade = gs.length ? gs[gs.length - 1] : 5;
      if (Number(eng.fromGrade) >= Number(eng.targetGrade)) eng.fromGrade = 0;
      await runCheck();
    }
  }
</script>

<section class="panel">
  <h2>Engineering <span class="sub">vendored blueprint data · your live inventory</span></h2>

  <div class="row" style="margin-bottom:0.5rem">
    <select bind:value={eng.moduleType} onchange={onModuleTypeChange}>
      <option value="">Module type…</option>
      {#each eng.moduleTypes as t}<option value={t}>{t}</option>{/each}
    </select>
    <select bind:value={eng.blueprintName} disabled={!eng.moduleType}>
      <option value="">Blueprint…</option>
      {#each blueprints as b}<option value={b.name}>{b.name}</option>{/each}
    </select>
    {#if eng.blueprintName}
      <label>from <select bind:value={eng.fromGrade}><option value={0}>0</option>{#each grades.slice(0, -1) as g}<option value={g}>{g}</option>{/each}</select></label>
      <label>to <select bind:value={eng.targetGrade}>{#each grades as g}<option value={g}>{g}</option>{/each}</select></label>
    {/if}
    <select bind:value={eng.experimentalName} disabled={!eng.moduleType || experimentals.length === 0} title="Experimental effect: one application, no grade">
      <option value="">Experimental…</option>
      {#each experimentals as x}<option value={x.name}>{x.name}</option>{/each}
    </select>
    <label title="On: N rolls at the target grade too, which completes it (each roll adds 1/N -- your G5 FSD roll was 0.2). Off: one roll, which only reaches the grade."><input type="checkbox" bind:checked={eng.completeTarget} /> complete target grade</label>
    <label title="Off: N rolls at grade N to unlock the next (measured from your crafts). On: one roll per grade, the theoretical minimum."><input type="checkbox" bind:checked={eng.minimumRolls} /> minimum rolls</label>
    <button onclick={runCheck} disabled={!eng.moduleType || (!eng.blueprintName && !eng.experimentalName) || loading}>{loading ? "…" : "Check"}</button>
    {#if eng.planSlot && eng.blueprintName}
      <button class="quiet" onclick={copyPlannedBuild} title="Copy the ship's build with this plan applied (nominal full-grade values)">Copy build with this plan</button>
    {/if}
  </div>
  {#if slefMsg}<p class="muted small">{slefMsg}</p>{/if}

  {#if error}<p class="error">{error}</p>{/if}

  {#if eng.access}
    <div class="eng.access {eng.access.reachable ? 'ok-box' : 'bad-box'}">
      {#if eng.access.reachable}
        <strong>{eng.access.name} grade {eng.access.grade}: reachable.</strong>
      {:else}
        <strong>{eng.access.name} grade {eng.access.grade}: not reachable.</strong>
        {#if eng.access.max_reachable_grade}Highest you can apply today: grade {eng.access.max_reachable_grade}.{:else}No unlocked engineer offers this.{/if}
      {/if}
      <div class="row small" style="margin-top:0.3rem">
        {#each eng.access.engineers as e}
          <span class="pill {e.unlocked ? 'ok' : e.status === 'Not known' ? 'bad' : 'warn'}">{e.engineer} · {e.status}{e.rank ? ` · rank ${e.rank}` : ""}</span>
        {/each}
      </div>
    </div>
  {/if}

  {#if eng.report}
    <h3>
      {eng.report.name} → Grade {eng.report.grade}{eng.completeTarget && !eng.minimumRolls ? ", complete" : ""} <span class="muted">{eng.minimumRolls ? "one roll per grade (minimum)" : eng.completeTarget ? `N rolls at grade N, including ${eng.report.grade} at grade ${eng.report.grade}` : "N rolls at grade N to unlock the next; target rolled once"}</span>
      <span class={eng.report.fully_met ? "ok" : "warn"}>{eng.report.fully_met ? "all materials in hand" : "missing materials"}</span>
      {#if eng.access && !eng.access.reachable}<span class="bad">no engineer available yet{eng.access.max_reachable_grade ? ` above grade ${eng.access.max_reachable_grade}` : ""}</span>{/if}
    </h3>
    <div class="table-wrap">
      <table>
        <thead><tr><th>Material</th><th class="r">Need</th><th class="r">Have</th><th>Short</th></tr></thead>
        <tbody>
          {#each eng.report.lines as line}
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
  {/if}

  {#if eng.experimental}
    <div class="eng.access {eng.experimental.reachable ? 'ok-box' : 'bad-box'}" style="margin-top:0.6rem">
      <strong>{eng.experimental.gap.name} (eng.experimental): {eng.experimental.reachable ? "reachable" : "no unlocked engineer offers this"}.</strong>
      <div class="row small" style="margin-top:0.3rem">
        {#each eng.experimental.engineers as e}
          <span class="pill {e.unlocked ? 'ok' : e.status === 'Not known' ? 'bad' : 'warn'}">{e.engineer} · {e.status}{e.rank ? ` · rank ${e.rank}` : ""}</span>
        {/each}
      </div>
    </div>
    <h3>
      {eng.experimental.gap.name} <span class="muted">eng.experimental effect</span>
      <span class={eng.experimental.gap.fully_met ? "ok" : "warn"}>{eng.experimental.gap.fully_met ? "all materials in hand" : "missing materials"}</span>
      {#if eng.experimental && !eng.experimental.reachable}<span class="bad">no engineer available yet</span>{/if}
    </h3>
    <div class="table-wrap">
      <table>
        <thead><tr><th>Material</th><th class="r">Need</th><th class="r">Have</th><th>Short</th></tr></thead>
        <tbody>
          {#each eng.experimental.gap.lines as line}
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
  {/if}

  {#if eng.shopping}
    <h3 style="margin-top:0.8rem">Shopping list <span class="muted">material traders · 6:1 per grade up, 3:1 per grade down, ×6 across groups</span></h3>
    {#if eng.shopping.error}
      <p class="error">{eng.shopping.error}</p>
    {:else}
      {#if eng.shopping.list.trades.length}
        <div class="table-wrap">
          <table>
            <thead><tr><th></th><th>At</th><th class="r">Give</th><th></th><th class="r">Get</th><th></th><th>Rate</th></tr></thead>
            <tbody>
              {#each eng.shopping.list.trades as t, i}
                <tr class={eng.picked.has(i) ? "" : "dim"}>
                  <td><input type="checkbox" checked={eng.picked.has(i)} onchange={() => togglePick(i)} /></td>
                  <td class="muted">{t.kind} trader</td>
                  <td class="r num">{t.give}</td><td>{t.give_material}</td>
                  <td class="r num">{t.get}</td><td>{t.get_material}</td>
                  <td class="muted">{t.rate}{t.same_group ? "" : " (cross-group)"}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
      {#if eng.shopping.list.still_short.length}
        <p class="warn small">Still short after trading: {eng.shopping.list.still_short.map(([m, n]) => `${n} ${m}`).join(", ")}.</p>
      {:else if eng.shopping.list.trades.length}
        <p class="ok small">Everything covered by trading what you carry.</p>
      {/if}
      {#each eng.shopping.traders.filter((t) => neededKinds.has(t.kind)) as t}
        <div class="small" style="margin:0.3rem 0">
          <strong>Nearest {t.kind} traders</strong>{eng.shopping.origin_system ? ` from ${eng.shopping.origin_system}` : ""}:
          {#if t.nearest.length}
            <div class="row small" style="margin-top:0.2rem">
              {#each t.nearest as n}
                <span class="pill">{n.station.name} · {n.station.system_name} · {n.distance_ly.toFixed(1)} ly{n.station.distance_to_arrival != null ? ` · ${Math.round(n.station.distance_to_arrival)} ls` : ""}
                  <button class="mini" onclick={() => requestRoute(n.station.system_name)} title="Plot a route there in the Route tab">route</button></span>
              {/each}
            </div>
          {:else}
            <span class="muted">none known within 150 ly{eng.shopping.origin_system ? "" : " (position unknown)"}</span>
          {/if}
        </div>
      {/each}
      {#each eng.shopping.farm as plan}
        <div class="small" style="margin:0.4rem 0">
          <strong>{plan.needed} {plan.material}</strong> — collect, or farm something and trade it in:
          <div class="table-wrap">
            <table>
              <thead><tr><th>Site</th><th class="r">Collect</th><th></th><th>Then</th><th class="r">ly</th><th></th></tr></thead>
              <tbody>
                {#each plan.options as o}
                  <tr>
                    <td>{o.site}{o.body ? ` · ${o.body}` : ""}</td>
                    <td class="r num">{o.collect}</td><td>{o.farm_material}</td>
                    <td class="muted">{o.rate ? `trade ${o.rate} → ${o.get} ${plan.material} at a ${o.kind} trader` : "use directly"}</td>
                    <td class="r num">{o.distance_ly != null ? o.distance_ly.toFixed(0) : "?"}</td>
                    <td>{#if o.system}<button class="mini" onclick={() => requestRoute(o.system)}>route</button>{/if}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </div>
      {/each}
      {#each eng.shopping.list.still_short.filter(([m]) => !eng.shopping.farm.some((p) => p.material === m)) as [m, n]}
        <p class="small muted">{n} {m}: no known farm site vendored — ask the ship computer where you've collected it before.</p>
      {/each}
    {/if}
  {/if}

  <h3 style="margin-top:1rem">Engineers <span class="muted">({unlocked.length} unlocked of {eng.engineers.length} known)</span></h3>
  <div class="row small">
    {#each eng.engineers as e}
      <span class="pill {e.progress === 'Unlocked' ? 'ok' : e.progress === 'Invited' ? 'warn' : ''}">{e.name} · {e.progress ?? "?"}{e.rank ? ` · ${e.rank}` : ""}</span>
    {/each}
  </div>
</section>

<style>
  h3 { font-size: 0.9rem; margin: 0.6rem 0 0.3rem; display: flex; gap: 0.6rem; align-items: baseline; }
  h3 span { font-size: 0.78rem; }
  .ok-box { border-color: #57c66d55; background: #57c66d10; }
  .bad-box { border-color: #ff5c5c55; background: #ff5c5c10; }
  .dim { opacity: 0.45; }
  .mini { font-size: 0.7rem; padding: 0 0.35rem; margin-left: 0.3rem; }
</style>
