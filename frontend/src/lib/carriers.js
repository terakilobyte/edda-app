/**
 * Which carriers belong on the Ships tab, and how to say what is aboard.
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

/**
 * The hold, readable: biggest first, a display name rather than a raw
 * symbol, and a cap with the remainder counted. Truncating is safe here
 * — unlike the stacking board, nothing is lost by not seeing entry 40,
 * and 48 run together in one sentence is what made it unreadable.
 * @param {{commodity: string, name?: string, tons: number}[]} lines
 */
export function holdSummary(lines, limit = 8) {
  const sorted = [...(lines ?? [])].sort((a, b) => (b?.tons ?? 0) - (a?.tons ?? 0));
  const shown = sorted.slice(0, limit).map((h) => ({
    label: h?.name || h?.commodity || "unknown",
    tons: h?.tons ?? 0,
  }));
  const rest = sorted.slice(limit);
  return {
    shown,
    more: rest.length,
    moreTons: rest.reduce((n, h) => n + (h?.tons ?? 0), 0),
  };
}
