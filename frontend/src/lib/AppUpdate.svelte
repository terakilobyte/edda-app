<script>
  // EDDA's own updates, on the Ship computer page (maintainer, 2026-09-07: the
  // System data page goes with the API-only client; "check for updates"
  // moves here). Updates come signed from the community API; the check
  // runs quietly in the background and nothing installs without a say-so.
  import { onMount } from "svelte";
  import { appUpdateCheck, appUpdateInstall, appRestart, onAppUpdate, releaseNotesGet } from "./api.js";
  import WhatsNew from "./WhatsNew.svelte";
  let releaseNotes = $state(null);
  let appVersion = $state("");
  let appUpdate = $state(null); // {phase, version?, downloaded?, total?, restarts_itself?}
  let appMsg = $state("");
  let appBusy = $state(false);
  // A button that does nothing must say why (the 0.2.6 lesson): every
  // non-happy outcome lands in appMsg.
  async function openNotes() {
    try {
      const n = await releaseNotesGet();
      if (!n?.markdown?.trim()) {
        appMsg = `Release notes came back empty for ${n?.version ?? "this build"} - it shipped without them.`;
        return;
      }
      releaseNotes = n;
    } catch (e) {
      appMsg = `Could not open the release notes: ${e?.message ?? e}`;
    }
  }
  async function checkAppUpdate() {
    appBusy = true; appMsg = "";
    try {
      const r = await appUpdateCheck();
      appVersion = r.current;
      appMsg = r.available ? `Version ${r.available} is available.` : `EDDA ${r.current} is the latest version.`;
      if (r.available && appUpdate?.phase !== "ready") appUpdate = { phase: "available", version: r.available };
    } catch (e) { appMsg = String(e); }
    appBusy = false;
  }
  async function installAppUpdate() {
    appBusy = true; appMsg = "Downloading the update…";
    try { await appUpdateInstall(); appMsg = "Update installed. Restart EDDA to apply it."; }
    catch (e) { appMsg = String(e); appBusy = false; return; }
    appBusy = false;
  }
  onMount(() => {
    const off = onAppUpdate((e) => { appUpdate = e.payload; });
    return () => { off?.(); };
  });
</script>

<section class="panel">
  <h2>Application <span class="sub">EDDA itself{appVersion ? ` · version ${appVersion}` : ""}</span></h2>
  <p class="muted small">Updates come signed from the community API. EDDA checks quietly in the background; nothing downloads or installs without your say-so, and applying an update is just a restart.</p>
  <div class="row" style="margin-top:0.5rem">
    {#if appUpdate?.phase === "ready"}
      <button onclick={appRestart}>Restart to update</button>
    {:else if appUpdate?.phase === "available"}
      <button onclick={installAppUpdate} disabled={appBusy}>Install version {appUpdate.version}</button>
    {:else if appUpdate?.phase === "downloading"}
      <span class="small">Downloading… {appUpdate.total ? `${Math.round((appUpdate.downloaded / appUpdate.total) * 100)}%` : `${(appUpdate.downloaded / 1048576).toFixed(0)} MB`}</span>
      <!-- On Windows the updater exits this process and the installer
           relaunches it, so there is no "Restart to update" step and no
           process left to explain the disappearance. Warn first. -->
      {#if appUpdate.restarts_itself}<span class="small muted">EDDA will close and reopen itself to finish — that's expected, don't relaunch it.</span>{/if}
    {/if}
    <button class="quiet" onclick={checkAppUpdate} disabled={appBusy}>Check for updates</button>
    <button class="quiet" onclick={openNotes}>What's new</button>
  </div>
  {#if appMsg}<p class="muted small" style="margin:0.3rem 0 0">{appMsg}</p>{/if}
  {#if releaseNotes}<WhatsNew notes={releaseNotes} onclose={() => (releaseNotes = null)} />{/if}
</section>
