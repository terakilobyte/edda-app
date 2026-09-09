//! Spoken forms of Elite's procedurally generated star-system names.
//!
//! A TTS engine mangles "Wredguia WD-K d8-1"; a ship computer says
//! "Wredguia Whiskey Delta dash Kilo Delta 8 dash 1". The detection
//! pattern, the spell-out rules and the fix-ups are ported from EDDI
//! (github.com/EDCD/EDDI, `SpeechService/SpeechConversions`), Copyright
//! EDDI contributors, licensed Apache-2.0 — see docs/THIRD-PARTY.md.
//! Differences from the original: plain NATO words instead of SSML/IPA
//! phoneme markup (EDDA's voice backends speak plain text), and the
//! transform runs over whole callout sentences rather than marked names.

/// NATO word for one ASCII letter.
fn nato(c: char) -> Option<&'static str> {
    Some(match c.to_ascii_uppercase() {
        'A' => "Alpha", 'B' => "Bravo", 'C' => "Charlie", 'D' => "Delta",
        'E' => "Echo", 'F' => "Foxtrot", 'G' => "Golf", 'H' => "Hotel",
        'I' => "India", 'J' => "Juliett", 'K' => "Kilo", 'L' => "Lima",
        'M' => "Mike", 'N' => "November", 'O' => "Oscar", 'P' => "Papa",
        'Q' => "Quebec", 'R' => "Romeo", 'S' => "Sierra", 'T' => "Tango",
        'U' => "Uniform", 'V' => "Victor", 'W' => "Whiskey", 'X' => "X-ray",
        'Y' => "Yankee", 'Z' => "Zulu",
        _ => return None,
    })
}

/// Speak one procgen coordinate block (`WD-K d8-1`): NATO for every
/// letter, numbers whole, dashes said.
fn spell_coordinates(coordinates: &str) -> String {
    let mut words: Vec<String> = Vec::new();
    let mut digits = String::new();
    let flush = |digits: &mut String, words: &mut Vec<String>| {
        if !digits.is_empty() {
            words.push(std::mem::take(digits));
        }
    };
    for c in coordinates.chars() {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            flush(&mut digits, &mut words);
            match c {
                '-' => words.push("dash".into()),
                letter => {
                    if let Some(word) = nato(letter) {
                        words.push(word.into());
                    }
                }
            }
        }
    }
    flush(&mut digits, &mut words);
    words.join(" ")
}

/// Rewrite every procgen coordinate block inside `text` into its spoken
/// form, and apply the handful of named fix-ups. Prose is left alone: the
/// pattern (`XX-X` then a lowercase mass code `a`–`h` with digits) does
/// not occur in English sentences.
pub fn speak_system_names(text: &str) -> String {
    // Named systems TTS reliably mangles (ported fix-ups).
    let mut out = text.replace("Sagittarius A*", "Sagittarius A Star");
    if out.contains("VESPER-M4") {
        out = out.replace("VESPER-M4", "Vesper M 4");
    }

    let bytes = out.as_bytes();
    let mut result = String::with_capacity(out.len() + 32);
    let mut at = 0;
    while at < bytes.len() {
        // A block only starts on a word boundary: "ABWD-K d8" inside a
        // longer word is not a coordinate.
        let on_boundary = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        match on_boundary.then(|| coordinate_block(&out[at..])).flatten() {
            Some(len) => {
                result.push_str(&spell_coordinates(&out[at..at + len]));
                at += len;
            }
            None => {
                let c = out[at..].chars().next().unwrap();
                result.push(c);
                at += c.len_utf8();
            }
        }
    }
    result
}

/// Length of a procgen coordinate block starting exactly at the head of
/// `s` (`AB-C d12`, `AB-C d12-3`), or `None`. The block must start at a
/// word boundary (caller guarantees by scanning) and end at one.
fn coordinate_block(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    // Word boundary on the left is the caller's scan position; require
    // the previous emit not to end mid-word by checking here: two
    // uppercase letters, dash, uppercase letter, space, lowercase a-h.
    if b.len() < 8 {
        return None;
    }
    if !(b[0].is_ascii_uppercase() && b[1].is_ascii_uppercase() && b[2] == b'-'
        && b[3].is_ascii_uppercase() && b[4] == b' '
        && (b'a'..=b'h').contains(&b[5]))
    {
        return None;
    }
    let mut at = 6;
    let digits = |b: &[u8], mut at: usize| -> usize {
        while at < b.len() && b[at].is_ascii_digit() {
            at += 1;
        }
        at
    };
    let after_first = digits(b, at);
    if after_first == at {
        return None;
    }
    at = after_first;
    if at < b.len() && b[at] == b'-' {
        let after_second = digits(b, at + 1);
        if after_second > at + 1 {
            at = after_second;
        }
    }
    // Must end the word: end of string or a non-alphanumeric.
    if at < b.len() && (b[at].is_ascii_alphanumeric() || b[at] == b'-') {
        return None;
    }
    Some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procgen_names_are_spelled_nato_inside_sentences() {
        assert_eq!(
            speak_system_names("Arrived in Wredguia WD-K d8-1."),
            "Arrived in Wredguia Whiskey Delta dash Kilo Delta 8 dash 1."
        );
        assert_eq!(
            speak_system_names("Warning: Oevasy SG-Y d0 would be a fuel trap."),
            "Warning: Oevasy Sierra Golf dash Yankee Delta 0 would be a fuel trap."
        );
        assert_eq!(
            speak_system_names("Next: Hypiae Phyloi LR-C d22, 4 jumps remaining"),
            "Next: Hypiae Phyloi Lima Romeo dash Charlie Delta 22, 4 jumps remaining"
        );
        assert_eq!(
            speak_system_names("Synuefe XR-H d11-102 then Col 285 Sector IY-W b16-5."),
            "Synuefe X-ray Romeo dash Hotel Delta 11 dash 102 then Col 285 Sector India Yankee dash Whiskey Bravo 16 dash 5."
        );
    }

    #[test]
    fn named_systems_and_prose_pass_through() {
        assert_eq!(speak_system_names("Arrived in Sol."), "Arrived in Sol.");
        assert_eq!(
            speak_system_names("Docked at Jameson Memorial, Shinrarta Dezhra."),
            "Docked at Jameson Memorial, Shinrarta Dezhra."
        );
        // Hyphenated prose and version-ish tokens are not coordinates.
        assert_eq!(
            speak_system_names("A well-known KGB-FOAM mnemonic, USS-scan at 12-3."),
            "A well-known KGB-FOAM mnemonic, USS-scan at 12-3."
        );
        assert_eq!(speak_system_names("HIP 12345 is 20 ly out."), "HIP 12345 is 20 ly out.");
    }

    #[test]
    fn ported_fixups_apply() {
        assert_eq!(
            speak_system_names("Course set for Sagittarius A*."),
            "Course set for Sagittarius A Star."
        );
        assert_eq!(speak_system_names("VESPER-M4 ahead."), "Vesper M 4 ahead.");
    }

    #[test]
    fn body_suffixes_after_the_block_survive() {
        assert_eq!(
            speak_system_names("Scoop at Bleae Thua HH-U e6-5 A."),
            "Scoop at Bleae Thua Hotel Hotel dash Uniform Echo 6 dash 5 A."
        );
    }
}
