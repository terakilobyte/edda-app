//! Pressing keys the way the game sees them.
//!
//! `SendInput` with `KEYEVENTF_SCANCODE` puts hardware scan codes on the
//! input queue, which DirectInput (Elite's reader) picks up; plain
//! virtual-key events do not register in-game. Modifiers are held around
//! the key, every press has a real hold time, and there is a short gap
//! between presses -- the game polls, and a zero-length press is missed.
//!
//! Nothing here checks that the game has focus. The app does that (window
//! title) before calling in, and refuses otherwise: typing a system name
//! into whatever window happens to be active is not a feature.

use crate::keys::{char_key, ScanCode};
use std::time::Duration;

/// Timings, in ms. Conservative; the game's menus animate.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub hold: u64,
    pub gap: u64,
    pub modifier_lead: u64,
}

impl Default for Timing {
    fn default() -> Self {
        // Human-like: the game samples input per frame and a chord whose
        // modifier arrives a frame or two before the key is not reliably
        // seen as a chord (Shift+H with a 30 ms lead opened nothing).
        Timing {
            hold: 70,
            gap: 50,
            modifier_lead: 120,
        }
    }
}

/// Everything the sender can do. A trait so the app can substitute a
/// recorder in tests and log what would have been pressed.
pub trait KeySink {
    fn key_down(&mut self, sc: ScanCode);
    fn key_up(&mut self, sc: ScanCode);
    fn sleep(&mut self, d: Duration);
    /// Park the mouse at a fraction (0..1) of the foreground window.
    /// Defaults to "cannot"; the native sinks override it.
    fn move_mouse(&mut self, _xf: f32, _yf: f32) -> bool {
        false
    }
    /// Left-click at a fraction (0..1) of the foreground window.
    fn click(&mut self, _xf: f32, _yf: f32) -> bool {
        false
    }
    /// Where the OS cursor is right now, in SCREEN coordinates — captured
    /// before a macro drives the mouse so it can be put back afterwards
    /// (item 35b: the app borrows the commander's cursor, it always
    /// returns it). `None` = cannot read; the restore is then skipped.
    fn cursor_pos(&mut self) -> Option<(i32, i32)> {
        None
    }
    /// Put the cursor back at SCREEN coordinates: the restore half.
    fn restore_cursor(&mut self, _x: i32, _y: i32) -> bool {
        false
    }
}

/// Press one chord: modifiers down, key tap, modifiers up.
pub fn press_chord(sink: &mut dyn KeySink, modifiers: &[ScanCode], key: ScanCode, t: Timing) {
    for m in modifiers {
        sink.key_down(*m);
    }
    if !modifiers.is_empty() {
        sink.sleep(Duration::from_millis(t.modifier_lead));
    }
    sink.key_down(key);
    sink.sleep(Duration::from_millis(t.hold));
    sink.key_up(key);
    for m in modifiers.iter().rev() {
        sink.key_up(*m);
    }
    sink.sleep(Duration::from_millis(t.gap));
}

/// Type text into a focused field (galaxy map search). Unknown characters
/// are skipped and reported back so the caller can warn.
pub fn type_text(sink: &mut dyn KeySink, text: &str, t: Timing) -> Vec<char> {
    let shift = crate::keys::scan_code("Key_LeftShift").expect("shift is known");
    let mut skipped = Vec::new();
    for c in text.chars() {
        match char_key(c) {
            Some((sc, upper)) => {
                let mods: &[ScanCode] = if upper {
                    std::slice::from_ref(&shift)
                } else {
                    &[]
                };
                press_chord(
                    sink,
                    mods,
                    sc,
                    Timing {
                        gap: t.gap / 2,
                        ..t
                    },
                );
            }
            None => skipped.push(c),
        }
    }
    skipped
}

/// Records presses instead of sending them.
#[derive(Debug, Default)]
pub struct Recorder {
    pub events: Vec<String>,
    /// What `cursor_pos` reports; `None` plays a platform that cannot read it.
    pub cursor: Option<(i32, i32)>,
}

impl KeySink for Recorder {
    fn key_down(&mut self, sc: ScanCode) {
        self.events.push(format!(
            "down {:#04x}{}",
            sc.code,
            if sc.extended { "e" } else { "" }
        ));
    }
    fn key_up(&mut self, sc: ScanCode) {
        self.events.push(format!(
            "up {:#04x}{}",
            sc.code,
            if sc.extended { "e" } else { "" }
        ));
    }
    fn sleep(&mut self, _d: Duration) {}
    fn move_mouse(&mut self, xf: f32, yf: f32) -> bool {
        self.events.push(format!("mouse {xf:.2},{yf:.2}"));
        true
    }
    fn click(&mut self, xf: f32, yf: f32) -> bool {
        self.events.push(format!("click {xf:.2},{yf:.2}"));
        true
    }
    fn cursor_pos(&mut self) -> Option<(i32, i32)> {
        self.cursor
    }
    fn restore_cursor(&mut self, x: i32, y: i32) -> bool {
        self.events.push(format!("restore {x},{y}"));
        true
    }
}

/// Sink for this platform. Three real adapters -- `SendInput` on Windows,
/// uinput/xdotool on Linux ([`crate::linux`]), CGEvent on macOS
/// ([`crate::macos`]) -- and a sleeping no-op for anything else, so the
/// timing behaviour of a macro can still be exercised there.
#[cfg(windows)]
pub type NativeSink = WindowsSink;
#[cfg(target_os = "linux")]
pub type NativeSink = crate::linux::LinuxSink;
#[cfg(target_os = "macos")]
pub type NativeSink = crate::macos::MacSink;
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub type NativeSink = NoopSink;

/// Sleeps but presses nothing. The [`NativeSink`] where no adapter exists.
#[derive(Debug, Default)]
pub struct NoopSink;

impl KeySink for NoopSink {
    fn key_down(&mut self, _sc: ScanCode) {}
    fn key_up(&mut self, _sc: ScanCode) {}
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The real thing.
#[cfg(windows)]
#[derive(Debug, Default)]
pub struct WindowsSink;

#[cfg(windows)]
impl KeySink for WindowsSink {
    fn key_down(&mut self, sc: ScanCode) {
        send(sc, false);
    }
    fn key_up(&mut self, sc: ScanCode) {
        send(sc, true);
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
    fn cursor_pos(&mut self) -> Option<(i32, i32)> {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        // SAFETY: plain Win32 call with a valid out-pointer to a POINT we own.
        unsafe {
            let mut p = POINT { x: 0, y: 0 };
            (GetCursorPos(&mut p) != 0).then_some((p.x, p.y))
        }
    }
    fn restore_cursor(&mut self, x: i32, y: i32) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;
        // SAFETY: plain Win32 call.
        unsafe { SetCursorPos(x, y) != 0 }
    }
}

#[cfg(windows)]
fn send(sc: ScanCode, up: bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
        KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    };
    let mut flags = KEYEVENTF_SCANCODE;
    if sc.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: sc.code,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    // SAFETY: a fully initialised INPUT array of length 1 and its size.
    let sent = unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        tracing::warn!(code = sc.code, "SendInput rejected the event");
    }
}

/// Park the mouse at a fraction (0..1) of the foreground window's client
/// area. The galaxy map focuses whatever the cursor is over, so UI keys
/// only behave predictably from a known spot.
#[cfg(windows)]
pub fn move_mouse_in_foreground(xf: f32, yf: f32) -> bool {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowRect, SetCursorPos,
    };
    // SAFETY: plain Win32 calls with a valid out-pointer to a RECT we own.
    unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            return false;
        }
        let mut r = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(h, &mut r) == 0 {
            return false;
        }
        let x = r.left + ((r.right - r.left) as f32 * xf.clamp(0.0, 1.0)) as i32;
        let y = r.top + ((r.bottom - r.top) as f32 * yf.clamp(0.0, 1.0)) as i32;
        SetCursorPos(x, y) != 0
    }
}

#[cfg(target_os = "linux")]
pub use crate::linux::move_mouse_in_foreground;
#[cfg(target_os = "macos")]
pub use crate::macos::move_mouse_in_foreground;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn move_mouse_in_foreground(_xf: f32, _yf: f32) -> bool {
    false
}

/// Move to a fraction of the foreground window and left-click there.
#[cfg(windows)]
pub fn click_in_foreground(xf: f32, yf: f32) -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
        MOUSEINPUT,
    };
    if !move_mouse_in_foreground(xf, yf) {
        return false;
    }
    std::thread::sleep(std::time::Duration::from_millis(60));
    let mk = |flags: u32| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let down = mk(MOUSEEVENTF_LEFTDOWN);
    let up = mk(MOUSEEVENTF_LEFTUP);
    // SAFETY: well-formed INPUT structs, sizes as SendInput expects.
    unsafe {
        let a = SendInput(1, &down, std::mem::size_of::<INPUT>() as i32);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let b = SendInput(1, &up, std::mem::size_of::<INPUT>() as i32);
        a == 1 && b == 1
    }
}

#[cfg(target_os = "linux")]
pub use crate::linux::click_in_foreground;
#[cfg(target_os = "macos")]
pub use crate::macos::click_in_foreground;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn click_in_foreground(_xf: f32, _yf: f32) -> bool {
    false
}

/// Title of the foreground window, for the "is the game focused" check.
#[cfg(windows)]
pub fn foreground_title() -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    // SAFETY: GetForegroundWindow returns a handle or null; GetWindowTextW
    // writes at most `len` UTF-16 units into the buffer we own.
    unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            return String::new();
        }
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

#[cfg(target_os = "linux")]
pub use crate::linux::foreground_title;
#[cfg(target_os = "macos")]
pub use crate::macos::foreground_title;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn foreground_title() -> String {
    String::new()
}

/// Elite's main window title starts with this.
pub const GAME_TITLE_PREFIX: &str = "Elite - Dangerous";

pub fn game_is_focused() -> bool {
    foreground_title().starts_with(GAME_TITLE_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::scan_code;

    #[test]
    fn a_chord_holds_modifiers_around_the_key() {
        let mut r = Recorder::default();
        let ctrl = scan_code("Key_LeftControl").unwrap();
        let five = scan_code("Key_5").unwrap();
        press_chord(&mut r, &[ctrl], five, Timing::default());
        assert_eq!(r.events, ["down 0x1d", "down 0x06", "up 0x06", "up 0x1d"]);
    }

    #[test]
    fn native_sink_is_a_real_adapter_on_every_desktop() {
        // Windows, Linux and macOS each get an adapter that injects input;
        // the sleeping no-op is only for hosts with nothing better.
        let name = std::any::type_name::<NativeSink>();
        assert!(!name.contains("Noop"), "NativeSink resolved to {name}");
    }

    #[test]
    fn typing_uses_shift_for_capitals_and_reports_what_it_cannot_type() {
        let mut r = Recorder::default();
        let skipped = type_text(&mut r, "Lhs 2ö", Timing::default());
        assert_eq!(skipped, vec!['ö']);
        // 'L' is shifted: shift down, L down/up, shift up.
        assert_eq!(
            &r.events[..4],
            ["down 0x2a", "down 0x26", "up 0x26", "up 0x2a"]
        );
        // 'h' is not.
        assert_eq!(&r.events[4..6], ["down 0x23", "up 0x23"]);
    }
}
