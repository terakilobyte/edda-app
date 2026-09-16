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
 * What a trader stop should SAY. Four different facts that used to render
 * as one sentence — an unreachable API and an empty galaxy read
 * identically to a commander, which is how a twenty-minute outage on
 * 2026-09-15 looked exactly like the station-economy gap we already knew
 * about.
 *
 * `asked: false` means the lookup never happened. That has two causes and
 * they are not the same: the API could not be reached, or there was
 * nothing to ask about because we do not know where the commander is.
 * Saying "could not reach the community API" for a missing position is
 * the same class of lie this function exists to stop (caught in review).
 *
 * @param {{asked?: boolean, kind_known?: boolean, nearest?: unknown[]}} stop
 * @param {string|null|undefined} originSystem the system the search ran from
 * @returns {{tone: "warn"|"muted"|"ok", text: string, title?: string}}
 */
export function traderStatus(stop, originSystem) {
  const found = (stop?.nearest ?? []).length;
  if (!originSystem) {
    return { tone: "muted", text: "position unknown — EDDA does not know which system you are in yet" };
  }
  if (stop?.asked === false) {
    return { tone: "warn", text: "could not reach the community API — this is not \u201cno traders nearby\u201d, it is \u201cwe could not ask\u201d" };
  }
  if (!found) return { tone: "muted", text: "none known within 300 ly" };
  if (stop?.kind_known === false) {
    return {
      tone: "muted",
      text: "(kind unknown \u2014 all traders shown)",
      title: "A trader's kind follows its station's economy, which the community API does not publish yet. These are every material trader in range.",
    };
  }
  return { tone: "ok", text: "" };
}
