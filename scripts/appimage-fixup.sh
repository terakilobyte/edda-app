#!/usr/bin/env bash
# Post-process a Tauri-built AppImage so it survives Mesa 25+ hosts
# (research 2026-09-06, field-proven by the tester's own build working
# where ours white-screened with EGL_BAD_PARAMETER):
#
#   Tauri's linuxdeploy bundling over-includes libraries from the build
#   host — critically libwayland-client — that the host's Mesa EGL
#   stack then resolves against, and the version mismatch makes
#   eglGetDisplay fail; WebKitWebProcess aborts, both windows render
#   white. (tauri-apps/tauri #15665, #11988; pkg2appimage's excludelist
#   has carried libwayland for exactly this reason.)
#
# Usage: appimage-fixup.sh <app.AppImage>   (rewrites it in place)
# Requires: appimagetool on PATH (or APPIMAGETOOL env pointing at it).
set -euo pipefail

APPIMAGE="${1:?usage: appimage-fixup.sh <app.AppImage>}"
APPIMAGETOOL="${APPIMAGETOOL:-appimagetool}"
command -v "$APPIMAGETOOL" >/dev/null || { echo "appimagetool not found (set APPIMAGETOOL)"; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cp "$APPIMAGE" "$WORK/app.AppImage"
chmod +x "$WORK/app.AppImage"
(cd "$WORK" && ./app.AppImage --appimage-extract >/dev/null)
APPDIR="$WORK/squashfs-root"

# The exclude set: host-graphics-adjacent libraries that must come
# from the RUNTIME machine, never the build machine.
STRIP_PATTERNS=(
    'libwayland-client.so*' 'libwayland-cursor.so*' 'libwayland-egl.so*' 'libwayland-server.so*'
    'libglib-2.0.so*' 'libgio-2.0.so*' 'libgobject-2.0.so*' 'libgmodule-2.0.so*' 'libgthread-2.0.so*'
    'libgst*'
    # Should never be bundled; verify-and-strip in case the bundler slips.
    'libEGL.so*' 'libGL.so*' 'libGLX.so*' 'libGLdispatch.so*' 'libgbm.so*' 'libdrm.so*' 'libglapi.so*'
)
stripped=0
for pattern in "${STRIP_PATTERNS[@]}"; do
    while IFS= read -r -d '' f; do
        rm -f "$f"
        echo "stripped: ${f#"$APPDIR"/}"
        stripped=$((stripped + 1))
    done < <(find "$APPDIR" -name "$pattern" -print0)
done

# tauri #15665 companion bug: AppRun exports GST_PLUGIN_SYSTEM_PATH_1_0
# pointing into the AppDir even when GStreamer isn't bundled, which
# disables the host's plugin search and can poison its registry cache.
if grep -q 'GST_PLUGIN_SYSTEM_PATH' "$APPDIR/AppRun" 2>/dev/null; then
    sed -i '/GST_PLUGIN_SYSTEM_PATH/d; /GST_REGISTRY/d' "$APPDIR/AppRun"
    echo "fixed: AppRun GStreamer exports removed"
fi

"$APPIMAGETOOL" "$APPDIR" "$WORK/fixed.AppImage" >/dev/null 2>&1
mv "$WORK/fixed.AppImage" "$APPIMAGE"
echo "repacked: $APPIMAGE (stripped $stripped libraries)"
echo "REMINDER: the release pin is a run on a Mesa 25+ box (Arch/Fedora) — a build that white-screens the pin fails."
