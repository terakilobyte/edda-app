<script>
  // The Voice tab: personality, output voice, callout kinds, signal watch,
  // and voice input (activation word / push-to-talk).
  import { onMount } from "svelte";
  import { voiceName, serverVoiceName } from "./voices.js";
  import {
    voiceStatus, voiceModels, voiceInstallDefault, voiceRemove, voiceUseWindows, setVoice, setMuted, personas, setPersona, voiceServerGet, voiceServerSet, voiceServerProbe, signalWatchGet, signalWatchSet,
    calloutsGet, calloutsSet,
    listenStatus, listenConfigSet, listenSetup, listenPtt, onListenState, onListenHeard, onListenReply, onListenSetup, onListenPartial, joyDevices, pttCapture, audioDevices,
  } from "./api.js";
  import { useListeners } from "./lifecycle.svelte.js";

  const listeners = useListeners();
  let voice = $state(null);
  let models = $state([]);
  let voiceInstalling = $state(false);
  let chosen = $state("");
  let msg = $state("");

  // Speech server (Kokoro and other OpenAI-compatible providers).
  let vs = $state(null); let vsEngine = $state("builtin"); let vsUrl = $state("http://localhost:8880"); let vsModel = $state("kokoro"); let vsVoice = $state("af_heart"); let vsKey = $state(""); let vsMsg = $state(""); let vsVoices = $state([]); let vsPreset = $state("kokoro"); let vsBusy = $state(false);
  const SPEECH_PRESETS = [
    { id: "kokoro", label: "Kokoro (on this machine)", url: "http://localhost:8880", model: "kokoro", voices: [] },
    { id: "openai", label: "OpenAI", url: "https://api.openai.com", model: "gpt-4o-mini-tts", voices: ["alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer"] },
    { id: "other", label: "Other", url: "", model: "", voices: [] },
  ];
  function applySpeechPreset() { const p = SPEECH_PRESETS.find((x) => x.id === vsPreset); if (p && p.id !== "other") { vsUrl = p.url; vsModel = p.model; vsVoices = p.voices; if (p.voices.length) vsVoice = p.voices[0]; } }
  async function connectSpeech() {
    vsBusy = true; vsMsg = "";
    try {
      const v = await voiceServerProbe({ url: vsUrl.trim(), model: vsModel.trim(), voice: vsVoice.trim(), api_key: vsKey ? vsKey : (vs?.config?.api_key ?? null) });
      vsVoices = v.length ? v : (SPEECH_PRESETS.find((x) => x.id === vsPreset)?.voices ?? []);
      if (vsVoices.length && !vsVoices.includes(vsVoice)) vsVoice = vsVoices.includes("af_heart") ? "af_heart" : vsVoices[0];
      vsMsg = vsVoices.length ? `Connected · ${vsVoices.length} voices` : "Connected";
      if (vsEngine === "server") await saveVoiceServer();
    } catch (e) { vsMsg = String(e); vsVoices = []; } finally { vsBusy = false; }
  }
  async function loadVoiceServer() {
    try { vs = await voiceServerGet(); vsEngine = vs.enabled ? "server" : "builtin"; if (vs.config.url) { vsUrl = vs.config.url; vsModel = vs.config.model || "kokoro"; vsVoice = vs.config.voice || "af_heart"; vsVoices = vs.voices ?? []; vsPreset = SPEECH_PRESETS.find((p) => p.url && vsUrl.startsWith(p.url))?.id ?? "other"; } } catch (e) { vsMsg = String(e); }
  }
  async function saveVoiceServer() {
    try {
      vs = await voiceServerSet(vsEngine === "server", { url: vsUrl.trim(), model: vsModel.trim(), voice: vsVoice.trim(), api_key: vsKey ? vsKey : (vs?.config?.api_key ?? null) });
      vsKey = ""; voice = await voiceStatus(); vsMsg = "";
    } catch (e) { vsMsg = String(e); }
  }
  onMount(loadVoiceServer);

  // Signal watch and callout kinds on/off.
  let signals = $state([]);
  let calloutKinds = $state([]);
  onMount(async () => { try { calloutKinds = await calloutsGet(); } catch {} });
  async function toggleCallout(id, on) {
    const off = calloutKinds.filter((k) => (k.id === id ? !on : !k.on)).map((k) => k.id);
    try { calloutKinds = await calloutsSet(off); } catch {}
  }
  onMount(async () => { try { signals = await signalWatchGet(); } catch {} });
  async function toggleSignal(id, on) {
    const ids = signals.filter((s) => (s.id === id ? on : s.on)).map((s) => s.id);
    try { signals = await signalWatchSet(ids); } catch (e) { msg = String(e); }
  }

  // Voice input.
  let lst = $state(null); let lcfg = $state({ enabled: false, wake_word: "hey edda", window_secs: 6 }); let lMsg = $state(""); let lHeard = $state(""); let lReply = $state(""); let lSetup = $state(null); let lSetupJobs = $state({});
  // Push-to-talk source: none | keyboard hotkey | joystick button (vJoy etc.).
  // No invented control: everything stays empty until the commander provides one.
  let pttKind = $state("none"); let pttHotkey = $state(""); let pttJoyDevice = $state(-1); let pttJoyButton = $state(1); let joys = $state([]); let capturing = $state(false);
  function pttFromConfig(c) {
    const p = c.ptt ?? (c.ptt_hotkey ? { kind: "keyboard", hotkey: c.ptt_hotkey } : { kind: "none" });
    pttKind = p.kind; if (p.kind === "keyboard") pttHotkey = p.hotkey; if (p.kind === "joystick") { pttJoyDevice = p.device; pttJoyButton = p.button; }
  }
  function pttToConfig() {
    // Half-filled choices (blank hotkey, no joystick picked) are no control yet.
    if (pttKind === "keyboard") { const hotkey = pttHotkey.trim(); return hotkey ? { kind: "keyboard", hotkey } : { kind: "none" }; }
    if (pttKind === "joystick") { if (pttJoyDevice < 0) return { kind: "none" }; const j = joys.find((x) => x.id === pttJoyDevice); return { kind: "joystick", device: pttJoyDevice, button: Number(pttJoyButton), name: `${j?.name ?? "joystick"} button ${pttJoyButton}` }; }
    return { kind: "none" };
  }
  async function pttKindChanged() { if (pttKind === "joystick") { try { joys = await joyDevices(); if (pttJoyDevice < 0 && joys.length) pttJoyDevice = joys[0].id; } catch (e) { lMsg = String(e); } } await saveListen(); }
  async function capturePtt() {
    capturing = true; lMsg = "Press the key or joystick button you want for push-to-talk…";
    try { const src = await pttCapture(8); pttFromConfig({ ptt: src }); if (src.kind === "joystick") joys = await joyDevices(); await saveListen(); lMsg = `Captured and saved: ${src.kind === "keyboard" ? src.hotkey : src.name}.`; } catch (e) { lMsg = String(e); } finally { capturing = false; }
  }
  async function loadListen() { try { lst = await listenStatus(); lcfg = { ...lst.config }; modelChoice = lcfg.model ?? "small"; pttFromConfig(lcfg); if (pttKind === "joystick") joys = await joyDevices(); audio = await audioDevices(); } catch (e) { lMsg = String(e); } }
  async function saveListen() { try { lcfg = await listenConfigSet({ ...lcfg, ptt: pttToConfig(), model: modelChoice }); lMsg = "Saved."; await loadListen(); } catch (e) { lMsg = String(e); } }
  let audio = $state(null); let modelChoice = $state("small");
  const modelDir = (m) => (m === "parakeet" ? "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8" : "vosk-model-small-en-us-0.15");
  async function setupListen() { try { lSetupJobs = {}; lSetup = { phase: "model", status: "downloading" }; await listenSetup(modelChoice); } catch (e) { lMsg = String(e); } }
  onMount(async () => {
    await loadListen();
    listeners.add(onListenState((e) => { if (lst) lst = { ...lst, phase: e.payload.phase, running: e.payload.phase !== "off" }; if (e.payload.error) lMsg = e.payload.error; }));
    listeners.add(onListenHeard((e) => (lHeard = e.payload.text || (e.payload.note ? `(${e.payload.note}${e.payload.peak != null ? `, mic peak ${e.payload.peak}` : ""})` : ""))));
    listeners.add(onListenPartial((e) => (lHeard = `… ${e.payload.text} (mic peak ${e.payload.peak})`)));
    listeners.add(onListenReply((e) => (lReply = e.payload.text)));
    listeners.add(onListenSetup(async (e) => { lSetup = e.payload; if (e.payload.phase !== "done") lSetupJobs = { ...lSetupJobs, [e.payload.phase]: e.payload }; if (e.payload.phase === "done") { lMsg = e.payload.status === "ok" ? "Voice models installed." : `Setup failed: ${e.payload.error}`; await loadListen(); } }));
  });

  let personaList = $state([]);
  let persona = $state("standard");

  async function choosePersona(id) {
    try {
      const v = await setPersona(id);
      persona = v.selected;
      voice = await voiceStatus();
      chosen = models.find((m) => voice.model && m.startsWith(voice.model)) ?? chosen;
    } catch (e) { msg = String(e); }
  }

  onMount(async () => {
    try {
      const pv = await personas();
      personaList = pv.personas;
      persona = pv.selected;
    } catch (e) { msg = String(e); }
    try {
      voice = await voiceStatus();
      models = await voiceModels();
      chosen = models.find((m) => voice.model && m.startsWith(voice.model)) ?? "";
    } catch (e) { msg = String(e); }
  });

  async function changeVoice(e) {
    chosen = e.target.value;
    if (!chosen) return;
    try {
      await setVoice(chosen);
      voice = await voiceStatus();
    } catch (err) { msg = String(err); }
  }
  async function removeChosenVoice() {
    if (!chosen || !confirm(`Remove ${voiceName(chosen)} from EDDA's data folder?`)) return;
    try { models = await voiceRemove(chosen); chosen = models.find((m) => voice.model && m.startsWith(voice.model)) ?? ""; vsMsg = "Voice removed."; }
    catch (e) { vsMsg = String(e); }
  }

  // The engine the panel offers, by name (maintainer, 2026-09-09: "why
  // isn't piper there?" — it hid under "built-in" with the Windows
  // voice). Three choices; the pill still says what is actually speaking.
  const engineChoice = $derived(vsEngine === "server" ? "server" : voice?.backend === "sapi" ? "windows" : "piper");
  async function pickEngine(which) {
    vsMsg = "";
    try {
      if (which === "server") {
        vsEngine = "server";
        if (vsVoices.length) await saveVoiceServer(); else await connectSpeech();
        return;
      }
      if (vsEngine === "server") { vsEngine = "builtin"; await saveVoiceServer(); }
      if (which === "windows") {
        voice = await voiceUseWindows();
        vsMsg = "Windows voice selected. No local model or helper process is required.";
      } else {
        const model = chosen || models[0];
        if (!model) { vsMsg = "No Piper voice is installed yet — install one below."; return; }
        await setVoice(model);
        chosen = model;
        voice = await voiceStatus();
      }
    } catch (e) { vsMsg = String(e); }
  }

  const pretty = voiceName;
  async function installVoice() {
    voiceInstalling = true;
    vsMsg = "Downloading Piper and the Lessac neural voice (~135 MB)…";
    try {
      const model = await voiceInstallDefault();
      models = await voiceModels();
      voice = await voiceStatus();
      chosen = model;
      vsMsg = "Neural voice installed and selected.";
    } catch (e) { vsMsg = String(e); }
    finally { voiceInstalling = false; }
  }
</script>

<section class="panel">
  <h2>Personality <span class="sub">tone of the ship computer and its callouts</span></h2>
  <div class="personas">
    {#each personaList as p}
      <button class="pcard {persona === p.id ? 'on' : ''}" onclick={() => choosePersona(p.id)}>
        <div class="pname">{p.name}</div>
        <div class="pblurb">{p.blurb}</div>
      </button>
    {/each}
  </div>
  {#if msg}<p class="muted small" style="margin:0.5rem 0 0">{msg}</p>{/if}
</section>

<section class="panel">
  <h2>Voice</h2>
  <p class="muted small">No neural voice model is bundled. Windows voice works without a download; install EDDA's curated local voice here, add other Piper models later, or connect your own speech server.</p>
  {#if voice}
    <dl class="kv">
      <dt>Engine</dt>
      <dd>
        <div class="row">
          <label><input type="radio" name="voice-engine" value="windows" checked={engineChoice === "windows"} onchange={() => pickEngine("windows")} /> Windows voice</label>
          <label><input type="radio" name="voice-engine" value="piper" checked={engineChoice === "piper"} onchange={() => pickEngine("piper")} /> Piper (neural, inside EDDA)</label>
          <label><input type="radio" name="voice-engine" value="server" checked={engineChoice === "server"} onchange={() => pickEngine("server")} /> speech server (Kokoro or compatible)</label>
          <span class="pill {voice.backend === 'server' ? 'cyan' : voice.backend === 'piper' ? 'ok' : voice.backend === 'sapi' ? 'warn' : 'bad'}">{voice.backend === "server" ? "speech server" : voice.backend === "piper" ? "Piper neural" : voice.backend === "sapi" ? "Windows voice" : "no voice"}</span>
          {#if vsEngine === "server" && voice.backend !== "server"}<span class="pill warn" title="The configured speech server is not answering; EDDA fell back to the next voice. Retry the server or pick another engine.">server configured, not running — using {voice.backend === "piper" ? "Piper" : voice.backend === "sapi" ? "Windows voice" : "no voice"}</span>{/if}
        </div>
        {#if vsEngine === "server"}
          <div class="row" style="margin-top:0.4rem">
            <select bind:value={vsPreset} onchange={applySpeechPreset}>{#each SPEECH_PRESETS as p}<option value={p.id}>{p.label}</option>{/each}</select>
            {#if vsPreset !== "kokoro"}<input placeholder="address" bind:value={vsUrl} style="min-width:16rem" /><input placeholder="model" bind:value={vsModel} style="min-width:8rem" />{/if}
            {#if vsPreset !== "kokoro"}<input type="password" placeholder={vs?.config?.api_key ? "key set" : "API key (if any)"} bind:value={vsKey} style="min-width:10rem" autocomplete="off" />{/if}
            <button class="quiet" onclick={connectSpeech} disabled={vsBusy}>{vsBusy ? "Connecting…" : "Connect"}</button>
          </div>
          <div class="row" style="margin-top:0.4rem">
            <span class="muted small">Voice</span>
            {#if vsVoices.length}
              <select bind:value={vsVoice} onchange={saveVoiceServer}>{#each vsVoices as v}<option value={v}>{serverVoiceName(v)}</option>{/each}</select>
            {:else}
              <span class="muted small">press Connect to list the voices</span>
            {/if}
          </div>
        {/if}
        {#if vsMsg}<div class="muted small" style="margin-top:0.3rem">{vsMsg}</div>{/if}
      </dd>
      {#if engineChoice === "piper"}
      <dt>Voice</dt>
      <dd class="row">
        <select value={chosen} onchange={changeVoice} disabled={voice.backend !== "piper"}>
          <option value="">— pick —</option>
          {#each models as m}<option value={m}>{pretty(m)}</option>{/each}
        </select>
        <span class="muted small">{models.length} installed</span>
        {#if !models.length}<button class="quiet" onclick={installVoice} disabled={voiceInstalling}>{voiceInstalling ? "Downloading…" : "Install neural voice (~135 MB)"}</button>{:else}<button class="ghost" onclick={installVoice} disabled={voiceInstalling} title="The curated voice is installed; this re-downloads it.">{voiceInstalling ? "Downloading…" : "Reinstall"}</button>{/if}
        {#if chosen}<button class="ghost" onclick={removeChosenVoice} disabled={voice?.model&&chosen.startsWith(voice.model)} title={voice?.model&&chosen.startsWith(voice.model)?"Select another voice before removing this one":"Remove this downloaded model"}>Remove selected voice</button>{/if}
      </dd>
      {/if}
      <dt>Muted</dt>
      <dd><label><input type="checkbox" checked={voice.muted} onchange={async (e) => (voice.muted = await setMuted(e.target.checked))} /> mute all callouts</label></dd>
    </dl>
  {/if}
</section>

<section class="panel">
  <h2>Callouts <span class="sub">what the ship computer speaks up about</span></h2>
  <div class="row" style="flex-wrap:wrap; gap:0.4rem 1.2rem">
    {#each calloutKinds as k}<label><input type="checkbox" checked={k.on} onchange={(e) => toggleCallout(k.id, e.target.checked)} /> {k.label}</label>{/each}
  </div>
  <p class="muted small" style="margin:0.5rem 0 0">A switched-off kind is neither spoken nor shown. Instructions while following a route (burn fuel, re-planning) always come through.</p>
</section>

<section class="panel">
  <h2>Signal watch <span class="sub">announced when the game reports them on your sensors, and when you drop into one</span></h2>
  <div class="row" style="flex-wrap:wrap; gap:0.4rem 1.2rem">
    {#each signals as s}<label><input type="checkbox" checked={s.on} onchange={(e) => toggleSignal(s.id, e.target.checked)} /> {s.label}</label>{/each}
  </div>
  <p class="muted small" style="margin:0.5rem 0 0">Or say "I'm looking for high grade emissions" / "stop looking for…".</p>
</section>

<section class="panel">
  <h2>Voice input <span class="sub">activation word and/or push-to-talk</span></h2>
  <p class="muted small">Optional and not bundled. Choose one model below; EDDA downloads it only when you press Download.</p>
  {#if lst}
    <dl class="kv">
      <dt>Engine</dt>
      <dd>
        <select bind:value={modelChoice} onchange={saveListen}><option value="small">Small — orders (55 MB)</option><option value="parakeet">Parakeet — best free speech (500 MB)</option></select>
        {#if audio?.models_installed?.includes(modelDir(modelChoice))}<span class="pill ok">installed</span>{:else}<span class="pill warn">not installed</span> <button class="quiet" onclick={setupListen} disabled={lst.setup_running}>Download {modelChoice === "parakeet" ? "(~500 MB)" : "(55 MB)"}</button>{/if}
        {#if lSetup && lSetup.phase !== "done"}<div style="margin-top:.35rem">{#each Object.values(lSetupJobs) as job}<div class="muted small">{job.phase}: {job.status}{job.bytes ? ` · ${(job.bytes / 1048576).toFixed(0)}${job.total ? ` / ${(job.total / 1048576).toFixed(0)}` : ""} MB` : ""}</div>{/each}</div>{/if}
      </dd>
      <dt>State</dt>
      <dd><span class="pill {lst.phase === 'listening' ? 'accent' : lst.phase === 'thinking' ? 'cyan' : lst.phase === 'idle' ? 'ok' : ''}">{lst.phase}</span>
        {#if lHeard}<span class="muted small"> heard: “{lHeard}”</span>{/if}</dd>
      <dt>Microphone</dt><dd><select bind:value={lcfg.mic_device} onchange={saveListen}><option value={null}>system default</option>{#each audio?.inputs ?? [] as d}<option value={d}>{d}</option>{/each}</select></dd>
      <dt>Speech output</dt><dd><select bind:value={lcfg.output_device} onchange={saveListen}><option value={null}>system default</option>{#each audio?.outputs ?? [] as d}<option value={d}>{d}</option>{/each}</select></dd>
      <dt>Enabled</dt><dd><label><input type="checkbox" bind:checked={lcfg.enabled} onchange={saveListen} /> open the microphone and listen</label></dd>
      <dt>Activation word</dt><dd class="row"><input bind:value={lcfg.wake_word} onchange={saveListen} placeholder="hey edda (blank = off)" style="min-width:14rem" /><span class="muted small">then speak within {lcfg.window_secs} s</span></dd>
      <dt>Push-to-talk</dt>
      <dd>
        <div class="row">
          <select bind:value={pttKind} onchange={pttKindChanged}>
            <option value="none">none</option>
            <option value="keyboard">keyboard hotkey</option>
            <option value="joystick">joystick / vJoy button</option>
          </select>
          {#if pttKind === "keyboard"}
            <input bind:value={pttHotkey} onchange={saveListen} placeholder="press Capture, or type keys" style="min-width:12rem" title="Shift cannot be used" />
          {:else if pttKind === "joystick"}
            <select bind:value={pttJoyDevice} onchange={() => { pttJoyButton = 1; saveListen(); }}>
              {#each joys as j}<option value={j.id}>{j.name} ({j.buttons} buttons)</option>{/each}
              {#if !joys.length}<option value={-1}>no joysticks detected</option>{/if}
            </select>
            <select bind:value={pttJoyButton} onchange={saveListen}>
              {#each Array.from({ length: joys.find((j) => j.id === pttJoyDevice)?.buttons ?? 32 }, (_, i) => i + 1) as b}<option value={b}>button {b}</option>{/each}
            </select>
          {/if}
          <button class="quiet" onclick={capturePtt} disabled={capturing} title="Press the key or joystick button you want within 8 s">{capturing ? "press it now…" : "Capture"}</button>
          <button class="quiet" onmousedown={() => listenPtt(true)} onmouseup={() => listenPtt(false)} onmouseleave={() => listenPtt(false)} title="Hold to talk">Hold to talk</button>
        </div>
        {#if pttKind === "joystick" && pttJoyDevice >= 0}<div class="muted small">{joys.find((j) => j.id === pttJoyDevice)?.name ?? "?"} · button {pttJoyButton}</div>{/if}
      </dd>
    </dl>
    {#if lReply}<p class="small" style="margin:0.4rem 0 0"><span class="muted">reply:</span> {lReply}</p>{/if}
    {#if lMsg}<p class="muted small" style="margin:0.3rem 0 0">{lMsg}</p>{/if}
  {/if}
</section>

<style>
  .personas { display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: 0.5rem; }
  .pcard {
    text-align: left; background: var(--bg-2); color: var(--text); border: 1px solid var(--line);
    border-radius: 6px; padding: 0.55rem 0.7rem; font-weight: 400; cursor: pointer;
    display: flex; flex-direction: column; gap: 0.2rem;
  }
  .pcard:hover { filter: none; border-color: var(--accent-2); }
  .pcard.on { border-color: var(--accent); background: #ff8c1a14; }
  .pname { font-weight: 600; color: var(--accent-2); }
  .pblurb { font-size: 0.8rem; line-height: 1.3; }
</style>
