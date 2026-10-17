// 2026-10-17: the Voice tab shows EDDA's managed Kokoro by its live state
// and offers the one action that state calls for (the maintainer removed
// Kokoro to test the flow, had to go to setup to reinstall it, and the
// Voice tab then probed a fixed address Kokoro never listens on). The
// status shapes are what speech_engine_status returned on his machine.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import ManagedEngine from "../lib/ManagedEngine.svelte";

const base = { engine: "kokoro", approximate_mb: 2800, available: true, note: "Natural local speech. EDDA downloads an isolated Python runtime, dependencies, and Kokoro model." };
const noop = () => {};

describe("ManagedEngine", () => {
  it("offers the install with its size when Kokoro is not installed", () => {
    const { body } = render(ManagedEngine, { props: { status: { ...base, installed: false, running: false, ready: false, url: null }, onInstall: noop, onStart: noop } });
    expect(body).toContain("Kokoro is not installed on this computer.");
    expect(body).toContain("Download and install (about 2.8 GB)");
    expect(body).not.toContain("Start and use");
  });

  it("offers the start when Kokoro is installed but stopped", () => {
    const { body } = render(ManagedEngine, { props: { status: { ...base, installed: true, running: false, ready: false, url: null }, onInstall: noop, onStart: noop } });
    expect(body).toContain("Kokoro installed, not running");
    expect(body).toContain("Start and use");
    expect(body).not.toContain("Download and install");
  });

  it("says starting while the process is up and not yet answering", () => {
    const { body } = render(ManagedEngine, { props: { status: { ...base, installed: true, running: true, ready: false, url: "http://127.0.0.1:54385" }, onInstall: noop, onStart: noop } });
    expect(body).toContain("Kokoro starting…");
    expect(body).not.toContain("<button");
  });

  it("names the live port when Kokoro answers, and shows install progress", () => {
    const { body } = render(ManagedEngine, { props: { status: { ...base, installed: true, running: true, ready: true, url: "http://127.0.0.1:54385" }, progress: { engine: "kokoro", phase: "starting", detail: "Starting Kokoro…", fraction: 0.95 }, onInstall: noop, onStart: noop } });
    expect(body).toContain("Kokoro running");
    expect(body).toContain("http://127.0.0.1:54385");
    expect(body).toContain("width:95%");
    expect(body).toContain("Starting Kokoro…");
  });

  it("hides the progress bar once the engine reports ready", () => {
    const { body } = render(ManagedEngine, { props: { status: { ...base, installed: true, running: true, ready: true, url: "http://127.0.0.1:54385" }, progress: { engine: "kokoro", phase: "ready", detail: "Kokoro is ready.", fraction: 1 }, onInstall: noop, onStart: noop } });
    expect(body).not.toContain("bar-fill");
  });

  it("waits for the status before claiming anything", () => {
    const { body } = render(ManagedEngine, { props: { status: null, onInstall: noop, onStart: noop } });
    expect(body).toContain("Checking Kokoro…");
  });
});
