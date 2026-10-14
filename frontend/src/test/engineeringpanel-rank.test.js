// 2026-10-14: the Engineering panel says what an engineer will apply for
// THIS commander (rank 4 with Elvira Martuuk = G4) beside what the
// blueprint offers (G5), so a G5 job is never read as reachable through
// an engineer whose rank stops short.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import EngineeringPanel from "../lib/EngineeringPanel.svelte";
import { eng } from "../lib/engineering.svelte.js";

describe("EngineeringPanel access pills", () => {
  it("shows the commander's own grade beside the engineer's cap", () => {
    eng.access = {
      name: "Increased FSD Range", grade: 5, reachable: true, max_reachable_grade: 5,
      engineers: [
        { engineer: "Felicity Farseer", status: "Unlocked", rank: 5, unlocked: true, max_grade: 5, rank_grade: 5 },
        { engineer: "Elvira Martuuk", status: "Unlocked", rank: 4, unlocked: true, max_grade: 5, rank_grade: 4 },
      ],
    };
    const { body } = render(EngineeringPanel);
    eng.access = null;
    // Svelte's SSR output interleaves markers; compare the visible text.
    const text = body.replace(/<!--.*?-->/g, "").replace(/<[^>]+>/g, "").replace(/\s+/g, " ");
    expect(text).toContain("Felicity Farseer · Unlocked · rank 5 · to G5");
    expect(text).not.toContain("G5 for you");
    expect(text).toContain("Elvira Martuuk · Unlocked · rank 4 · to G5 · G4 for you");
  });
});
