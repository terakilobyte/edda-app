//! The telemetry shipper — the client half of the 2026-09-05 wire
//! contract, under the standing law: EDDA is not a surveillance tool.
//!
//! What leaves the machine, and nothing else: warn/error CALLSITES with
//! counts (targets, never rendered messages — messages can name star
//! systems), four timing families as durations, which feature toggles
//! are on, app version and OS. No identity of any kind; the server
//! cannot tell two batches came from the same commander.
//!
//! Consent is the maintainer's opt-out checkbox ("Send anonymous usage
//! data", default on) — checked AT SEND TIME, every batch, never
//! cached. Failure handling is fail-silent by contract: a batch that
//! cannot send is DROPPED — telemetry is never queued to disk.

use std::sync::Mutex;
use std::sync::OnceLock;

// 2-minute cadence (maintainer, 2026-09-05: faster feedback while the fleet is
// small and the new search-parameter distributions are filling). A quiet
// interval still sends nothing — take_batch returns None — so idle
// clients cost the same as before; only active ones report more often.
const BATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(2 * 60);
const MAX_TIMINGS: usize = 256;
const MAX_SEARCHES: usize = 256;

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct EventRow {
    pub callsite: String,
    pub level: &'static str,
    pub count: u32,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct TimingRow {
    pub kind: &'static str,
    pub ms: u32,
    pub ok: bool,
}

/// One search parameter the commander chose — the price-freshness window
/// or the effective jump range a plot ran at. A NUMBER only, aggregated
/// server-side into a distribution so the app can pick better defaults;
/// the same anonymity shape as a timing (no identity in the batch, capped,
/// unlinkable). `kind` is the contract's closed set, rejected at the
/// server boundary like timing kinds.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct SearchRow {
    pub kind: &'static str,
    pub value: u32,
}

#[derive(Debug, serde::Serialize)]
pub struct Batch {
    pub version: String,
    pub os: String,
    pub enabled_features: Vec<&'static str>,
    pub events: Vec<EventRow>,
    pub timings: Vec<TimingRow>,
    pub searches: Vec<SearchRow>,
    pub created_at: String,
}

static TIMINGS: OnceLock<Mutex<Vec<TimingRow>>> = OnceLock::new();
static SEARCHES: OnceLock<Mutex<Vec<SearchRow>>> = OnceLock::new();

/// Record one operation's duration. `kind` must be one of the contract's
/// closed set (plot|trade|sync|hydrate) — a typo here would be rejected
/// at the server boundary, which is the test's job to prevent.
pub fn record_timing(kind: &'static str, ms: u128, ok: bool) {
    debug_assert!(matches!(kind, "plot" | "trade" | "sync" | "hydrate"));
    let mut timings = TIMINGS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if timings.len() < MAX_TIMINGS {
        timings.push(TimingRow { kind, ms: ms.min(u128::from(u32::MAX)) as u32, ok });
    }
}

/// Record one search parameter the commander chose. `kind` must be one of
/// the contract's closed set (trade_max_age_hours|plot_range_ly) — a typo
/// would be rejected at the server boundary, the test's job to prevent.
pub fn record_search(kind: &'static str, value: u32) {
    debug_assert!(matches!(kind, "trade_max_age_hours" | "plot_router_gate_ly"));
    let mut searches = SEARCHES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if searches.len() < MAX_SEARCHES {
        searches.push(SearchRow { kind, value });
    }
}

/// Assemble a batch from everything accumulated since the last one.
/// `None` when there is nothing to say — quiet clients send nothing.
pub fn take_batch(enabled_features: Vec<&'static str>, router_gate_ly: u32, created_at: String) -> Option<Batch> {
    let mut events: Vec<EventRow> = ed_store::observe::drain_event_counts()
        .into_iter()
        .map(|(mut callsite, level, count)| {
            callsite.truncate(160);
            EventRow { callsite, level, count }
        })
        .collect();
    events.sort_by(|a, b| (&a.callsite, a.level).cmp(&(&b.callsite, b.level)));
    events.truncate(128);
    let timings = std::mem::take(
        &mut *TIMINGS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()),
    );
    let mut searches = std::mem::take(
        &mut *SEARCHES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()),
    );
    if events.is_empty() && timings.is_empty() && searches.is_empty() {
        return None;
    }
    // A non-empty batch carries one snapshot of the router-distance gate
    // (game_route_max_ly) — the distance up to which EDDA hands plotting
    // to the game's own map, beyond which EDDA plans itself. Sampled per
    // active batch, never per plot: a high gate means short hops never
    // reach the plotter, so per-plot sampling would hide exactly the
    // commanders who set it highest. A number only, same anonymity shape.
    searches.push(SearchRow { kind: "plot_router_gate_ly", value: router_gate_ly });
    Some(Batch {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        enabled_features,
        events,
        timings,
        searches,
        created_at,
    })
}

type ConfigHandle = std::sync::Arc<Mutex<crate::state::AppConfig>>;

/// Is telemetry enabled right now? Opt-out: absent choice means yes.
pub fn consented(config: &ConfigHandle) -> bool {
    config.lock().unwrap_or_else(|e| e.into_inner()).send_telemetry.unwrap_or(true)
}

/// Which speech engine is actually answering (maintainer, 2026-09-05: the
/// dashboard should show kokoro vs piper vs windows). Stamped by the
/// voice layer whenever the winning backend changes; a name, never a
/// voice ID or any content.
static VOICE_ENGINE: Mutex<&'static str> = Mutex::new("voice_none");

pub fn set_voice_engine(flag: &'static str) {
    *VOICE_ENGINE.lock().unwrap_or_else(|e| e.into_inner()) = flag;
}

/// The router-distance gate the contract snapshots per batch: the
/// distance (ly) up to which EDDA hands plotting to the game's own map.
/// A setting, not a per-search choice — sampled here so its distribution
/// isn't biased by how often anyone plots.
pub fn router_gate_ly(config: &ConfigHandle) -> u32 {
    config.lock().unwrap_or_else(|e| e.into_inner()).game_route_max_ly
}

/// The feature-toggle snapshot the contract carries.
pub fn feature_flags(config: &ConfigHandle) -> Vec<&'static str> {
    let config = config.lock().unwrap_or_else(|e| e.into_inner());
    let mut on = Vec::new();
    if config.auto_update.unwrap_or(true) {
        on.push("auto_update");
    }
    on.push(*VOICE_ENGINE.lock().unwrap_or_else(|e| e.into_inner()));
    on
}

/// One send attempt: consent gate, assemble, POST, forget. Every exit
/// path drops the batch — by contract, telemetry never persists.
pub async fn flush(http: &reqwest::Client, config: &ConfigHandle) {
    if !consented(config) {
        // Consent off: discard accumulations so nothing lingers.
        let _ = ed_store::observe::drain_event_counts();
        TIMINGS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).clear();
        SEARCHES.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).clear();
        return;
    }
    let (dev_local, saved) = {
        let c = config.lock().unwrap_or_else(|e| e.into_inner());
        // Same resolution as exchange::endpoint — dev-build batches go
        // to the dev server, never into prod's histograms.
        (cfg!(debug_assertions) && c.dev_api_local == Some(true), c.community_api_url.clone())
    };
    let Some(api) = crate::exchange::pick_endpoint(std::env::var("EDDA_API_URL").ok(), dev_local, saved) else {
        return;
    };
    let Some(batch) = take_batch(feature_flags(config), router_gate_ly(config), chrono::Utc::now().to_rfc3339()) else {
        return;
    };
    let sent = http
        .post(format!("{api}/v1/telemetry"))
        .json(&batch)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    match sent {
        Ok(r) if r.status().is_success() => {
            tracing::debug!(events = batch.events.len(), timings = batch.timings.len(), "telemetry batch sent")
        }
        Ok(r) => tracing::debug!(status = %r.status(), "telemetry batch refused; dropped"),
        Err(_) => tracing::debug!("telemetry batch unsendable; dropped"),
    }
}

/// The background cadence: a flush every [`BATCH_EVERY`]. The supervisor's
/// token ends it; lib.rs also flushes once on shutdown.
pub async fn run(
    token: tokio_util::sync::CancellationToken,
    http: reqwest::Client,
    config: ConfigHandle,
) {
    loop {
        tokio::select! {
            _ = token.cancelled() => {
                // The contract's clean-shutdown flush.
                flush(&http, &config).await;
                return;
            }
            _ = tokio::time::sleep(BATCH_EVERY) => {}
        }
        flush(&http, &config).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The batch honors the wire contract: closed timing kinds, caps,
    /// sorted-deterministic events, and — the law — nothing
    /// identity-shaped anywhere in the serialized form.
    #[test]
    fn batches_are_capped_closed_and_anonymous() {
        let _ = ed_store::observe::drain_event_counts();
        for _ in 0..300 {
            record_timing("plot", 723, true);
        }
        record_timing("hydrate", 90_000, false);
        for _ in 0..300 {
            record_search("trade_max_age_hours", 2);
        }
        let batch = take_batch(vec!["overlay"], 1000, "2026-09-05T00:00:00Z".into()).expect("timings exist");
        assert!(batch.timings.len() <= MAX_TIMINGS, "timing cap holds: {}", batch.timings.len());
        assert!(batch.timings.iter().all(|t| matches!(t.kind, "plot" | "trade" | "sync" | "hydrate")));
        // Recorded search params are capped; the per-batch router-gate
        // snapshot rides on top of that cap as exactly one sample.
        assert!(
            batch.searches.iter().filter(|s| s.kind == "trade_max_age_hours").count() <= MAX_SEARCHES,
            "search cap holds"
        );
        assert_eq!(
            batch.searches.iter().filter(|s| s.kind == "plot_router_gate_ly").count(),
            1,
            "one router-gate snapshot per non-empty batch"
        );
        assert!(batch.searches.iter().all(|s| matches!(s.kind, "trade_max_age_hours" | "plot_router_gate_ly")));
        let json = serde_json::to_string(&batch).unwrap();
        for forbidden in ["commander", "cmdr", "name\"", "id\"", "email", "system\""] {
            assert!(!json.contains(forbidden), "{forbidden} must not ride: {json}");
        }
        // Emptied by the take: a quiet interval sends nothing (the gate
        // snapshot only rides a batch that already has something to say).
        assert!(take_batch(vec![], 1000, "t".into()).is_none());
    }
}
