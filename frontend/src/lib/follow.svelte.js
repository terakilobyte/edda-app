// The route being followed in the game (persisted backend-side; the
// watcher advances it on every FSDJump and pushes `route-follow`). Small
// enough for the HUD to import on its own.
import { routeActivate, routeClear, routeFollowStatus, routeTargetNext, onRouteFollow } from "./api.js";
import { routing } from "./route.svelte.js";
import { FOLLOW_IDLE } from "./types.js";

/** @type {import("./types.js").FollowView & {error: string, targeting: boolean}} */
export const follow = $state({ ...FOLLOW_IDLE, error: "", targeting: false });

function applyFollow(v) {
  // Stopping a followed route no longer clears the plotted route from
  // the tab (maintainer, 2026-09-05: Stop was the only working way to clear
  // the game route, and it nuked the app's plan too). The Route tab
  // keeps what it plotted; clearing it is its own affordance.
  Object.assign(follow, v, { error: follow.error });
}

let off = null;

/** Load the follow state and subscribe to its updates. Idempotent. */
export function start() {
  if (off) return;
  off = onRouteFollow((e) => { applyFollow(e.payload); follow.error = ""; });
  routeFollowStatus().then(applyFollow).catch(() => {});
}

export function stop() {
  off?.();
  off = null;
}

export async function followRoute(route, source = "plot") {
  try { applyFollow(await routeActivate(route, source)); follow.error = ""; } catch (e) { follow.error = String(e); }
}
export async function stopFollowing() {
  try { await routeClear(); applyFollow({ ...FOLLOW_IDLE }); } catch (e) { follow.error = String(e); }
}
export async function targetNext() {
  follow.targeting = true; follow.error = "";
  try { await routeTargetNext(); } catch (e) { follow.error = String(e); } finally { follow.targeting = false; }
}

/** Follow the route the Route tab shows, labelled by where it came from. */
export function followShownRoute(route) {
  return followRoute(route, routing.origin === "ai" ? "ai" : "plot");
}
