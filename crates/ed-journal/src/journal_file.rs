//! Journal file names, and the order they belong in.
//!
//! The game has used two name formats. Until late 2022 a file was
//! `Journal.YYMMDDHHMMSS.NN.log`; since then it is
//! `Journal.YYYY-MM-DDTHHMMSS.NN.log`. Compared as plain strings the two
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
    format!("{canonical}.{part}.{suffix}")
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
        assert_eq!(sort_key("Journal.221231235959.01.log"), "2022-12-31T235959.01.z");
    }
}
