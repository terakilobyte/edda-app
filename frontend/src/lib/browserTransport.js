// The browser-only transport the seam in transport.js foresaw: the app
// in a plain browser, no Tauri, answering from fixtures. Reached with
// `?transport=fake` on the dev server (main.js), which is how the
// Playwright tests and an agent's browser see the real tabs render real
// answers — a template exception that leaves a blank tab fails a test
// here instead of a release (2026-10-07: the Route tab blanked for five
// days on an unimported component while every build stayed green).
//
// Unknown commands answer null, the way the vitest fakeTransport does;
// every panel tolerates that. Add a fixture when a test needs a panel
// to show something.
import colonia from "../test/fixtures/colonia-route-2026-10-07.json";

export function browserTransport() {
  const listeners = new Map();
  const handlers = {
    plot_route: async () => structuredClone(colonia),
    personas: async () => ({ selected: "default", personas: [] }),
    release_notes_get: async () => ({ sections: [], unseen: false, latest: null }),
    app_version: async () => "0.0.0-dev",
    frontend_log: async () => null,
    setup_completed: async () => null,
    // The Voice tab after "Remove installed voices" (2026-10-17): nothing
    // speaks, no server is configured, Kokoro is not installed. The status
    // shape is speech_engine_status's, figures as it reports them.
    voice_status: async () => ({ backend: "silent", model: null, muted: true }),
    voice_server_get: async () => ({ enabled: false, config: { url: "", model: "", voice: "", api_key: null }, voices: [] }),
    speech_engine_status: async () => [{ engine: "kokoro", installed: false, running: false, ready: false, url: null, approximate_mb: 2800, available: true, note: "Natural local speech. EDDA downloads an isolated Python runtime, dependencies, and Kokoro model." }],
    voices_installed: async () => [],
  };
  return {
    async invoke(name, args) {
      const h = handlers[name];
      if (typeof h === "function") return h(args);
      // A "get" of one thing answers null (nothing active, nothing
      // remembered); every other unanswered command is a list read, and
      // an empty list is what a fresh install sees.
      if (/(^get_|_get$)/.test(name)) return null;
      return [];
    },
    async listen(event, cb) {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event).add(cb);
      return () => listeners.get(event)?.delete(cb);
    },
    emit(event, payload) {
      for (const cb of [...(listeners.get(event) ?? [])]) cb({ event, payload });
    },
  };
}
