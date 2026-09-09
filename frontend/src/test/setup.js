// Every test runs with Tauri stubbed out: nothing here may reach a webview.
// `tauri.invoke` / `tauri.listen` record what the real transport would
// have done, so a test can assert that importing a module makes no calls.
import { vi } from "vitest";

export const tauri = { invoke: vi.fn(async () => null), listen: vi.fn(async () => () => {}) };
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a) => tauri.invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...a) => tauri.listen(...a) }));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => ({ startDragging: async () => {} }) }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: async () => {} }));
