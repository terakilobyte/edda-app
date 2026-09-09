// Small presentational helpers shared by the main window and the HUD.
import { fmtInt } from "./format.js";

/** Callout priority → CSS class. */
export const prioClass = (p) => (p >= 3 ? "bad" : p === 2 ? "warn" : p === 1 ? "accent" : "");

/**
 * Fuel as a 0–100 percentage of the main tank, or `null` when the status
 * does not know (no ship yet, capacity unknown). Callers that need a bar
 * width use `fuelPct(s) ?? 0`; callers that colour "low fuel" must not
 * treat unknown as empty.
 * @param {{fuel_main?: number|null, fuel_capacity?: number|null}|null|undefined} status
 * @returns {number|null}
 */
export function fuelPct(status) {
  const f = status?.fuel_main;
  const cap = status?.fuel_capacity;
  if (f == null || !cap) return null;
  return Math.max(0, Math.min(100, Math.round((f / cap) * 100)));
}

/**
 * Project FSD integrity along a route the way fuel is projected: `start`
 * (0..1) minus `loss` per boosted hop, and whenever the next boost would
 * take the drive below `repairAt` a repair is assumed first -- integrity
 * goes back to 100 % and the count goes on -- so a rim route shows every
 * repair stop (about every 19 boosts on a standard drive) and an honest
 * end figure. Neither is a planning input; the loss is a static value per
 * drive (1 %, 0 on the Mk II SCO). `null` when the route has no
 * integrity data.
 * @param {{boosted:boolean}[]} hops
 * @param {number|null|undefined} start
 * @param {number|null|undefined} loss
 * @param {number} [repairAt]
 * @returns {{after:number[], repairs:number[], repairBefore:number|null, end:number, boosts:number}|null}
 */
export function fsdIntegrityPlan(hops, start, loss, repairAt = 0.81) {
  if (start == null || loss == null || !Array.isArray(hops)) return null;
  const after = [];
  const repairs = [];
  let level = start;
  let boosts = 0;
  hops.forEach((h, i) => {
    if (h.boosted) {
      boosts += 1;
      // Exactly the repair line is still fine (19 boosts from full leave
      // 81 %); a hair of floating point must not make it a repair.
      if (level - loss < repairAt - 1e-6) {
        repairs.push(i);
        level = 1.0;
      }
      level = Math.max(0, level - loss);
    }
    after.push(level);
  });
  return { after, repairs, repairBefore: repairs.length ? repairs[0] : null, end: level, boosts };
}

/**
 * Time projection for a plotted route: seconds per hop and totals, for
 * the reader, never the planner. Prices: ~50 s per jump (charge,
 * witchspace, arrival), 35 s to line up a neutron cone / 60 s a white
 * dwarf's (charged at the star that grants the NEXT hop's boost), scoop
 * time from the tank deficit at each refuel stop against the fitted
 * scoop's rate, and a flat 240 s supercruise allowance when the refuel
 * star itself is not scoopable (the scoop is at a companion). The burn
 * of the jump into a stop is approximated at the drive cap — boosted
 * highway hops burn exactly that, ordinary ones slightly less, so scoop
 * times round a touch high rather than low.
 * @param {{class?:string, boosted:boolean, refuel:boolean, scoopable?:boolean, fuel_after:number|null}[]} hops
 * @param {{scoop_rate_t_per_s:number|null, max_fuel_per_jump:number, capacity:number}|null} scoop
 * @returns {{perHop:number[], totalS:number, scoopS:number, boostS:number}}
 */
export function etaPlan(hops, scoop) {
  const JUMP_S = 50;
  const NEUTRON_LINE_UP_S = 35;
  const WHITE_DWARF_LINE_UP_S = 60;
  const COMPANION_SUPERCRUISE_S = 240;
  const perHop = [];
  let scoopS = 0;
  let boostS = 0;
  (hops ?? []).forEach((h, i) => {
    let t = JUMP_S;
    const next = hops[i + 1];
    if (next?.boosted) {
      const lineUp = /white/i.test(h.class ?? "") ? WHITE_DWARF_LINE_UP_S : NEUTRON_LINE_UP_S;
      t += lineUp;
      boostS += lineUp;
    }
    if (h.refuel && scoop?.scoop_rate_t_per_s && h.fuel_after != null) {
      const before = Math.max(0, (hops[i - 1]?.fuel_after ?? scoop.capacity) - scoop.max_fuel_per_jump);
      const scooped = Math.max(0, h.fuel_after - before);
      const st = scooped / scoop.scoop_rate_t_per_s;
      t += st;
      scoopS += st;
      if (h.scoopable === false) t += COMPANION_SUPERCRUISE_S;
    }
    perHop.push(t);
  });
  return { perHop, totalS: perHop.reduce((a, b) => a + b, 0), scoopS, boostS };
}

/**
 * Seconds as a commander would say them: "45 s", "8 m", "1 h 42 m".
 * @param {number} seconds
 * @returns {string}
 */
export function fmtDuration(seconds) {
  const s = Math.round(seconds);
  if (s < 60) return `${s} s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} m`;
  return `${Math.floor(m / 60)} h ${m % 60} m`;
}

/**
 * "12.3 / 32 t", or a plain em dash when the game has not reported fuel
 * (not connected, no ship yet): "-- / 32 t" reads like an empty tank.
 * @param {{fuel_main?: number|null, fuel_capacity?: number|null}|null|undefined} status
 */
export function fuelLabel(status) {
  if (status?.fuel_main == null) return "—";
  const cap = status.fuel_capacity != null ? status.fuel_capacity.toFixed(0) : "?";
  return `${status.fuel_main.toFixed(1)} / ${cap} t`;
}
