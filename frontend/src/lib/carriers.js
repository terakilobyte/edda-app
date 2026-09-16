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
  return (carriers ?? []).filter((c) => c?.owned || c?.carrier_type === "SquadronCarrier");
}
