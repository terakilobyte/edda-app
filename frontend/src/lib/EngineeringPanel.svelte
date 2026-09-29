<script>
  // Engineering planner: a graded blueprint (from → to) and, separately, an
  // eng.experimental effect (no grade). Each shows what you're short from your
  // live inventory and whether an unlocked engineer can apply it.
  import { onMount } from "svelte";
  // Per-module engineering lives on the Ships tab; "Plan" there lands here.
  import { eng } from "./engineering.svelte.js";
  import ShoppingReport from "./ShoppingReport.svelte";
  import { listModuleTypes, listBlueprintNames, checkBlueprint, blueprintAccess, checkExperimental, listEngineers, materialShopping, shipSlef, engineerDirectory } from "./api.js";

  let error = $state("");
  let loading = $state(false);
  $effect(() => { if (eng.shopping?.list) eng.picked = new Set(eng.shopping.list.trades.map((_, i) => i)); });
  function togglePick(i) { const s = new Set(eng.picked); s.has(i) ? s.delete(i) : s.add(i); eng.picked = s; }

  const blueprints = $derived(eng.options.filter((b) => b.grades.length > 0));
  const experimentals = $derived(eng.options.filter((b) => b.grades.length === 0));
  const grades = $derived(blueprints.find((b) => b.name === eng.blueprintName)?.grades ?? []);
  // The engineers directory: every engineer, status, where, what they do.
  let directory = $state([]);
  let directoryError = $state("");
  let directoryFilter = $state("");
  const directoryRows = $derived.by(() => {
    const f = directoryFilter.trim().toLowerCase();
    if (!f) return directory;
    return directory.filter((e) => e.name.toLowerCase().includes(f) || (e.system ?? "").toLowerCase().includes(f) || (e.base ?? "").toLowerCase().includes(f) || e.does.some((d) => d.module_type.toLowerCase().includes(f)));
  });
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
      if (!directory.length) {
        try { directory = await engineerDirectory(); } catch (e) { directoryError = String(e); }
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
          <span class="pill {e.unlocked ? 'ok' : e.status === 'Not known' ? 'bad' : 'warn'} {e.max_grade < eng.access.grade ? 'dim' : ''}" title={e.max_grade < eng.access.grade ? `${e.engineer} stops at grade ${e.max_grade}` : `${e.engineer} offers this to grade ${e.max_grade}`}>{e.engineer} · {e.status}{e.rank ? ` · rank ${e.rank}` : ""} · to G{e.max_grade}</span>
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

  <ShoppingReport shopping={eng.shopping} picked={eng.picked} onToggle={togglePick} />

  <!-- The directory (maintainer, 2026-09-29): every engineer, the journal's
       word on them, where they are, what they do to what grade, and — for
       one not unlocked — what unlocking them adds over the grades the
       unlocked ones already reach, most first: the order to unlock in.
       Filter by a name, a system or a module type; a grade pill filters too. -->
  <h3 style="margin-top:1rem">Engineers
    <span class="muted">{directory.filter((e) => e.unlocked).length} unlocked · {directory.filter((e) => e.status === "Invited").length} invited · {directory.filter((e) => e.status === "Known").length} known · {directory.filter((e) => e.status === "Not known").length} not yet met · of {directory.length}</span>
    <input placeholder="filter: engineer, system, or module type" bind:value={directoryFilter} style="margin-left:0.6rem; min-width:16rem" />
    {#if directoryFilter}<button class="quiet mini" onclick={() => (directoryFilter = "")}>clear</button>{/if}
  </h3>
  {#if directoryError}<p class="error small">{directoryError}</p>{/if}
  <div class="table-wrap">
    <table>
      <thead><tr><th>Engineer</th><th>Status</th><th>Where</th><th>Does, to grade</th><th>Unlocking adds</th></tr></thead>
      <tbody>
        {#each directoryRows as e (e.name)}
          <tr class={e.unlocked ? "" : e.status === "Not known" ? "dim" : ""}>
            <td><strong>{e.name}</strong>{#if e.guide_step != null}<span class="muted small"> · guide step {e.guide_step}</span>{/if}</td>
            <td><span class="pill {e.unlocked ? 'ok' : e.status === 'Invited' ? 'warn' : ''}">{e.status}{e.rank ? ` · rank ${e.rank}` : ""}</span>
              {#if !e.unlocked && e.unlock}<div class="muted small" title={e.invite ?? ""}>{e.status === "Not known" && e.invite ? `invite: ${e.invite} · ` : ""}unlock: {e.unlock}</div>{/if}</td>
            <td class="small">{e.system ?? "—"}{e.base ? ` · ${e.base}` : ""}</td>
            <td>
              {#each e.does as d}
                <button class="pill {eng.moduleType === d.module_type ? 'accent' : ''}" style="cursor:pointer; margin:0.1rem" title="Filter the directory to {d.module_type}" onclick={() => (directoryFilter = d.module_type)}>{d.module_type} G{d.max_grade}</button>
              {/each}
            </td>
            <td class="small">
              {#if e.unlocked}
                <span class="muted">—</span>
              {:else if e.gains.length}
                {#each e.gains as g}
                  <span class="pill ok" style="margin:0.1rem" title="{g.from_grade ? `you reach G${g.from_grade} now` : 'no unlocked engineer does this'}">{g.module_type} {g.from_grade ? `G${g.from_grade}→G${g.to_grade}` : `G${g.to_grade} (none now)`}</span>
                {/each}
              {:else}
                <span class="muted">nothing beyond your unlocked engineers</span>
              {/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
</section>

<style>
  /* An engineer who stops below the asked grade is still named, quietly. */
  .pill.dim { opacity: 0.7; font-style: italic; }
  h3 { font-size: 0.9rem; margin: 0.6rem 0 0.3rem; display: flex; gap: 0.6rem; align-items: baseline; }
  h3 span { font-size: 0.78rem; }
  .ok-box { border-color: #57c66d55; background: #57c66d10; }
  .bad-box { border-color: #ff5c5c55; background: #ff5c5c10; }
  .dim { opacity: 0.45; }
  .mini { font-size: 0.7rem; padding: 0 0.35rem; margin-left: 0.3rem; }
</style>
