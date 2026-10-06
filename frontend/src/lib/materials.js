// Material trader arithmetic, the game's own rates, measured against every
// trade in the maintainer's journal (docs/benches/2026-10-04-material-trade-ratios.csv).
// Mirrors ed_journal::mat_trade::ratio; the Rust side is the source of
// truth and this copy is pinned to the same nine journal trades.

/** What you pay of `a` to get `b`: [give, receive], or null when the
 *  trader will not (different trader type, same material, untradeable). */
export function ratio(a, b) {
  if (!a || !b || a.symbol === b.symbol || a.kind !== b.kind || !a.group || !b.group) return null;
  const d = a.grade - b.grade;
  if (a.group === b.group) return d === 0 ? null : d > 0 ? [1, 3 ** d] : [6 ** -d, 1];
  return d === 0 ? [6, 1] : d > 0 ? [2, 3 ** (d - 1)] : [6 ** (-d + 1), 1];
}

/** The game's storage cap by grade. */
export const cap = (grade) => ({ 1: 300, 2: 250, 3: 200, 4: 150 })[grade] ?? 100;
