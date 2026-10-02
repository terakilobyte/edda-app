// One clipboard for every "copy this name" button: the text last copied,
// so the button that copied it shows a tick for a moment and every other
// one does not. Used by Place.svelte; the Trade panel had its own copy of
// this until 2026-10-02.
export const copied = $state({ text: "" });

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text; document.body.appendChild(ta); ta.select();
    try { document.execCommand("copy"); } catch {}
    ta.remove();
  }
  copied.text = text;
  setTimeout(() => { if (copied.text === text) copied.text = ""; }, 1200);
}
