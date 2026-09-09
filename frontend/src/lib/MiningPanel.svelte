<script>
  // The Mining page (maintainer shape, ledgered 2026-09-06): search one
  // material and get three honesty levels — YOUR marks (first-hand),
  // ring HOTSPOTS (exact, from the galaxy data), and could-have BODIES
  // (landable, ranked by surface concentration — a probability, not a
  // promise). Marks are private and local, permanently.
  import { onMount } from "svelte";
  import { miningSearch, miningMaterials, markAdd, markRemove, markHere } from "./api.js";
  import { fmtInt, fmtLy, fmtLs } from "./format.js";
  import Autocomplete from "./Autocomplete.svelte";
  import { journalResource } from "./lifecycle.svelte.js";

  // Autocomplete over what the data can actually answer: hotspot
  // minerals, surface raw materials, and the laser-mined goods that
  // resolve to a ring type. Display puts spaces back into wire names.
  let vocab = $state({ entries: [], laser: [] });
  const display = (stored) => stored.replace(/([a-z])([A-Z])/g, "$1 $2");
  const choices = $derived.by(() => {
    const seen = new Set();
    const out = [];
    for (const e of vocab.entries) {
      const name = display(e.stored);
      if (seen.has(name)) { continue; }
      seen.add(name);
      out.push({ name, detail: e.kind === "hotspot" ? "ring hotspots" : "surface material" });
    }
    for (const name of vocab.laser) {
      if (!seen.has(name)) { seen.add(name); out.push({ name, detail: "ring mining" }); }
    }
    return out.sort((a, b) => a.name.localeCompare(b.name));
  });
  function materialComplete(value) {
    const needle = value.trim().toLocaleLowerCase();
    if (!needle) return choices.slice(0, 20);
    return choices.filter((c) => c.name.toLocaleLowerCase().includes(needle)).slice(0, 20);
  }

  // What an unnamed mark is called -- one constant, so the button and
  // the row it creates never disagree.
  const MARK_FALLBACK = "Mining spot";

  let text = $state("");
  let radius = $state(100);
  let report = $state(null);
  let loading = $state(false);
  let error = $state("");
  let elapsed = $state(null);
  let fix = $state(null);
  // The mark form: label prefills from the search.
  let markBody = $state("");
  let markNote = $state("");
  let markMsg = $state("");

  // What a bookmark saved RIGHT NOW would record, refreshed on every
  // journal change. The button used to name a system fetched once at
  // mount, which went stale the moment the commander moved; worse, it
  // only ever said the system, while a surface bookmark is worth having
  // precisely because it can say the body and the exact spot.
  journalResource(async () => {
    fix = await markHere();
  });
  // The finest thing the fix actually names -- never more than the game
  // told us (maintainer: "the more granular position we can get the better").
  const fixWhere = $derived.by(() => {
    if (!fix?.system) return "";
    let s = fix.system;
    if (fix.station) s += ` — ${fix.station}`;
    if (fix.body) s += ` — ${fix.body}`;
    if (fix.latitude != null && fix.longitude != null) {
      s += ` @ ${fix.latitude.toFixed(4)}, ${fix.longitude.toFixed(4)}`;
    }
    return s;
  });
  const GRAIN = {
    surface: "exact surface position",
    body: "the body",
    station: "the station",
    system: "the system only",
  };

  onMount(async () => {
    try { vocab = await miningMaterials(); } catch {}
    run();
  });

  async function run() {
    loading = true; error = "";
    const started = performance.now();
    // The results area renders THE SEARCH, never the live input box
    // (maintainer: "we show search results, not what we hope").
    const searched = text.trim();
    try { report = { ...(await miningSearch(searched, Number(radius))), searched }; }
    catch (e) { error = String(e); }
    finally { elapsed = performance.now() - started; loading = false; }
  }

  async function addMark() {
    markMsg = "";
    try {
      const r = await markAdd(text.trim() || MARK_FALLBACK, markBody, markNote);
      // Report what was actually stored, not what was hoped for.
      const at = [r.station, r.body].filter(Boolean).join(" — ");
      const pos = r.latitude != null && r.longitude != null
        ? ` @ ${r.latitude.toFixed(4)}, ${r.longitude.toFixed(4)}` : "";
      markMsg = `Marked in ${r.system}${at ? ` — ${at}` : ""}${pos}.`;
      markBody = ""; markNote = "";
      run();
    } catch (e) { markMsg = String(e); }
  }

  async function removeMark(id) {
    try { await markRemove(id); run(); } catch (e) { markMsg = String(e); }
  }
</script>

<section class="panel">
  <h2>Mining <span class="sub">your marks · ring hotspots · candidate bodies</span></h2>

  <div class="search-grid">
    <label>Looking for
      <Autocomplete bind:value={text} minWidth="18rem" fetch={materialComplete} onenter={run}
        placeholder="Platinum, Gold, Iridium…" />
    </label>
    <label>Within <span class="inline"><input class="short" type="number" min="1" max="500" bind:value={radius} /> ly</span></label>
    <button class="go" onclick={run} disabled={loading}>{loading ? "Searching…" : "Search"}</button>
    {#if elapsed != null}<span class="muted small">{elapsed < 1000 ? `${elapsed.toFixed(0)} ms` : `${(elapsed / 1000).toFixed(1)} s`}</span>{/if}
  </div>
  {#if error}<p class="error">{error}</p>{/if}

  <!-- Marks: first-hand knowledge outranks everything. -->
  <h3 style="margin-top:0.9rem">Your marks {#if report?.marks?.length}<span class="muted small">({report.marks.length})</span>{/if}</h3>
  {#if report?.marks?.length}
    <div class="table-wrap"><table>
      <thead><tr><th>What</th><th>System</th><th>Where</th><th class="num">Distance</th><th>Note</th><th></th></tr></thead>
      <tbody>
        {#each report.marks as m}
          <tr>
            <td><strong>{m.label}</strong></td>
            <td>{m.system}</td>
            <td>{[m.station, m.body].filter(Boolean).join(" — ")}{m.latitude != null && m.longitude != null ? ` @ ${m.latitude.toFixed(3)}, ${m.longitude.toFixed(3)}` : ""}</td>
            <td class="num">{m.distance_ly != null ? fmtLy(m.distance_ly) : "—"}</td>
            <td class="muted">{m.note ?? ""}</td>
            <td><button class="ghost" title="Remove this mark" onclick={() => removeMark(m.id)}>✕</button></td>
          </tr>
        {/each}
      </tbody>
    </table></div>
  {:else}
    <p class="muted small">No bookmarks{report?.searched ? ` matching “${report.searched}”` : ""} yet — private notes, stored only on this machine. Label or note is what gets searched, so anything works: a hotspot, a brain-tree site, a good pad.</p>
  {/if}
  <div class="row small" style="margin-top:0.4rem; gap:0.6rem; align-items:center">
    <span class="muted">Bookmark {text.trim() || MARK_FALLBACK} at <strong>{fixWhere || "current position"}</strong></span>
    {#if fix?.grain}<span class="pill {fix.grain === "surface" ? "ok" : ""}" title="How precisely this bookmark can be saved right now">{GRAIN[fix.grain] ?? fix.grain}</span>{/if}
    <input bind:value={markBody} placeholder="body (optional)" style="width:9rem" />
    <input bind:value={markNote} placeholder="note (optional)" style="width:16rem" />
    <button class="quiet" onclick={addMark}>Bookmark here</button>
    {#if markMsg}<span class="muted">{markMsg}</span>{/if}
  </div>

  {#if report?.searched}
    <!-- Hotspots: exact knowledge from the community galaxy data. -->
    <h3 style="margin-top:1.1rem">Ring hotspots {#if report?.hotspots?.length}<span class="muted small">({report.hotspots.length})</span>{/if}</h3>
    {#if report?.hotspots?.length}
      <div class="table-wrap"><table>
        <thead><tr><th>System</th><th>Ring</th><th>Type</th><th class="num">Hotspots</th><th class="num">Distance</th><th class="num">Arrival</th></tr></thead>
        <tbody>
          {#each report.hotspots as h}
            <tr>
              <td>{h.system}</td>
              <td>{h.ring}</td>
              <td>{h.ring_type ?? "?"}</td>
              <td class="num">{h.count}</td>
              <td class="num">{fmtLy(h.distance_ly)}</td>
              <td class="num">{h.distance_to_arrival != null ? fmtLs(h.distance_to_arrival) : "—"}</td>
            </tr>
          {/each}
        </tbody>
      </table></div>
      <p class="muted small">Reserve levels (pristine…) aren't in the current data and will join with a future galaxy update — nothing here is guessed.</p>
    {:else if report && report.known_hotspot}
      <p class="muted small">No {report.searched} hotspots within {radius} ly — try a wider radius.</p>
    {:else if report && !report.data_installed}
      <p class="muted small">The community API didn’t answer the hotspot search; your marks are always here. Try again in a moment.</p>
    {:else if report && report.ring_hint}
      <p class="muted small">{report.searched} has no hotspot mechanic — it is {report.ring_hint.why}; the nearest suitable rings are listed below.</p>
    {:else if report}
      <p class="muted small">“{report.searched}” isn’t hotspot-mapped — mined commodities like iridium come from Rhino surface sites, which aren’t chartable from any data source yet. Mark the ones you find.</p>
    {/if}

    {#if report?.rings?.length}
      <h3 style="margin-top:1.1rem">Nearest {report.rings[0].ring_type} rings <span class="muted small">({report.rings.length})</span></h3>
      <div class="table-wrap"><table>
        <thead><tr><th>System</th><th>Body</th><th>Ring</th><th class="num">Distance</th><th class="num">Arrival</th></tr></thead>
        <tbody>
          {#each report.rings as r}
            <tr>
              <td>{r.system}</td>
              <td>{r.body ?? "?"}</td>
              <td>{r.ring}</td>
              <td class="num">{fmtLy(r.distance_ly)}</td>
              <td class="num">{r.distance_to_arrival != null ? fmtLs(r.distance_to_arrival) : "—"}</td>
            </tr>
          {/each}
        </tbody>
      </table></div>
      <p class="muted small">Any ring of this type can carry {report.searched} — laser mining, no hotspot needed.</p>
    {/if}

    <!-- Bodies: a probability, labeled as one. -->
    <h3 style="margin-top:1.1rem">Bodies that could have it {#if report?.bodies?.length}<span class="muted small">({report.bodies.length})</span>{/if}</h3>
    {#if report?.bodies?.length}
      <div class="table-wrap"><table>
        <thead><tr><th>System</th><th>Body</th><th>Type</th><th class="num">Concentration</th><th class="num">Distance</th><th class="num">Arrival</th><th class="num">Gravity</th></tr></thead>
        <tbody>
          {#each report.bodies as b}
            <tr>
              <td>{b.system}</td>
              <td>{b.body ?? "?"}</td>
              <td class="muted">{b.sub_type ?? ""}</td>
              <td class="num">{b.percent.toFixed(1)}%</td>
              <td class="num">{fmtLy(b.distance_ly)}</td>
              <td class="num">{b.distance_to_arrival != null ? fmtLs(b.distance_to_arrival) : "—"}</td>
              <td class="num">{b.gravity != null ? b.gravity.toFixed(2) + "g" : "—"}</td>
            </tr>
          {/each}
        </tbody>
      </table></div>
      <p class="muted small">Landable bodies ranked by surface concentration — good odds for prospecting raw materials, not a guarantee, and surface <em>commodity</em> yields (gold, minerals for the Rhino) aren't knowable from composition. Mark what you actually find.</p>
    {:else if report && report.known_surface}
      <p class="muted small">No landable bodies with {report.searched} composition within {radius} ly.</p>
    {/if}
  {:else}
    <p class="muted small" style="margin-top:0.8rem">Type a material to search hotspots and candidate bodies; your marks are always shown.</p>
  {/if}
</section>
