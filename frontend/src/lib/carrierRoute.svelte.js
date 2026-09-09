// The carrier route (Item 52 C, maintainer's item 6): plotted once, followed
// across sessions by the backend, which advances it on the journal's
// carrier events and pushes the view here. Long-lived by design: a
// carrier moves at one jump per twenty-plus minutes.
import { carrierRoutePlot, carrierRouteStart, carrierRouteStatus, carrierRouteClear, carrierRouteNext, onCarrierRoute } from "./api.js";

export const CARRIER_IDLE = { active: false, remaining: 0, jumps: 0, next: 0, hops: [] };

export const carrierRoute = $state({
  follow: { ...CARRIER_IDLE },
  plan: null,
  to: "",
  from: "",
  loading: false,
  error: "",
  copied: "",
});

let off = null;
export function start() {
  if (off) return;
  off = onCarrierRoute((e) => { carrierRoute.follow = e.payload ?? { ...CARRIER_IDLE }; });
  carrierRouteStatus().then((v) => { carrierRoute.follow = v ?? { ...CARRIER_IDLE }; }).catch(() => {});
}
export function stop() {
  if (off) { off(); off = null; }
}

export async function plotCarrier() {
  if (carrierRoute.loading || !carrierRoute.to.trim()) return;
  carrierRoute.loading = true; carrierRoute.error = ""; carrierRoute.plan = null;
  try {
    carrierRoute.plan = await carrierRoutePlot(carrierRoute.to.trim(), carrierRoute.from.trim() || null);
  } catch (e) {
    carrierRoute.error = String(e);
  } finally {
    carrierRoute.loading = false;
  }
}

export async function followCarrier() {
  if (!carrierRoute.plan) return;
  try { carrierRoute.follow = await carrierRouteStart(carrierRoute.plan); } catch (e) { carrierRoute.error = String(e); }
}

export async function nextCarrierJump() {
  try { carrierRoute.copied = await carrierRouteNext(); } catch (e) { carrierRoute.error = String(e); }
}

export async function clearCarrier() {
  try { carrierRoute.follow = await carrierRouteClear(); carrierRoute.copied = ""; } catch (e) { carrierRoute.error = String(e); }
}
