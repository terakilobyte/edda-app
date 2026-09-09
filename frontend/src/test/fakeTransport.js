// An in-memory `{ invoke, listen }` for tests: records every call and lets a
// test emit backend events to whoever subscribed. The second adapter for
// the transport seam (the first is Tauri).
export function fakeTransport(handlers = {}) {
  const calls = [];
  const listeners = new Map();
  return {
    calls,
    listeners,
    async invoke(name, args) {
      calls.push({ name, args });
      const h = handlers[name];
      if (typeof h === "function") return h(args);
      if (h instanceof Error) throw h;
      return h ?? null;
    },
    async listen(event, cb) {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event).add(cb);
      return () => listeners.get(event)?.delete(cb);
    },
    emit(event, payload) {
      for (const cb of [...(listeners.get(event) ?? [])]) cb({ event, payload });
    },
    count(event) {
      return listeners.get(event)?.size ?? 0;
    },
  };
}
