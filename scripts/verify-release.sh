#!/usr/bin/env bash
# Verify a PUBLISHED release actually installs: the bytes the API serves
# must be the bytes the manifest's signature was made from.
#
# Field case 2026-09-06 (0.2.6): release-app.ps1 REBUILDS and RE-SIGNS on
# every run. It was run a second time to fix the notes, which produced a
# new installer and a new signature — but only latest.json was uploaded
# to the box. Prod then served the OLD binary under the NEW signature,
# and every commander's auto-update failed signature verification. The
# script said "published"; the release was broken. Exit codes are not
# evidence — the served bytes are.
#
#   scripts/verify-release.sh [api_base] [local_artifact_dir] [required_platforms]
#
# Every platform in the manifest is downloaded and checked against the
# name its own signature was made from. Byte-comparison additionally
# needs the local signed artifact, and a release is now TWO machines:
# the Windows half publishes from Windows, the Linux half from WSL, and
# neither holds the other's bytes. So $3 names the platforms this run
# actually shipped — those MUST byte-compare or the release fails.
# Others are reported as "not compared here", never as verified. With
# $3 empty, at least one platform must compare: a run that compared
# nothing is the false pass that shipped 0.2.6.
#
# Read-only: downloads to a temp file and deletes it.
set -uo pipefail

API="${1:-https://api.edda-app.com}"
# The artifact dir release-app.ps1 publishes into. A WORKTREE has no
# .data of its own, so resolve the main checkout: this script is often
# run from a worktree while the release publishes into the main tree.
DEFAULT_LOCAL="${EDDA_ARTIFACT_DIR:-.data/api/artifacts}/app"
LOCAL="${2:-$DEFAULT_LOCAL}"
REQUIRE="${3:-}"
fail=0
compared=0
note() { printf '%s\n' "$*"; }
bad() { printf 'FAIL: %s\n' "$*"; fail=1; }

manifest="$(curl -fsS -m 60 "$API/v1/app/latest.json")" || { bad "cannot fetch $API/v1/app/latest.json"; exit 1; }
version="$(printf '%s' "$manifest" | python3 -c 'import sys,json; print(json.load(sys.stdin)["version"])')"
# An unparseable manifest must FAIL, not sail past with empty strings.
# 0.2.6 was "verified" twice by this script while it compared nothing:
# once because WSL has python3 but no `python`, so every parse produced
# "" and the platform loop never ran. Silence is not success.
[[ -n "$version" ]] || { bad "could not parse a version out of latest.json (is python3 present?)"; exit 1; }
note "published version: $version"

# Every platform the manifest offers must actually download, and the
# Windows one must match the local artifact byte for byte.
platforms="$(printf '%s' "$manifest" | python3 -c 'import sys,json; print(" ".join(json.load(sys.stdin)["platforms"]))')"
[[ -n "$platforms" ]] || { bad "manifest lists no platforms - nothing to verify"; exit 1; }
note "platforms: $platforms"

for p in $platforms; do
  url="$(printf '%s' "$manifest" | python3 -c "import sys,json; print(json.load(sys.stdin)['platforms']['$p']['url'])")"
  tmp="$(mktemp)"
  code="$(curl -sS -m 600 -o "$tmp" -w '%{http_code}' "$url")"
  if [[ "$code" != "200" ]]; then
    bad "$p: $url returned $code"
    rm -f "$tmp"; continue
  fi
  served_sha="$(sha256sum "$tmp" | cut -d' ' -f1)"
  served_size="$(stat -c%s "$tmp")"
  note "$p: http 200, $served_size bytes, sha ${served_sha:0:16}"

  # The signature's trusted comment names the file it was made from.
  sigfile="$(printf '%s' "$manifest" | python3 -c "
import sys,json,base64
sig = base64.b64decode(json.load(sys.stdin)['platforms']['$p']['signature']).decode('utf8','replace')
print(next((l.split('file:')[1].strip() for l in sig.splitlines() if 'file:' in l), ''))
")"
  [[ -n "$sigfile" ]] && note "$p: signature was made from '$sigfile'"
  case "$url" in
    *"$sigfile") ;;
    *) [[ -n "$sigfile" ]] && bad "$p: signature names '$sigfile' but the URL serves $(basename "$url")" ;;
  esac

  local_file="$LOCAL/$(basename "$url")"
  if [[ -f "$local_file" ]]; then
    local_sha="$(sha256sum "$local_file" | cut -d' ' -f1)"
    if [[ "$local_sha" == "$served_sha" ]]; then
      note "$p: MATCHES the local signed artifact"
      compared=$(( compared + 1 ))
    else
      bad "$p: served bytes DIFFER from the local signed artifact — upload the installer, not just latest.json"
      note "     local  ${local_sha:0:16} ($(stat -c%s "$local_file") bytes)"
      note "     served ${served_sha:0:16} ($served_size bytes)"
    fi
  elif [[ -z "$REQUIRE" || " $REQUIRE " == *" $p "* ]]; then
    # A missing local copy is NOT a pass for a platform this run
    # shipped. The whole point of this script is comparing served bytes
    # to signed bytes; without the local artifact it has verified
    # nothing, and saying "verified" anyway is the same false
    # confidence that shipped the broken 0.2.6.
    bad "$p: no local copy at $local_file — cannot compare served bytes to signed bytes. Pass the artifact dir as \$2."
  else
    # Published from the other half of the release, on another machine.
    # Downloaded and name-checked above; NOT byte-compared here.
    note "$p: not compared here (no local copy; published by another platform's release run)"
  fi
  rm -f "$tmp"
done

if (( compared == 0 )) && [[ -z "$REQUIRE" ]]; then
  bad "no platform was byte-compared - this run verified nothing"
fi
if (( fail )); then
  note ""
  note "RELEASE IS BROKEN — do not announce it. Upload BOTH the installer and latest.json together."
  exit 1
fi
note ""
note "release verified: the served bytes are the signed bytes."
