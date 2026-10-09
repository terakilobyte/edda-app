// Toasts: a short-lived notice in the corner of the window, for things
// that happen while the commander is looking elsewhere (a plot refused
// with its reasons, 2026-10-09: "rather than just show red text maybe we
// can make this a toast"). The panel keeps its own line for the record;
// the toast is the nudge.
export const toasts = $state({ items: [] });

let next = 1;

/** Show `text` for `ttlMs` (0 = until dismissed). Returns the toast id. */
export function toast(text, kind = "info", ttlMs = 12000) {
  const id = next++;
  toasts.items.push({ id, text: String(text), kind });
  if (ttlMs > 0) setTimeout(() => dismiss(id), ttlMs);
  return id;
}

export function dismiss(id) {
  const i = toasts.items.findIndex((t) => t.id === id);
  if (i >= 0) toasts.items.splice(i, 1);
}

export function dismissAll() {
  toasts.items.length = 0;
}
