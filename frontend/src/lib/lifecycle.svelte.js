// Component lifecycle helpers shared by the panels.
//
// `journalResource(load)`: run `load` on mount and again on every
// `journal-changed`, expose the last error, unsubscribe on destroy. The
// `create…` form has no Svelte lifecycle so it can be driven by a test.
//
// `useListeners()`: collect unsubscribe functions (or promises of them)
// and run them all on destroy.
import { onMount, onDestroy, getContext } from "svelte";
import { onJournalChanged } from "./api.js";

/** Context key: TabHost provides `() => boolean`, whether the panel is showing. */
export const TAB_ACTIVE = Symbol("edda:tab-active");

/** The enclosing tab's visibility accessor, or "always" outside a TabHost (the HUD, tests). */
export function useTabActive() {
  return getContext(TAB_ACTIVE) ?? (() => true);
}

// `isActive` gates the journal-changed refresh: a hidden panel does not
// re-query on every kill, it remembers that it is stale and catches up
// once when shown (TabHost keeps panels mounted across tab switches).
export function createJournalResource(load, isActive = () => true) {
  let error = $state("");
  let stale = $state(false);
  let off = null;
  let started = false;
  async function refresh() {
    stale = false;
    try { await load(); error = ""; } catch (e) { error = String(e); }
  }
  function onChange() {
    if (isActive()) refresh(); else stale = true;
  }
  return {
    get error() { return error; },
    get stale() { return stale; },
    refresh,
    /** Refresh only if a change arrived while hidden. */
    catchUp() { if (stale) return refresh(); },
    start() {
      if (started) return;
      started = true;
      refresh().then(() => { if (started) off = onJournalChanged(onChange); });
    },
    stop() {
      started = false;
      off?.(); off = null;
    },
  };
}
export function journalResource(load) {
  const active = useTabActive();
  const res = createJournalResource(load, active);
  onMount(res.start);
  onDestroy(res.stop);
  // Shown again after changes arrived while hidden: one catch-up.
  $effect(() => { if (active()) res.catchUp(); });
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
