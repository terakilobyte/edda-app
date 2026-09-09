#!/usr/bin/env python3
"""Emit the release-notes FEED from src-tauri/RELEASE-NOTES.md.

One source of truth: the same file the app compiles in for its
what's-new splash produces `notes.json`, which the release pipeline
publishes beside latest.json at https://api.edda-app.com/v1/app/notes.json.
The website's /notes/ page is a static shell that fetches that feed
(maintainer, 2026-09-07: "Release notes need to fetch from json ... I want to
get away from these damn manual website updates"), so the site can
never announce a different release than the API serves, and nobody
regenerates HTML by hand again.

    python3 site/build-notes.py --out notes.json

Shape (newest first):
    [{"version": "0.2.7", "shortNotes": "…one paragraph…",
      "longNotes": ["**Lead.** paragraph", …]}, …]
`longNotes` keeps the markdown bold leads; the page renders them.
"""
import argparse
import json
import pathlib
import re

root = pathlib.Path(__file__).resolve().parent.parent


def feed(md: str) -> list[dict]:
    out = []
    for section in re.split(r"^## ", md, flags=re.M)[1:]:
        version, _, body = section.partition("\n")
        paras = [p.strip() for p in re.split(r"\n\n+", body.strip()) if p.strip()]
        out.append({
            "version": version.strip(),
            "shortNotes": paras[0].replace("\n", " ") if paras else "",
            "longNotes": [p.replace("\n", " ") for p in paras[1:]],
        })
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(root / "notes.json"))
    a = ap.parse_args()
    md = (root / "src-tauri" / "RELEASE-NOTES.md").read_text(encoding="utf-8")
    data = feed(md)
    pathlib.Path(a.out).write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {a.out} ({len(data)} versions, newest {data[0]['version'] if data else 'none'})")


if __name__ == "__main__":
    main()
