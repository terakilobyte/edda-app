// The ONE pipe for "the ship the commander is flying" (maintainer ruling
// 2026-09-05: the ship computer knew about a swap while the Route
// dropdown and Ships tab slept on mount-time snapshots). Sourced from
// ships_list — the only command carrying ship_id + current — and
// refreshed on every journal-changed, which the watcher emits within a
// poll of the Loadout a swap writes. Panels read this store and never
// fetch ship identity themselves.
import { shipsList, onJournalChanged } from "./api.js";

export const ship = $state({
  /** @type {Array<{ship_id: number, name: string|null, ident: string|null, ship: string, current: boolean}>} */
  list: [],
  /** @type {number|null} */
  currentId: null,
  /** @type {object|null} the list entry being flown */
  current: null,
});

async function refresh() {
  try {
    const list = await shipsList();
    const current = list.find((s) => s.current) ?? null;
    ship.list = list;
    ship.current = current;
    ship.currentId = current?.ship_id ?? null;
  } catch {}
}

let off = null;

/** Load and subscribe. Idempotent; started once in App.svelte. */
export function start() {
  if (off) return;
  off = onJournalChanged(refresh);
  refresh();
}

export function stop() {
  off?.();
  off = null;
}
