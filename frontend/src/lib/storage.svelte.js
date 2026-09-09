// Every `edda.*` localStorage key the app uses, and one way to read them.
// Both windows share this storage; `pinnedLoop` is also a message from the
// Trade tab to the HUD, which follows it with `sync: true`.

export const KEYS = Object.freeze({
  tab: "edda.tab",                                  // last main-window tab
  onboardingComplete: "edda.onboarding.complete",   // "1" once setup was finished
  onboardingStep: "edda.onboarding.step",           // 0–7 while setup is in progress
  onboardingSpoken: "edda.onboarding.spoken",       // JSON: step indices already narrated (each only once, ever)
  pinnedLoop: "edda.pinnedLoop",                    // JSON: trade loop shown on the HUD
  hudAlpha: "edda.hudAlpha",                        // HUD background opacity, 0–1 (Settings → HUD; storage is the bus)
  hudScale: "edda.hudScale",                        // HUD content scale, 0.7–1.5
  plotInjections: "edda.plotInjections",            // route plotter: use FSD injections
  plotWhiteDwarfs: "edda.plotWhiteDwarfs",          // route plotter: boost off white dwarfs too (opt-in)
  plotMinFuel: "edda.plotMinFuel",                  // route plotter: only stop for fuel when required (opt-in)
  plotSafeMargins: "edda.plotSafeMargins",          // route plotter: plan reaches with the 2 t safety band (opt-in; default is optimistic)
  plotStopWeight: "edda.plotStopWeight",            // route plotter: jumps-vs-refuels dial (1 = the measured time model)
  plotTryHard: "edda.plotTryHard",                  // route plotter: absolute fewest jumps, whatever the search costs (opt-in)
  plotEffort: "edda.plotEffort",                    // route plotter: "high" | "medium" | "low"
  routeMap: "edda.routeMap",                        // route tab: show the map
  speak: "edda.speak",                              // ship computer: speak replies
  galaxySuspendAnimations: "edda.galaxy.suspendAnimations",
  galaxyStarBrightness: "edda.galaxy.starBrightness",   // 0-100, all modes
  settingsSection: "edda.settings.section",         // last Settings-tab section
});

const safe = (fn, fallback) => { try { return fn(); } catch { return fallback; } };

// Booleans are stored as "1"/"0" (the app's one spelling), strings as-is,
// anything else as JSON.
function decode(raw, def, json) {
  if (raw == null) return def;
  if (json) return safe(() => JSON.parse(raw), def);
  if (typeof def === "boolean") return raw === "1";
  if (typeof def === "number") { const n = Number(raw); return Number.isFinite(n) ? n : def; }
  return raw;
}
function encode(v, json) {
  if (json) return JSON.stringify(v);
  if (typeof v === "boolean") return v ? "1" : "0";
  return String(v);
}

export function readKey(key, def = null, { json = false } = {}) {
  return decode(safe(() => localStorage.getItem(key), null), def, json);
}
export function writeKey(key, value, { json = false } = {}) {
  safe(() => (value == null && !json ? localStorage.removeItem(key) : localStorage.setItem(key, encode(value, json))));
}
export function removeKey(key) {
  safe(() => localStorage.removeItem(key));
}

/**
 * A reactive value backed by localStorage. Reads once, writes on every set,
 * never throws (a blocked storage just means the default). With `sync`,
 * writes from the other window are picked up via the `storage` event until
 * `dispose()` is called.
 * @template T
 * @param {string} key one of KEYS
 * @param {T} def
 * @param {{json?: boolean, sync?: boolean}} [opts]
 * @returns {{value: T, dispose: () => void}}
 */
export function persisted(key, def, { json = false, sync = false } = {}) {
  let value = $state(readKey(key, def, { json }));
  let onStorage = null;
  if (sync && typeof window !== "undefined") {
    onStorage = (e) => { if (!e || e.key == null || e.key === key) value = readKey(key, def, { json }); };
    window.addEventListener("storage", onStorage);
  }
  return {
    get value() { return value; },
    set value(v) { value = v; writeKey(key, v, { json }); },
    dispose() { if (onStorage) window.removeEventListener("storage", onStorage); onStorage = null; },
  };
}
