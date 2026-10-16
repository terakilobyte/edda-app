// 2026-10-16: confirmations are the app's own modal, never a browser
// dialog (maintainer: "no window alerts, ever"). Rendered server-side with
// the voice-removal question as the panel asks it.
import { describe, expect, it } from "vitest";
import { render } from "svelte/server";
import ConfirmModal from "../lib/ConfirmModal.svelte";
import fs from "node:fs";
import path from "node:path";

describe("ConfirmModal", () => {
  it("renders the question, its lines and both buttons", () => {
    const { body } = render(ConfirmModal, { props: { title: "Remove every installed voice?", lines: ["Deleted from EDDA's data folder: Kokoro (9.2 GB).", "Voice input models stay."], confirmLabel: "Remove voices", cancelLabel: "Keep", danger: true, onConfirm: () => {}, onCancel: () => {} } });
    expect(body).toContain('role="dialog"');
    expect(body).toContain("Remove every installed voice?");
    expect(body).toContain("Kokoro (9.2 GB)");
    expect(body).toContain("Voice input models stay.");
    expect(body).toContain(">Remove voices<");
    expect(body).toContain(">Keep<");
    expect(body).toContain('class="danger ');
  });

  it("shows the busy state while the work runs", () => {
    const { body } = render(ConfirmModal, { props: { title: "x", busy: true, onConfirm: () => {}, onCancel: () => {} } });
    expect(body).toContain("Working…");
    expect(body).toContain("disabled");
  });

  it("is the only confirmation path: no browser dialogs anywhere in the frontend", () => {
    const lib = path.resolve(__dirname, "../lib");
    const offenders = [];
    const walk = (dir) => {
      for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const p = path.join(dir, entry.name);
        if (entry.isDirectory()) walk(p);
        else if (/\.(svelte|js)$/.test(entry.name) && /(^|[^.\w])(confirm|alert|prompt)\(/.test(fs.readFileSync(p, "utf8"))) offenders.push(path.relative(lib, p));
      }
    };
    walk(lib);
    expect(offenders).toEqual([]);
  });
});
