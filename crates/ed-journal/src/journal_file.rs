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
    // The alpha and beta clients write JournalAlpha./JournalBeta. — a
    // commander who took part in an Odyssey alpha has them in the same
    // folder. Unrecognised names sort after every dated one, so leaving
    // these out replayed a 2021 alpha as the newest flight: the same
    // failure the two name formats caused, wearing a different prefix.
    // EDMarketConnector matches `Journal(Alpha|Beta)?` for this reason.
    let stem = ["Journal.", "JournalAlpha.", "JournalBeta."]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .and_then(|s| s.strip_suffix(".log"));
    let Some(stem) = stem else {
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

    /// The alpha and beta clients write their own prefix, and a
    /// commander who flew an Odyssey alpha still has those files. They
    /// were falling into the unknown-name bucket, which sorts after
    /// every dated file — so a 2021 alpha replayed as the newest
    /// flight, the same failure the two date formats caused.
    #[test]
    fn alpha_and_beta_journals_sort_by_their_date_too() {
        for name in [
            "JournalAlpha.210615080000.01.log",
            "JournalBeta.2021-06-15T080000.01.log",
        ] {
            assert!(
                sort_key(name) < sort_key("Journal.2026-09-15T090000.01.log"),
                "{name} must not sort after a 2026 flight: {}",
                sort_key(name)
            );
        }
        let mut names = vec![
            "Journal.2026-09-15T090000.01.log",
            "JournalBeta.2021-06-15T080000.01.log",
            "Journal.220315152335.01.log",
            "JournalAlpha.210101000000.01.log",
        ];
        names.sort_by_key(|n| sort_key(n));
        assert_eq!(
            names,
            vec![
                "JournalAlpha.210101000000.01.log",
                "JournalBeta.2021-06-15T080000.01.log",
                "Journal.220315152335.01.log",
                "Journal.2026-09-15T090000.01.log",
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
