// A whole build planned at once on the Ships tab: one row per engineerable
// module, and the pure shaping between those rows, what is saved per ship,
// and what the backend is asked for. No Svelte here so it can be tested.

const TOP_GRADE = 5;

/**
 * One plan row per engineerable module. A saved plan (by slot) wins over
 * the defaults; the defaults are the fitted blueprint continued to the top
 * grade, included when there is something left to do, and an unengineered
 * module left out until a blueprint is picked.
 * @param {Array<{slot:string, slot_name:string, item_name:string, module_type?:string|null, blueprint?:string|null, grade?:number|null}>} modules
 * @param {Record<string, {blueprint?:string, target_grade?:number, experimental?:string, include?:boolean}>} saved
 */
export function planRows(modules, saved = {}) {
  return (modules ?? [])
    .filter((m) => m.module_type)
    .map((m) => {
      const s = saved?.[m.slot] ?? {};
      const from = m.grade ?? 0;
      const blueprint = s.blueprint ?? m.blueprint ?? "";
      return {
        slot: m.slot,
        slot_name: m.slot_name,
        item_name: m.item_name,
        module_type: m.module_type,
        blueprint,
        from_grade: from,
        target_grade: Number(s.target_grade ?? TOP_GRADE),
        experimental: s.experimental ?? "",
        include: s.include ?? (Boolean(blueprint) && from < TOP_GRADE),
      };
    });
}

/** Copy one row's choices onto every row of the same module type ("all 9 pulse lasers"). */
export function sameForAll(rows, row) {
  return rows.map((r) =>
    r.module_type === row.module_type
      ? { ...r, blueprint: row.blueprint, target_grade: row.target_grade, experimental: row.experimental, include: row.include && hasWork({ ...r, blueprint: row.blueprint, target_grade: row.target_grade, experimental: row.experimental }) }
      : r,
  );
}

/** How many rows share each module type, for the "same for all N" button. */
export function groupCounts(rows) {
  const counts = new Map();
  for (const r of rows) counts.set(r.module_type, (counts.get(r.module_type) ?? 0) + 1);
  return counts;
}

/** The blueprint still has grades to roll (a module at the top grade has none). */
export function rollsLeft(row) {
  return Boolean(row.blueprint) && Number(row.target_grade) > row.from_grade;
}

/** Something to do: grades to roll, or an experimental to apply (a top-grade module can still take one). */
export function hasWork(row) {
  return rollsLeft(row) || Boolean(row.experimental);
}

/** A row is planned when it is included and has work. */
export function isPlanned(row) {
  return Boolean(row.include) && hasWork(row);
}

/** What the backend is asked for: the planned rows, shaped as PlanItem. */
export function planRequest(rows) {
  return rows.filter(isPlanned).map((r) => ({
    slot: r.slot,
    module_type: r.module_type,
    // A top-grade module with only an experimental planned sends no blueprint: nothing to roll.
    blueprint: rollsLeft(r) ? r.blueprint : null,
    from_grade: r.from_grade,
    target_grade: Number(r.target_grade),
    experimental: r.experimental || null,
  }));
}

/** The planned blueprints at their target grade, for the SLEF export. */
export function proposedFor(rows) {
  return rows
    .filter((r) => isPlanned(r) && rollsLeft(r))
    .map((r) => ({ slot: r.slot, module_type: r.module_type, blueprint: r.blueprint, grade: Number(r.target_grade) }));
}

/** What is saved per ship: the choices, by slot. */
export function savedFrom(rows) {
  const out = {};
  for (const r of rows) out[r.slot] = { blueprint: r.blueprint, target_grade: Number(r.target_grade), experimental: r.experimental, include: r.include };
  return out;
}
