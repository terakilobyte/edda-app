// How the Galaxy tab reconciles the three sources that may know a system.
// The rule: the journal (what the commander has seen) wins; the local star
// index supplies coordinates and the primary star; EDSM fills in the rest
// and is marked as external so the panel can say where the data came from.

/**
 * @param {object|null} local    find_system: from the journal/community DB
 * @param {object|null} indexed  galaxy_find: from the star index ({name, pos, id64, class})
 * @param {object|null} external edsm_system: community data fetched on demand
 * @returns {object|null}
 */
export function mergeSystem(local, indexed, external) {
  const coords = local?.coords ?? indexed?.pos ?? external?.coords ?? null;
  if (local) return { ...local, coords };
  if (!external && !indexed) return null;
  return {
    name: external?.name ?? indexed.name,
    id64: indexed?.id64 ?? null,
    coords,
    allegiance: external?.allegiance ?? null,
    government: external?.government ?? null,
    primary_economy: external?.primary_economy ?? null,
    security: external?.security ?? null,
    population: external?.population ?? null,
    primary_star: external?.primary_star ?? indexed?.class ?? null,
    scoopable: external?.scoopable ?? null,
    station_count: 0,
    external: !!external,
  };
}
