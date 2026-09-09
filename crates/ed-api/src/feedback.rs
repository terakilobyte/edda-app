//! Anonymous problem reports (wire contract, ledger 2026-09-05): the app
//! POSTs {version, os, text, log_tail?, created_at} to /v1/feedback; the
//! server answers 202 {id}. Truncation-not-rejection on every field, an
//! in-memory per-source rate limit (never persisted — the privacy law
//! forbids identity at rest), and no identity columns in the table.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const TEXT_CAP: usize = 4_096;
pub const LOG_CAP: usize = 32_768;
pub const META_CAP: usize = 128;
pub const WINDOW: Duration = Duration::from_secs(3_600);
pub const PER_WINDOW: u32 = 10;

/// Clip to `cap` bytes on a char boundary — the server-side half of the
/// truncation contract (the client clips too; the server must not trust
/// that).
pub fn clip(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_owned();
    }
    let mut at = cap;
    while at > 0 && !s.is_char_boundary(at) {
        at -= 1;
    }
    s[..at].to_owned()
}

/// Fixed-window in-memory limiter keyed by source string (the proxy's
/// X-Forwarded-For). Prunes expired windows on every call so the map
/// cannot grow past the set of sources active in the last window.
/// `Default` keeps the original feedback budget (10/hour); endpoints
/// with different realities (`/v1/route`, the knowledge proxy) build
/// their own via [`Limiter::new`].
pub struct Limiter {
    window: Duration,
    per_window: u32,
    entries: Mutex<HashMap<String, (u32, Instant)>>,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new(WINDOW, PER_WINDOW)
    }
}

impl Limiter {
    pub fn new(window: Duration, per_window: u32) -> Self {
        Limiter { window, per_window, entries: Mutex::default() }
    }

    pub fn allow(&self, source: &str, now: Instant) -> bool {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|_, (_, start)| now.duration_since(*start) < self.window);
        let entry = entries.entry(source.to_owned()).or_insert((0, now));
        if now.duration_since(entry.1) >= self.window {
            *entry = (0, now);
        }
        entry.0 += 1;
        entry.0 <= self.per_window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_respects_caps_and_char_boundaries() {
        assert_eq!(clip("hello", 10), "hello");
        assert_eq!(clip("hello", 3), "hel");
        // Multi-byte: never split a char.
        let s = "aé"; // 'é' is 2 bytes starting at index 1
        assert_eq!(clip(s, 2), "a");
        assert_eq!(clip(s, 3), "aé");
    }

    #[test]
    fn limiter_caps_per_window_and_rolls_over() {
        let limiter = Limiter::default();
        let t0 = Instant::now();
        for _ in 0..PER_WINDOW {
            assert!(limiter.allow("1.2.3.4", t0), "within the window budget");
        }
        assert!(!limiter.allow("1.2.3.4", t0), "over budget: refused");
        assert!(limiter.allow("5.6.7.8", t0), "another source is unaffected");
        let later = t0 + WINDOW + Duration::from_secs(1);
        assert!(limiter.allow("1.2.3.4", later), "a new window starts clean");
    }
}

/// Why a request was refused by a [`Budget`], for the outcome label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// Too many in the last few seconds — a loop or a scraper, not a
    /// person; the hourly count was not touched.
    Burst,
    /// The hourly budget is spent.
    Hourly,
}

/// The two-speed per-source budget (ledger 2026-09-07: "a per-second
/// burst cap is what actually protects the box; the hourly number is a
/// scraper ceiling"). The burst window is checked FIRST so a burst
/// refusal does not consume an hourly token: a runaway client loop
/// gets 429s for ten seconds and still has its hour when it stops.
pub struct Budget {
    hourly: Limiter,
    burst: Limiter,
}

/// The burst window. Ten seconds is long enough that a commander's
/// fastest real pattern (a replot per jump, a search per stop) never
/// sees it and short enough that a loop is stopped before it matters.
pub const BURST_WINDOW: Duration = Duration::from_secs(10);

impl Budget {
    pub fn new(hourly: Limiter, burst_per_10s: u32) -> Self {
        Budget { hourly, burst: Limiter::new(BURST_WINDOW, burst_per_10s) }
    }

    pub fn allow(&self, source: &str, now: Instant) -> Result<(), Refused> {
        if !self.burst.allow(source, now) {
            metrics::counter!("edda_budget_refusals_total", "reason" => "burst").increment(1);
            return Err(Refused::Burst);
        }
        if !self.hourly.allow(source, now) {
            metrics::counter!("edda_budget_refusals_total", "reason" => "hourly").increment(1);
            return Err(Refused::Hourly);
        }
        Ok(())
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn a_burst_is_refused_before_the_hour_is_touched() {
        let budget = Budget::new(Limiter::new(Duration::from_secs(3_600), 1_000), 5);
        let t0 = Instant::now();
        for _ in 0..5 {
            assert_eq!(budget.allow("cmdr", t0), Ok(()));
        }
        assert_eq!(budget.allow("cmdr", t0), Err(Refused::Burst));
        // Ten seconds later the loop has stopped: the hour has only
        // spent the five that were served, not the refusals.
        let later = t0 + BURST_WINDOW + Duration::from_secs(1);
        for _ in 0..5 {
            assert_eq!(budget.allow("cmdr", later), Ok(()));
        }
        assert_eq!(budget.allow("cmdr", later), Err(Refused::Burst));
    }

    #[test]
    fn the_hour_still_binds_a_polite_client() {
        let budget = Budget::new(Limiter::new(Duration::from_secs(3_600), 3), 100);
        let t0 = Instant::now();
        assert_eq!(budget.allow("cmdr", t0), Ok(()));
        assert_eq!(budget.allow("cmdr", t0), Ok(()));
        assert_eq!(budget.allow("cmdr", t0), Ok(()));
        assert_eq!(budget.allow("cmdr", t0), Err(Refused::Hourly));
        assert_eq!(budget.allow("other", t0), Ok(()), "another source is unaffected");
    }
}
