// 2026-10-16: the voice engine picker gained "no voice" and the one place
// every installed voice can be removed. A template exception leaves the
// Voice tab blank, so this renders the picker server-side in each state
// with the sizes the backend measured on the maintainer's machine.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import VoiceEngineChoice from "../lib/VoiceEngineChoice.svelte";

const installed = [
  { engine: "kokoro", label: "Kokoro", bytes: 2_966_000_000 },
  { engine: "piper", label: "Piper voice en_US-lessac-high", bytes: 127_000_000 },
  { engine: "piper", label: "Piper engine", bytes: 21_000_000 },
];

describe("VoiceEngineChoice", () => {
  it("offers the four engines, names what speaks, and lists what can be removed", () => {
    const { body } = render(VoiceEngineChoice, { props: { engine: "piper", backend: "piper", installed, onPick: () => {}, onRemoveAll: () => {} } });
    for (const value of ["windows", "piper", "server", "none"]) expect(body).toContain(`value="${value}"`);
    expect(body).toContain("no voice");
    expect(body).toContain("Piper neural");
    expect(body).toContain("Kokoro 2.8 GB");
    expect(body).toContain("Piper voice en_US-lessac-high 121 MB");
    expect(body).toContain("Remove installed voices (2.9 GB)");
  });

  it("explains no voice and skips the fallback warning while it is chosen", () => {
    const { body } = render(VoiceEngineChoice, { props: { engine: "none", backend: "silent", serverConfigured: true, installed: [], onPick: () => {}, onRemoveAll: () => {} } });
    expect(body).toContain("all callouts are muted");
    expect(body).not.toContain("server configured, not running");
    expect(body).not.toContain("Remove installed voices");
  });

  it("warns when a configured server is not the one speaking", () => {
    const { body } = render(VoiceEngineChoice, { props: { engine: "server", backend: "sapi", serverConfigured: true, installed: [], onPick: () => {}, onRemoveAll: () => {} } });
    expect(body).toContain("server configured, not running — using Windows voice");
  });
});
