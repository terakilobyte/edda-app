// Toasts (2026-10-09): a refused plot nudges the commander wherever they
// are looking; the panel keeps the line. Pinned: push, auto-dismiss,
// click-dismiss, and the stack rendered server-side.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render } from "svelte/server";
import Toasts from "../lib/Toasts.svelte";
import { dismiss, dismissAll, toast, toasts } from "../lib/toast.svelte.js";

beforeEach(() => { vi.useFakeTimers(); dismissAll(); });
afterEach(() => { vi.useRealTimers(); dismissAll(); });

describe("toasts", () => {
  it("shows, then goes away by itself", () => {
    const id = toast("no known star within 37.1 ly of Jongou XM-W d1-0", "warn", 1000);
    expect(toasts.items.map((t) => t.id)).toEqual([id]);
    vi.advanceTimersByTime(999);
    expect(toasts.items.length).toBe(1);
    vi.advanceTimersByTime(1);
    expect(toasts.items.length).toBe(0);
  });

  it("stays until dismissed when asked to", () => {
    const id = toast("stay", "info", 0);
    vi.advanceTimersByTime(60_000);
    expect(toasts.items.length).toBe(1);
    dismiss(id);
    expect(toasts.items.length).toBe(0);
    dismiss(id);
  });

  it("renders the stack with each toast's kind", () => {
    toast("first", "warn", 0);
    toast("second", "ok", 0);
    const { body } = render(Toasts);
    expect(body).toContain("first");
    expect(body).toContain("second");
    expect(body).toContain("toast warn");
    expect(body).toContain("toast ok");
  });

  it("renders nothing when empty", () => {
    const { body } = render(Toasts);
    expect(body).not.toContain("toasts");
  });
});
