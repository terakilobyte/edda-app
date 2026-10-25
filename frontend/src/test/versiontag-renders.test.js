// 2026-10-25: the maintainer, "we should show the version of edda in the
// app, no?" -- it showed only in Settings, and only after an update check.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import { readFileSync } from "node:fs";
import VersionTag from "../lib/VersionTag.svelte";

describe("VersionTag", () => {
  it("shows the release version", () => {
    const { body } = render(VersionTag, { props: { version: "0.4.8" } });
    expect(body).toContain("v0.4.8");
    expect(body).toContain('class="version');
  });

  it("shows a dev build as dev", () => {
    const { body } = render(VersionTag, { props: { version: "0.4.8-dev" } });
    expect(body).toContain("v0.4.8-dev");
  });

  it("renders nothing before the version is known", () => {
    const { body } = render(VersionTag, { props: { version: "" } });
    expect(body).not.toContain('class="version');
  });
});

describe("the version's wiring", () => {
  const app = readFileSync(new URL("../App.svelte", import.meta.url), "utf8");
  const update = readFileSync(new URL("../lib/AppUpdate.svelte", import.meta.url), "utf8");
  it("the header asks the backend and shows the tag", () => {
    expect(app).toMatch(/appVersionGet\(\)/);
    expect(app).toMatch(/<VersionTag \{version\} \/>/);
  });
  it("Settings > Application knows the version without an update check", () => {
    expect(update).toMatch(/appVersionGet\(\)/);
  });
});
