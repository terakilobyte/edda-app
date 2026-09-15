// Engineering tab state, kept outside the component so it survives tab
// switches (the panel unmounts when another tab is shown).
export const eng = $state({
  moduleTypes: [],
  options: [],          // [{name, grades}]
  moduleType: "",
  blueprintName: "",
  experimentalName: "",
  fromGrade: 0,
  targetGrade: 5,
  minimumRolls: false,
  completeTarget: true,
  report: null,
  access: null,
  experimental: null,
  shopping: null,       // ShoppingReport | {error}
  picked: new Set(),    // selected trade indexes
  engineers: [],
  ship: null,
  openTab: false,     // App switches to the Engineering tab and clears it
  planRequest: null,  // a ship module handed over by the Ships tab
  planSlot: null,     // {slot, shipId} while the current plan is for a ship module
});

/// The Ships tab asks the Engineering tab to plan this module's next
/// grades; the panel consumes the request when it renders.
export function requestPlan(module) {
  eng.planRequest = module;
  eng.openTab = true;
}

/**
 * What a trader stop should SAY, which is three different facts that used
 * to render as one sentence. An unreachable API and an empty galaxy read
 * identically to a commander, and that is how a twenty-minute outage on
 * 2026-09-15 looked exactly like the station-economy gap we already knew
 * about. `asked: false` means the lookup never happened.
 * @param {{asked?: boolean, kind_known?: boolean, nearest?: unknown[]}} stop
 * @returns {{tone: "warn"|"muted"|"ok", text: string}}
 */
export function traderStatus(stop) {
  const found = (stop?.nearest ?? []).length;
  if (stop?.asked === false) {
    return { tone: "warn", text: "could not reach the community API — this is not “no traders nearby”, it is “we could not ask”" };
  }
  if (!found) return { tone: "muted", text: "none known within 300 ly" };
  if (stop?.kind_known === false) {
    return { tone: "ok", text: "kind unknown — every material trader in range is listed" };
  }
  return { tone: "ok", text: "" };
}
