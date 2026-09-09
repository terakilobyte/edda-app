// Component lifecycle helpers shared by the panels.
//
// `journalResource(load)`: run `load` on mount and again on every
// `journal-changed`, expose the last error, unsubscribe on destroy. The
// `create…` form has no Svelte lifecycle so it can be driven by a test.
//
// `useListeners()`: collect unsubscribe functions (or promises of them)
// and run them all on destroy.
import { onMount, onDestroy } from "svelte";
import { onJournalChanged } from "./api.js";

export function createJournalResource(load) {
  let error = $state("");
  let off = null;
  let started = false;
  async function refresh() {
    try { await load(); error = ""; } catch (e) { error = String(e); }
  }
  return {
    get error() { return error; },
    refresh,
    start() {
      if (started) return;
      started = true;
      refresh().then(() => { if (started) off = onJournalChanged(refresh); });
    },
    stop() {
      started = false;
      off?.(); off = null;
    },
  };
}

export function journalResource(load) {
  const res = createJournalResource(load);
  onMount(res.start);
  onDestroy(res.stop);
  return res;
}

export function listenerSet() {
  const items = [];
  return {
    add(u) { if (u) items.push(u); return u; },
    dispose() {
      for (const u of items.splice(0)) {
        if (typeof u === "function") u();
        else u?.then?.((f) => f?.());
      }
    },
  };
}

export function useListeners() {
  const set = listenerSet();
  onDestroy(set.dispose);
  return set;
}
