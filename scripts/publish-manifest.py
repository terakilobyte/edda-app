#!/usr/bin/env python3
"""Merge platform entries into the updater manifest (latest.json).

Ported from scripts/release-app.sh / release-app.ps1 for CI: `platforms`
is ONE map holding every OS. Entries published for the SAME version are
kept byte-for-byte; a version bump drops stale foreign entries loudly —
an old-version entry preserved into a new manifest would hand that OS an
old binary dressed as new, so that platform simply sees no update until
it is re-released.

    publish-manifest.py --version 0.2.8 --notes "…" --api-base https://api.edda-app.com \
        --current current.json --out latest.json \
        --platform windows-x86_64=EDDA_0.2.8_x64-setup.exe=path/to/installer.sig \
        [--platform linux-x86_64=EDDA_0.2.8_amd64.AppImage=path/to/app.sig]
"""
import argparse
import datetime
import json
import sys


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--version", required=True)
    ap.add_argument("--notes", default="")
    ap.add_argument("--api-base", required=True)
    ap.add_argument("--current", required=True, help="the manifest the API serves now ({} if none)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--platform", action="append", default=[], help="key=filename=sigpath")
    a = ap.parse_args()

    try:
        current = json.load(open(a.current, encoding="utf-8"))
        if not isinstance(current, dict):
            current = {}
    except Exception:
        current = {}
    platforms = dict(current.get("platforms", {}) or {})
    ours = {}
    for spec in a.platform:
        key, filename, sig_path = spec.split("=", 2)
        ours[key] = {
            "signature": open(sig_path, encoding="utf-8").read().strip(),
            "url": f"{a.api_base.rstrip('/')}/v1/app/{filename}",
        }
    if current.get("version") != a.version:
        for stale in sorted(set(platforms) - set(ours)):
            print(
                f"WARNING: dropping stale {stale} entry (published for {current.get('version')}, "
                f"not {a.version}) — re-release that platform or its users see no update",
                file=sys.stderr,
            )
        platforms = {}
    platforms.update(ours)
    if not platforms:
        print("ERROR: no platforms to publish", file=sys.stderr)
        return 1
    manifest = {
        "version": a.version,
        "notes": a.notes,
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }
    json.dump(manifest, open(a.out, "w", encoding="utf-8"), indent=2)
    print("platforms in manifest:", ", ".join(sorted(platforms)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
