# Building EDDA for Linux

First done 2026-09-04 in WSL (Ubuntu, node 22, rust stable) for the
first external Linux tester. Everything below was learned the hard way
that evening; keep it current as the list grows.

## System packages (Ubuntu 24.04)

```
sudo apt-get install -y \
    libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
    libayatana-appindicator3-dev librsvg2-dev patchelf \
    build-essential pkg-config \
    libasound2-dev libudev-dev
```

- The first block is Tauri v2's standard Linux set.
- `libasound2-dev`: alsa-sys, pulled in by the audio/voice stack — the
  first build failure on a clean box.
- `libudev-dev`: device enumeration (joystick/PTT layer).

## Toolchain

- `cargo install tauri-cli --locked` — the build was made with 2.11.4;
  the ancient rc.4 that was lying around predates config keys we use.
- Frontend `node_modules` must be installed ON Linux (`npm ci` in a
  Linux-side checkout): the Windows tree carries win32 native binaries
  (esbuild, rollup) that fail under WSL.

## Build

From a Linux-side clone (never /mnt/c — native FS is drastically
faster and avoids permission-bit weirdness):

```
export CARGO_TARGET_DIR=$HOME/edda-app-target
export TAURI_SIGNING_PRIVATE_KEY="$(cat <path to edda-updater.key>)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
cargo tauri build
```

Products land in `$CARGO_TARGET_DIR/release/bundle/`: an AppImage
(portable, updater-serviceable — the format to hand testers) and a
.deb. The updater `latest.json` platform key is `linux-x86_64`.

## What works / what's untested on Linux (2026-09-04 state)

- WORKS BY DESIGN: journal detection under Steam Proton
  (`platform.rs` knows native, `.local/share`, and Flatpak Steam
  layouts — with tests); community sync from api.edda-app.com; voice
  output via Piper.
- UNTESTED TIER (say so to testers): voice input (downloads the RIGHT
  libraries since 2026-09-05 — sherpa linux-x64 + vosk linux-x86_64,
  platform-picked in listen.rs — but never exercised on real
  hardware), the HUD overlay (Wayland especially), galaxy-map
  automation (input injection), Kokoro managed engine (uv bootstrap is
  platform-mapped, untested). Voice OUTPUT (Piper) has the full
  platform matrix and should just work.

## Tester environment notes

- Any DE runs the app (X11 or Wayland). For the HUD overlay and
  global-hotkey PTT, recommend an X11 session ("GNOME on Xorg" /
  "Plasma (X11)"): Wayland's protocol restricts always-on-top
  positioning and global shortcuts. Elite itself runs under
  Proton/XWayland regardless.
- NVIDIA + webkit2gtk can render a BLANK WINDOW; launch with
  `WEBKIT_DISABLE_DMABUF_RENDERER=1` if so.

## CI consideration (maintainer, 2026-09-04)

Azure is under consideration for CI — motivated by Azure Trusted
Signing (~$10/mo) as the affordable Windows Authenticode path: the
NSIS installer is currently unsigned, so SmartScreen warns every
Windows tester; Trusted Signing fixes that and the same pipeline
could cross-build all three platforms and publish to the
/v1/app/ channel via the release scripts. Decision pending.

## Tray feature (0.2.5+)

The `tray-icon` cargo feature links libappindicator on Linux even
though EDDA's hide-to-tray is compiled out there (vanilla GNOME shows
no tray without a shell extension, so on Linux close means close).
Build needs: `sudo apt install libayatana-appindicator3-dev`. The deb
bundler adds the runtime dependency itself.
