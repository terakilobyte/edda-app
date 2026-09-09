//! macOS adapter: CoreGraphics `CGEvent` key and mouse injection, plus the
//! on-screen window list for "what is in front".
//!
//! The game does not run on macOS, so this exists for the developer
//! loop: macros can be exercised end to end against any focused window.
//! Posting events needs the app to be trusted under System Settings >
//! Privacy & Security > Accessibility; without that the events are
//! silently dropped and every call here logs once.

use crate::keymap::mac_keycode;
use crate::keys::ScanCode;
use crate::send::KeySink;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::display::CGDisplay;
use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGRect};
use core_graphics::window::{kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly};
use std::time::Duration;

/// The macOS [`KeySink`].
#[derive(Default)]
pub struct MacSink;

fn source() -> Option<CGEventSource> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState).ok()
}

fn send(sc: ScanCode, down: bool) {
    let Some(code) = mac_keycode(sc) else {
        tracing::warn!(code = sc.code, extended = sc.extended, "no macOS keycode for scan code");
        return;
    };
    let Some(src) = source() else {
        tracing::warn!("could not create a CGEventSource");
        return;
    };
    match CGEvent::new_keyboard_event(src, code, down) {
        Ok(ev) => ev.post(CGEventTapLocation::HID),
        Err(()) => tracing::warn!(code, "CGEventCreateKeyboardEvent failed"),
    }
}

impl KeySink for MacSink {
    fn key_down(&mut self, sc: ScanCode) {
        send(sc, true);
    }
    fn key_up(&mut self, sc: ScanCode) {
        send(sc, false);
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

/// The frontmost normal window: (owner application, title, bounds).
fn front_window() -> Option<(String, String, CGRect)> {
    let list = CGDisplay::window_list_info(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements, None)?;
    for item in list.iter() {
        // SAFETY: CGWindowListCopyWindowInfo returns an array of CFDictionary.
        let dict: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(*item as *const _) };
        let layer = dict
            .find(CFString::from_static_string("kCGWindowLayer"))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .unwrap_or(-1);
        if layer != 0 {
            continue; // menu bar, dock, status items
        }
        let text = |key: &'static str| {
            dict.find(CFString::from_static_string(key))
                .and_then(|v| v.downcast::<CFString>())
                .map(|s| s.to_string())
                .unwrap_or_default()
        };
        let bounds = dict
            .find(CFString::from_static_string("kCGWindowBounds"))
            .and_then(|v| v.downcast::<CFDictionary>())
            .and_then(|d| CGRect::from_dict_representation(&d))?;
        return Some((text("kCGWindowOwnerName"), text("kCGWindowName"), bounds));
    }
    None
}

/// Title of the frontmost window, or its owning app's name when the
/// window is untitled.
pub fn foreground_title() -> String {
    match front_window() {
        Some((owner, title, _)) if title.is_empty() => owner,
        Some((_, title, _)) => title,
        None => String::new(),
    }
}

fn point_in_front(xf: f32, yf: f32) -> Option<CGPoint> {
    let (_, _, r) = front_window()?;
    Some(CGPoint::new(
        r.origin.x + r.size.width * xf.clamp(0.0, 1.0) as f64,
        r.origin.y + r.size.height * yf.clamp(0.0, 1.0) as f64,
    ))
}

fn mouse(kind: CGEventType, p: CGPoint) -> bool {
    let Some(src) = source() else { return false };
    match CGEvent::new_mouse_event(src, kind, p, CGMouseButton::Left) {
        Ok(ev) => {
            ev.post(CGEventTapLocation::HID);
            true
        }
        Err(()) => false,
    }
}

pub fn move_mouse_in_foreground(xf: f32, yf: f32) -> bool {
    point_in_front(xf, yf).is_some_and(|p| mouse(CGEventType::MouseMoved, p))
}

pub fn click_in_foreground(xf: f32, yf: f32) -> bool {
    let Some(p) = point_in_front(xf, yf) else { return false };
    if !mouse(CGEventType::MouseMoved, p) {
        return false;
    }
    std::thread::sleep(Duration::from_millis(60));
    let a = mouse(CGEventType::LeftMouseDown, p);
    std::thread::sleep(Duration::from_millis(50));
    let b = mouse(CGEventType::LeftMouseUp, p);
    a && b
}
