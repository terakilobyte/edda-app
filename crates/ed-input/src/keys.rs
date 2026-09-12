//! `Key_*` names as the binds file spells them, mapped to PC/AT set-1 scan
//! codes -- the codes `SendInput` needs for DirectInput to see the press.
//!
//! Extended keys (arrows, navigation cluster, right-hand modifiers, numpad
//! Enter/divide) carry the E0 prefix and are flagged here so the sender
//! sets `KEYEVENTF_EXTENDEDKEY`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanCode {
    pub code: u16,
    pub extended: bool,
}

const fn sc(code: u16) -> ScanCode {
    ScanCode {
        code,
        extended: false,
    }
}
const fn ext(code: u16) -> ScanCode {
    ScanCode {
        code,
        extended: true,
    }
}

/// Scan code for a binds-file key name. `None` for names we do not know,
/// which the caller must treat as "cannot press", never as a guess.
pub fn scan_code(key: &str) -> Option<ScanCode> {
    let k = key.strip_prefix("Key_").unwrap_or(key);
    Some(match k {
        "Escape" => sc(0x01),
        "1" => sc(0x02),
        "2" => sc(0x03),
        "3" => sc(0x04),
        "4" => sc(0x05),
        "5" => sc(0x06),
        "6" => sc(0x07),
        "7" => sc(0x08),
        "8" => sc(0x09),
        "9" => sc(0x0A),
        "0" => sc(0x0B),
        "Minus" => sc(0x0C),
        "Equals" | "Plus" => sc(0x0D),
        "Backspace" => sc(0x0E),
        "Tab" => sc(0x0F),
        "Q" => sc(0x10),
        "W" => sc(0x11),
        "E" => sc(0x12),
        "R" => sc(0x13),
        "T" => sc(0x14),
        "Y" => sc(0x15),
        "U" => sc(0x16),
        "I" => sc(0x17),
        "O" => sc(0x18),
        "P" => sc(0x19),
        "LeftBracket" => sc(0x1A),
        "RightBracket" => sc(0x1B),
        "Enter" => sc(0x1C),
        "LeftControl" => sc(0x1D),
        "A" => sc(0x1E),
        "S" => sc(0x1F),
        "D" => sc(0x20),
        "F" => sc(0x21),
        "G" => sc(0x22),
        "H" => sc(0x23),
        "J" => sc(0x24),
        "K" => sc(0x25),
        "L" => sc(0x26),
        "SemiColon" => sc(0x27),
        "Apostrophe" => sc(0x28),
        "Grave" => sc(0x29),
        "LeftShift" => sc(0x2A),
        "BackSlash" => sc(0x2B),
        "Z" => sc(0x2C),
        "X" => sc(0x2D),
        "C" => sc(0x2E),
        "V" => sc(0x2F),
        "B" => sc(0x30),
        "N" => sc(0x31),
        "M" => sc(0x32),
        "Comma" => sc(0x33),
        "Period" => sc(0x34),
        "Slash" => sc(0x35),
        "RightShift" => sc(0x36),
        "Numpad_Multiply" => sc(0x37),
        "LeftAlt" => sc(0x38),
        "Space" => sc(0x39),
        "CapsLock" => sc(0x3A),
        "F1" => sc(0x3B),
        "F2" => sc(0x3C),
        "F3" => sc(0x3D),
        "F4" => sc(0x3E),
        "F5" => sc(0x3F),
        "F6" => sc(0x40),
        "F7" => sc(0x41),
        "F8" => sc(0x42),
        "F9" => sc(0x43),
        "F10" => sc(0x44),
        "NumLock" => sc(0x45),
        "ScrollLock" => sc(0x46),
        "Numpad_7" => sc(0x47),
        "Numpad_8" => sc(0x48),
        "Numpad_9" => sc(0x49),
        "Numpad_Subtract" => sc(0x4A),
        "Numpad_4" => sc(0x4B),
        "Numpad_5" => sc(0x4C),
        "Numpad_6" => sc(0x4D),
        "Numpad_Add" => sc(0x4E),
        "Numpad_1" => sc(0x4F),
        "Numpad_2" => sc(0x50),
        "Numpad_3" => sc(0x51),
        "Numpad_0" => sc(0x52),
        "Numpad_Decimal" => sc(0x53),
        "F11" => sc(0x57),
        "F12" => sc(0x58),
        // Extended (E0) keys.
        "Numpad_Enter" => ext(0x1C),
        "RightControl" => ext(0x1D),
        "Numpad_Divide" => ext(0x35),
        "RightAlt" => ext(0x38),
        "Home" => ext(0x47),
        "UpArrow" => ext(0x48),
        "PageUp" => ext(0x49),
        "LeftArrow" => ext(0x4B),
        "RightArrow" => ext(0x4D),
        "End" => ext(0x4F),
        "DownArrow" => ext(0x50),
        "PageDown" => ext(0x51),
        "Insert" => ext(0x52),
        "Delete" => ext(0x53),
        "LeftWin" => ext(0x5B),
        "RightWin" => ext(0x5C),
        _ => return None,
    })
}

/// Scan code for a character to type into a text field (galaxy map search).
/// US layout, which is what the binds file declares (`en-US`); letters are
/// lower-case presses, upper-case adds shift.
pub fn char_key(c: char) -> Option<(ScanCode, bool)> {
    let upper = c.is_ascii_uppercase();
    let name = match c.to_ascii_lowercase() {
        'a'..='z' => c.to_ascii_uppercase().to_string(),
        '0'..='9' => c.to_string(),
        ' ' => "Space".into(),
        '-' => "Minus".into(),
        '\'' => "Apostrophe".into(),
        '.' => "Period".into(),
        ',' => "Comma".into(),
        '/' => "Slash".into(),
        _ => return None,
    };
    scan_code(&name).map(|s| (s, upper))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_names_resolve_and_unknowns_do_not() {
        assert_eq!(scan_code("Key_H"), Some(sc(0x23)));
        assert_eq!(scan_code("Key_LeftShift"), Some(sc(0x2A)));
        assert_eq!(scan_code("Key_Numpad_0"), Some(sc(0x52)));
        assert_eq!(scan_code("Key_UpArrow"), Some(ext(0x48)));
        assert_eq!(scan_code("Key_Bogus"), None);
    }

    #[test]
    fn typing_a_system_name_is_expressible() {
        for c in "LHS 20 Col 285 Sector KH-A b15-4 Jackson's".chars() {
            assert!(char_key(c).is_some(), "{c:?}");
        }
        assert!(char_key('A').unwrap().1);
        assert!(!char_key('a').unwrap().1);
    }
}
