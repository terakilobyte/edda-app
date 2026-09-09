// Release-notes parsing shared by the post-update splash and the
// reader (maintainer, 2026-09-06): every "## x.y.z" section leads with a
// one-paragraph summary — the same blurb the website shows — and
// everything after it is the full notes, folded behind an expander on
// both surfaces. One parser so the app and its tests agree on where
// the fold is.

/** Split bundled markdown into `{version, summary, details[]}` sections. */
export function parseNotes(markdown) {
  return (markdown ?? "")
    .split(/^## /m)
    .slice(1)
    .map((part) => {
      const nl = part.indexOf("\n");
      const version = (nl === -1 ? part : part.slice(0, nl)).trim();
      const body = nl === -1 ? "" : part.slice(nl + 1);
      const paras = body
        .trim()
        .split(/\n\n+/)
        .map((p) => p.trim())
        .filter(Boolean);
      return { version, summary: paras[0] ?? "", details: paras.slice(1) };
    });
}

/** `**bold** lead` runs for safe text-node rendering — never innerHTML. */
export function runs(paragraph) {
  return paragraph.split(/\*\*([^*]+)\*\*/).map((text, i) => ({ text, bold: i % 2 === 1 }));
}
