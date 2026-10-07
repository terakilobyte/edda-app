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
    frontend_log: async () => null,
    setup_completed: async () => null,
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
