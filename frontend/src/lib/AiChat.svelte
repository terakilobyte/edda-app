<script>
  // The ship computer. Decision 6: the model is the interface -- it reads
  // the request and composes the same tools the panels use. Replies can be
  // spoken through the ship's voice.
  import { aiAsk, aiReset, say } from "./api.js";
  async function newChat() { messages = []; try { await aiReset(); } catch {} }
  import { openUrl } from "@tauri-apps/plugin-opener";
  import { showFromAi } from "./route.svelte.js";
  import { trade } from "./trade.svelte.js";
  import { pickView } from "./tradeView.js";
  import { KEYS, persisted } from "./storage.svelte.js";

  let question = $state("");
  let messages = $state([]); // {role, text, citations?: [{url, title}]}

  // Sources open in the default browser, never inside the app's webview.
  function openSource(e, url) {
    e.preventDefault();
    openUrl(url).catch(() => {});
  }

  const host = (url) => {
    try { return new URL(url).hostname.replace(/^www\./, ""); } catch { return url; }
  };
  let sending = $state(false);
  const speak = persisted(KEYS.speak, false);
  let log;

  const suggestions = [
    "What should I fill up with here, and where do I sell it?",
    "Best round trip within 30 ly of me for my ship?",
    "How did my last combat session go?",
    "Can I get grade 5 Overcharged on my power plant, and what am I short?",
    "Nearest outfitting with a large pad that isn't a carrier?",
    "Which Powerplay systems have I seen in a Fortified state?",
  ];

  async function send(text) {
    const q = (text ?? question).trim();
    if (!q || sending) return;
    messages.push({ role: "you", text: q });
    question = "";
    sending = true;
    try {
      const answer = await aiAsk(q);
      // Anything the ship computer plotted or searched shows up in its tab too.
      const updated = [];
      if (answer.route) { showFromAi(answer.route); updated.push("Route"); }
      if (answer.profit) { trade.report = answer.profit; trade.view = pickView(answer.profit); trade.error = ""; updated.push("Trade"); }
      messages.push({ role: "ship", text: answer.text, citations: answer.citations ?? [], tools: answer.tools_used ?? [], updated });
      if (speak.value) say(answer.text.split(/\n/).filter(Boolean).slice(0, 3).join(" ")).catch(() => {});
    } catch (e) {
      messages.push({ role: "ship", text: `[error] ${e}`, error: true });
    } finally {
      sending = false;
      queueMicrotask(() => log?.scrollTo({ top: log.scrollHeight, behavior: "smooth" }));
    }
  }

  function onKeydown(e) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  }

  function toggleSpeak(e) {
    speak.value = e.target.checked;
  }
</script>

<section class="panel chat">
  <h2>Ship computer <button class="quiet tiny" onclick={newChat} title="Forget this conversation">New chat</button> <label class="spk"><input type="checkbox" checked={speak.value} onchange={toggleSpeak} /> speak replies</label></h2>

  <div class="log" bind:this={log}>
    {#if messages.length === 0}
      <p class="muted small">Ask in plain language. It reads your live journal and the galaxy database, and says where each fact came from.</p>
      <div class="sugg">
        {#each suggestions as s}
          <button class="quiet" onclick={() => send(s)}>{s}</button>
        {/each}
      </div>
    {/if}
    {#each messages as m}
      <div class="msg {m.role} {m.error ? 'error' : ''}">
        <span class="who">{m.role === "you" ? "You" : "Ship"}</span>
        <div class="text">{m.text}</div>
        {#if m.tools?.length}<div class="muted small tools">used: {[...new Set(m.tools)].join(", ")}{#if m.updated?.length} · {m.updated.join(" and ")} tab updated{/if}</div>{/if}
        {#if m.citations?.length}
          <div class="sources">
            <span class="src-label">Sources</span>
            {#each m.citations as c, i}
              <a href={c.url} title={c.url} onclick={(e) => openSource(e, c.url)}>{i + 1}. {c.title} <span class="host">{host(c.url)}</span></a>
            {/each}
          </div>
        {/if}
      </div>
    {/each}
    {#if sending}<div class="msg ship"><span class="who">Ship</span><div class="text muted">working…</div></div>{/if}
  </div>

  <div class="row" style="flex-wrap:nowrap">
    <textarea bind:value={question} onkeydown={onKeydown} placeholder="e.g. what's the best trade from here?" rows="2"></textarea>
    <button onclick={() => send()} disabled={sending}>Send</button>
  </div>
</section>

<style>
  .chat { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .tiny { padding: 0 0.45rem; font-size: 0.72rem; font-weight: 400; text-transform: none; letter-spacing: 0; }
  .spk { margin-left: auto; font-weight: 400; text-transform: none; letter-spacing: 0; color: var(--muted); display: inline-flex; gap: 0.3rem; align-items: center; }
  .log { flex: 1; overflow-y: auto; background: var(--bg-2); border: 1px solid var(--line); border-radius: 5px; padding: 0.6rem; margin-bottom: 0.5rem; min-height: 180px; }
  .sugg { display: flex; flex-direction: column; gap: 0.3rem; }
  .sugg button { text-align: left; font-size: 0.8rem; }
  .msg { margin-bottom: 0.6rem; line-height: 1.4; }
  .who { display: block; font-size: 0.7rem; text-transform: uppercase; letter-spacing: 0.05em; color: var(--muted); }
  .msg.you .who { color: var(--accent); }
  .msg.ship .who { color: var(--cyan); }
  .text { white-space: pre-wrap; font-size: 0.88rem; }
  .tools { margin-top: 0.2rem; font-size: 0.72rem; }
  .sources { display: flex; flex-wrap: wrap; gap: 0.25rem 0.7rem; margin-top: 0.3rem; font-size: 0.75rem; }
  .src-label { text-transform: uppercase; letter-spacing: 0.05em; color: var(--muted); }
  .sources a { color: var(--cyan); text-decoration: none; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
  .sources a:hover { text-decoration: underline; }
  .sources .host { color: var(--muted); }
  textarea { flex: 1; resize: vertical; }
</style>
