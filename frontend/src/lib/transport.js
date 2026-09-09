// The transport seam: everything the frontend says to the backend goes
// through one `{ invoke, listen }` pair. The default adapter is Tauri; tests
// (and a future browser-only dev mode) swap it with `setTransport`.
//
// The Tauri modules are imported lazily so importing any frontend module
// never touches a webview API; the first call does.

/**
 * @typedef {object} Transport
 * @property {(command: string, args?: object) => Promise<any>} invoke
 * @property {(event: string, cb: (e: {event: string, payload: any}) => void) => Promise<() => void>} listen
 */

/** @type {Transport} */
export const tauriTransport = {
  invoke: async (command, args) => (await import("@tauri-apps/api/core")).invoke(command, args),
  listen: async (event, cb) => (await import("@tauri-apps/api/event")).listen(event, cb),
};

let current = tauriTransport;

/** @param {Transport} t */
export function setTransport(t) {
  current = t;
}

/** @returns {Transport} */
export function getTransport() {
  return current;
}
