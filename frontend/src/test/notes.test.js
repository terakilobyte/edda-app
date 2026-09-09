import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { parseNotes, runs } from "../lib/notes.js";

const fixture = `# EDDA release notes

Preamble the reader never shows.

## 0.9.9

One-line summary of the release.

**First heading.** Detail paragraph one.

**Second heading.** Detail paragraph
that wraps a line.

## 0.9.8

A single-paragraph release.
`;

describe("parseNotes", () => {
  it("drops the preamble and splits sections on ## headers, newest first", () => {
    const sections = parseNotes(fixture);
    expect(sections.map((s) => s.version)).toEqual(["0.9.9", "0.9.8"]);
  });

  it("folds everything after the first paragraph into details", () => {
    const [latest] = parseNotes(fixture);
    expect(latest.summary).toBe("One-line summary of the release.");
    expect(latest.details).toEqual([
      "**First heading.** Detail paragraph one.",
      "**Second heading.** Detail paragraph\nthat wraps a line.",
    ]);
  });

  it("a single-paragraph section has a summary and nothing to expand", () => {
    const [, older] = parseNotes(fixture);
    expect(older.summary).toBe("A single-paragraph release.");
    expect(older.details).toEqual([]);
  });

  it("tolerates empty input", () => {
    expect(parseNotes("")).toEqual([]);
    expect(parseNotes(undefined)).toEqual([]);
  });
});

describe("runs", () => {
  it("splits **bold** leads into text runs without markup", () => {
    expect(runs("**Lead.** Rest of it.")).toEqual([
      { text: "", bold: false },
      { text: "Lead.", bold: true },
      { text: " Rest of it.", bold: false },
    ]);
  });
});

describe("the bundled RELEASE-NOTES.md honours the fold", () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const md = readFileSync(resolve(here, "../../../src-tauri/RELEASE-NOTES.md"), "utf8");
  const sections = parseNotes(md);

  it("every version leads with a non-empty summary paragraph", () => {
    expect(sections.length).toBeGreaterThan(0);
    for (const s of sections) {
      expect(s.summary, `${s.version} has no summary`).not.toBe("");
      expect(s.summary.startsWith("**"), `${s.version} leads with a heading, not a summary`).toBe(false);
    }
  });

  // NOT "every release has details": a fix-only release honestly has
  // nothing to fold (0.2.7 is one line, maintainer's call). What must hold is
  // that a section with details keeps them BEHIND the fold, so the
  // splash never becomes a wall of text.
  it("details, where a section has them, stay behind the fold", () => {
    const withDetails = sections.filter((s) => s.details.length > 0);
    expect(withDetails.length, "no release has ever carried details").toBeGreaterThan(0);
    for (const s of withDetails) {
      for (const d of s.details) {
        expect(d.trim(), `${s.version} has an empty detail paragraph`).not.toBe("");
      }
    }
  });
});
