#!/usr/bin/env python3
"""Carve the canonical long-read (docs/blog/blazingly-fast.html) into the
site's three blog entries, byte-preserving the frozen text.

The source is the single canonical file — the same content as the live
artifact; TODO voice marks stay for the maintainer. This script only splits and
wraps: content sections are divided at their natural act seams, each page
gets exactly the figure-script blocks whose canvases it carries (plus the
shared-helpers block), and every per-figure IIFE gains a null guard on its
canvas lookup so a block shared across seams degrades to a no-op instead
of throwing (the source IIFEs have none — verified before writing this).

Run from the repo root or site/:  python3 site/build-blog.py
"""

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "docs" / "blog" / "blazingly-fast.html"
OUT = ROOT / "site" / "blog"

ENTRIES = [
    {
        "slug": "chapter-1-instruments-before-opinions",
        "title": "Chapter 1 — Instruments Before Opinions",
        "blurb": "In which 199 million stars meet a 7.5-second plot, we build the timers and the synthetic galaxy before touching the search, and the fastest code turns out to be the code that stops waiting.",
        "clock": "COLONIA CLOCK · 7.5 s \u2192 951 ms",
        "sections": (1, 10),   # Prologue .. Act 5
    },
    {
        "slug": "chapter-2-funerals-and-an-exhumation",
        "title": "Chapter 2 — Funerals, and an Exhumation",
        "blurb": "In which our proudest idea dies on contact with the real galaxy, reverse planning dies twice, meeting in the middle wins the week, and a buried heuristic climbs out of its grave with receipts.",
        "clock": "COLONIA CLOCK · 951 ms \u2192 732 ms · the impossible cell completes",
        "sections": (10, 15),  # Act 6 .. Act 10
    },
    {
        "slug": "chapter-3-the-bug-report",
        "title": "Chapter 3 — The Bug Report We Almost Filed",
        "blurb": "In which the density map grows a suspicious cross, we nearly file a bug against the Milky Way itself, greed becomes a navigation strategy with one caveat, and Spansh humbles us in public.",
        "clock": "THE GALAXY ITSELF IS THE BUG",
        "sections": (15, 20),  # Part II interlude .. Act 14
    },
    {
        "slug": "chapter-4-why-humans",
        "title": "Chapter 4 — Why Humans Are Still Needed",
        "blurb": "In which the machine is wrong, the human is also wrong, everyone is wrong at different times about different things, and the scoreboard survives anyway. Interactive bench table included; argue with it yourself.",
        "clock": "WHEELS DOWN · THE SCOREBOARD",
        "sections": (20, None),  # Act 15 .. interactive epilogue
    },
]

NAV_CSS = """
  .sitenav { display:flex; align-items:center; gap:1rem; padding:1.1rem 0 0;
    font-family: ui-monospace, "JetBrains Mono", Menlo, monospace; font-size:0.8rem; }
  .sitenav a { color: var(--accent); text-decoration: none; }
  .sitenav a:hover { text-decoration: underline; }
  .sitenav .crumb { color: var(--ink-faint); }
  .entrynav { display:flex; justify-content:space-between; gap:1rem;
    border-top:1px solid var(--rule); margin-top:3rem; padding-top:1.4rem;
    font-family: ui-monospace, Menlo, monospace; font-size:0.85rem; }
"""


def main() -> None:
    html = SOURCE.read_text()

    # Head = everything before the content wrapper's first section split.
    style_end = html.index("</style>") + len("</style>")
    head = html[:style_end]
    fonts_match = re.search(r'<link rel="stylesheet"[^>]*>', html[style_end:])
    fonts = fonts_match.group(0) if fonts_match else ""

    body = html[html.index('<div class="wrap">'):]
    first_script = body.index("<script>")
    content, tail = body[:first_script], body[first_script:]

    # Content sections at h2 seams (the masthead/disclosure ride with 0).
    parts = content.split("<h2")
    sections = ["<h2" + p for p in parts[1:]]
    preamble = parts[0]

    # Script blocks, each mapped to the element ids it drives. The shared
    # helpers (mulberry, onVisible, REDUCED) live at the top of the first
    # block, fused with the fig-starmap figure — split them into their own
    # always-included block or every other page calls undefined functions.
    blocks = re.findall(r"<script>.*?</script>", tail, flags=re.S)
    seam = blocks[0].index("/* ---------- fig-starmap")
    helpers_block = blocks[0][:seam] + "</script>"
    blocks[0] = "<script>\n" + blocks[0][seam:]
    blocks.insert(0, helpers_block)
    block_ids = [set(re.findall(r'getElementById\("([^"]+)"\)', b)) for b in blocks]

    # Null-guard every per-figure IIFE opening so shared blocks no-op when
    # their canvas is on another page.
    def guard(block: str) -> str:
        return re.sub(
            r'(const cv = document\.getElementById\("[^"]+"\);)',
            r"\1 if (!cv) return;",
            block,
        )

    OUT.mkdir(parents=True, exist_ok=True)
    written = []
    for index, entry in enumerate(ENTRIES):
        start, end = entry["sections"]
        page_sections = sections[start - 1 : (end - 1 if end else None)]
        page_content = "".join(page_sections)
        page_ids = set(re.findall(r'id="([^"]+)"', page_content))

        include = []
        for block, ids in zip(blocks, block_ids):
            if not ids:  # shared helpers: every page with figures needs it
                include.append(guard(block))
            elif ids & page_ids:
                include.append(guard(block))

        prev_entry = ENTRIES[index - 1] if index > 0 else None
        next_entry = ENTRIES[index + 1] if index + 1 < len(ENTRIES) else None
        nav = '<div class="sitenav"><a href="/">EDDA</a> <span class="crumb">/</span> <a href="/blog/">build log</a> <span class="crumb">/</span> <span class="crumb">%s</span></div>' % entry["title"]
        entrynav = '<div class="entrynav"><span>%s</span><span>%s</span></div>' % (
            f'<a href="/blog/{prev_entry["slug"]}.html">&larr; {prev_entry["title"]}</a>' if prev_entry else "",
            f'<a href="/blog/{next_entry["slug"]}.html">{next_entry["title"]} &rarr;</a>' if next_entry else "",
        )

        page_head = head.replace(
            "<title>How We Made Galaxy Routing Blazingly Fast</title>",
            f"<title>{entry['title']} · EDDA build log</title>",
        ).replace("</style>", NAV_CSS + "</style>")

        first_page = index == 0
        masthead = preamble if first_page else '<div class="wrap">\n'
        doc = (
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">"
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">"
            + page_head + fonts + "</head><body>\n"
            + masthead.replace('<div class="wrap">', f'<div class="wrap">{nav}', 1)
            + page_content
            + entrynav
            + "\n</div>\n"
            + "\n".join(include)
            + "\n</body></html>\n"
        )
        out = OUT / f"{entry['slug']}.html"
        out.write_text(doc)
        written.append((out.name, len(doc), len(include), len(page_sections)))

    # The listing page.
    cards = "\n".join(
        f'<a class="card" href="/blog/{e["slug"]}.html"><span class="node"></span>'
        f'<span class="k">chapter {i+1:02}</span>'
        f'<h2>{e["title"].split("\u2014")[-1].strip()}</h2><p>{e["blurb"]}</p>'
        f'<span class="clock">{e["clock"]}</span></a>'
        for i, e in enumerate(ENTRIES)
    )
    listing = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>EDDA build log</title>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,500..800&family=IBM+Plex+Mono:wght@400;500;600&display=swap">
<style>
  :root {{ --bg:#0a0c10; --panel:#131820; --rule:#262e3a; --ink:#e4e6ea; --muted:#8b95a5; --accent:#ff8c1a; }}
  body {{ margin:0; background:var(--bg); color:var(--ink); font:16px/1.6 system-ui,"Segoe UI",sans-serif; }}
  .wrap {{ max-width:46rem; margin:0 auto; padding:2rem 1.4rem 5rem; }}
  .sitenav {{ font-family:"IBM Plex Mono",monospace; font-size:0.8rem; margin-bottom:2.5rem; }}
  .sitenav a {{ color:var(--accent); text-decoration:none; }}
  h1 {{ font-family:"Bricolage Grotesque",sans-serif; font-weight:800; font-size:2.2rem; margin:0 0 0.4rem; }}
  .sub {{ color:var(--muted); margin:0 0 2.4rem; }}
  /* The chapters are hops on one route: a rail ties the cards together,
     each with its waypoint node and the Colonia clock reading at that
     point in the story. */
  .route {{ position:relative; padding-left:1.6rem; }}
  .route::before {{ content:""; position:absolute; left:6px; top:14px; bottom:24px;
    width:2px; background:linear-gradient(var(--accent), var(--rule)); }}
  .card {{ position:relative; display:block; background:var(--panel); border:1px solid var(--rule);
    border-radius:10px; padding:1.3rem 1.5rem; margin-bottom:1.1rem; text-decoration:none; color:var(--ink); }}
  .card:hover {{ border-color:var(--accent); }}
  .card .node {{ position:absolute; left:-1.6rem; top:1.35rem; width:10px; height:10px;
    margin-left:2px; border-radius:50%; background:var(--bg); border:2px solid var(--accent); }}
  .card:hover .node {{ background:var(--accent); }}
  .card .k {{ font-family:"IBM Plex Mono",monospace; font-size:0.7rem; letter-spacing:0.14em;
    text-transform:uppercase; color:var(--accent); }}
  .card h2 {{ font-family:"Bricolage Grotesque",sans-serif; font-size:1.3rem; margin:0.4rem 0 0.5rem; }}
  .card p {{ color:var(--muted); margin:0 0 0.8rem; font-size:0.95rem; }}
  .card .clock {{ font-family:"IBM Plex Mono",monospace; font-size:0.72rem; letter-spacing:0.06em;
    color:var(--cyan); border:1px solid var(--rule); border-radius:999px; padding:0.18rem 0.6rem;
    background:#10141b; }}
</style></head><body><div class="wrap">
<div class="sitenav"><a href="/">EDDA</a> <span style="color:#667081">/ build log</span></div>
<h1>The build log</h1>
<p class="sub">How we made galaxy routing <em>blazingly</em> fast — the honest ledger, in four chapters. One number ties them together: the wall-clock time to plot Wongi \u2192 Colonia, which starts at 7.5 seconds and ends somewhere you should read for yourself. The trophies are real because the graveyard is bigger.</p>
<div class="route">
{cards}
</div>
</div></body></html>
"""
    (OUT / "index.html").write_text(listing)
    for name, size, scripts, secs in written:
        print(f"{name}: {size/1024:.0f} KB, {secs} sections, {scripts} script blocks")
    print("index.html: listing")


if __name__ == "__main__":
    main()
