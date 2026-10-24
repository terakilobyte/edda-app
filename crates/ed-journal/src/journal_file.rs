//! Journal file names, and the order they belong in.
//!
//! The game has used two name formats: `Journal.YYMMDDHHMMSS.NN.log`
//! and `Journal.YYYY-MM-DDTHHMMSS.NN.log`. The four-digit form arrived
//! with Odyssey Update 11 in March 2022, and the two did NOT change over
//! on a date — they ran side by side, the name following which client
//! the commander launched. EDMarketConnector's monitor.py records the
//! pair from the same day: "Odyssey Update 11 has, e.g.
//! Journal.2022-03-15T152503.01.log; Horizons Update 11 equivalent:
//! Journal.220315152335.01.log; So we can no longer use a naive sort." Compared as plain strings the two
//! do not interleave chronologically: `Journal.22...` sorts after
//! `Journal.2026-...` because `'2' > '0'` at the ninth character. A
//! commander who played through the change therefore had 2021 and 2022
//! replayed AFTER 2026 by anything that trusted name order, and read as
//! still flying a 2022 ship (tester, 2026-09-15). `sort_key` puts both
//! formats on one chronological axis.

/// A key that sorts journal file names chronologically across both name
/// formats: `YYYY-MM-DDTHHMMSS.NN` plus any archive suffix. Names that fit
/// neither format sort after every dated one, among themselves by name.
///
/// The ingest's truncation archive renames `Journal.X.NN.log` to
/// `Journal.X.NN.a.log` and relies on the archive sorting BEFORE the
/// original; the key keeps that (`.a` before `.z`).
pub fn sort_key(name: &str) -> String {
    // Only the live client's names. JournalAlpha./JournalBeta. are
    // deliberately NOT recognised here — see `journal_files`.
    let Some(stem) = name.strip_prefix("Journal.").and_then(|s| s.strip_suffix(".log")) else {
        return format!("~{name}");
    };
    let mut parts = stem.splitn(3, '.');
    let (Some(date), Some(part)) = (parts.next(), parts.next()) else {
        return format!("~{name}");
    };
    let suffix = parts.next().unwrap_or("z");
    let canonical = if date.len() == 12 && date.bytes().all(|b| b.is_ascii_digit()) {
        // YYMMDDHHMMSS: the game's journals begin in 3300 (2014), so the
        // century is not in doubt.
        format!("20{}-{}-{}T{}", &date[0..2], &date[2..4], &date[4..6], &date[6..12])
    } else if is_new_format(date) {
        date.to_string()
    } else {
        return format!("~{name}");
    };
    // The part number is a counter, so it must compare as a number and
    // not as text: unpadded, "100" sorts before "99". The game has only
    // ever been seen to write two digits (EDMC's regex requires exactly
    // two), so this is belt and braces rather than an observed failure.
    format!("{canonical}.{:0>4}.{suffix}", part)
}

/// `YYYY-MM-DDTHHMMSS`, digits and separators in the right places.
fn is_new_format(date: &str) -> bool {
    let b = date.as_bytes();
    b.len() == 17
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b.iter().enumerate().all(|(i, c)| matches!(i, 4 | 7 | 10) || c.is_ascii_digit())
}

/// Who a journal file belongs to. The game writes one file per launch and
/// names the account in its `Commander` line (and again in `LoadGame`),
/// within the first ~45 lines; `Friends` and `Music` can precede it. A
/// file never changes hands (0 of a two-account player's 1,396 files did,
/// 2026-10-24): the first FID is the owner for the file's life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Owner {
    /// The account's Frontier ID, e.g. `F1234567`.
    Fid(String),
    /// No FID yet and the file has not ended: read it again later.
    Pending,
    /// Ended (`Shutdown`) without ever naming a commander: the launcher
    /// was opened and closed at the menu. Nothing of value inside.
    MenuOnly,
}

/// Decide the owner from a file's lines, in order. Unparsable lines are
/// skipped, as the ingest skips them.
pub fn owner_of_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Owner {
    let mut last_event: Option<String> = None;
    for line in lines {
        let text = line.trim().trim_start_matches('\u{feff}');
        if text.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
            continue;
        };
        let Some(event) = v.get("event").and_then(|e| e.as_str()) else {
            continue;
        };
        if matches!(event, "Commander" | "LoadGame") {
            if let Some(fid) = v.get("FID").and_then(|f| f.as_str()).filter(|f| !f.is_empty()) {
                return Owner::Fid(fid.to_string());
            }
        }
        last_event = Some(event.to_string());
    }
    if last_event.as_deref() == Some("Shutdown") {
        Owner::MenuOnly
    } else {
        Owner::Pending
    }
}

/// `owner_of_lines` over a file on disk. Stops at the first FID, so an
/// active file costs its first few kilobytes; a menu-only file is small.
pub fn owner(path: &std::path::Path) -> std::io::Result<Owner> {
    use std::io::BufRead as _;
    let f = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(f);
    let mut last_event: Option<String> = None;
    for line in reader.split(b'\n') {
        let line = line?;
        let Ok(text) = std::str::from_utf8(&line) else { continue };
        match owner_of_lines([text]) {
            Owner::Fid(fid) => return Ok(Owner::Fid(fid)),
            Owner::MenuOnly => last_event = Some("Shutdown".into()),
            Owner::Pending => {
                let t = text.trim().trim_start_matches('\u{feff}');
                if !t.is_empty() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
                        if let Some(e) = v.get("event").and_then(|e| e.as_str()) {
                            last_event = Some(e.to_string());
                        }
                    }
                }
            }
        }
    }
    Ok(if last_event.as_deref() == Some("Shutdown") { Owner::MenuOnly } else { Owner::Pending })
}

#[cfg(test)]
mod tests {
    use super::sort_key;

    /// The failure that started this: a plain string sort puts 2021 and 2022
    /// after 2026. The key does not.
    #[test]
    fn both_name_formats_sort_on_one_time_axis() {
        let mut names = vec![
            "Journal.2026-09-15T090000.01.log",
            "Journal.221231235959.01.log",
            "Journal.2024-01-01T120000.01.log",
            "Journal.200101000000.01.log",
            "Journal.210615080000.02.log",
            "Journal.210615080000.01.log",
        ];
        let plain = { let mut v = names.clone(); v.sort(); v };
        assert_eq!(plain[5], "Journal.221231235959.01.log", "plain sort: 2022 last, which is the bug");
        names.sort_by_key(|n| sort_key(n));
        assert_eq!(
            names,
            vec![
                "Journal.200101000000.01.log",
                "Journal.210615080000.01.log",
                "Journal.210615080000.02.log",
                "Journal.221231235959.01.log",
                "Journal.2024-01-01T120000.01.log",
                "Journal.2026-09-15T090000.01.log",
            ]
        );
    }

    /// The two formats OVERLAP in time, so the key must interleave them
    /// rather than segregate them.
    ///
    /// Sourced, not assumed: EDMarketConnector's monitor.py carries the
    /// pair as a comment — "Odyssey Update 11 has, e.g.
    /// Journal.2022-03-15T152503.01.log; Horizons Update 11 equivalent:
    /// Journal.220315152335.01.log; So we can no longer use a naive
    /// sort." Both were current on the same day in March 2022: the
    /// format followed which CLIENT the commander launched, not the
    /// date. Anything that merely put every two-digit-year name before
    /// every four-digit one would order this pair correctly by luck and
    /// the reverse pair wrongly, so the key normalises instead. (EDMC
    /// sidesteps the question by sorting on file ctime; we cannot,
    /// because we replay archived folders where ctime is the copy date.)
    #[test]
    fn the_two_formats_interleave_within_one_day() {
        // Horizons 15:23:35, then Odyssey 15:25:03, same afternoon.
        let horizons = "Journal.220315152335.01.log";
        let odyssey = "Journal.2022-03-15T152503.01.log";
        assert!(
            sort_key(horizons) < sort_key(odyssey),
            "the earlier file must sort first regardless of which format it is: {} vs {}",
            sort_key(horizons),
            sort_key(odyssey)
        );
        // And the other way round: an Odyssey file from the morning
        // precedes a Horizons file from the evening of the same day.
        assert!(
            sort_key("Journal.2022-03-15T090000.01.log") < sort_key("Journal.220315210000.01.log"),
            "a two-digit-year name is not automatically older"
        );
        // A whole day of mixed clients lands in clock order.
        let mut names = vec![
            "Journal.220315210000.01.log",
            "Journal.2022-03-15T152503.01.log",
            "Journal.220315152335.01.log",
            "Journal.2022-03-15T090000.01.log",
        ];
        names.sort_by_key(|n| sort_key(n));
        assert_eq!(
            names,
            vec![
                "Journal.2022-03-15T090000.01.log",
                "Journal.220315152335.01.log",
                "Journal.2022-03-15T152503.01.log",
                "Journal.220315210000.01.log",
            ]
        );
    }

    /// A counter compares as a number, not as text: unpadded, 100 would
    /// sort before 99. Not observed from the game, which writes two
    /// digits, but free to get right.
    #[test]
    fn the_part_number_compares_as_a_number() {
        let mut names = vec![
            "Journal.2026-09-15T090000.99.log",
            "Journal.2026-09-15T090000.100.log",
            "Journal.2026-09-15T090000.02.log",
        ];
        names.sort_by_key(|n| sort_key(n));
        assert_eq!(
            names,
            vec![
                "Journal.2026-09-15T090000.02.log",
                "Journal.2026-09-15T090000.99.log",
                "Journal.2026-09-15T090000.100.log",
            ]
        );
    }

    /// The truncation archive must still land before the file it was cut from.
    #[test]
    fn an_archived_file_sorts_before_its_original() {
        assert!(sort_key("Journal.2026-08-01T100000.01.a.log") < sort_key("Journal.2026-08-01T100000.01.log"));
        assert!(sort_key("Journal.220801100000.01.a.log") < sort_key("Journal.220801100000.01.log"));
    }

    /// A name in neither format is not guessed at; it sorts after every dated
    /// one and stays stable among its kind.
    #[test]
    fn unknown_names_sort_last_and_stable() {
        assert!(sort_key("Journal.weird.log") > sort_key("Journal.2026-09-15T090000.01.log"));
        assert!(sort_key("Journal.weird.log") > sort_key("Journal.221231235959.01.log"));
        assert!(sort_key("Journal.a.log") < sort_key("Journal.b.log"));
        // The canonical form, spelled out: normalised date, the part
        // zero-padded so it compares as a number, and `z` standing in for
        // the absent archive suffix so a real one sorts first.
        assert_eq!(sort_key("Journal.221231235959.01.log"), "2022-12-31T235959.0001.z");
    }
}

#[cfg(test)]
mod owner_tests {
    use super::*;

    const HEAD: &str = r#"{"timestamp":"2026-10-17T18:06:48Z","event":"Fileheader","part":1,"language":"English/UK","Odyssey":true,"gameversion":"4.4.2.0","build":"r318000/r0 "}"#;
    const FRIEND: &str = r#"{"timestamp":"2026-10-17T18:06:50Z","event":"Friends","Status":"Online","Name":"Someone"}"#;
    const CMDR_A: &str = r#"{"timestamp":"2026-10-17T18:06:55Z","event":"Commander","FID":"F0000001","Name":"Alpha"}"#;
    const LOAD_A: &str = r#"{"timestamp":"2026-10-17T18:06:56Z","event":"LoadGame","FID":"F0000001","Commander":"Alpha","Ship":"Mandalay","ShipID":31,"Credits":1000}"#;
    const CMDR_B: &str = r#"{"timestamp":"2026-10-17T18:30:00Z","event":"Commander","FID":"F0000002","Name":"Beta"}"#;
    const SHUTDOWN: &str = r#"{"timestamp":"2026-10-17T18:07:00Z","event":"Shutdown"}"#;

    #[test]
    fn the_first_commander_line_names_the_owner() {
        assert_eq!(owner_of_lines([HEAD, FRIEND, FRIEND, CMDR_A, LOAD_A]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn a_loadgame_line_names_the_owner_when_commander_is_missing() {
        assert_eq!(owner_of_lines([HEAD, LOAD_A]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn a_second_commander_event_with_the_same_fid_is_the_same_owner() {
        // A relog to the menu and back writes Commander again (1,492 times in
        // the donor data); the file stays one commander's.
        assert_eq!(owner_of_lines([HEAD, CMDR_A, LOAD_A, CMDR_A, LOAD_A]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn the_first_fid_wins_even_if_a_later_line_names_another() {
        // Never observed (0 of 1,396 files); the ingest refuses the later
        // lines (Task 2). The owner is still the first.
        assert_eq!(owner_of_lines([HEAD, CMDR_A, CMDR_B]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn a_file_that_shut_down_without_a_commander_is_menu_only() {
        assert_eq!(owner_of_lines([HEAD, FRIEND, SHUTDOWN]), Owner::MenuOnly);
    }

    #[test]
    fn a_file_without_a_commander_that_has_not_shut_down_is_pending() {
        assert_eq!(owner_of_lines([HEAD, FRIEND]), Owner::Pending);
        assert_eq!(owner_of_lines(Vec::<&str>::new()), Owner::Pending);
    }

    #[test]
    fn junk_lines_are_skipped() {
        assert_eq!(owner_of_lines([HEAD, "not json", "", "\u{feff}", CMDR_A]), Owner::Fid("F0000001".into()));
    }

    #[test]
    fn owner_reads_a_file_from_disk() {
        let dir = std::env::temp_dir().join(format!("edda-owner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("Journal.2026-10-17T180648.01.log");
        std::fs::write(&p, format!("{HEAD}\n{FRIEND}\n{CMDR_A}\n{LOAD_A}\n")).unwrap();
        assert_eq!(owner(&p).unwrap(), Owner::Fid("F0000001".into()));
        std::fs::write(&p, format!("{HEAD}\n{SHUTDOWN}\n")).unwrap();
        assert_eq!(owner(&p).unwrap(), Owner::MenuOnly);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
