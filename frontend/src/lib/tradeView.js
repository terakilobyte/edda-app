// Which tab a fresh profit report opens on (maintainer, 2026-09-07): if the
// best round trip out-earns the best single leg, show the loops first
// instead of always leading with legs. Legs are ranked by the
// repeating rate the panel headlines; loops and rings by their own
// cr/h. Ties go to legs — the simpler thing to read.

/** @param {any} report @returns {"legs"|"trips"|"rings"} */
export function pickView(report) {
  const legRate = report?.legs?.[0]?.profit_per_hour_repeat ?? 0;
  const tripRate = report?.round_trips?.[0]?.profit_per_hour ?? 0;
  const ringRate = report?.rings?.[0]?.profit_per_hour ?? 0;
  if (ringRate > legRate && ringRate > tripRate) return "rings";
  if (tripRate > legRate) return "trips";
  return "legs";
}

/**
 * One phrase for the server's docked-board verdict (ProfitReport.board,
 * f0f750e): which board a from-station search actually priced against.
 * Null when the search sent none.
 * @param {{used: boolean, reason: string, rows: number}|null|undefined} board
 */
export function boardLine(board) {
  if (!board) return null;
  if (board.used) return `your board from the dock (${board.rows} rows, newer than the fleet's)`;
  switch (board.reason) {
    case "older": return "the fleet's board (yours from the dock was older)";
    case "absent": return `your board from the dock (${board.rows} rows; the fleet had none)`;
    case "mismatch": return "the fleet's board (your Market.json is from another station)";
    case "unknown_station": return "the fleet's board (that station is unknown to the API)";
    default: return `the fleet's board (${board.reason})`;
  }
}
