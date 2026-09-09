<script>
  // The what's-new splash and release-notes reader (maintainer, 2026-09-05).
  // Bundled notes arrive from the backend; `latestOnly` shows just the
  // newest section (the post-update splash), otherwise the full history.
  // Each section shows its one-paragraph summary with the full notes
  // behind an expander (maintainer, 2026-09-06) — the same fold the website
  // uses, from the same file.
  import { releaseNotesSeen } from "./api.js";
  import { parseNotes, runs } from "./notes.js";

  let { notes, latestOnly = false, onclose } = $props();

  const sections = $derived.by(() => {
    const all = parseNotes(notes?.markdown);
    return latestOnly ? all.slice(0, 1) : all;
  });

  // CLOSE FIRST, tell the backend after. This used to await
  // releaseNotesSeen() before calling onclose, and on 0.2.8 that call
  // hung rather than failed -- so onclose was never reached and the
  // splash became a permanent full-screen scrim over the whole app.
  // The maintainer lost plotting, both maps, market search, inventory, trade
  // and the settings radios in one go: every click was landing on this
  // element. `catch {}` did not save it, because a promise that never
  // settles never rejects either.
  //
  // The rule this encodes: nothing a modal needs in order to CLOSE may
  // depend on the backend answering. Losing the "seen" write costs one
  // extra splash next launch; losing the close costs the application.
  function dismiss() {
    onclose?.();
    releaseNotesSeen().catch((e) => console.error("could not record the notes as seen:", e));
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && dismiss()} />

<div class="scrim" role="presentation" onclick={dismiss}>
  <div class="sheet" role="dialog" tabindex="-1" aria-label="Release notes" onclick={(e) => e.stopPropagation()}>
    <header>
      <h2>{latestOnly ? `New in EDDA ${notes.version}` : "Release notes"}</h2>
      <button class="ghost" onclick={dismiss}>✕</button>
    </header>
    <div class="body">
      {#each sections as s}
        {#if !latestOnly}<h3>{s.version}</h3>{/if}
        <p class="summary">{#each runs(s.summary) as r}{#if r.bold}<strong>{r.text}</strong>{:else}{r.text}{/if}{/each}</p>
        {#if s.details.length}
          <details>
            <summary>Full notes</summary>
            {#each s.details as para}
              <p>{#each runs(para) as r}{#if r.bold}<strong>{r.text}</strong>{:else}{r.text}{/if}{/each}</p>
            {/each}
          </details>
        {/if}
      {/each}
    </div>
    <footer>
      <button onclick={dismiss}>{latestOnly ? "Fly on" : "Close"}</button>
    </footer>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; background: #000a; display: grid; place-items: center; z-index: 90; }
  .sheet { background: var(--panel, #14181f); border: 1px solid var(--line, #2a3340); border-radius: 8px;
           width: min(44rem, 92vw); max-height: 80vh; display: flex; flex-direction: column; }
  header { display: flex; justify-content: space-between; align-items: center; padding: 0.9rem 1.2rem 0; }
  header h2 { margin: 0; }
  .body { overflow-y: auto; padding: 0.4rem 1.2rem; }
  .body h3 { margin: 1rem 0 0.2rem; color: var(--accent, #ff8c1a); }
  .body p { margin: 0.5rem 0; line-height: 1.45; }
  .body p.summary { margin-top: 0.4rem; }
  details { margin: 0.2rem 0 0.8rem; }
  summary { cursor: pointer; color: var(--accent, #ff8c1a); font-size: 0.9rem; user-select: none; }
  summary::marker { color: var(--muted, #8b95a5); }
  details[open] summary { margin-bottom: 0.2rem; }
  details p { padding-left: 0.9rem; border-left: 2px solid var(--line, #2a3340); }
  footer { padding: 0.6rem 1.2rem 1rem; text-align: right; }
</style>
