#!/usr/bin/env bash
# Release EDDA, macOS half: bump, build, sign, merge into the shared
# updater manifest.
#
#   ./scripts/release-app.sh --notes "what changed" \
#       --dest user@api.edda-app.com:/srv/edda/artifacts
#
# The updater serves /v1/app/latest.json; `platforms` is ONE map holding
# every OS, so this script MERGES its darwin-aarch64 entry into the
# manifest the API currently serves instead of overwriting the file
# (the Windows script writes its half; whoever runs second must not
# clobber the first). A foreign platform entry is kept ONLY when it was
# published for this same version — a stale entry would hand that OS an
# old binary dressed as the new version, so mismatches are dropped
# loudly and that platform simply sees no update until re-released.
#
# Signing: the minisign private key NEVER enters the repo. The tauri
# CLI reads the key CONTENT from TAURI_SIGNING_PRIVATE_KEY — the _PATH
# variant is silently ignored (field lesson from the 0.2.0 Windows
# build) — so the key file is read here and exported as content.
#
# Publish order is installer-first, manifest-last: the pointer flips
# only once everything it names is in place (the house install rule,
# same as routing).

set -euo pipefail

NOTES="" DEST="" API_BASE="https://api.edda-app.com"
KEY_PATH="${EDDA_UPDATER_KEY:-$HOME/.edda-keys/edda-updater.key}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --notes) NOTES="$2"; shift 2 ;;
    --dest) DEST="$2"; shift 2 ;;
    --api-base) API_BASE="$2"; shift 2 ;;
    --key) KEY_PATH="$2"; shift 2 ;;
    *) echo "unknown argument: $1 (this script takes no --version; the toml is the source of truth)" >&2; exit 2 ;;
  esac
done
[[ -n "$DEST" ]] || { echo "--dest is required (local dir, or user@host:path for the server)" >&2; exit 2; }
[[ -f "$KEY_PATH" ]] || { echo "signing key not found at $KEY_PATH (set --key or EDDA_UPDATER_KEY)" >&2; exit 2; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# 1. The version IS src-tauri/Cargo.toml's (mirrors release-app.ps1 after
# the 2026-09-05 inversion): the toml is the one source of truth, Tauri
# v2 inherits it (tauri.conf.json has no version field), and telemetry's
# env!(CARGO_PKG_VERSION) is true by construction. A release builds
# COMMITTED source — refuse a dirty src-tauri so the shipped version and
# the tagged commit can never disagree.
VERSION="$(sed -nE 's/^version = "([0-9]+\.[0-9]+\.[0-9]+)"/\1/p' src-tauri/Cargo.toml | head -1)"
[[ -n "$VERSION" ]] || { echo "could not read version from src-tauri/Cargo.toml" >&2; exit 2; }
if ! git diff --quiet -- src-tauri || ! git diff --cached --quiet -- src-tauri; then
  echo "src-tauri has uncommitted changes; a release builds committed source. Commit the version bump first." >&2
  exit 2
fi
# A release is cut from a TAG, not from wherever HEAD has drifted to
# (field case 2026-09-06: a whole feature the maintainer had ruled OUT of 0.2.6
# landed on main minutes after the version bump, and nothing here would
# have noticed). Mirrors release-app.ps1's guard. ALLOW_UNTAGGED=1 for a
# throwaway build only.
TAG="v$VERSION"
HEAD_SHA="$(git rev-parse HEAD)"
TAG_SHA="$(git rev-parse --verify --quiet "$TAG^{commit}" || true)"
if [[ "${ALLOW_UNTAGGED:-}" == "1" ]]; then
  echo "ALLOW_UNTAGGED=1: shipping HEAD without checking it against $TAG" >&2
elif [[ -z "$TAG_SHA" ]]; then
  echo "no tag $TAG - tag the exact commit you mean to ship (git tag $TAG <sha>), then release." >&2
  exit 2
elif [[ "$TAG_SHA" != "$HEAD_SHA" ]]; then
  echo "HEAD is $(git rev-list --count "$TAG..HEAD") commit(s) past $TAG - check out the tag to ship it, or move the tag if you truly mean to ship HEAD." >&2
  exit 2
fi
echo "releasing version $VERSION (from src-tauri/Cargo.toml)"

# 2. Build with signing (createUpdaterArtifacts writes EDDA.app.tar.gz
# and its .sig beside the bundle).
export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY_PATH")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
cargo tauri build --target aarch64-apple-darwin

# 3. Collect the artifacts.
BUNDLE="target/aarch64-apple-darwin/release/bundle"
ARCHIVE="$(ls "$BUNDLE"/macos/*.app.tar.gz 2>/dev/null | head -1)"
[[ -n "$ARCHIVE" ]] || { echo "updater archive not found under $BUNDLE/macos" >&2; exit 1; }
[[ -f "$ARCHIVE.sig" ]] || { echo "signature missing beside $ARCHIVE — signing did not run" >&2; exit 1; }
DMG="$(ls "$BUNDLE"/dmg/*.dmg 2>/dev/null | head -1 || true)"
ARCHIVE_NAME="EDDA_${VERSION}_aarch64.app.tar.gz"
DMG_NAME="EDDA_${VERSION}_aarch64.dmg"

# 4. Merge the darwin entry into the manifest the API serves right now.
# Fetching from the API (not a local file) means the merge sees exactly
# what installed apps see; a missing manifest starts a fresh one.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
curl -fsS "$API_BASE/v1/app/latest.json" -o "$STAGE/current.json" 2>/dev/null || echo '{}' > "$STAGE/current.json"
python3 - "$VERSION" "$NOTES" "$ARCHIVE.sig" "$API_BASE" "$ARCHIVE_NAME" "$STAGE" <<'EOF'
import datetime, json, sys
version, notes, sig_path, api_base, archive_name, stage = sys.argv[1:7]
try:
    current = json.load(open(f"{stage}/current.json"))
except Exception:
    current = {}
platforms = current.get("platforms", {}) if isinstance(current, dict) else {}
if current.get("version") != version:
    for stale in sorted(set(platforms) - {"darwin-aarch64"}):
        print(f"WARNING: dropping stale {stale} entry (published for "
              f"{current.get('version')}, not {version}) — re-release that "
              f"platform or its users see no update", file=sys.stderr)
    platforms = {}
platforms["darwin-aarch64"] = {
    "signature": open(sig_path).read().strip(),
    "url": f"{api_base}/v1/app/{archive_name}",
}
manifest = {
    "version": version,
    "notes": notes,
    "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": platforms,
}
json.dump(manifest, open(f"{stage}/latest.json", "w"), indent=2)
print("platforms in manifest:", ", ".join(sorted(platforms)))
EOF

# 5. Publish: packages first, manifest last.
publish() { # src, destname
  if [[ "$DEST" == *:* ]]; then
    scp -q "$1" "$DEST/app/$2"
  else
    mkdir -p "$DEST/app"
    cp "$1" "$DEST/app/$2"
  fi
}
if [[ "$DEST" == *:* ]]; then
  ssh -q "${DEST%%:*}" "mkdir -p '${DEST#*:}/app'"
fi
publish "$ARCHIVE" "$ARCHIVE_NAME"
[[ -n "$DMG" ]] && publish "$DMG" "$DMG_NAME"
publish "$STAGE/latest.json" "latest.json"
echo "published $ARCHIVE_NAME${DMG:+ + $DMG_NAME} + latest.json -> $DEST/app"
echo "installed apps will offer $VERSION on their next check"
