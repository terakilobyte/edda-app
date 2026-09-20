// A whole build planned at once on the Build planner tab: one row per slot
// (or, without the slot table, per engineerable module), and the pure
// shaping between those rows, what is saved per ship, and what the backend
// is asked for. No Svelte here so it can be tested.

const TOP_GRADE = 5;
/** The swap that clears a slot (the backend's `ed_ships::EMPTY`). */
export const EMPTY = "empty";

/**
 * One plan row per slot when the slot table is given (every slot of the
 * hull, fitted or empty, so a module can be swapped in), else one per
 * engineerable module. A saved plan (by slot) wins over the defaults; the
 * defaults are the fitted blueprint continued to the top grade, included
 * when there is something left to do, and an unengineered module left out
 * until a blueprint is picked. A saved swap puts the new module in the row:
 * it starts at grade 0 with the new module's type.
 * @param {Array<{slot:string, slot_name:string, item?:string, item_name:string, module_type?:string|null, blueprint?:string|null, grade?:number|null}>} modules
 * @param {Record<string, {blueprint?:string, target_grade?:number, experimental?:string, include?:boolean, swap?:string|null}>} saved
 * @param {Array<{slot:string, slot_name:string, group:string, size:number, fitted?:string|null, fitted_name?:string|null, candidates:Array<{item:string, item_name:string, module_type?:string|null}>}>|null} slots
 */
export function planRows(modules, saved = {}, slots = null) {
  const bySlot = new Map((modules ?? []).map((m) => [m.slot.toLowerCase(), m]));
  const shape = (m, slotInfo) => {
    const slot = slotInfo?.slot ?? m.slot;
    const s = saved?.[slot] ?? {};
    const fittedGrade = m?.grade ?? 0;
    const fittedBlueprint = m?.blueprint ?? "";
    const emptied = s.swap === EMPTY && (slotInfo?.can_empty ?? true);
    const candidate = s.swap && !emptied ? findCandidate(slotInfo?.candidates ?? [], s.swap, s.preset ?? null) : null;
    const swap = emptied ? EMPTY : candidate ? candidate.item : null;
    const preset = candidate?.preset ?? null;
    // A pre-engineered module comes with its grade; a plain swap starts at 0.
    const from = swap ? (candidate?.preset_grade ?? 0) : fittedGrade;
    const blueprint = swap ? (candidate?.preset_blueprint ?? s.blueprint ?? "") : s.blueprint ?? fittedBlueprint;
    const module_type = emptied ? null : swap ? candidate.module_type ?? null : m?.module_type ?? null;
    return {
      slot,
      slot_name: slotInfo?.slot_name ?? m.slot_name,
      group: slotInfo?.group ?? null,
      size: slotInfo?.size ?? null,
      fitted_item: m?.item ?? slotInfo?.fitted ?? null,
      fitted_name: m?.item_name ?? slotInfo?.fitted_name ?? null,
      fitted_grade: fittedGrade,
      fitted_blueprint: fittedBlueprint,
      fitted_module_type: m?.module_type ?? null,
      swap,
      preset,
      item: emptied ? null : swap ?? m?.item ?? slotInfo?.fitted ?? null,
      item_name: emptied ? null : swap ? candidate.item_name : m?.item_name ?? slotInfo?.fitted_name ?? null,
      module_type,
      blueprint: module_type ? blueprint : "",
      from_grade: from,
      target_grade: Number(s.target_grade ?? TOP_GRADE),
      experimental: module_type ? s.experimental ?? "" : "",
      include: module_type ? s.include ?? (Boolean(blueprint) && from < TOP_GRADE) : false,
    };
  };
  if (slots) return slots.map((si) => shape(bySlot.get(si.slot.toLowerCase()) ?? null, si));
  return (modules ?? []).filter((m) => m.module_type).map((m) => shape(m, null));
}

/** The candidate with this item and (when given) this pre-engineered variant. */
export function findCandidate(candidates, item, preset = null) {
  const want = String(item).toLowerCase();
  return candidates.find((c) => c.item.toLowerCase() === want && (c.preset ?? null) === (preset ?? null)) ?? null;
}

/** The key a swap dropdown option carries: item, and the variant when there is one. */
export const swapKey = (item, preset = null) => (item ? `${item}|${preset ?? ""}` : "");

/**
 * A row with another module swapped into its slot (a candidate from the
 * slot table), emptied (`candidate.item === EMPTY`), or, with `candidate`
 * null or the fitted module itself, back to what is fitted. A swap starts
 * over: grade 0, no blueprint, not included until one is picked — except
 * a pre-engineered module, which comes at its grade with its blueprint,
 * so only an experimental is left to plan.
 */
export function withSwap(row, candidate) {
  if (!candidate || (row.fitted_item && !candidate.preset && candidate.item.toLowerCase() === row.fitted_item.toLowerCase())) {
    const blueprint = row.fitted_blueprint ?? "";
    const type = row.fitted_module_type ?? null;
    return { ...row, swap: null, preset: null, item: row.fitted_item, item_name: row.fitted_name, module_type: type, blueprint: type ? blueprint : "", from_grade: row.fitted_grade ?? 0, target_grade: TOP_GRADE, experimental: "", include: Boolean(type) && Boolean(blueprint) && (row.fitted_grade ?? 0) < TOP_GRADE };
  }
  if (candidate.item === EMPTY) {
    return { ...row, swap: EMPTY, preset: null, item: null, item_name: null, module_type: null, blueprint: "", from_grade: 0, target_grade: TOP_GRADE, experimental: "", include: false };
  }
  const from = candidate.preset_grade ?? 0;
  return { ...row, swap: candidate.item, preset: candidate.preset ?? null, item: candidate.item, item_name: candidate.item_name, module_type: candidate.module_type ?? null, blueprint: candidate.preset_blueprint ?? "", from_grade: from, target_grade: TOP_GRADE, experimental: "", include: false };
}

/** The swaps the rows carry, shaped for the backend. */
export function swapsFrom(rows) {
  return rows.filter((r) => r.swap).map((r) => (r.preset ? { slot: r.slot, item: r.swap, preset: r.preset } : { slot: r.slot, item: r.swap }));
}

/** Copy one row's choices onto every row of the same module type ("all 9 pulse lasers"). */
export function sameForAll(rows, row) {
  return rows.map((r) =>
    r.module_type && r.module_type === row.module_type
      ? { ...r, blueprint: row.blueprint, target_grade: row.target_grade, experimental: row.experimental, include: row.include && hasWork({ ...r, blueprint: row.blueprint, target_grade: row.target_grade, experimental: row.experimental }) }
      : r,
  );
}

/** How many rows share each module type, for the "same for all N" button. */
export function groupCounts(rows) {
  const counts = new Map();
  for (const r of rows) if (r.module_type) counts.set(r.module_type, (counts.get(r.module_type) ?? 0) + 1);
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
  return rows.filter((r) => r.module_type && isPlanned(r)).map((r) => ({
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
    .filter((r) => r.module_type && isPlanned(r) && rollsLeft(r))
    .map((r) => ({ slot: r.slot, module_type: r.module_type, blueprint: r.blueprint, grade: Number(r.target_grade) }));
}

/** What is saved per ship: the choices, by slot (a swap only when there is one). */
export function savedFrom(rows) {
  const out = {};
  for (const r of rows) {
    const s = { blueprint: r.blueprint, target_grade: Number(r.target_grade), experimental: r.experimental, include: r.include };
    if (r.swap) s.swap = r.swap;
    if (r.preset) s.preset = r.preset;
    out[r.slot] = s;
  }
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

const emptyRow = (slot, slot_name) => ({ slot, slot_name, group: null, size: null, fitted_item: null, fitted_name: null, fitted_grade: 0, fitted_blueprint: "", fitted_module_type: null, swap: null, preset: null, item: null, item_name: null, module_type: null, blueprint: "", from_grade: 0, target_grade: TOP_GRADE, experimental: "", include: false });

/**
 * An imported build laid over the rows: every module the build has where
 * the ship has something else becomes that row's swap; every engineered
 * module of the build becomes that slot's plan (a swapped module starts
 * from grade 0, the same blueprint continues from the fitted grade, a
 * module the ship already has is left unticked); a slot the rows do not
 * have gets one; and everything the build does not engineer is unticked.
 */
export function applyImport(rows, imported) {
  const bySlot = new Map((imported?.items ?? []).map((it) => [it.slot, it]));
  const swapBySlot = new Map((imported?.swaps ?? []).map((sw) => [sw.slot, sw]));
  const lay = (r, it, sw) => {
    const emptied = sw?.want_item === EMPTY;
    const swapped = emptied
      ? { swap: EMPTY, preset: null, item: null, item_name: null, module_type: null, from_grade: 0 }
      : sw ? { swap: sw.want_item, preset: sw.preset ?? null, item: sw.want_item, item_name: sw.want, module_type: it?.module_type ?? null, from_grade: 0 } : {};
    if (!it) return { ...r, ...swapped, blueprint: "", experimental: "", include: false };
    return {
      ...r,
      ...swapped,
      item_name: it.item_name,
      module_type: it.module_type,
      blueprint: it.blueprint ?? "",
      from_grade: it.from_grade,
      target_grade: it.target_grade,
      experimental: it.experimental ?? "",
      include: !it.done,
    };
  };
  const out = rows.map((r) => lay(r, bySlot.get(r.slot), swapBySlot.get(r.slot)));
  const have = new Set(rows.map((r) => r.slot));
  for (const slot of new Set([...bySlot.keys(), ...swapBySlot.keys()])) {
    if (have.has(slot)) continue;
    const it = bySlot.get(slot);
    const sw = swapBySlot.get(slot);
    out.push(lay(emptyRow(slot, it?.slot_name ?? sw?.slot_name ?? slot), it, sw));
  }
  return out;
}
