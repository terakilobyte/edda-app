// What an EMPTY services search should say. An empty list has three
// different meanings, and one of them is not empty at all: the default
// search excludes fleet carriers, and some services live only on carriers
// in the community data — every redemption office in it, for one
// (2026-09-16, 15,875 of 15,875). A commander asking for one got a blank
// table with no hint that every match was something they had filtered out.
// Same shape as the trader list's "could not ask" fix, one layer over.

/**
 * @param {{ label: string, radius: number, carriersIncluded: boolean, onCarriers: number }} p
 *   `onCarriers`: how many the same search finds with carriers included
 *   (only consulted when they were excluded).
 * @returns {string}
 */
export function emptyServiceHint({ label, radius, carriersIncluded, onCarriers }) {
  const r = Number(radius) || 0;
  if (carriersIncluded) return `No ${label} within ${r} ly, on stations or fleet carriers.`;
  if (onCarriers > 0) {
    const n = onCarriers === 1 ? "1 is on a fleet carrier" : `${onCarriers} are on fleet carriers`;
    return `No ${label} at a station within ${r} ly, but ${n} — tick “carriers” to see them.`;
  }
  return `No ${label} within ${r} ly (fleet carriers excluded; none on those either).`;
}
