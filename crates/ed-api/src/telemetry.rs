//! /v1/telemetry ingest (wire contract: ledger 2026-09-05, the reviewer's
//! proposal, adopted). The client ships batched, anonymous, allowlisted
//! aggregates; this boundary REJECTS unknown fields, levels, kinds and
//! malformed label values outright, truncates over-cap SIZES, and
//! re-emits everything as edda_client_* metrics — nothing row-stored.
//!
//! Boundary addition beyond the proposal (review note, review): the
//! contract's "cardinality safe by construction" is true of OUR client,
//! but the endpoint is public — a hostile POSTer could mint unbounded
//! label values and bloat VictoriaMetrics forever. The CallsiteGuard
//! caps distinct callsite labels per process; past the cap, events fold
//! into the "_overflow" bucket (counted, never lost, never unbounded).

use std::collections::HashSet;
use std::sync::Mutex;

pub const MAX_EVENTS: usize = 128;
pub const MAX_TIMINGS: usize = 256;
pub const MAX_FEATURES: usize = 64;
pub const MAX_CALLSITE: usize = 160;
pub const MAX_NAME: usize = 64;
pub const MAX_VERSION: usize = 32;
pub const MAX_DISTINCT_CALLSITES: usize = 2_000;
pub const OVERFLOW_CALLSITE: &str = "_overflow";
/// Clamp for a single timing sample: an hour. Anything longer is a
/// clock bug, not a measurement.
pub const MAX_TIMING_MS: f64 = 3_600_000.0;
/// Clamp for one event entry's count: a client aggregates ~15 minutes,
/// and a million identical warns in that window is a loop, not news.
pub const MAX_COUNT: u64 = 100_000;

#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub enabled_features: Vec<String>,
    #[serde(default)]
    pub events: Vec<EventEntry>,
    #[serde(default)]
    pub timings: Vec<TimingEntry>,
    /// Search-parameter samples (wire expansion, 2026-09-05, maintainer-OK'd):
    /// raw capped values the client chose — a preference distribution,
    /// no identity; the server buckets them into histograms.
    #[serde(default)]
    pub searches: Vec<SearchEntry>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct EventEntry {
    pub callsite: String,
    pub level: String,
    pub count: u64,
}

#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct TimingEntry {
    pub kind: String,
    pub ms: f64,
    pub ok: bool,
}

#[derive(serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct SearchEntry {
    pub kind: String,
    pub value: u32,
}

pub const MAX_SEARCHES: usize = 256;
/// Clamp for one search sample: nothing legitimate exceeds a month of
/// freshness-hours or a thousand light-years of range.
pub const MAX_SEARCH_VALUE: u32 = 100_000;

/// The closed search-parameter kinds, each with its OWN metric name —
/// hours and light-years need different bucket scales, and a
/// Prometheus histogram carries exactly one `le` set per name.
/// `plot_router_gate_ly` is the router-distance GATE setting snapshot
/// (maintainer clarification 2026-09-05: the game_route_max_ly slider, not
/// ship range — per batch, since a high gate means short hops never
/// reach EDDA's plotter at all). The never-shipped plot_range_ly kind
/// was retired before any client emitted it.
pub fn search_metric(kind: &str) -> Option<&'static str> {
    match kind {
        "trade_max_age_hours" => Some("edda_client_search_age_hours"),
        "plot_router_gate_ly" => Some("edda_client_router_gate_ly"),
        // Item 52 A: how STALE a commander's carrier picture is when
        // they ask about it. CarrierStats only fires when the carrier
        // management panel is opened, so this distribution answers
        // whether journal-derived carrier data is fresh enough to be
        // useful on its own, or whether B (the CAPI fetch) is doing the
        // real work. Pooled and unlabelled: an age is not a position,
        // and nothing here says whose carrier or where.
        "carrier_stats_age_hours" => Some("edda_client_carrier_stats_age_hours"),
        _ => None,
    }
}

pub fn valid_level(level: &str) -> bool {
    matches!(level, "warn" | "error")
}

pub fn valid_kind(kind: &str) -> bool {
    matches!(kind, "plot" | "trade" | "sync" | "hydrate")
}

/// Label values become Prometheus labels: a closed charset, never empty,
/// never over cap. Anything else is rejected with the batch — malformed
/// VALUES are the same attack surface as unknown fields.
pub fn label_ok(value: &str, cap: usize) -> bool {
    !value.is_empty()
        && value.len() <= cap
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.' | ' '))
}

/// Validate a whole batch per the contract: Err(reason) rejects it.
/// Over-cap LIST SIZES are not an error — callers truncate.
pub fn validate(batch: &Batch) -> Result<(), &'static str> {
    if !label_ok(&batch.version, MAX_VERSION) {
        return Err("bad version");
    }
    for event in batch.events.iter().take(MAX_EVENTS) {
        if !valid_level(&event.level) {
            return Err("unknown level");
        }
        if !label_ok(&event.callsite, MAX_CALLSITE) {
            return Err("bad callsite");
        }
    }
    for timing in batch.timings.iter().take(MAX_TIMINGS) {
        if !valid_kind(&timing.kind) {
            return Err("unknown kind");
        }
        if !timing.ms.is_finite() || timing.ms < 0.0 {
            return Err("bad timing");
        }
    }
    for name in batch.enabled_features.iter().take(MAX_FEATURES) {
        if !label_ok(name, MAX_NAME) {
            return Err("bad feature name");
        }
    }
    for search in batch.searches.iter().take(MAX_SEARCHES) {
        if search_metric(&search.kind).is_none() {
            return Err("unknown search kind");
        }
        if search.value > MAX_SEARCH_VALUE {
            return Err("bad search value");
        }
    }
    Ok(())
}

/// The cardinality fence. Admission returns the label to USE: the
/// callsite itself while the distinct set has room, the overflow bucket
/// after.
#[derive(Default)]
pub struct CallsiteGuard {
    seen: Mutex<HashSet<String>>,
}

impl CallsiteGuard {
    pub fn admit(&self, callsite: &str) -> String {
        let mut seen = self.seen.lock().unwrap_or_else(|e| e.into_inner());
        if seen.contains(callsite) {
            return callsite.to_owned();
        }
        if seen.len() < MAX_DISTINCT_CALLSITES {
            seen.insert(callsite.to_owned());
            return callsite.to_owned();
        }
        OVERFLOW_CALLSITE.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(json: &str) -> Result<Batch, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn unknown_fields_levels_and_kinds_are_rejected() {
        assert!(
            batch(r#"{"version":"0.2.1","surprise":1}"#).is_err(),
            "unknown top-level field"
        );
        assert!(
            batch(r#"{"events":[{"callsite":"a","level":"warn","count":1,"extra":2}]}"#).is_err(),
            "unknown entry field"
        );
        let info =
            batch(r#"{"version":"0.2.1","events":[{"callsite":"a","level":"info","count":1}]}"#)
                .unwrap();
        assert_eq!(
            validate(&info),
            Err("unknown level"),
            "info is not in the allowlist"
        );
        let decode =
            batch(r#"{"version":"0.2.1","timings":[{"kind":"decode","ms":5,"ok":true}]}"#).unwrap();
        assert_eq!(validate(&decode), Err("unknown kind"));
    }

    #[test]
    fn label_values_are_a_closed_charset() {
        assert!(label_ok("edda::follow::fuel_chatter", MAX_CALLSITE));
        assert!(label_ok("windows x86_64", MAX_NAME));
        assert!(!label_ok("", MAX_NAME), "empty is not a label");
        assert!(
            !label_ok("a\"};evil{", MAX_NAME),
            "injection charset refused"
        );
        assert!(
            !label_ok(&"x".repeat(MAX_CALLSITE + 1), MAX_CALLSITE),
            "over cap refused"
        );
        let sneaky =
            batch(r#"{"version":"0.2.1","events":[{"callsite":"a{b}","level":"warn","count":1}]}"#)
                .unwrap();
        assert_eq!(validate(&sneaky), Err("bad callsite"));
    }

    #[test]
    fn the_cardinality_fence_folds_overflow_without_losing_counts() {
        let guard = CallsiteGuard::default();
        for i in 0..MAX_DISTINCT_CALLSITES {
            assert_eq!(guard.admit(&format!("site{i}")), format!("site{i}"));
        }
        assert_eq!(
            guard.admit("site0"),
            "site0",
            "known callsites keep their label forever"
        );
        assert_eq!(
            guard.admit("brand-new"),
            OVERFLOW_CALLSITE,
            "past the cap: the overflow bucket"
        );
    }

    /// The searches wire expansion (2026-09-05): closed kinds, each on
    /// its own metric name (hours and light-years share no bucket
    /// scale); unknown kinds and absurd values reject the batch; an
    /// absent field parses as empty for pre-expansion clients.
    #[test]
    fn search_entries_are_closed_kinded_and_capped() {
        let batch = |kind: &str, value: u32| Batch {
            version: "0.2.5".into(),
            os: "windows".into(),
            enabled_features: vec![],
            events: vec![],
            timings: vec![],
            searches: vec![SearchEntry {
                kind: kind.into(),
                value,
            }],
            created_at: None,
        };
        assert!(validate(&batch("trade_max_age_hours", 2)).is_ok());
        assert!(
            validate(&batch("plot_router_gate_ly", 0)).is_ok(),
            "gate 0 = EDDA always plans"
        );
        assert!(validate(&batch("plot_router_gate_ly", 1000)).is_ok());
        assert_eq!(
            validate(&batch("favourite_station", 1)),
            Err("unknown search kind")
        );
        assert_eq!(
            validate(&batch("plot_range_ly", 62)),
            Err("unknown search kind"),
            "the retired kind stays retired"
        );
        assert_eq!(
            validate(&batch("plot_router_gate_ly", 2_000_000)),
            Err("bad search value")
        );
        assert_eq!(
            search_metric("trade_max_age_hours"),
            Some("edda_client_search_age_hours")
        );
        assert_eq!(
            search_metric("plot_router_gate_ly"),
            Some("edda_client_router_gate_ly")
        );
        // Item 52 A: carrier-picture staleness. Its own metric name
        // because hours and light-years cannot share one `le` set, and a
        // batch carrying it must be ACCEPTED -- an unmapped kind rejects
        // the WHOLE batch, blacking out every other series the client
        // sent, which is why review held it client-side until now.
        assert!(
            validate(&batch("carrier_stats_age_hours", 0)).is_ok(),
            "a fresh carrier reading is 0 h"
        );
        assert!(
            validate(&batch("carrier_stats_age_hours", 720)).is_ok(),
            "a month-old picture still reports"
        );
        assert_eq!(
            search_metric("carrier_stats_age_hours"),
            Some("edda_client_carrier_stats_age_hours")
        );
        let old: Batch =
            serde_json::from_str(r#"{"version":"0.2.4","os":"w","events":[],"timings":[]}"#)
                .unwrap();
        assert!(old.searches.is_empty(), "pre-expansion batches still parse");
    }
}
