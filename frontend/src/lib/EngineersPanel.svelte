<script>
  // The engineers directory (maintainer, 2026-09-29: "one thing I do miss
  // from the engineer tab is showing which engineers can do what — and who
  // is unlocked, known, unknown"; "which engineers can do which grades
  // would be nice so I can prioritize who to unlock first"). The old
  // Engineering tab was folded into the build planner in 0.3.5; this tab
  // is the directory alone: every engineer, the journal's word on them,
  // where they are and how they are met, what they do to what grade, and —
  // for one not unlocked — what unlocking them adds over the grades the
  // unlocked ones already reach, most first. Filter by a name, a system or
  // a module type; a grade pill filters too.
  import { onMount } from "svelte";
  import { engineerDirectory } from "./api.js";
  import { journalResource } from "./lifecycle.svelte.js";
  import Place from "./Place.svelte";

  let directory = $state([]);
  let error = $state("");
  let filter = $state("");
  const rows = $derived.by(() => {
    const f = filter.trim().toLowerCase();
    if (!f) return directory;
    return directory.filter((e) => e.name.toLowerCase().includes(f) || (e.system ?? "").toLowerCase().includes(f) || (e.base ?? "").toLowerCase().includes(f) || e.does.some((d) => d.module_type.toLowerCase().includes(f)));
  });
  const count = (status) => directory.filter((e) => (status === "Unlocked" ? e.unlocked : e.status === status)).length;

  async function load() {
    try { directory = await engineerDirectory(); error = ""; } catch (e) { error = String(e); }
  }
  // An unlock or an invite in the journal moves an engineer up the list.
  journalResource(load);
  onMount(() => { if (!directory.length) load(); });
</script>

<section class="panel">
  <h2>Engineers <span class="sub">who you have · where they are · what they do, to which grade · who to unlock next</span></h2>
  <div class="row" style="margin:0.4rem 0">
    <span class="pill ok">{count("Unlocked")} unlocked</span>
    <span class="pill warn">{count("Invited")} invited</span>
    <span class="pill">{count("Known")} known</span>
    <span class="pill">{count("Not known")} not yet met</span>
    <span class="muted small">of {directory.length}</span>
    <input placeholder="filter: engineer, system, or module type" bind:value={filter} style="min-width:18rem" />
    {#if filter}<button class="quiet small" onclick={() => (filter = "")}>clear</button>{/if}
  </div>
  {#if error}<p class="error small">{error}</p>{/if}
  <div class="table-wrap">
    <table>
      <thead><tr><th>Engineer</th><th>Status</th><th>Where</th><th>Does, to grade</th><th>Unlocking adds</th></tr></thead>
      <tbody>
        {#each rows as e (e.name)}
          <tr class={e.unlocked ? "" : e.status === "Not known" ? "dim" : ""}>
            <td><strong>{e.name}</strong>{#if e.guide_step != null}<span class="muted small"> · guide step {e.guide_step}</span>{/if}</td>
            <td><span class="pill {e.unlocked ? 'ok' : e.status === 'Invited' ? 'warn' : ''}">{e.status}{e.rank ? ` · rank ${e.rank}` : ""}</span>
              {#if !e.unlocked && e.unlock}<div class="muted small" title={e.invite ?? ""}>{e.status === "Not known" && e.invite ? `invite: ${e.invite} · ` : ""}unlock: {e.unlock}</div>{/if}</td>
            <td class="small"><Place system={e.system} station={e.base} />{e.base ? ` · ${e.base}` : ""}</td>
            <td>
              {#each e.does as d}
                <button class="pill" style="cursor:pointer; margin:0.1rem" title="Filter to {d.module_type}" onclick={() => (filter = d.module_type)}>{d.module_type} G{d.max_grade}</button>
              {/each}
            </td>
            <td class="small">
              {#if e.unlocked}
                <span class="muted">—</span>
              {:else if e.gains.length}
                {#each e.gains as g}
                  <span class="pill ok" style="margin:0.1rem" title={g.from_grade ? `you reach G${g.from_grade} now` : "no unlocked engineer does this"}>{g.module_type} {g.from_grade ? `G${g.from_grade}→G${g.to_grade}` : `G${g.to_grade} (none now)`}</span>
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
  .dim { opacity: 0.55; }
</style>
