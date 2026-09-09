import { describe, it, expect } from "vitest";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { topics, routeLegend } from "../lib/helpTopics.js";

// The tabs App.svelte renders; a help chip pointing anywhere else is dead.
const TABS = ["setup", "trade", "market", "combat", "missions", "route", "galaxy", "powerplay", "engineering", "inventory", "ships", "voice", "settings", "help"];

describe("help topics", () => {
  it("have unique ids (they are deep-link targets)", () => {
    const ids = topics.map((t) => t.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("all have a title and at least one paragraph", () => {
    for (const t of topics) {
      expect(t.title, t.id).toBeTruthy();
      expect(t.body.length, t.id).toBeGreaterThan(0);
      for (const p of t.body) expect(typeof p, t.id).toBe("string");
    }
  });

  it("chips point at tabs (and Settings sections) that exist", () => {
    const SECTIONS = ["computer", "data", "index", "follow", "hud", "database", "setup"];
    for (const t of topics) {
      for (const [target] of t.tabs ?? []) {
        const [tab, section] = target.split(":");
        expect(TABS, `${t.id} links to unknown tab ${tab}`).toContain(tab);
        if (section != null) {
          expect(tab, `${t.id}: only settings has sections`).toBe("settings");
          expect(SECTIONS, `${t.id} links to unknown settings section ${section}`).toContain(section);
        }
      }
    }
  });

  it("does not document removed features", () => {
    const text = JSON.stringify(topics);
    // The effort dial was replaced by quick-plot + "Try harder".
    expect(text).not.toMatch(/[Ee]ffort/);
    expect(text).toContain("Try harder");
  });

  it("every openHelp() in the app points at a real topic", () => {
    const lib = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "lib");
    const ids = new Set(topics.map((t) => t.id));
    for (const file of fs.readdirSync(lib).filter((f) => f.endsWith(".svelte"))) {
      const source = fs.readFileSync(path.join(lib, file), "utf-8");
      for (const m of source.matchAll(/openHelp\("([^"]+)"\)/g)) {
        expect(ids, `${file} links to unknown help topic ${m[1]}`).toContain(m[1]);
      }
    }
  });

  it("route legend entries are icon + description pairs", () => {
    for (const [icon, what] of routeLegend) {
      expect(icon).toBeTruthy();
      expect(what.length).toBeGreaterThan(3);
    }
  });
});
