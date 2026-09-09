<script>
  // Following a route in the game: the captured "next system" control, or
  // the galaxy-map macro taught from four clicked places.
  import { onMount } from "svelte";
  import {
    targetMacroEnabledGet, targetMacroEnabledSet, targetMacroGet, targetMacroSet, targetMacroCheck, targetMacroPresets, targetMacroTest, mapPointCapture, mapPointsGet, mapPointsClear, mapDelaySet, targetKeyStatus, targetTriggerCapture, targetTriggerClear, macroRecordStart, macroRecordStop, gameRouteMaxGet, gameRouteMaxSet,
  } from "./api.js";

  // Route coverage distance, previously reachable only in setup (maintainer,
  // 2026-09-05: expose it here too). Same log-scaled slider as the
  // onboarding Route index step; both write the same backend value.
  const routeTicks = [0, 100, 500, 1000, 5000, 10000, 20000];
  const scaleToLy = (x) => { const p = Math.max(0, Math.min(600, Number(x))) / 100, i = Math.min(5, Math.floor(p)), f = p - i, a = routeTicks[i], b = routeTicks[i + 1]; return i === 0 ? Math.round(100 * f) : Math.round(Math.exp(Math.log(a) + (Math.log(b) - Math.log(a)) * f)); };
  const lyToScale = (v) => { if (v <= 100) return v; let j = routeTicks.findIndex((x) => v <= x); if (j <= 1) return v; j--; return (j + (Math.log(v) - Math.log(routeTicks[j])) / (Math.log(routeTicks[j + 1]) - Math.log(routeTicks[j]))) * 100; };
  let gameRouteMax = $state(1000); let routeScale = $state(300);
  onMount(async () => { try { gameRouteMax = await gameRouteMaxGet(); routeScale = lyToScale(gameRouteMax); } catch {} });
  async function saveRouteScale() { try { gameRouteMax = await gameRouteMaxSet(scaleToLy(routeScale)); routeScale = lyToScale(gameRouteMax); } catch (e) { macroMsg = String(e); } }
  const fmtLyLabel = (v) => (v === 0 ? "0 ly" : v >= 1000 ? `${(v / 1000).toFixed(v % 1000 === 0 ? 0 : 1)}k ly` : `${v} ly`);

  // "Target next system" macro: steps from the commander's own binds.
  let macro = $state([]); let macroText = $state(""); let macroCheck = $state(null); let macroMsg = $state(""); let macroPresets = $state([]);
  const fmtMacro = (m) => JSON.stringify(m, null, 1).replace(/\n\s*/g, " ").replace(/\},\s*\{/g, "},\n {");
  async function loadMacro() {
    try { macro = await targetMacroGet(); macroText = fmtMacro(macro); macroCheck = await targetMacroCheck(); macroPresets = await targetMacroPresets(); } catch (e) { macroMsg = String(e); }
  }
  async function testMacro() { macroMsg = "Sending… switch to the game within 3 s."; await new Promise((r) => setTimeout(r, 3000)); try { macroMsg = await targetMacroTest(); } catch (e) { macroMsg = String(e); } }
  let recording = $state(false); let recordSystem = $state("");
  // The four taught places on the galaxy map.
  const WAVES = [
    { key: "search", label: "Search box", hint: "the text field at the top left of the galaxy map", delay: "open", delayLabel: "wait after the map opens", def: 500 },
    { key: "result", label: "First result", hint: "the first system in the list after a search", delay: "search", delayLabel: "wait for the results", def: 1800 },
    { key: "target", label: "Target button", hint: "the target control in the system's info panel", delay: "target", delayLabel: "wait after the result is clicked", def: 1200 },
    { key: "plot", label: "Plot route button", hint: "the control that asks Elite to calculate a route", delay: "target", delayLabel: "wait after the result is clicked", def: 1200 },
  ];
  async function setDelay(which, v, def) { try { mapPoints = await mapDelaySet(which, Number.isFinite(Number(v)) ? Number(v) : def); macroMsg = "Saved."; } catch (e) { macroMsg = String(e); } }
  let mapPoints = $state(null); let capturingPoint = $state(null);
  // "Next system" for app routes: the game's own bindings plus one captured key or stick button.
  let targetKeys = $state({ bindings: [], trigger: null }); let capturingTrigger = $state(false);
  async function loadTargetKeys() { try { targetKeys = await targetKeyStatus(); } catch {} }
  onMount(loadTargetKeys);
  async function captureTrigger() {
    capturingTrigger = true; macroMsg = "Press the key or stick button you use for next system in route, within 10 s.";
    try { targetKeys = await targetTriggerCapture(); macroMsg = "Captured and watching."; } catch (e) { macroMsg = String(e); } finally { capturingTrigger = false; }
  }
  async function clearTrigger() { try { targetKeys = await targetTriggerClear(); macroMsg = "Cleared."; } catch (e) { macroMsg = String(e); } }
  onMount(async () => { try { mapPoints = await mapPointsGet(); } catch {} });
  async function capturePoint(which) {
    const instructions = { search: "Alt-Tab to Elite, open the Galaxy Map, click Search so EDDA records it, search for Sol, then return without closing the map.", result: "Return to Elite, click Sol in the results so EDDA records it, then return without closing the map.", target: "Return to Elite, click Target for Sol so EDDA records it, then return without closing the map.", plot: "Return to Elite, click Plot Route for Sol so EDDA records it, then return without closing the map." };
    capturingPoint = which; macroMsg = `${instructions[which]} You have 30 seconds.`;
    try { mapPoints = await mapPointCapture(which); macroCheck = await targetMacroCheck(); macroMsg = mapPoints.search && mapPoints.result && mapPoints.target && mapPoints.plot ? "All four taught. Press Test with the game open." : "Got it."; }
    catch (e) { macroMsg = String(e); }
    finally { capturingPoint = null; }
  }
  async function clearPoints() { try { mapPoints = await mapPointsClear(); macroCheck = await targetMacroCheck(); macroMsg = "Cleared; the built-in recipe is back."; } catch (e) { macroMsg = String(e); } }
  async function startRecord() {
    try { await macroRecordStart(); recording = true; macroMsg = "Recording keys and clicks. Switch to the game, do the whole thing by hand (open the map, search, type the system name, click the result, click the target button), then come back and press Stop."; } catch (e) { macroMsg = String(e); }
  }
  async function stopRecord() {
    try {
      const steps = await macroRecordStop(recordSystem.trim() || null);
      recording = false;
      macro = await targetMacroSet(steps);
      macroCheck = await targetMacroCheck();
      macroMsg = `Recorded ${steps.length} steps and saved them. Press Test to try it.`;
    } catch (e) { recording = false; macroMsg = String(e); }
  }
  onMount(loadMacro);
  let macroOn = $state(false);
  onMount(async () => { try { macroOn = await targetMacroEnabledGet(); } catch {} });
</script>

<section class="panel">
  <h2>Follow a route in the game <span class="sub">say “Guidance, target next”</span></h2>
  <p class="small">The voice command targets the next stop in the route you are following.</p>
  <p class="small" style="margin:0.5rem 0 0.2rem">For one-button use, capture the same control you use in Elite for <b>Target Next System in Route</b>. One press then keeps both Elite and EDDA in step.</p>
  <ul class="waves">
    {#each targetKeys.bindings as k}
      <li>
        <span class="wave-name">{k.source === "game" ? "in the game" : "captured"}</span>
        <span>{k.human}</span>
        {#if k.watched}<span class="pill ok">watched</span>{:else}<span class="pill warn" title={k.note ?? ""}>not watched</span>{/if}
        {#if k.note}<span class="muted small">{k.note}</span>{/if}
      </li>
    {/each}
    <li>
      <span class="wave-name">capture</span>
      <button class="quiet" onclick={captureTrigger} disabled={capturingTrigger}>{capturingTrigger ? "press it now…" : (targetKeys.trigger ? "Capture again" : "Capture key or button")}</button>
      {#if targetKeys.trigger}<button class="ghost" onclick={clearTrigger}>Forget</button>{/if}
    </li>
  </ul>
  <label class="small"><input type="checkbox" bind:checked={macroOn} onchange={(e) => targetMacroEnabledSet(e.target.checked)} /> Enable EDDA routing control</label>
  {#if macroOn}
    <p class="small" style="margin:0.5rem 0 0.3rem">Teach it four places once. Target handles an EDDA route hop; Plot route asks Elite to calculate the full in-game route for nearer destinations.</p>
    <ol class="waves">
      {#each WAVES as w, n}
        <li>
          <span class="wave-name">{n + 1}. {w.label}</span>
          {#if mapPoints?.[w.key]}<span class="pill ok" title="{mapPoints[w.key][0]}, {mapPoints[w.key][1]} of the game window">taught</span>{:else}<span class="pill warn">not yet</span>{/if}
          <button class="quiet" onclick={() => capturePoint(w.key)} disabled={capturingPoint !== null}>{capturingPoint === w.key ? "click it in the game…" : "Capture"}</button>
          <span class="muted small">{w.hint}</span>
          <label class="small" style="margin-left:auto" title="Raise this if the map is slow on your machine">{w.delayLabel} <input type="number" min="0" max="10000" step="100" style="width:5.5rem" value={mapPoints?.[`${w.delay}_delay_ms`] ?? w.def} onchange={(e) => setDelay(w.delay, e.target.value, w.def)} /> ms</label>
        </li>
      {/each}
    </ol>
    <div class="row">
      <button class="quiet" onclick={testMacro} disabled={!(mapPoints?.search && mapPoints?.result && mapPoints?.target)} title="Runs the recipe in the game after a 3 s delay (the next system of a followed route, or Sol)">Test (3 s delay)</button>
      <button class="ghost" onclick={clearPoints} disabled={!mapPoints?.search && !mapPoints?.result && !mapPoints?.target}>Start over</button>
      {#if macroCheck && !macroCheck.ok}<span class="pill bad" title="The recipe presses your galaxy-map key; bind Galaxy Map in the game's controls">Galaxy Map has no key bound in the game</span>{/if}
    </div>
  {/if}
  {#if macroMsg}<p class="muted small" style="margin:0.3rem 0 0">{macroMsg}</p>{/if}

  <h2 style="margin-top:1rem">Route coverage <span class="sub">how far EDDA plans on its own</span></h2>
  <!-- Label matched to what the code actually does (2026-09-05: it
       described the exact opposite): SHORT journeys go to the game's
       own plotter, EDDA plans the long ones. -->
  <p class="small">Journeys up to this distance are handed to the game's own plotter (needs the taught Galaxy Map controls); beyond it, EDDA plans and follows the route itself. {gameRouteMax === 0 ? "Currently: EDDA always plans." : `Currently ${fmtLyLabel(gameRouteMax)}.`}</p>
  <div class="row">
    <input type="range" min="0" max="600" bind:value={routeScale} onchange={saveRouteScale} style="flex:1" aria-label="Route coverage distance in light years" />
    <span class="num" style="min-width:6rem">{fmtLyLabel(scaleToLy(routeScale))}</span>
  </div>
</section>

<style>
  .waves { margin: 0.3rem 0 0.5rem; padding-left: 0; list-style: none; display: flex; flex-direction: column; gap: 0.35rem; }
  .waves li { display: flex; align-items: center; gap: 0.6rem; flex-wrap: wrap; }
  .wave-name { min-width: 9rem; font-weight: 600; }
</style>
