<script>
  // Everything the ship computer has said or noted, newest first. Same
  // stream the overlay shows, kept longer here.
  import { onMount } from "svelte";
  import { recentCallouts, onCallout, voiceStatus, setMuted } from "./api.js";
  import { fmtTs } from "./format.js";
  import { prioClass } from "./ui.js";
  import { useListeners } from "./lifecycle.svelte.js";

  let items = $state([]);
  let voice = $state(null);
  const listeners = useListeners();

  onMount(async () => {
    try {
      items = (await recentCallouts()).reverse();
    } catch {}
    try {
      voice = await voiceStatus();
    } catch {}
    listeners.add(onCallout((e) => {
      items = [e.payload, ...items].slice(0, 100);
    }));
  });

  async function toggleMute() {
    if (!voice) return;
    voice.muted = await setMuted(!voice.muted);
  }
</script>

<section class="panel feed">
  <h2>
    Callouts
    {#if voice}
      <button class="quiet tiny" onclick={toggleMute}>{voice.muted ? "Unmute" : "Mute"}</button>
    {/if}
  </h2>
  {#if items.length === 0}
    <p class="muted">Nothing yet. Fly somewhere.</p>
  {:else}
    <ul>
      {#each items as c, i (c.ts + c.text + i)}
        <li class={prioClass(c.priority)}>
          <span class="meta">{fmtTs(c.ts).slice(11, 16)} · {c.kind}{c.speak ? " 🔈" : ""}</span>
          <span>{c.text}</span>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .feed { flex: 1; min-height: 160px; display: flex; flex-direction: column; }
  ul { list-style: none; margin: 0; padding: 0; overflow-y: auto; flex: 1; display: flex; flex-direction: column; gap: 0.35rem; }
  li { font-size: 0.83rem; line-height: 1.3; }
  li .meta { display: block; color: var(--muted); font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.04em; }
  li.accent { color: var(--accent-2); }
  li.warn { color: var(--warn); }
  li.bad { color: var(--bad); }
  .tiny { margin-left: auto; padding: 0.05rem 0.45rem; font-size: 0.7rem; }
</style>
