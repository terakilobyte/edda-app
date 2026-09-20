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

/**
 * Slot names folded into ranges: ["Large hardpoint 1", "Large hardpoint 2",
 * "Large hardpoint 4", "Utility 1"] → "Large hardpoints 1–2, 4; Utility 1".
 * One line for nine lasers instead of nine lines (maintainer, 2026-09-19:
 * "seems a bit spammy and repeated").
 */
export function compactSlots(names) {
  const groups = new Map();
  for (const n of names) {
    const m = /^(.*?)\s*(\d+)$/.exec(n);
    if (!m) { groups.set(n, null); continue; }
    if (!groups.has(m[1])) groups.set(m[1], []);
    groups.get(m[1]).push(Number(m[2]));
  }
  const parts = [];
  for (const [prefix, nums] of groups) {
    if (!nums) { parts.push(prefix); continue; }
    nums.sort((a, b) => a - b);
    const ranges = [];
    let start = nums[0], prev = nums[0];
    for (const n of nums.slice(1)) {
      if (n === prev + 1) { prev = n; continue; }
      ranges.push(start === prev ? `${start}` : `${start}–${prev}`);
      start = prev = n;
    }
    ranges.push(start === prev ? `${start}` : `${start}–${prev}`);
    // "Large hardpoints 1–4" reads right; "Utilitys" does not: only the word that takes an s gets one.
    const plural = nums.length > 1 && /hardpoint$/i.test(prefix) ? `${prefix}s` : prefix;
    parts.push(`${plural} ${ranges.join(", ")}`);
  }
  return parts.join("; ");
}

const jobKey = (it) => `${it.blueprint} G${it.target_grade}`;

/** The itinerary, one line per engineer, each job grouped by blueprint and grade. */
export function itinerary(report) {
  return (report?.engineers ?? []).map((stop) => {
    const mine = report.items.filter((it) => it.assigned_to === stop.engineer);
    const groups = new Map();
    for (const it of mine) {
      const k = jobKey(it);
      if (!groups.has(k)) groups.set(k, { what: k, module_type: it.module_type, slots: [] });
      groups.get(k).slots.push(it.slot_name);
    }
    const jobs = [...groups.values()]
      .map((g) => ({ what: g.what, module_type: g.module_type, count: g.slots.length, slots: compactSlots(g.slots) }))
      .sort((a, b) => b.count - a.count || a.what.localeCompare(b.what));
    return { engineer: stop.engineer, rank: stop.rank, jobs };
  });
}

/**
 * What no unlocked engineer can do at the asked grade, grouped, with the
 * way out: who takes it part-way today, and who to unlock for the rest.
 */
export function blocked(report) {
  const groups = new Map();
  for (const it of report?.items ?? []) {
    if (!it.blueprint || it.reachable) continue;
    const k = `${it.module_type}|${jobKey(it)}`;
    if (!groups.has(k)) {
      groups.set(k, {
        what: jobKey(it),
        module_type: it.module_type,
        target_grade: it.target_grade,
        max_reachable_grade: it.max_reachable_grade ?? null,
        today: it.engineers.filter((e) => e.unlocked && it.max_reachable_grade && e.max_grade >= it.max_reachable_grade).map((e) => e.engineer),
        unlock: it.engineers.filter((e) => !e.unlocked && e.max_grade >= it.target_grade).map((e) => ({ engineer: e.engineer, status: e.status })),
        slots: [],
      });
    }
    groups.get(k).slots.push(it.slot_name);
  }
  return [...groups.values()].map((g) => ({ ...g, count: g.slots.length, slots: compactSlots(g.slots) }));
}

/**
 * An imported build laid over the rows: every engineered module of the
 * build becomes that slot's plan (a swapped module starts from grade 0,
 * the same blueprint continues from the fitted grade, a module the ship
 * already has is left unticked), a slot the ship has empty gets a row,
 * and everything the build does not engineer is unticked.
 */
export function applyImport(rows, imported) {
  const bySlot = new Map((imported?.items ?? []).map((it) => [it.slot, it]));
  const out = rows.map((r) => {
    const it = bySlot.get(r.slot);
    if (!it) return { ...r, include: false };
    return {
      ...r,
      item_name: it.item_name,
      module_type: it.module_type,
      blueprint: it.blueprint ?? "",
      from_grade: it.from_grade,
      target_grade: it.target_grade,
      experimental: it.experimental ?? "",
      include: !it.done,
    };
  });
  const have = new Set(rows.map((r) => r.slot));
  for (const it of imported?.items ?? []) {
    if (have.has(it.slot)) continue;
    out.push({
      slot: it.slot,
      slot_name: it.slot_name,
      item_name: it.item_name,
      module_type: it.module_type,
      blueprint: it.blueprint ?? "",
      from_grade: it.from_grade,
      target_grade: it.target_grade,
      experimental: it.experimental ?? "",
      include: !it.done,
    });
  }
  return out;
}
