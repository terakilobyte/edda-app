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
