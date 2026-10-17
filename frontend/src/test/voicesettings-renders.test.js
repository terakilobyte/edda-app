// 2026-10-17: the Voice tab gained the managed-Kokoro controls and a
// refresh on every show. A template exception leaves the whole tab blank
// at runtime while the build stays green (Route tab, 0.4.2-0.4.3), so the
// tab is rendered server-side here. Nothing has loaded yet in that render,
// so this covers the static shell; ManagedEngine's states have their own
// render test.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import VoiceSettings from "../lib/VoiceSettings.svelte";

describe("VoiceSettings", () => {
  it("renders its sections before anything has loaded", () => {
    const { body } = render(VoiceSettings);
    expect(body).toContain("Voice");
    expect(body).toContain("connect your own speech server");
  });
});
