// The HUD's layout: which sections show, in what order, and which are
// compact. Pure data and shaping so it can be tested; Settings → HUD
// writes it, the overlay window reads it, localStorage is the bus (the
// same way opacity and scale travel). A layout can be remembered per ship.

/** Every section the HUD can show, in the default order. */
export const SECTIONS = [
  { id: "location", label: "System and dock" },
  { id: "powerplay", label: "Powerplay control" },
  { id: "next", label: "Next system" },
  { id: "route", label: "Route, or the route being followed" },
  { id: "gauges", label: "Fuel and cargo" },
  { id: "trade", label: "Trade route or pinned loop" },
  { id: "missions", label: "Missions, or the stacking board" },
  { id: "callouts", label: "Callouts and voice" },
];

const IDS = SECTIONS.map((s) => s.id);

export const DEFAULT_LAYOUT = Object.freeze({ order: [...IDS], hidden: [], compact: [] });

/** Three ready layouts; "default" restores the shipped one. */
export const PRESETS = {
  default: { order: [...IDS], hidden: [], compact: [] },
  combat: { order: ["location", "missions", "gauges", "callouts", "next", "route", "powerplay", "trade"], hidden: ["trade", "powerplay"], compact: ["next", "route"] },
  trade: { order: ["location", "trade", "gauges", "route", "next", "callouts", "missions", "powerplay"], hidden: ["missions"], compact: ["powerplay"] },
  explore: { order: ["location", "next", "route", "gauges", "callouts", "powerplay", "missions", "trade"], hidden: ["trade", "missions", "powerplay"], compact: [] },
};

/**
 * A layout as read from storage, made whole: every section exactly once
 * in `order` (unknown ids dropped, missing ones appended in default order),
 * `hidden` and `compact` as subsets of the known ids. Anything unreadable
 * is the default.
 */
export function normalise(layout) {
  if (!layout || typeof layout !== "object") return { ...DEFAULT_LAYOUT, order: [...IDS] };
  const seen = new Set();
  const order = [];
  for (const id of Array.isArray(layout.order) ? layout.order : []) {
    if (IDS.includes(id) && !seen.has(id)) { seen.add(id); order.push(id); }
  }
  for (const id of IDS) if (!seen.has(id)) order.push(id);
  const subset = (xs) => (Array.isArray(xs) ? xs.filter((id, i) => IDS.includes(id) && xs.indexOf(id) === i) : []);
  return { order, hidden: subset(layout.hidden), compact: subset(layout.compact) };
}

/** Move a section up (delta -1) or down (+1) in the order. */
export function move(layout, id, delta) {
  const l = normalise(layout);
  const i = l.order.indexOf(id);
  const j = i + delta;
  if (i < 0 || j < 0 || j >= l.order.length) return l;
  const order = [...l.order];
  [order[i], order[j]] = [order[j], order[i]];
  return { ...l, order };
}

function toggle(list, id) {
  return list.includes(id) ? list.filter((x) => x !== id) : [...list, id];
}

export function toggleHidden(layout, id) {
  const l = normalise(layout);
  return { ...l, hidden: toggle(l.hidden, id) };
}

export function toggleCompact(layout, id) {
  const l = normalise(layout);
  return { ...l, compact: toggle(l.compact, id) };
}

export function preset(name) {
  return normalise(PRESETS[name] ?? PRESETS.default);
}

/**
 * The layout the HUD applies: the one remembered for the current ship if
 * there is one, else the global one.
 * @param {object} global the global layout
 * @param {Record<string, object>} byShip layouts by ship id
 * @param {number|string|null} shipId the ship being flown
 */
export function resolve(global, byShip, shipId) {
  const own = shipId != null && byShip && byShip[String(shipId)];
  return normalise(own || global);
}

export function isHidden(layout, id) {
  return normalise(layout).hidden.includes(id);
}

export function isCompact(layout, id) {
  return normalise(layout).compact.includes(id);
}
