// The Build planner tab's hand-off: the Ships tab (or anything else) asks
// for a ship to be planned; App switches tabs; the planner selects it.
export const planner = $state({
  openTab: false,   // App switches to the Build planner tab and clears it
  shipId: null,     // the ship to select on arrival (null = the one being flown)
});

/** Open the Build planner on this ship. */
export function requestPlanner(shipId = null) {
  planner.shipId = shipId;
  planner.openTab = true;
}
