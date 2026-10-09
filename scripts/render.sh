#!/usr/bin/env bash
# Render a page with headless Chrome for a before/after pair (edda-ui
# skill). The Chrome extension cannot open file:// or localhost pages;
# headless Chrome can, so a local page is served over http and rendered
# here.
#
#   scripts/render.sh <url | path/to/index.html> <out.png> [width] [height]
#
# A path is served from its directory on a free port for the duration of
# the render. Width defaults to 1232 (the extension's viewport, so a
# render and a live screenshot line up), height to 1700.
set -euo pipefail
target="${1:?url or html file}"; out="${2:?output png}"; width="${3:-1232}"; height="${4:-1700}"
chrome=""
py="$(command -v python3 || command -v python || true)"
for c in "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" "/Applications/Chromium.app/Contents/MacOS/Chromium" "$(command -v google-chrome || true)" "$(command -v chromium || true)" "$(command -v chromium-browser || true)"          "/c/Program Files/Google/Chrome/Application/chrome.exe" "/c/Program Files (x86)/Google/Chrome/Application/chrome.exe" "${LOCALAPPDATA:-}/Google/Chrome/Application/chrome.exe" "/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"; do
    [ -n "$c" ] && [ -x "$c" ] && { chrome="$c"; break; }
done
[ -n "$chrome" ] || { echo "no Chrome or Chromium found" >&2; exit 2; }
server=""
if [ -f "$target" ]; then
    dir="$(cd "$(dirname "$target")" && pwd)"; file="$(basename "$target")"
    port=$("$py" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
    (cd "$dir" && "$py" -m http.server "$port" --bind 127.0.0.1 >/dev/null 2>&1) &
    server=$!
    for _ in $(seq 1 30); do curl -sf "http://127.0.0.1:$port/$file" >/dev/null && break; sleep 0.2; done
    target="http://127.0.0.1:$port/$file"
fi
trap '[ -n "$server" ] && kill "$server" 2>/dev/null || true' EXIT
# Windows Chrome wants a Windows path for the screenshot (Git Bash gives /c/...).
out_native="$out"; command -v cygpath >/dev/null 2>&1 && out_native="$(cygpath -w "$out")"
"$chrome" --headless=new --disable-gpu --hide-scrollbars --window-size="${width},${height}" --screenshot="$out_native" "$target" >/dev/null 2>&1
[ -s "$out" ] || { echo "render produced no image" >&2; exit 1; }
echo "$out ($("$py" -c "import struct;d=open('$out','rb').read(24);print('%dx%d'%struct.unpack('>II',d[16:24]))"))"
