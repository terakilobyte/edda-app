// Contract tests between the frontend and src-tauri, checked from source:
// every event we listen for is emitted, every emitted event is listened
// for, every command we invoke is registered, every wrapper has a caller.
import { describe, it, expect } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const FRONT = resolve(import.meta.dirname, "..");
const RUST = resolve(import.meta.dirname, "../../../src-tauri/src");

function walk(dir, ext) {
  const out = [];
  for (const f of readdirSync(dir)) {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) out.push(...walk(p, ext));
    else if (ext.some((e) => p.endsWith(e))) out.push(p);
  }
  return out;
}
const rustSrc = () => walk(RUST, [".rs"]).map((p) => readFileSync(p, "utf8")).join("\n");
// Paths are normalised to forward slashes: on Windows `join` yields
// backslashes, `/test/` never matched, the test files (which reference
// every export) leaked into the "consumers", and the dead-export check
// passed locally while failing in CI (2026-09-07, ten dangling exports
// after the API-only removals).
const frontFiles = () => walk(FRONT, [".js", ".svelte"]).map((p) => p.replaceAll("\\", "/")).filter((p) => !p.includes("/test/") && !p.endsWith(".test.js"));
const frontSrcExcept = (name) => frontFiles().filter((p) => !p.endsWith(name)).map((p) => readFileSync(p, "utf8")).join("\n");

// Names the backend can emit: the `pub const NAME: &str = "kebab-case";`
// table in src-tauri/src/events.rs, plus any literal `.emit("kebab-case", …)`
// / `.emit_to(target, "kebab-case", …)` left in the Rust tree.
function backendEvents() {
  const table = readFileSync(join(RUST, "events.rs"), "utf8");
  const consts = /pub const [A-Z_]+: &str = "([a-z]+(?:-[a-z]+)*)";/g;
  const src = rustSrc();
  const re = /\.emit(?:_to)?\(\s*(?:[^,()"]+,\s*)?"([a-z]+(?:-[a-z]+)*)"/g;
  return new Set([...[...table.matchAll(consts)].map((m) => m[1]), ...[...src.matchAll(re)].map((m) => m[1])]);
}

// Names from the api module, whichever shape it has: the EVENTS/COMMANDS
// tables of the seam, or the literal listen("…")/invoke("…") calls before it.
async function listenedEvents() {
  const api = await import("../lib/api.js");
  const src = readFileSync(join(FRONT, "lib/api.js"), "utf8");
  return [...new Set([...Object.values(api.EVENTS ?? {}), ...[...src.matchAll(/listen\("([a-z-]+)"/g)].map((m) => m[1])])];
}
async function invokedCommands() {
  const api = await import("../lib/api.js");
  const src = readFileSync(join(FRONT, "lib/api.js"), "utf8");
  const table = Object.values(api.COMMANDS ?? {}).map((c) => (Array.isArray(c) ? c[0] : c));
  return [...new Set([...table, ...[...src.matchAll(/invoke\("([a-z_]+)"/g)].map((m) => m[1])])];
}

function handlerCommands() {
  const lib = readFileSync(join(RUST, "lib.rs"), "utf8");
  const block = /generate_handler!\[([\s\S]*?)\]/.exec(lib)[1];
  return new Set(block.split(",").map((s) => s.trim().split("::").pop()).filter(Boolean));
}

describe("frontend/backend event contract", () => {
  it("every event api.js listens for is emitted by the backend", async () => {
    const listened = await listenedEvents();
    const emitted = backendEvents();
    const dead = listened.filter((e) => !emitted.has(e));
    expect(dead, "listened but never emitted").toEqual([]);
  });

  it("every event the backend emits has a listener in api.js", async () => {
    const listened = new Set(await listenedEvents());
    const missing = [...backendEvents()].filter((e) => !listened.has(e));
    expect(missing, "emitted but nobody listens").toEqual([]);
  });
});

describe("frontend/backend command contract", () => {
  it("every command api.js invokes is registered in generate_handler!", async () => {
    const registered = handlerCommands();
    const invoked = await invokedCommands();
    expect(invoked.length).toBeGreaterThan(100);
    expect(invoked.filter((c) => !registered.has(c))).toEqual([]);
  });

  // 2026-09-10: a stale `onSearchProgress` import survived the unit
  // suite (which only checks that exports have consumers) and broke the
  // production bundle. The bundler's rule, pinned: every name imported
  // from api.js is exported by it.
  it("every name imported from api.js is exported by api.js", async () => {
    const api = await import("../lib/api.js");
    const offenders = [];
    for (const p of frontFiles()) {
      if (p.endsWith("api.js")) continue;
      const text = readFileSync(p, "utf8");
      for (const m of text.matchAll(/import\s*\{([^}]*)\}\s*from\s*"[./]*\/?(?:lib\/)?api\.js"/g)) {
        for (const raw of m[1].split(",")) {
          const name = raw.trim().split(/\s+as\s+/)[0].trim();
          if (name && !(name in api)) offenders.push(`${p.split(/[\/]/).slice(-2).join("/")}: ${name}`);
        }
      }
    }
    expect(offenders, "imported from api.js but not exported").toEqual([]);
  });

  it("every api.js export has a consumer outside api.js", async () => {
    const api = await import("../lib/api.js");
    const infra = new Set(["call", "on", "ApiError", "COMMANDS", "EVENTS"]);
    const rest = frontSrcExcept("lib/api.js");
    const dead = Object.keys(api).filter((k) => !infra.has(k) && !new RegExp(`\\b${k}\\b`).test(rest));
    expect(dead, "exported but never imported").toEqual([]);
  });
});

describe("locality contracts", () => {
  it("formatting helpers live in format.js, not the API seam", async () => {
    const api = await import("../lib/api.js");
    const fmt = await import("../lib/format.js");
    for (const k of ["fmtInt", "fmtCr", "fmtCrShort", "fmtLy", "fmtLs", "fmtMin", "fmtAge", "fmtTs"]) {
      expect(typeof fmt[k], k).toBe("function");
      expect(k in api, `${k} still exported from api.js`).toBe(false);
    }
  });

  it("prioClass and fuelPct are defined exactly once", () => {
    const dups = frontFiles().filter((p) => /(?:function|const)\s+(?:prioClass|fuelPct)\b/.test(readFileSync(p, "utf8")));
    expect(dups.map((p) => p.replace(FRONT.replaceAll("\\", "/"), "").replace(/\\/g, "/"))).toEqual(["/lib/ui.js"]);
  });

  it("no component drives the route store through a source string", () => {
    const offenders = frontFiles().filter((p) => p.endsWith(".svelte") && /routing\.source\s*(=|===)/.test(readFileSync(p, "utf8")));
    expect(offenders.map((p) => p.replace(FRONT.replaceAll("\\", "/"), "").replace(/\\/g, "/"))).toEqual([]);
  });

  it("the loading-line tables are not in the route store", () => {
    const store = readFileSync(join(FRONT, "lib/route.svelte.js"), "utf8");
    expect(store).not.toContain("Planning potty breaks");
    expect(readFileSync(join(FRONT, "lib/loadingLines.js"), "utf8")).toContain("Planning potty breaks");
  });

  it("Overlay imports the follow store, not the whole route module", () => {
    const overlay = readFileSync(join(FRONT, "lib/Overlay.svelte"), "utf8");
    expect(overlay).not.toMatch(/route\.svelte\.js/);
    expect(overlay).not.toMatch(/window\.addEventListener\("storage"/);
  });

  it("SettingsPanel is split along its part prop", () => {
    const app = readFileSync(join(FRONT, "App.svelte"), "utf8");
    expect(app).not.toMatch(/<SettingsPanel\s+part=/);
    expect(app).toMatch(/<VoiceSettings/);
    expect(app).toMatch(/<SystemSettings/);
    expect(frontFiles().some((p) => p.endsWith("SettingsPanel.svelte"))).toBe(false);
  });

  it("every edda.* localStorage key used in the source is declared in storage.svelte.js", async () => {
    const { KEYS } = await import("../lib/storage.svelte.js");
    const declared = new Set(Object.values(KEYS));
    const used = new Set();
    for (const p of frontFiles()) for (const m of readFileSync(p, "utf8").matchAll(/"(edda\.[A-Za-z.]+)"/g)) used.add(m[1]);
    expect([...used].filter((k) => !declared.has(k))).toEqual([]);
    // And nobody spells a key by hand outside the declaration.
    const literal = frontFiles().filter((p) => !p.endsWith("storage.svelte.js") && /"edda\.[A-Za-z.]+"/.test(readFileSync(p, "utf8")));
    expect(literal.map((p) => p.replace(FRONT.replaceAll("\\", "/"), "").replace(/\\/g, "/"))).toEqual([]);
  });
});
