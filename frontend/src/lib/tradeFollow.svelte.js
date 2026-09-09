// The trade route being followed (persisted backend-side; the watcher's
// Docked handler advances it and pushes `trade-follow`). The layer ABOVE
// the jump-route follower: one cyclic list of buy/sell stops.
import { tradeFollowStart, tradeFollowStop, tradeFollowStatus, onTradeFollow } from "./api.js";

export const TRADE_FOLLOW_IDLE = { active: false, kind: "", lap: 0, at: 0, phase: "travelling", stops: [], expected_profit_per_lap: 0 };

export const tradeFollow = $state({ ...TRADE_FOLLOW_IDLE, error: "" });

function apply(v) {
  Object.assign(tradeFollow, TRADE_FOLLOW_IDLE, v, { error: tradeFollow.error });
}

let off = null;

/** Load the trade-follow state and subscribe. Idempotent. */
export function start() {
  if (off) return;
  off = onTradeFollow((e) => { apply(e.payload); tradeFollow.error = ""; });
  tradeFollowStatus().then(apply).catch(() => {});
}

export function stop() {
  off?.();
  off = null;
}

/** Follow the clicked row: `legs` is exactly its legs ([leg], [out, back], or ring.legs). */
export async function followTrade(legs, kind) {
  try {
    apply(await tradeFollowStart(legs, kind));
    tradeFollow.error = "";
  } catch (e) {
    tradeFollow.error = String(e);
  }
}

export async function stopTrade() {
  try {
    apply(await tradeFollowStop());
  } catch (e) {
    tradeFollow.error = String(e);
  }
}
