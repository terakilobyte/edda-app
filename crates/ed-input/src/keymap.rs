//! Set-1 scan codes translated for the hosts that do not speak them.
//!
//! [`crate::keys`] resolves binds-file names to the PC/AT set-1 scan codes
//! the game reads on Windows. Linux (evdev, and so Proton) and macOS
//! (CoreGraphics virtual key codes) number the same physical keys
//! differently; the X keysym column feeds the `xdotool` fallback. One
//! table, three lookups, no `cfg`: every host can test all three.

use crate::keys::ScanCode;

/// (scan code, extended, evdev KEY_* code, macOS CGKeyCode, X keysym).
const TABLE: &[(u16, bool, u16, u16, &str)] = &[
    (0x01, false, 1, 53, "Escape"),
    (0x02, false, 2, 18, "1"), (0x03, false, 3, 19, "2"), (0x04, false, 4, 20, "3"),
    (0x05, false, 5, 21, "4"), (0x06, false, 6, 23, "5"), (0x07, false, 7, 22, "6"),
    (0x08, false, 8, 26, "7"), (0x09, false, 9, 28, "8"), (0x0A, false, 10, 25, "9"),
    (0x0B, false, 11, 29, "0"),
    (0x0C, false, 12, 27, "minus"), (0x0D, false, 13, 24, "equal"),
    (0x0E, false, 14, 51, "BackSpace"), (0x0F, false, 15, 48, "Tab"),
    (0x10, false, 16, 12, "q"), (0x11, false, 17, 13, "w"), (0x12, false, 18, 14, "e"),
    (0x13, false, 19, 15, "r"), (0x14, false, 20, 17, "t"), (0x15, false, 21, 16, "y"),
    (0x16, false, 22, 32, "u"), (0x17, false, 23, 34, "i"), (0x18, false, 24, 31, "o"),
    (0x19, false, 25, 35, "p"),
    (0x1A, false, 26, 33, "bracketleft"), (0x1B, false, 27, 30, "bracketright"),
    (0x1C, false, 28, 36, "Return"), (0x1D, false, 29, 59, "Control_L"),
    (0x1E, false, 30, 0, "a"), (0x1F, false, 31, 1, "s"), (0x20, false, 32, 2, "d"),
    (0x21, false, 33, 3, "f"), (0x22, false, 34, 5, "g"), (0x23, false, 35, 4, "h"),
    (0x24, false, 36, 38, "j"), (0x25, false, 37, 40, "k"), (0x26, false, 38, 37, "l"),
    (0x27, false, 39, 41, "semicolon"), (0x28, false, 40, 39, "apostrophe"),
    (0x29, false, 41, 50, "grave"), (0x2A, false, 42, 56, "Shift_L"),
    (0x2B, false, 43, 42, "backslash"),
    (0x2C, false, 44, 6, "z"), (0x2D, false, 45, 7, "x"), (0x2E, false, 46, 8, "c"),
    (0x2F, false, 47, 9, "v"), (0x30, false, 48, 11, "b"), (0x31, false, 49, 45, "n"),
    (0x32, false, 50, 46, "m"), (0x33, false, 51, 43, "comma"), (0x34, false, 52, 47, "period"),
    (0x35, false, 53, 44, "slash"), (0x36, false, 54, 60, "Shift_R"),
    (0x37, false, 55, 67, "KP_Multiply"), (0x38, false, 56, 58, "Alt_L"),
    (0x39, false, 57, 49, "space"), (0x3A, false, 58, 57, "Caps_Lock"),
    (0x3B, false, 59, 122, "F1"), (0x3C, false, 60, 120, "F2"), (0x3D, false, 61, 99, "F3"),
    (0x3E, false, 62, 118, "F4"), (0x3F, false, 63, 96, "F5"), (0x40, false, 64, 97, "F6"),
    (0x41, false, 65, 98, "F7"), (0x42, false, 66, 100, "F8"), (0x43, false, 67, 101, "F9"),
    (0x44, false, 68, 109, "F10"),
    (0x45, false, 69, 71, "Num_Lock"), (0x46, false, 70, 107, "Scroll_Lock"),
    (0x47, false, 71, 89, "KP_7"), (0x48, false, 72, 91, "KP_8"), (0x49, false, 73, 92, "KP_9"),
    (0x4A, false, 74, 78, "KP_Subtract"),
    (0x4B, false, 75, 86, "KP_4"), (0x4C, false, 76, 87, "KP_5"), (0x4D, false, 77, 88, "KP_6"),
    (0x4E, false, 78, 69, "KP_Add"),
    (0x4F, false, 79, 83, "KP_1"), (0x50, false, 80, 84, "KP_2"), (0x51, false, 81, 85, "KP_3"),
    (0x52, false, 82, 82, "KP_0"), (0x53, false, 83, 65, "KP_Decimal"),
    (0x57, false, 87, 103, "F11"), (0x58, false, 88, 111, "F12"),
    // Extended (E0) keys: the one place the evdev numbering diverges.
    (0x1C, true, 96, 76, "KP_Enter"), (0x1D, true, 97, 62, "Control_R"),
    (0x35, true, 98, 75, "KP_Divide"), (0x38, true, 100, 61, "Alt_R"),
    (0x47, true, 102, 115, "Home"), (0x48, true, 103, 126, "Up"), (0x49, true, 104, 116, "Prior"),
    (0x4B, true, 105, 123, "Left"), (0x4D, true, 106, 124, "Right"), (0x4F, true, 107, 119, "End"),
    (0x50, true, 108, 125, "Down"), (0x51, true, 109, 121, "Next"), (0x52, true, 110, 114, "Insert"),
    (0x53, true, 111, 117, "Delete"), (0x5B, true, 125, 55, "Super_L"), (0x5C, true, 126, 54, "Super_R"),
];

fn row(sc: ScanCode) -> Option<&'static (u16, bool, u16, u16, &'static str)> {
    TABLE.iter().find(|r| r.0 == sc.code && r.1 == sc.extended)
}

/// Linux `KEY_*` code (what `uinput` emits and Proton's DirectInput reads).
pub fn evdev_code(sc: ScanCode) -> Option<u16> {
    row(sc).map(|r| r.2)
}

/// macOS virtual key code for `CGEventCreateKeyboardEvent` (ANSI US).
pub fn mac_keycode(sc: ScanCode) -> Option<u16> {
    row(sc).map(|r| r.3)
}

/// X11 keysym name, as `xdotool key` spells it.
pub fn x_keysym(sc: ScanCode) -> Option<&'static str> {
    row(sc).map(|r| r.4)
}

/// Every evdev code this crate can emit: what a uinput device must
/// advertise before the kernel will accept a press for it.
pub fn all_evdev_codes() -> impl Iterator<Item = u16> {
    TABLE.iter().map(|r| r.2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::scan_code;

    #[test]
    fn every_known_key_name_has_a_row_on_every_host() {
        // Every name keys.rs resolves must be pressable on all three hosts,
        // or a bind that works on Windows silently does nothing elsewhere.
        for name in [
            "Key_Escape", "Key_A", "Key_5", "Key_Enter", "Key_LeftControl", "Key_F12", "Key_Numpad_Decimal",
            "Key_Numpad_Enter", "Key_RightControl", "Key_Numpad_Divide", "Key_RightAlt", "Key_Home", "Key_UpArrow",
            "Key_PageUp", "Key_LeftArrow", "Key_RightArrow", "Key_End", "Key_DownArrow", "Key_PageDown",
            "Key_Insert", "Key_Delete", "Key_LeftWin", "Key_RightWin", "Key_Space", "Key_Grave",
        ] {
            let sc = scan_code(name).unwrap();
            assert!(evdev_code(sc).is_some(), "{name}: no evdev code");
            assert!(mac_keycode(sc).is_some(), "{name}: no mac keycode");
            assert!(x_keysym(sc).is_some(), "{name}: no X keysym");
        }
    }

    #[test]
    fn plain_scan_codes_are_evdev_codes_and_extended_ones_are_not() {
        // Linux numbered its keys after set 1, so the non-E0 block is the
        // identity; E0 keys got their own numbers.
        let v = scan_code("Key_V").unwrap();
        assert_eq!(evdev_code(v), Some(0x2F));
        let down = scan_code("Key_DownArrow").unwrap();
        assert_eq!(evdev_code(down), Some(108));
        let kp_enter = scan_code("Key_Numpad_Enter").unwrap();
        assert_eq!(evdev_code(kp_enter), Some(96));
        assert_eq!(evdev_code(scan_code("Key_Enter").unwrap()), Some(28));
    }

    #[test]
    fn mac_and_x_use_their_own_numbering() {
        let v = scan_code("Key_V").unwrap();
        assert_eq!(mac_keycode(v), Some(9));
        assert_eq!(x_keysym(v), Some("v"));
        let down = scan_code("Key_DownArrow").unwrap();
        assert_eq!(mac_keycode(down), Some(125));
        assert_eq!(x_keysym(down), Some("Down"));
        assert_eq!(mac_keycode(scan_code("Key_LeftControl").unwrap()), Some(59));
        assert_eq!(x_keysym(scan_code("Key_LeftControl").unwrap()), Some("Control_L"));
    }
}
