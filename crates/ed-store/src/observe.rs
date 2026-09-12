//! Logging and flight observation.
//!
//! Two separate outputs, because they answer different questions:
//!
//! * **The log** (`.data/logs/edda.log.YYYY-MM-DD`) is JSON lines of
//!   everything the app did -- syncs, feed stats, errors. This is for
//!   debugging: what happened, when, and what broke.
//!
//! * **The observation journal** (`.data/logs/observations.jsonl`) is
//!   append-only, one row per *measurable game fact*: a sale and the merits
//!   it earned, a Powerplay reading, an engineering craft. This is for
//!   science. `docs/PLAN.md` §2 leaves the merit constant `K` per-station
//!   with an unknown driver, and the only way to resolve it is to accumulate
//!   observations across many stations and states while the commander plays.
//!
//! Keeping them apart matters: the log rotates and is disposable, the
//! observation journal must never be, and mixing debug noise into a dataset
//! is how it stops being usable.

use anyhow::Result;
#[cfg(test)]
use rusqlite::Connection;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static OBSERVATION_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Directory holding both outputs, alongside the database.
pub fn log_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .map(|p| p.join("logs"))
        .unwrap_or_else(|| PathBuf::from("logs"))
}

/// Initialise logging. Returns the guard that must stay alive for the
/// process lifetime -- dropping it stops the background writer and silently
/// loses buffered lines.
pub fn init(db_path: &Path, console: bool) -> Result<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, EnvFilter};

    let dir = log_dir(db_path);
    std::fs::create_dir_all(&dir)?;
    OBSERVATION_PATH.get_or_init(|| dir.join("observations.jsonl"));

    let appender = tracing_appender::rolling::daily(&dir, "edda.log");
    let (file_writer, guard) = tracing_appender::non_blocking(appender);

    // RUST_LOG overrides; the default prod floor is INFO (maintainer 2026-09-05:
    // "in prod we don't need debug, just >= info"). The diagnostics that
    // once needed a blanket debug default — the galaxy-open / bubble
    // fallback that went dark on 2026-09-03 — are written at info/warn and
    // survive here; anything genuinely needing debug in the field is one
    // RUST_LOG=<crate>=debug away (e.g. RUST_LOG=ed_voice=debug for a mic
    // issue), which is also the lever a support reply hands a user.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = fmt::layer()
        .json()
        .with_writer(file_writer)
        .with_current_span(false)
        .with_span_list(false);

    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(WarnErrorCounts)
        .with(file_layer);

    if console {
        registry
            .with(fmt::layer().with_target(false).with_ansi(true).compact())
            .init();
    } else {
        registry.init();
    }

    tracing::info!(
        database = %db_path.display(),
        logs = %dir.display(),
        "logging started"
    );
    // Tripwire (field case 2026-09-04 14:57: an installed build ran with a
    // 0-byte edda.log for its whole session, and none of the launch
    // contexts reproduced it). A dead log cannot report itself through
    // tracing, so this checks the file directly and names the symptom the
    // way the kokoro zero-byte diagnosis does. The marker is removed on
    // every healthy init so it can never go stale.
    let marker = dir.join("logging-dead.marker");
    let _ = std::fs::remove_file(&marker);
    let probe_dir = dir.clone();
    std::thread::spawn(move || {
        // Field-calibrated 2026-09-04: a fresh install's appender started
        // LATE (empty at 5 s, flowing by two minutes) rather than dead, so
        // the alarm gets a second chance at 60 s and a recovered log
        // records its own late start — the delay signature is the clue to
        // the one session that never recovered.
        let newest_len = |dir: &Path| {
            std::fs::read_dir(dir)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("edda.log."))
                .filter_map(|entry| entry.metadata().ok())
                .max_by_key(|meta| meta.modified().ok())
                .map(|meta| meta.len())
        };
        std::thread::sleep(std::time::Duration::from_secs(5));
        if newest_len(&probe_dir) != Some(0) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(55));
        if newest_len(&probe_dir) != Some(0) {
            tracing::warn!("file log started late: still empty 5 s after init, recovered by 60 s");
            return;
        }
        let _ = std::fs::write(
            &marker,
            format!(
                "the tracing file log was still empty 60 s after init (pid {}). \
                 The subscriber accepted \"logging started\" but the appender \
                 never wrote it: suspect the non-blocking writer thread or the \
                 launch context. Session otherwise undiagnosable from logs.\n",
                std::process::id()
            ),
        );
    });
    Ok(guard)
}

/// One measurable game fact, appended to the observation journal.
///
/// Deliberately a flat, self-describing record rather than a reference into
/// the database: the point is that it stays interpretable years later, after
/// the schema has moved on.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Observation {
    /// A sale and the merits it earned -- the merit-model dataset.
    MeritSale {
        ts: String,
        commodity: String,
        tons: i64,
        sell_price: Option<i64>,
        total_sale: Option<i64>,
        avg_price_paid: Option<i64>,
        profit: Option<i64>,
        merits: i64,
        merit_events: usize,
        market_id: Option<i64>,
        station: Option<String>,
        system: Option<String>,
        powerplay_state: Option<String>,
        controlling_power: Option<String>,
        control_progress: Option<f64>,
        /// profit / merits -- the per-station constant this is trying to pin.
        implied_k: Option<f64>,
    },
    /// A combat kill and the merits it earned.
    ///
    /// Added after a RES session showed bounties earn merits under the same
    /// `floor(credits / K)` law as trade sales -- 8 kills pinned K to a
    /// 22-credit interval. The merit model is about credits earned, not about
    /// trading, so this and [`Observation::MeritSale`] measure the same
    /// underlying constant from two different activities.
    MeritKill {
        ts: String,
        award: String,
        target_ship: Option<String>,
        pilot_name: Option<String>,
        faction: Option<String>,
        reward: i64,
        merits: i64,
        merit_events: usize,
        system: Option<String>,
        powerplay_state: Option<String>,
        controlling_power: Option<String>,
        implied_k: Option<f64>,
        /// Whether `implied_k` can be trusted.
        ///
        /// `PowerplayMerits` never says what earned it, so attribution is
        /// inference. It only holds when a kill is temporally isolated: in a
        /// busy resource site, awards from the flat mechanic and from other
        /// Powerplay activity land between kills and get charged to whichever
        /// one preceded them. That produced a 600 cr bounty "earning" 176
        /// merits in a dense session. Only isolated rows belong in a fit.
        isolated: bool,
    },
    /// A Powerplay award that no credit-earning event explains.
    ///
    /// Observed as a flat +7 during combat, independent of any bounty -- a
    /// second merit mechanic whose trigger is unidentified. Recorded rather
    /// than guessed at, and kept out of the credit-scaled dataset so it
    /// cannot skew the constant being fitted.
    MeritUnattributed {
        ts: String,
        merits: i64,
        power: Option<String>,
        /// Event types seen shortly before, as a hint at the trigger.
        preceding: Vec<String>,
    },
    /// A merit award attributed to whatever earned it.
    ///
    /// Supersedes the separate sale/kill records: awards from trade, combat,
    /// scans and Powerplay deliveries interleave, so they have to be
    /// attributed on one timeline rather than by source-specific guesswork.
    MeritAward {
        ts: String,
        source: String,
        label: Option<String>,
        detail: Option<String>,
        credits: Option<i64>,
        merits: i64,
        power: Option<String>,
        system: Option<String>,
        /// How many merit-worthy events sat inside the window. More than one
        /// means the attribution is a guess.
        competing_candidates: usize,
        /// Only set when the source is credit-scaled AND unambiguous.
        implied_k: Option<f64>,
    },
    /// A Powerplay reading from a jump.
    Powerplay {
        ts: String,
        system: String,
        controlling_power: Option<String>,
        powerplay_state: Option<String>,
        control_progress: Option<f64>,
        reinforcement: Option<i64>,
        undermining: Option<i64>,
    },
}

/// Keys of every merit award already in the observation journal, in the
/// same `award|ts|merits` form `merit_capture` uses for its `seen` set.
///
/// Seeding from the file is what makes idempotence hold *across* process
/// restarts: without it every launch re-attributed the whole history and
/// appended it again, and the dataset filled with duplicates.
pub fn seen_award_keys() -> std::collections::HashSet<String> {
    let mut keys = std::collections::HashSet::new();
    let Some(path) = OBSERVATION_PATH.get() else {
        return keys;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return keys;
    };
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = v
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if kind != "merit_award" && kind != "merit_unattributed" {
            continue;
        }
        if let (Some(ts), Some(merits)) = (
            v.get("ts").and_then(serde_json::Value::as_str),
            v.get("merits").and_then(serde_json::Value::as_i64),
        ) {
            keys.insert(format!("award|{ts}|{merits}"));
        }
    }
    keys
}

/// Append one observation. Failures are logged, never propagated -- losing a
/// data point must not break a sync.
pub fn record(obs: &Observation) {
    let Some(path) = OBSERVATION_PATH.get() else {
        tracing::warn!("observation recorded before logging was initialised");
        return;
    };
    let line = match serde_json::to_string(obs) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(error = %e, "could not serialise observation");
            return;
        }
    };
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| writeln!(f, "{line}"));
    if let Err(e) = result {
        tracing::error!(error = %e, path = %path.display(), "could not append observation");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_sits_beside_the_database() {
        let p = Path::new("/some/where/.data/edda.sqlite3");
        assert_eq!(log_dir(p), Path::new("/some/where/.data/logs"));
    }

    #[test]
    fn observations_serialise_with_a_discriminating_kind() {
        let obs = Observation::Powerplay {
            ts: "2026-08-25T00:00:00Z".into(),
            system: "Deciat".into(),
            controlling_power: Some("A. Lavigny-Duval".into()),
            powerplay_state: Some("Stronghold".into()),
            control_progress: Some(0.43),
            reinforcement: Some(1),
            undermining: Some(2),
        };
        let json = serde_json::to_string(&obs).unwrap();
        assert!(json.contains(r#""kind":"powerplay""#));
        assert!(json.contains("Deciat"));
    }

    #[test]
    fn capture_is_idempotent_across_repeated_ebexs() {
        // Sync runs on every journal write, so a single award must not be
        // recorded dozens of times over a session.
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO events (file,offset,ts,event,raw) VALUES
               ('J.log',0,'2026-08-25T10:00:00Z','MarketSell',
                '{\"timestamp\":\"2026-08-25T10:00:00Z\",\"event\":\"MarketSell\",\"Type\":\"gold\",\"Count\":10,\"TotalSale\":1000,\"AvgPricePaid\":0}');
             INSERT INTO merit_events (file,offset,ts,power,merits_gained,total_merits)
               VALUES ('J.log',1,'2026-08-25T10:00:01Z','Aisling Duval',7,7);",
        )
        .unwrap();

        let mut seen = std::collections::HashSet::new();
        assert_eq!(crate::merit_capture::capture(&conn, &mut seen), 1);
        assert_eq!(crate::merit_capture::capture(&conn, &mut seen), 0);
    }
}

/// Warn/error callsite counts for the telemetry shipper (client telemetry
/// law, ledger 2026-09-05): TARGET and LEVEL only — the rendered message,
/// which can name star systems, never leaves the process through this
/// path. Distinct callsites cap at 128 per drain window; excess folds
/// into "_overflow" (mirrors the server''s own guard).
static EVENT_COUNTS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<(String, &'static str), u32>>,
> = std::sync::OnceLock::new();

pub struct WarnErrorCounts;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for WarnErrorCounts {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let level = *event.metadata().level();
        let level = if level == tracing::Level::ERROR {
            "error"
        } else if level == tracing::Level::WARN {
            "warn"
        } else {
            return;
        };
        let mut counts = EVENT_COUNTS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let target = event.metadata().target();
        let key = if counts.len() >= 128 && !counts.contains_key(&(target.to_owned(), level)) {
            ("_overflow".to_owned(), level)
        } else {
            (target.to_owned(), level)
        };
        *counts.entry(key).or_insert(0) += 1;
    }
}

/// Take and clear the accumulated warn/error counts.
pub fn drain_event_counts() -> Vec<(String, &'static str, u32)> {
    let mut counts = EVENT_COUNTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    counts.drain().map(|((t, l), c)| (t, l, c)).collect()
}

#[cfg(test)]
mod count_tests {
    /// The layer counts warn/error by TARGET and level — never message
    /// content — and drain clears.
    #[test]
    fn warn_error_events_count_by_callsite_and_drain_clears() {
        use tracing_subscriber::layer::SubscriberExt as _;
        let subscriber = tracing_subscriber::registry().with(super::WarnErrorCounts);
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(target: "edda::test_a", "message with a SECRET SYSTEM NAME");
            tracing::warn!(target: "edda::test_a", "again");
            tracing::error!(target: "edda::test_b", "boom");
            tracing::info!(target: "edda::test_c", "info never counts");
        });
        let mut drained = super::drain_event_counts();
        drained.sort();
        let ours: Vec<_> = drained
            .iter()
            .filter(|(t, _, _)| t.starts_with("edda::test"))
            .collect();
        assert_eq!(ours.len(), 2);
        assert!(ours
            .iter()
            .any(|(t, l, c)| t == "edda::test_a" && *l == "warn" && *c == 2));
        assert!(ours
            .iter()
            .any(|(t, l, c)| t == "edda::test_b" && *l == "error" && *c == 1));
        for (t, _, _) in &drained {
            assert!(!t.contains("SECRET"), "only targets travel, never messages");
        }
        assert!(
            super::drain_event_counts()
                .iter()
                .all(|(t, _, _)| !t.starts_with("edda::test")),
            "drain clears"
        );
    }
}
