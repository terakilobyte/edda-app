// Stacking mode (maintainer, 2026-09-16). At a mission board the question
// is not "what do I shoot next" but "whose missions do I already hold
// against this target" — a giver hidden behind "+9" is how a duplicate
// gets accepted, and a duplicate is a wasted mission because progress on
// two missions from the same giver is consecutive, not concurrent. So the
// HUD lists every giver, and these helpers word the list.

/** The chip text for one giver: the name, ×N when several, and how many are done. */
export function giverLabel(g) {
  let s = g.faction;
  if (g.missions > 1) s += ` ×${g.missions}`;
  if (g.ready > 0) s += g.ready === g.missions ? " ✓" : ` ${g.ready}✓`;
  return s;
}

/** The hover text: what the chip means in words. */
export function giverTitle(g) {
  const n = g.missions;
  const held = `${n} mission${n === 1 ? "" : "s"} from ${g.faction}`;
  const done = g.ready === 0 ? "" : g.ready === n ? ", all ready to turn in" : `, ${g.ready} ready to turn in`;
  const dup = g.duplicate ? " — duplicate: same giver, same target, so these progress one after another" : "";
  return held + done + dup;
}

/** The line under the target: how many givers, duplicates, and what is against another target. */
export function stackSummary(stack) {
  const n = stack.givers.length;
  const parts = [`${n} giver${n === 1 ? "" : "s"}`];
  const dups = stack.givers.filter((g) => g.duplicate).length;
  if (dups) parts.push(`${dups} duplicate${dups === 1 ? "" : "s"}`);
  if (stack.other_targets) parts.push(`+${stack.other_targets} against another target`);
  return parts.join(" · ");
}

/** Credits as the HUD shows them: 12.3M, 850k, 900. */
export function shortCr(n) {
  n = Number(n) || 0;
  if (Math.abs(n) >= 1e9) return `${(n / 1e9).toFixed(2)}B`;
  if (Math.abs(n) >= 1e6) return `${(n / 1e6).toFixed(1)}M`;
  if (Math.abs(n) >= 1e3) return `${Math.round(n / 1e3)}k`;
  return String(n);
}

/**
 * The stack's figures in one line, every one a stated field summed
 * (never an estimate): the kills that clear it and what is still to
 * make, what those kills are worth in mission credit, the stack's value
 * and what is ready to collect now. The idea of putting these next to
 * the givers is ODEliteTracker's (studied 2026-09-20; no code copied).
 */
export function stackEconomics(stack) {
  if (!stack || !stack.kills_needed) return "";
  const parts = [];
  const where = stack.target_system ? ` in ${stack.target_system}` : "";
  parts.push(stack.kills_remaining > 0 ? `${stack.kills_remaining} kills to go${where} (${stack.kills_needed} for the stack)` : `all ${stack.kills_needed} kills made${where}`);
  const ratio = stack.kills_needed ? (stack.kills_credited / stack.kills_needed).toFixed(1) : null;
  parts.push(`${stack.kills_credited} credited${ratio ? ` · ${ratio}× per kill` : ""}`);
  if (stack.value) {
    let v = `${shortCr(stack.value)} cr`;
    const bits = [];
    if (stack.value_ready) bits.push(`${shortCr(stack.value_ready)} ready to collect`);
    if (stack.value_shareable && stack.value_shareable !== stack.value) bits.push(`${shortCr(stack.value_shareable)} wing-shared`);
    else if (stack.value_shareable) bits.push("all wing-shared");
    if (bits.length) v += ` (${bits.join(", ")})`;
    parts.push(v);
  }
  return parts.join(" · ");
}

/** "3 missions ready to hand in here · 2.4M cr", for the dock. */
export function handInsLabel(here) {
  if (!here || !here.missions?.length) return "";
  const n = here.missions.length;
  const cr = here.credits ? ` · ${shortCr(here.credits)} cr` : "";
  return `${n} mission${n === 1 ? "" : "s"} ready to hand in here${cr}`;
}
