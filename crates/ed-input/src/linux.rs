//! Linux adapter: a virtual keyboard through `uinput`, with `xdotool` as
//! the fallback when `/dev/uinput` is not writable.
//!
//! The game runs under Proton, whose DirectInput layer reads evdev
//! keycodes -- so a uinput device is the faithful equivalent of Windows'
//! scan-code `SendInput`: the press arrives as if from a real keyboard,
//! with the right key number, whatever the X/Wayland layout says. That
//! needs write access to `/dev/uinput` (a udev rule, see the README).
//! Without it, `xdotool` (XTEST) is used; it works for X11 and XWayland
//! windows, which is where Proton games live, but sends keysyms rather
//! than key numbers.
//!
//! Window queries are best-effort: X11 via `xdotool`, nothing on a pure
//! Wayland session (there is no portable "active window" there).

use crate::keymap::{all_evdev_codes, evdev_code, x_keysym};
use crate::keys::ScanCode;
use crate::send::KeySink;
use std::process::Command;
use std::time::Duration;

enum Backend {
    Uinput(evdev::uinput::VirtualDevice),
    Xdotool,
}

/// The Linux [`KeySink`]: uinput when it can be opened, xdotool otherwise.
pub struct LinuxSink {
    backend: Backend,
}

impl Default for LinuxSink {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxSink {
    pub fn new() -> Self {
        match open_uinput() {
            Ok(dev) => LinuxSink { backend: Backend::Uinput(dev) },
            Err(e) => {
                tracing::warn!(error = %e, "uinput unavailable; falling back to xdotool (add the udev rule from the README for scan-code fidelity)");
                LinuxSink { backend: Backend::Xdotool }
            }
        }
    }

    /// Which backend this sink ended up with, for status displays.
    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Uinput(_) => "uinput",
            Backend::Xdotool => "xdotool",
        }
    }
}

fn open_uinput() -> anyhow::Result<evdev::uinput::VirtualDevice> {
    use evdev::{AttributeSet, KeyCode};
    let mut keys = AttributeSet::<KeyCode>::new();
    for code in all_evdev_codes() {
        keys.insert(KeyCode::new(code));
    }
    let dev = evdev::uinput::VirtualDevice::builder()?
        .name("EDDA virtual keyboard")
        .with_keys(&keys)?
        .build()?;
    // The kernel takes a moment to register the new device with the X
    // server / compositor; presses before then are dropped.
    std::thread::sleep(Duration::from_millis(200));
    Ok(dev)
}

fn send(backend: &mut Backend, sc: ScanCode, down: bool) {
    match backend {
        Backend::Uinput(dev) => {
            let Some(code) = evdev_code(sc) else {
                tracing::warn!(code = sc.code, extended = sc.extended, "no evdev code for scan code");
                return;
            };
            let ev = evdev::InputEvent::new(evdev::EventType::KEY.0, code, if down { 1 } else { 0 });
            if let Err(e) = dev.emit(&[ev]) {
                tracing::warn!(error = %e, code, "uinput rejected the event");
            }
        }
        Backend::Xdotool => {
            let Some(sym) = x_keysym(sc) else {
                tracing::warn!(code = sc.code, extended = sc.extended, "no X keysym for scan code");
                return;
            };
            let verb = if down { "keydown" } else { "keyup" };
            match Command::new("xdotool").args([verb, sym]).status() {
                Ok(s) if s.success() => {}
                Ok(s) => tracing::warn!(%s, sym, "xdotool failed"),
                Err(e) => tracing::warn!(error = %e, "xdotool not runnable; install it or enable uinput"),
            }
        }
    }
}

impl KeySink for LinuxSink {
    fn key_down(&mut self, sc: ScanCode) {
        send(&mut self.backend, sc, true);
    }
    fn key_up(&mut self, sc: ScanCode) {
        send(&mut self.backend, sc, false);
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn move_mouse(&mut self, xf: f32, yf: f32) -> bool {
        move_mouse_in_foreground(xf, yf)
    }
    fn click(&mut self, xf: f32, yf: f32) -> bool {
        click_in_foreground(xf, yf)
    }
}

fn xdotool(args: &[&str]) -> Option<String> {
    let out = Command::new("xdotool").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Title of the active X11 window; empty on Wayland or without xdotool.
pub fn foreground_title() -> String {
    xdotool(&["getactivewindow", "getwindowname"]).unwrap_or_default()
}

/// Geometry of the active window as (x, y, w, h), from xdotool's shell output.
fn active_geometry() -> Option<(i32, i32, i32, i32)> {
    let text = xdotool(&["getactivewindow", "getwindowgeometry", "--shell"])?;
    parse_geometry(&text)
}

fn parse_geometry(text: &str) -> Option<(i32, i32, i32, i32)> {
    let mut x = None;
    let mut y = None;
    let mut w = None;
    let mut h = None;
    for line in text.lines() {
        let (k, v) = line.split_once('=')?;
        let v: i32 = v.trim().parse().ok()?;
        match k.trim() {
            "X" => x = Some(v),
            "Y" => y = Some(v),
            "WIDTH" => w = Some(v),
            "HEIGHT" => h = Some(v),
            _ => {}
        }
    }
    Some((x?, y?, w?, h?))
}

pub fn move_mouse_in_foreground(xf: f32, yf: f32) -> bool {
    let Some((x, y, w, h)) = active_geometry() else { return false };
    let px = x + (w as f32 * xf.clamp(0.0, 1.0)) as i32;
    let py = y + (h as f32 * yf.clamp(0.0, 1.0)) as i32;
    xdotool(&["mousemove", &px.to_string(), &py.to_string()]).is_some()
}

pub fn click_in_foreground(xf: f32, yf: f32) -> bool {
    if !move_mouse_in_foreground(xf, yf) {
        return false;
    }
    std::thread::sleep(Duration::from_millis(60));
    xdotool(&["click", "1"]).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdotool_geometry_parses_to_a_rect() {
        let text = "WINDOW=123\nX=100\nY=50\nWIDTH=1920\nHEIGHT=1080\nSCREEN=0\n";
        assert_eq!(parse_geometry(text), Some((100, 50, 1920, 1080)));
        assert_eq!(parse_geometry("X=1\n"), None);
    }
}
