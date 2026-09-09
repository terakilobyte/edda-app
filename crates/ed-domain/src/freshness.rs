//! The one freshness rule.
//!
//! Every store that holds galaxy observations -- the desktop's SQLite, the
//! service's PostgreSQL, the EBEX hydrator -- has to answer the same
//! question: *is this incoming observation newer than what I already hold?*
//! Answering it four different ways is how an older snapshot ends up
//! replacing a newer one. The rule lives here, once, with no database in
//! sight, and every adapter either calls [`accept`] or writes SQL that says
//! exactly what [`accept`] says and names it in a comment.
//!
//! The rule:
//!
//! * **Strictly newer wins.** `incoming > stored` applies; anything else is
//!   a no-op. Equal timestamps are the same observation seen twice (EDDN
//!   relays duplicate, and a replay journal re-sends on purpose), so they
//!   must not rewrite rows.
//! * **Nothing stored means accept.** A station we have never seen has no
//!   opinion.
//! * **The stored value is a watermark, not an aggregate.** It is the
//!   timestamp of the last snapshot *applied* to the station, kept on the
//!   station (or in a dedicated watermark row). It is never `MAX(updated)`
//!   over the rows that happen to remain: an empty snapshot legitimately
//!   deletes every row, after which MAX is NULL and an older replay would be
//!   accepted -- that is the bug this module exists to close.
//!
//! Timestamps are whole seconds since the Unix epoch, as `i64`, everywhere.
//! Strings are parsed once at the edge ([`parse_timestamp`]) and never
//! round-tripped back through text to be parsed again.

/// What to do with an incoming observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Newer than anything stored: replace.
    Apply,
    /// Same age or older than the stored watermark: leave the store alone.
    Skip,
}

impl Decision {
    pub fn is_apply(self) -> bool {
        matches!(self, Decision::Apply)
    }
}

/// Decide whether `incoming` (epoch seconds) may replace what `stored`
/// describes. `stored` is the station-level watermark, `None` when the
/// station has never had a snapshot applied.
///
/// SQL adapters express this as
/// `stored IS NULL OR stored < incoming`
/// and should say so in a comment next to the statement.
pub fn accept(stored: Option<i64>, incoming: i64) -> Decision {
    match stored {
        Some(stored) if stored >= incoming => Decision::Skip,
        _ => Decision::Apply,
    }
}

/// Convenience for adapters that want a boolean.
pub fn is_newer(stored: Option<i64>, incoming: i64) -> bool {
    accept(stored, incoming).is_apply()
}

/// Hours between `now` and an observation, both in epoch seconds. An
/// observation with no timestamp is infinitely old -- it must never pass a
/// "fresher than N hours" filter.
pub fn age_hours(now: i64, updated: Option<i64>) -> f64 {
    match updated {
        Some(t) => (now - t) as f64 / 3600.0,
        None => f64::INFINITY,
    }
}

/// Epoch seconds from the timestamp shapes the galaxy sources produce:
///
/// * EDDN / journal: `2026-08-24T01:00:00Z`, optionally with fractional
///   seconds (`...:00.123Z`).
/// * Spansh dumps: `2026-08-24 01:00:00+00` (space separator, hour-only
///   offset).
/// * Either with a full `+HH:MM` offset, or no zone suffix at all (taken as
///   UTC, which is what SQLite's `datetime()` emits).
///
/// Anything else is `None` rather than a guess.
pub fn parse_timestamp(text: &str) -> Option<i64> {
    let text = text.trim();
    let b = text.as_bytes();
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !(b[10] == b'T' || b[10] == b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let s = text.get(range)?;
        if !s.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        s.parse().ok()
    };
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }

    // Fractional seconds are ignored; the zone suffix is honoured.
    let mut rest = &text[19..];
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits = after_dot.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        rest = &after_dot[digits..];
    }
    let offset_secs = match rest {
        "" | "Z" | "z" => 0,
        s if s.starts_with('+') || s.starts_with('-') => {
            let sign = if s.starts_with('-') { -1 } else { 1 };
            let body = &s[1..];
            let (h, m) = match body.len() {
                2 => (body.parse::<i64>().ok()?, 0),
                4 => (body[..2].parse::<i64>().ok()?, body[2..].parse::<i64>().ok()?),
                5 if body.as_bytes()[2] == b':' => (
                    body[..2].parse::<i64>().ok()?,
                    body[3..].parse::<i64>().ok()?,
                ),
                _ => return None,
            };
            if !body
                .bytes()
                .all(|c| c.is_ascii_digit() || c == b':')
            {
                return None;
            }
            sign * (h * 3600 + m * 60)
        }
        _ => return None,
    };

    Some(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset_secs)
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strictly_newer_wins_and_equal_is_a_no_op() {
        assert_eq!(accept(Some(100), 101), Decision::Apply);
        assert_eq!(accept(Some(100), 100), Decision::Skip);
        assert_eq!(accept(Some(100), 99), Decision::Skip);
    }

    #[test]
    fn nothing_stored_accepts_anything() {
        assert_eq!(accept(None, 0), Decision::Apply);
        assert_eq!(accept(None, -5), Decision::Apply);
    }

    /// The watermark survives an empty snapshot. A store that derived
    /// "stored" from the remaining rows would see `None` here and let the
    /// older replay through.
    #[test]
    fn watermark_from_an_empty_snapshot_still_rejects_an_older_replay() {
        let mut watermark: Option<i64> = None;
        for (incoming, rows) in [(100, 3), (200, 0), (150, 2)] {
            if accept(watermark, incoming).is_apply() {
                watermark = Some(incoming);
            }
            let _ = rows;
        }
        assert_eq!(watermark, Some(200));
        assert_eq!(accept(watermark, 150), Decision::Skip);
    }

    #[test]
    fn age_is_hours_and_missing_is_infinite() {
        assert_eq!(age_hours(7200, Some(0)), 2.0);
        assert_eq!(age_hours(0, Some(3600)), -1.0);
        assert!(age_hours(0, None).is_infinite());
    }

    #[test]
    fn parses_every_source_shape_to_the_same_epoch() {
        let expected = Some(1_787_533_200);
        for text in [
            "2026-08-24T01:00:00Z",
            "2026-08-24T01:00:00.250Z",
            "2026-08-24 01:00:00+00",
            "2026-08-24 01:00:00+0000",
            "2026-08-24T01:00:00+00:00",
            "2026-08-24 01:00:00",
            "2026-08-24T02:00:00+01:00",
            "2026-08-24T00:00:00-01",
        ] {
            assert_eq!(parse_timestamp(text), expected, "{text}");
        }
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2000-01-01T00:00:00Z"), Some(946_684_800));
    }

    #[test]
    fn garbage_is_none_not_a_guess() {
        for text in [
            "",
            "yesterday",
            "2026-13-01T00:00:00Z",
            "2026-08-24T25:00:00Z",
            "2026-08-24T01:00:00 UTC",
            "2026-08-24T01:00:00+",
            "2026-08-24T01:00:00.Z",
            "1787619600",
        ] {
            assert_eq!(parse_timestamp(text), None, "{text:?}");
        }
    }
}

/// The inverse of [`parse_timestamp`]: epoch seconds → `YYYY-MM-DDTHH:MM:SSZ`.
/// Proleptic Gregorian, UTC, no dependencies (Howard Hinnant's
/// civil-from-days). Negative epochs are clamped to 1970.
pub fn format_timestamp(epoch: i64) -> String {
    let epoch = epoch.max(0);
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", secs / 3600, (secs % 3600) / 60, secs % 60)
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn format_round_trips_parse() {
        for text in ["2026-09-06T22:57:10Z", "1970-01-01T00:00:00Z", "2000-02-29T23:59:59Z", "2024-12-31T00:00:00Z"] {
            let epoch = parse_timestamp(text).unwrap();
            assert_eq!(format_timestamp(epoch), text);
        }
    }
}
