// FSD injections on a plotted route, held against the materials aboard
// (the boss, 2026-10-09: "we could actually tell them if they have the
// materials or not"). Pure functions: the Route tab renders what these
// say, and the tests pin the words.
//
// `grades` is what the app's `injections_available` command returns:
//   [{ grade: "premium", mult: 2, can_make: 1, materials: [{ name, have }] }, ...]
// `route.injections` counts the hops that need one; each such hop carries
// `injection: "<grade>"`, and one route uses one grade throughout.

/** The grade a route's injected hops use, or null when it needs none. */
export function routeInjectionGrade(route) {
  if (!route || !(route.injections > 0)) return null;
  const hop = (route.hops ?? []).find((h) => h.injection);
  return hop?.injection ?? null;
}

/**
 * How the commander stands against a route's injections.
 * Returns null for a route that needs none; otherwise
 * { grade, needed, canMake, short, missing: [material names with none aboard] }.
 */
export function injectionShortfall(route, grades) {
  const grade = routeInjectionGrade(route);
  if (!grade) return null;
  const needed = route.injections;
  const g = (grades ?? []).find((x) => x.grade === grade);
  const canMake = g?.can_make ?? 0;
  const missing = (g?.materials ?? []).filter((m) => !(m.have > 0)).map((m) => m.name);
  return { grade, needed, canMake, short: Math.max(0, needed - canMake), missing };
}

/** The one-line banner the Route tab shows above an injected route. */
export function injectionBanner(route, grades) {
  const s = injectionShortfall(route, grades);
  if (!s) return null;
  const plural = s.needed === 1 ? "injection" : "injections";
  const head = `FSD ${plural} required: ${s.needed} × ${s.grade}`;
  if (s.canMake >= s.needed) return `${head} — you can make ${s.canMake}`;
  if (s.canMake === 0) {
    const why = s.missing.length ? ` (no ${s.missing.join(", ")} aboard)` : "";
    return `${head} — you cannot synthesise any${why}`;
  }
  return `${head} — you can make ${s.canMake}, short ${s.short}`;
}
