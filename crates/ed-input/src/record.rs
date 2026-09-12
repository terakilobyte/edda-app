//! Recording the commander's own keystrokes, so a macro can be taught by
//! doing it once instead of guessed at. A low-level keyboard hook (the
//! same one screen recorders use) captures scan codes with timing while
//! the game has focus; `stop` returns the events.
//!
//! Only key events are captured, only while recording, and the buffer is
//! handed back once -- nothing is stored or sent anywhere.

use crate::keys::ScanCode;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyEvent {
    pub sc: ScanCode,
    pub down: bool,
    /// Milliseconds since recording started.
    pub at_ms: u64,
    /// The game window was in the foreground: keys pressed in this app
    /// (or the Alt+Tab to get to the game) are not part of the recipe.
    pub game: bool,
}

/// A left click during recording, as a fraction of the foreground window.
#[derive(Debug, Clone, Copy)]
pub struct Click {
    pub xf: f32,
    pub yf: f32,
    pub at_ms: u64,
    /// The click landed on the game window (not on this app's Stop button).
    pub game: bool,
}

/// Whether the foreground window is Elite Dangerous.
#[cfg(windows)]
pub fn foreground_is_game() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW};
    // SAFETY: plain Win32 queries on the foreground window handle.
    unsafe {
        let h = GetForegroundWindow();
        if h.is_null() {
            return false;
        }
        let mut buf = [0u16; 128];
        let n = GetWindowTextW(h, buf.as_mut_ptr(), buf.len() as i32);
        let title = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
        title.starts_with("Elite - Dangerous")
    }
}

#[cfg(not(windows))]
pub fn foreground_is_game() -> bool {
    true
}

struct Recording {
    #[cfg_attr(not(windows), allow(dead_code))]
    started: Instant,
    clicks: Vec<Click>,
    events: Vec<KeyEvent>,
}

static REC: OnceLock<Mutex<Option<Recording>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Recording>> {
    REC.get_or_init(|| Mutex::new(None))
}

/// Copy of the events captured so far (recording continues).
pub fn peek() -> Vec<KeyEvent> {
    slot()
        .lock()
        .map(|r| r.as_ref().map(|x| x.events.clone()).unwrap_or_default())
        .unwrap_or_default()
}

/// Copy of the clicks captured so far (recording continues).
pub fn peek_clicks() -> Vec<Click> {
    slot()
        .lock()
        .map(|r| r.as_ref().map(|x| x.clicks.clone()).unwrap_or_default())
        .unwrap_or_default()
}

pub fn is_recording() -> bool {
    slot().lock().map(|r| r.is_some()).unwrap_or(false)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let k = &*(lparam as *const KBDLLHOOKSTRUCT);
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let up = matches!(wparam as u32, WM_KEYUP | WM_SYSKEYUP);
            if down || up {
                let sc = ScanCode {
                    code: k.scanCode as u16,
                    extended: k.flags & LLKHF_EXTENDED != 0,
                };
                if let Ok(mut r) = slot().lock() {
                    if let Some(rec) = r.as_mut() {
                        let at_ms = rec.started.elapsed().as_millis() as u64;
                        rec.events.push(KeyEvent {
                            sc,
                            down,
                            at_ms,
                            game: foreground_is_game(),
                        });
                    }
                }
                // Watched keys: fire on the press only (auto-repeat sends more downs).
                if let Ok(mut w) = watchers().lock() {
                    for wt in w.iter_mut() {
                        if wt.sc == sc {
                            if down && !wt.held {
                                (wt.f)();
                            }
                            wt.held = down;
                        }
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    /// Observe a key without consuming it; `f` runs on the hook thread, so
    /// it must only hand off (send on a channel, spawn a thread).
    pub fn watch(sc: ScanCode, f: Box<dyn Fn() + Send>) -> Result<(), String> {
        watchers()
            .lock()
            .map_err(|e| e.to_string())?
            .push(Watcher { sc, f, held: false });
        ensure_hook_thread()
    }

    pub fn unwatch_all() {
        if let Ok(mut w) = watchers().lock() {
            w.clear();
        }
    }

    fn ensure_hook_thread() -> Result<(), String> {
        if hook_thread().lock().map(|t| t.is_some()).unwrap_or(false) {
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel::<Result<u32, String>>();
        std::thread::spawn(move || {
            // SAFETY: a low-level hook needs a message loop on the installing
            // thread; this thread is dedicated to it.
            unsafe {
                let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), std::ptr::null_mut(), 0);
                if h.is_null() {
                    let _ = tx.send(Err("could not install the keyboard hook".into()));
                    return;
                }
                let hm = SetWindowsHookExW(
                    windows_sys::Win32::UI::WindowsAndMessaging::WH_MOUSE_LL,
                    Some(mouse_hook),
                    std::ptr::null_mut(),
                    0,
                );
                let tid = windows_sys::Win32::System::Threading::GetCurrentThreadId();
                *hook_thread().lock().unwrap_or_else(|e| e.into_inner()) = Some(tid);
                let _ = tx.send(Ok(tid));
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    if msg.message == WM_QUIT {
                        break;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                UnhookWindowsHookEx(h);
                if !hm.is_null() {
                    UnhookWindowsHookEx(hm);
                }
                *hook_thread().lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
        });
        rx.recv().map_err(|e| e.to_string())?.map(|_| ())
    }

    pub fn start() -> Result<(), String> {
        if is_recording() {
            return Err("already recording".into());
        }
        *slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(Recording {
            started: Instant::now(),
            events: Vec::new(),
            clicks: Vec::new(),
        });
        ensure_hook_thread()
    }

    unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, GetWindowRect, MSLLHOOKSTRUCT, WM_LBUTTONDOWN,
        };
        if code >= 0 && wparam as u32 == WM_LBUTTONDOWN {
            let m = &*(lparam as *const MSLLHOOKSTRUCT);
            let h = GetForegroundWindow();
            let mut r = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            if !h.is_null() && GetWindowRect(h, &mut r) != 0 && r.right > r.left && r.bottom > r.top
            {
                let xf = (m.pt.x - r.left) as f32 / (r.right - r.left) as f32;
                let yf = (m.pt.y - r.top) as f32 / (r.bottom - r.top) as f32;
                if let Ok(mut s) = slot().lock() {
                    if let Some(rec) = s.as_mut() {
                        let at_ms = rec.started.elapsed().as_millis() as u64;
                        rec.clicks.push(Click {
                            xf,
                            yf,
                            at_ms,
                            game: foreground_is_game(),
                        });
                    }
                }
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    /// Stop recording: the key events and the clicks.
    pub fn stop_with_clicks() -> (Vec<KeyEvent>, Vec<Click>) {
        let rec = slot().lock().unwrap_or_else(|e| e.into_inner()).take();
        let watching = watchers().lock().map(|w| !w.is_empty()).unwrap_or(false);
        if !watching {
            if let Some(tid) = hook_thread()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                // SAFETY: posting WM_QUIT to the hook thread ends its loop.
                unsafe {
                    PostThreadMessageW(tid, WM_QUIT, 0, 0);
                }
            }
        }
        rec.map(|r| (r.events, r.clicks)).unwrap_or_default()
    }

    #[allow(dead_code)]
    fn start_legacy() -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel::<Result<u32, String>>();
        std::thread::spawn(move || {
            // SAFETY: a low-level hook needs a message loop on the installing
            // thread; this thread is dedicated to it and unhooks before exit.
            unsafe {
                let h = SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), std::ptr::null_mut(), 0);
                if h.is_null() {
                    let _ = tx.send(Err("could not install the keyboard hook".into()));
                    return;
                }
                let tid = windows_sys::Win32::System::Threading::GetCurrentThreadId();
                *slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(Recording {
                    started: Instant::now(),
                    events: Vec::new(),
                    clicks: Vec::new(),
                });
                let _ = tx.send(Ok(tid));
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    if msg.message == WM_QUIT {
                        break;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                UnhookWindowsHookEx(h);
            }
        });
        rx.recv().map_err(|e| e.to_string())?.map(|_| ())
    }

    pub fn stop() -> Vec<KeyEvent> {
        stop_with_clicks().0
    }

    struct Watcher {
        sc: ScanCode,
        f: Box<dyn Fn() + Send>,
        held: bool,
    }

    fn watchers() -> &'static std::sync::Mutex<Vec<Watcher>> {
        static W: std::sync::OnceLock<std::sync::Mutex<Vec<Watcher>>> = std::sync::OnceLock::new();
        W.get_or_init(|| std::sync::Mutex::new(Vec::new()))
    }

    fn hook_thread() -> &'static std::sync::Mutex<Option<u32>> {
        static T: std::sync::OnceLock<std::sync::Mutex<Option<u32>>> = std::sync::OnceLock::new();
        T.get_or_init(|| std::sync::Mutex::new(None))
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn start() -> Result<(), String> {
        Err("recording is Windows-only".into())
    }
    pub fn stop() -> Vec<KeyEvent> {
        Vec::new()
    }
    pub fn stop_with_clicks() -> (Vec<KeyEvent>, Vec<Click>) {
        (Vec::new(), Vec::new())
    }
    pub fn watch(_sc: ScanCode, _f: Box<dyn Fn() + Send>) -> Result<(), String> {
        Err("key watching is Windows-only".into())
    }
    pub fn unwatch_all() {}
}

pub use imp::{start, stop, stop_with_clicks, unwatch_all, watch};

/// Binds-style key name for a scan code, if we know it.
pub fn key_name(sc: ScanCode) -> Option<&'static str> {
    const NAMES: &[&str] = &[
        "Escape",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "0",
        "Minus",
        "Equals",
        "Backspace",
        "Tab",
        "Q",
        "W",
        "E",
        "R",
        "T",
        "Y",
        "U",
        "I",
        "O",
        "P",
        "LeftBracket",
        "RightBracket",
        "Enter",
        "LeftControl",
        "A",
        "S",
        "D",
        "F",
        "G",
        "H",
        "J",
        "K",
        "L",
        "SemiColon",
        "Apostrophe",
        "Grave",
        "LeftShift",
        "Backslash",
        "Z",
        "X",
        "C",
        "V",
        "B",
        "N",
        "M",
        "Comma",
        "Period",
        "Slash",
        "RightShift",
        "Numpad_Multiply",
        "LeftAlt",
        "Space",
        "CapsLock",
        "F1",
        "F2",
        "F3",
        "F4",
        "F5",
        "F6",
        "F7",
        "F8",
        "F9",
        "F10",
        "F11",
        "F12",
        "NumLock",
        "ScrollLock",
        "Numpad_7",
        "Numpad_8",
        "Numpad_9",
        "Numpad_Subtract",
        "Numpad_4",
        "Numpad_5",
        "Numpad_6",
        "Numpad_Add",
        "Numpad_1",
        "Numpad_2",
        "Numpad_3",
        "Numpad_0",
        "Numpad_Decimal",
        "Numpad_Enter",
        "RightControl",
        "Numpad_Divide",
        "RightAlt",
        "Home",
        "UpArrow",
        "PageUp",
        "LeftArrow",
        "RightArrow",
        "End",
        "DownArrow",
        "PageDown",
        "Insert",
        "Delete",
        "Pause",
    ];
    NAMES
        .iter()
        .copied()
        .find(|n| crate::keys::scan_code(n) == Some(sc))
}
