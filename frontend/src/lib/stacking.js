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
