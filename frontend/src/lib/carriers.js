/**
 * Which carriers belong on the Ships tab.
 *
 * The panel listed every carrier row in the store, and `CarrierJump` /
 * `CarrierLocation` create a row for any carrier the commander has
 * docked at or ridden. On a seven-year journal that is five nameless
 * strangers' carriers with locations a year old (maintainer, 2026-09-16:
 * "just their own carriers should be shown or their squadrons").
 *
 * The rows stay in the store — they cost nothing and other readers may
 * want them. This is a display rule.
 */
export function ownCarriers(carriers) {
  const kept = (carriers ?? []).filter(
    (c) => c?.owned || (c?.carrier_type === "SquadronCarrier" && (c.name || c.callsign)),
  );
  // A squadron's carrier is identified by the squadron's callsign; two ids
  // under one callsign (2026-10-24: the squadron replaced its carrier) are
  // one card, the most recently heard of. A carrier with only a location
  // heartbeat (no name, no callsign) says nothing a commander can use.
  const newest = new Map();
  for (const c of kept) {
    if (c.owned || c.carrier_type !== "SquadronCarrier" || !c.callsign) continue;
    const prev = newest.get(c.callsign);
    if (!prev || freshness(c) < freshness(prev)) newest.set(c.callsign, c);
  }
  return kept.filter((c) => c.owned || c.carrier_type !== "SquadronCarrier" || !c.callsign || newest.get(c.callsign) === c);
}

/** Hours since anything was heard of the carrier (smallest age). */
function freshness(c) {
  const ages = [c.location, c.capacity, c.tank_tritium_t].map((a) => a?.age_hours).filter((h) => h != null);
  return ages.length ? Math.min(...ages) : Infinity;
}

/** The ownership pill. A squadron carrier the commander bought is still the squadron's. */
export function carrierBadge(c) {
  if (c?.owned) return { text: c.carrier_type === "SquadronCarrier" ? "squadron · yours" : "yours", cls: "ok" };
  return { text: "squadron", cls: "" };
}

/** The scheduled-jump line; a departure in the past reads as past. */
export function jumpText(p) {
  const m = p?.minutes_to_departure;
  if (m == null) return `Jump scheduled to ${p.system}`;
  if (m < 0) return `Jump to ${p.system} · departed ${-m} min ago`;
  return `Jump scheduled to ${p.system} · departs in ${m} min`;
}
