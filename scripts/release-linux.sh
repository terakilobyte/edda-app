#!/usr/bin/env bash
# Release EDDA, Linux half: build, sign, MERGE into the shared updater
# manifest the API is serving right now.
#
#   ./scripts/release-linux.sh --repo ~/edda-linux
#
# WHY THIS EXISTS. The Windows half is release-app.ps1 and the macOS
# half is release-app.sh. Linux had no script: it was an assistant
# session typing cargo and scp by hand each time, and then an assistant
# session stopped — prod served Linux 0.2.5 while Windows went to 0.2.6
# and 0.2.7, so every Linux user sat three releases behind with no
# update path at all (the manifest had no linux-x86_64 entry, so their
# updater correctly reported "up to date" forever). A step that lives
# only in an agent's head is a step that gets skipped; this file is the
# fix.
#
# `platforms` is ONE map holding every OS, so this MERGES rather than
# overwrites: it fetches the live manifest, keeps foreign entries only
# when they were published for THIS version (a stale entry would hand
# that OS an old binary dressed as new), and adds linux-x86_64.
#
# --repo points at a Linux checkout (the Windows worktree is on /mnt/c
# and cannot build ELF bundles usefully); it must be checked out at the
# tag being shipped. Signing key content comes from the Windows keys
# dir by default — WSL can read /mnt/c.
set -euo pipefail

REPO="" NOTES="" API_BASE="https://api.edda-app.com"
KEY_PATH="${EDDA_UPDATER_KEY:?set EDDA_UPDATER_KEY to the minisign private key path}"
REMOTE="${EDDA_DEPLOY_HOST:?set EDDA_DEPLOY_HOST, e.g. root@api.example.com}"
REMOTE_DIR="/var/lib/edda/artifacts/app"
SKIP_UPLOAD=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --notes) NOTES="$2"; shift 2 ;;
    --api-base) API_BASE="$2"; shift 2 ;;
    --key) KEY_PATH="$2"; shift 2 ;;
    --remote) REMOTE="$2"; shift 2 ;;
    --skip-upload) SKIP_UPLOAD=1; shift ;;
    *) echo "unknown argument: $1 (no --version; src-tauri/Cargo.toml is the source of truth)" >&2; exit 2 ;;
  esac
done
[[ -n "$REPO" ]] || REPO="$(cd "$(dirname "$0")/.." && pwd)"
[[ -f "$KEY_PATH" ]] || { echo "signing key not found at $KEY_PATH" >&2; exit 2; }
cd "$REPO"

# 1. The version IS src-tauri/Cargo.toml's, and a release is cut from
# its TAG — same two guards as the other two halves, for the same
# reason (0.2.6 nearly shipped a feature the maintainer had ruled out).
VERSION="$(sed -nE 's/^version = "([0-9]+\.[0-9]+\.[0-9]+)"/\1/p' src-tauri/Cargo.toml | head -1)"
[[ -n "$VERSION" ]] || { echo "could not read version from src-tauri/Cargo.toml" >&2; exit 2; }
if ! git diff --quiet -- src-tauri || ! git diff --cached --quiet -- src-tauri; then
  echo "src-tauri is dirty in $REPO; a release builds committed source." >&2; exit 2
fi
TAG="v$VERSION"
HEAD_SHA="$(git rev-parse HEAD)"
TAG_SHA="$(git rev-parse --verify --quiet "$TAG^{commit}" || true)"
if [[ "${ALLOW_UNTAGGED:-}" == "1" ]]; then
  echo "ALLOW_UNTAGGED=1: shipping HEAD without checking it against $TAG" >&2
elif [[ "$TAG_SHA" != "$HEAD_SHA" ]]; then
  echo "$REPO is not at $TAG (HEAD $HEAD_SHA, tag ${TAG_SHA:-missing}) - check out the tag to ship it." >&2
  exit 2
fi
echo "releasing Linux $VERSION from $TAG ($HEAD_SHA)"

# 2. Build with signing. The tauri CLI reads the key CONTENT from
# TAURI_SIGNING_PRIVATE_KEY; the _PATH variant is silently ignored.
export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY_PATH")"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
(cd frontend && npm ci --silent 2>/dev/null || npm install --silent)
cargo tauri build --bundles appimage,deb

# 3. Collect. On Linux the updater artifact IS the AppImage, signed
# beside itself by createUpdaterArtifacts.
BUNDLE="target/release/bundle"
APPIMAGE="$(ls "$BUNDLE"/appimage/*.AppImage 2>/dev/null | head -1)"
[[ -n "$APPIMAGE" ]] || { echo "no AppImage under $BUNDLE/appimage" >&2; exit 1; }
[[ -f "$APPIMAGE.sig" ]] || { echo "signature missing beside $APPIMAGE - signing did not run" >&2; exit 1; }
DEB="$(ls "$BUNDLE"/deb/*.deb 2>/dev/null | head -1 || true)"
APPIMAGE_NAME="EDDA_${VERSION}_amd64.AppImage"
DEB_NAME="EDDA_${VERSION}_amd64.deb"

# 4. Merge into the manifest the API serves RIGHT NOW (not a local
# copy): the merge then sees exactly what installed apps see.
STAGE="$(mktemp -d)"; trap 'rm -rf "$STAGE"' EXIT
curl -fsS "$API_BASE/v1/app/latest.json" -o "$STAGE/current.json" || echo '{}' > "$STAGE/current.json"
NOTES="$NOTES" python3 - "$VERSION" "$APPIMAGE.sig" "$API_BASE" "$APPIMAGE_NAME" "$STAGE" <<'EOF'
import datetime, json, os, sys
version, sig_path, api_base, name, stage = sys.argv[1:6]
try:
    current = json.load(open(f"{stage}/current.json"))
except Exception:
    current = {}
if not isinstance(current, dict):
    current = {}
platforms = current.get("platforms") or {}
notes = os.environ.get("NOTES") or current.get("notes") or ""
if current.get("version") != version:
    for stale in sorted(set(platforms) - {"linux-x86_64"}):
        print(f"WARNING: dropping stale {stale} entry (published for "
              f"{current.get('version')}, not {version}) - re-release that "
              f"platform or its users see no update", file=sys.stderr)
    platforms = {}
    if not notes:
        sys.exit("no notes: this is the FIRST platform published for "
                 f"{version}, so --notes is required (the manifest blurb "
                 "is the update splash)")
platforms["linux-x86_64"] = {
    "signature": open(sig_path).read().strip(),
    "url": f"{api_base}/v1/app/{name}",
}
json.dump({
    "version": version,
    "notes": notes,
    "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "platforms": platforms,
}, open(f"{stage}/latest.json", "w"), indent=2)
print("platforms in manifest:", ", ".join(sorted(platforms)))
EOF

cp "$APPIMAGE" "$STAGE/$APPIMAGE_NAME"
[[ -n "$DEB" ]] && cp "$DEB" "$STAGE/$DEB_NAME"

if [[ "$SKIP_UPLOAD" == "1" ]]; then
  echo "--skip-upload: built and signed in $STAGE; nothing shipped."
  trap - EXIT
  exit 0
fi

# 5. Packages first, manifest last, each moved into place atomically:
# the pointer flips only once the bytes it names are there. 0.2.6 broke
# in exactly this gap (a re-signed installer whose bytes were never
# uploaded), so the two halves ship together or not at all.
put() { # local, remotename
  scp -o ConnectTimeout=30 -q "$1" "$REMOTE:$REMOTE_DIR/$2.new"
  ssh -o ConnectTimeout=30 "$REMOTE" \
    "mv '$REMOTE_DIR/$2.new' '$REMOTE_DIR/$2' && chown edda:edda '$REMOTE_DIR/$2' && chmod 644 '$REMOTE_DIR/$2'"
}
put "$STAGE/$APPIMAGE_NAME" "$APPIMAGE_NAME"
[[ -n "$DEB" ]] && put "$STAGE/$DEB_NAME" "$DEB_NAME"
put "$STAGE/latest.json" "latest.json"
echo "published $APPIMAGE_NAME${DEB:+ + $DEB_NAME} + latest.json"

# 6. Verify what the API SERVES against what was signed. A "published"
# line is not evidence.
bash "$(cd "$(dirname "$0")" && pwd)/verify-release.sh" "$API_BASE" "$STAGE" "linux-x86_64"
echo "installed Linux apps will offer $VERSION on their next check"
