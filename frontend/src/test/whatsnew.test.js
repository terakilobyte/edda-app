// The what's-new splash rendered against the REAL bundled notes, not a
// fixture (0.2.6 field report: "release notes are broken" on an upgraded
// install; the parser test passed on a fixture and never saw the shipped
// section). Server-side render is enough to catch a template that throws
// or a section that renders empty.
import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { render } from "svelte/server";
import WhatsNew from "../lib/WhatsNew.svelte";
import { parseNotes } from "../lib/notes.js";

const here = dirname(fileURLToPath(import.meta.url));
const markdown = readFileSync(resolve(here, "../../../src-tauri/RELEASE-NOTES.md"), "utf8");
const [latest] = parseNotes(markdown);

describe("WhatsNew against the shipped RELEASE-NOTES.md", () => {
  it("the post-update splash renders the newest section's summary and folds the rest", () => {
    const { body } = render(WhatsNew, { props: { notes: { version: latest.version, markdown, unseen: true }, latestOnly: true } });
    expect(body).toContain(`New in EDDA ${latest.version}`);
    // The first sentence of the blurb, whatever the version.
    const firstWords = latest.summary.split(/\s+/).slice(0, 4).join(" ");
    expect(body).toContain(firstWords.replace(/&/g, "&amp;").replace(/'/g, "&#39;").slice(0, 12));
    // A fix-only release honestly has nothing to fold (0.2.7 is one
    // line, maintainer's call), so the fold and the bold-lead rendering are
    // only asserted when the section actually carries details.
    if (latest.details.length > 0) {
      expect(body).toContain("Full notes");
      expect(body).toContain("<strong>");
    } else {
      expect(body).not.toContain("Full notes");
    }
    // Markdown never leaks as literal ** either way.
    expect(body).not.toContain("**");
  });

  it("the full reader renders every version", () => {
    const { body } = render(WhatsNew, { props: { notes: { version: latest.version, markdown, unseen: false }, latestOnly: false } });
    for (const s of parseNotes(markdown)) {
      expect(body).toMatch(new RegExp(`<h3[^>]*>${s.version.replace(/\./g, "\\.")}</h3>`));
    }
  });
});
