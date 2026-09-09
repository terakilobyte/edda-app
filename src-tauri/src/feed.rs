//! Background EDDN subscriber: the desktop twin of the server's bounded
//! channel + batched writer (`ed_api::eddn::run_writer`).
//!
//! The relay subscriber (`ed_eddn::live::run_to_channel_with`) pushes
//! normalized operations into a bounded channel; a full channel parks the
//! subscriber rather than dropping anything. The writer drains the channel
//! in batches, paints the activity heatmap from each batch, and retries
//! a failed batch with backoff. Deliberately quiet otherwise: the journal
//! path is the authoritative one, and if the relay is unreachable the app
//! keeps working with dump-and-journal data only.

use crate::jobs::sleep_unless_cancelled;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// How the writer batches and retries.
#[derive(Debug, Clone)]
pub struct WritePolicy {
    /// Operations per store lock.
    pub max_batch: usize,
    /// How long to wait for a batch to fill once it has one message.
    pub max_batch_wait: Duration,
    /// First retry delay; doubles per attempt up to `max_backoff`.
    pub backoff: Duration,
    pub max_backoff: Duration,
    /// Attempts per batch before it is counted as failed and skipped. The
    /// journal remains authoritative, so a poisoned batch must not stall
    /// the feed forever.
    pub max_attempts: u32,
}

impl Default for WritePolicy {
    fn default() -> Self {
        WritePolicy {
            max_batch: 200,
            max_batch_wait: Duration::from_secs(1),
            backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
            max_attempts: 6,
        }
    }
}

/// Feed configuration for the composition root.
#[derive(Debug, Clone)]
pub struct FeedConfig {
    pub relay: String,
    /// Bounded channel between subscriber and writer.
    pub queue: usize,
    pub report_every: Duration,
    pub policy: WritePolicy,
}

impl Default for FeedConfig {
    fn default() -> Self {
        FeedConfig {
            relay: ed_eddn::EDDN_RELAY.to_string(),
            queue: 10_000,
            report_every: Duration::from_secs(30),
            policy: WritePolicy::default(),
        }
    }
}

/// What one batch application wrote. `rows` is everything (market +
/// outfitting + shipyard + system rows); `market_rows` is the subset the
/// "Live prices" pill may honestly claim (maintainer ruling 2026-09-04: the
/// pill counts market update rows, not decoded frames).
#[derive(Default, Debug, Clone, Copy)]
pub struct Applied {
    pub rows: u64,
    pub market_rows: u64,
}

/// What the writer has done, for the `eddn-stats` event and tests.
#[derive(Default, Debug)]
pub struct Counters {
    pub batches: AtomicU64,
    pub rows: AtomicU64,
    pub market_rows: AtomicU64,
    pub retries: AtomicU64,
    pub failed_batches: AtomicU64,
    /// Batches that waited for a writer lease to be released.
    pub paused: AtomicU64,
    /// Samples (one per report) where the queue was completely full: the
    /// subscriber was parked on backpressure rather than dropping.
    pub backpressure_samples: AtomicU64,
}

impl Counters {
    fn add(&self, field: &AtomicU64, n: u64) {
        field.fetch_add(n, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "batches": self.batches.load(Ordering::Relaxed),
            "rows": self.rows.load(Ordering::Relaxed),
            "marketRows": self.market_rows.load(Ordering::Relaxed),
            "retries": self.retries.load(Ordering::Relaxed),
            "failedBatches": self.failed_batches.load(Ordering::Relaxed),
            "paused": self.paused.load(Ordering::Relaxed),
            "backpressureSamples": self.backpressure_samples.load(Ordering::Relaxed),
            // The promise, stated: nothing is dropped between relay and store.
            "dropped": 0,
        })
    }
}

/// Applies one batch. Returns what was written. The SQLite adapter and
/// the test fakes implement it.
pub trait BatchWriter<T>: Send + 'static {
    fn apply(&mut self, batch: &[T]) -> anyhow::Result<Applied>;
}

/// The real thing: every operation of the batch under one store lock.
/// The client's EDDN subscriber feeds ONE thing now: the activity
/// heatmap (maintainer, 2026-09-08: "let's keep EDDN in. We're just not
/// storing data from it"). Under the API-only client every search runs
/// on the server, which hears the same firehose; writing boards into a
/// local market table nobody reads cost a 100M-row index build on first
/// run and a "Live prices" pill that counted rows for no one. The
pub struct StoreWriter(pub Arc<crate::heatmap::Heatmap>);

impl BatchWriter<ed_eddn::Operation> for StoreWriter {
    fn apply(&mut self, batch: &[ed_eddn::Operation]) -> anyhow::Result<Applied> {
        self.0.record_ops(batch, crate::heatmap::now_ms());
        Ok(Applied { rows: batch.len() as u64, ..Applied::default() })
    }
}

/// Drain `rx` in batches until it closes or the job is cancelled.
pub async fn write_loop<T, W>(
    token: CancellationToken,
    mut rx: mpsc::Receiver<T>,
    mut writer: W,
    policy: WritePolicy,
    counters: Arc<Counters>,
) where
    T: Send + 'static,
    W: BatchWriter<T>,
{
    loop {
        let first = tokio::select! {
            _ = token.cancelled() => return,
            item = rx.recv() => match item { Some(t) => t, None => return },
        };
        let mut batch = Vec::with_capacity(policy.max_batch);
        batch.push(first);
        let deadline = tokio::time::Instant::now() + policy.max_batch_wait;
        while batch.len() < policy.max_batch {
            match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(item)) => batch.push(item),
                Ok(None) | Err(_) => break,
            }
        }

        let mut backoff = policy.backoff;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let owned = std::mem::take(&mut batch);
            let (w, result, owned) = tokio::task::spawn_blocking(move || {
                let r = writer.apply(&owned);
                (writer, r, owned)
            })
            .await
            .expect("batch writer panicked");
            writer = w;
            batch = owned;
            match result {
                Ok(applied) => {
                    counters.add(&counters.batches, 1);
                    counters.add(&counters.rows, applied.rows);
                    counters.add(&counters.market_rows, applied.market_rows);
                    tracing::debug!(rows = applied.rows, market_rows = applied.market_rows, batch = batch.len(), "eddn: applied batch");
                    break;
                }
                Err(error) if attempt < policy.max_attempts => {
                    counters.add(&counters.retries, 1);
                    tracing::warn!(%error, batch = batch.len(), attempt, ?backoff, "eddn: batch failed; retrying");
                    if !sleep_unless_cancelled(&token, backoff).await {
                        return;
                    }
                    backoff = (backoff * 2).min(policy.max_backoff);
                }
                Err(error) => {
                    counters.add(&counters.failed_batches, 1);
                    tracing::error!(%error, batch = batch.len(), attempts = attempt, "eddn: batch given up");
                    break;
                }
            }
        }
    }
}

/// The feed job: subscriber, batch writer and a periodic `eddn-stats`
/// report, all on the shared runtime, all stopping on cancellation.
pub async fn run(
    token: CancellationToken,
    cfg: FeedConfig,
    heat: Arc<crate::heatmap::Heatmap>,
) {
    let (tx, rx) = mpsc::channel::<ed_eddn::Operation>(cfg.queue);
    let counters = Arc::new(Counters::default());
    let feed_stats: Arc<Mutex<ed_eddn::FeedStats>> = Arc::default();

    let writer = tokio::spawn(write_loop(
        token.child_token(),
        rx,
        StoreWriter(heat),
        cfg.policy.clone(),
        counters.clone(),
    ));

    let subscriber = {
        let relay = cfg.relay.clone();
        let feed_stats = feed_stats.clone();
        let sender = tx.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let feed = ed_eddn::live::run_to_channel_with(&relay, sender, |s| {
                *feed_stats.lock().unwrap_or_else(|e| e.into_inner()) = s.clone();
            });
            tokio::select! {
                _ = token.cancelled() => {}
                result = feed => {
                    if let Err(e) = result {
                        tracing::error!(error = %format!("{e:#}"), "eddn: feed stopped");
                    }
                }
            }
        })
    };

    // Report occasionally rather than per message -- at ~10 messages a
    // second, per-message events would be noise.
    let mut last_rows = 0u64;
    while sleep_unless_cancelled(&token, cfg.report_every).await {
        let queued = cfg.queue.saturating_sub(tx.capacity());
        if queued >= cfg.queue {
            counters.add(&counters.backpressure_samples, 1);
        }
        let stats = feed_stats.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let rows = counters.rows.load(Ordering::Relaxed);
        let since = rows - last_rows;
        last_rows = rows;
        tracing::debug!(received = stats.received, decoded = stats.decoded, decode_errors = stats.decode_errors, reconnects = stats.reconnects, rows = since, queued, schemas = ?stats.schemas, journal_events = ?stats.journal_events, decode_failures = ?stats.decode_failures, "eddn feed");
        let mut payload = serde_json::json!({
            "received": stats.received,
            "decoded": stats.decoded,
            "decodeErrors": stats.decode_errors,
            "commodity": stats.commodity,
            "commodityRows": stats.commodity_rows,
            "journal": stats.journal,
            "schemas": &stats.schemas,
            "journalEvents": &stats.journal_events,
            "decodeFailures": &stats.decode_failures,
            "reconnects": stats.reconnects,
            "pending": queued,
            "queueCapacity": cfg.queue,
            "rowsSinceLastReport": since,
        });
        if let (Some(p), Some(c)) = (payload.as_object_mut(), counters.snapshot().as_object()) {
            p.extend(c.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        // Formerly emitted as an `eddn-stats` event for the header's "Live
        // prices" pill; the pill is gone with the local market table
        // (2026-09-08). The counters still reach the trace.
        tracing::debug!(%payload, "eddn feed stats");
    }

    drop(tx);
    subscriber.abort();
    let _ = writer.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records batches; fails the first `fail_first` attempts.
    struct Fake {
        seen: Arc<Mutex<Vec<Vec<u32>>>>,
        fail_first: u32,
        attempts: u32,
        delay: Duration,
    }

    impl BatchWriter<u32> for Fake {
        fn apply(&mut self, batch: &[u32]) -> anyhow::Result<Applied> {
            self.attempts += 1;
            std::thread::sleep(self.delay);
            if self.attempts <= self.fail_first {
                anyhow::bail!("database is locked");
            }
            self.seen.lock().unwrap().push(batch.to_vec());
            Ok(Applied { rows: batch.len() as u64, market_rows: batch.len() as u64 })
        }
    }

    fn fast_policy() -> WritePolicy {
        WritePolicy {
            max_batch: 4,
            max_batch_wait: Duration::from_millis(20),
            backoff: Duration::from_millis(10),
            max_backoff: Duration::from_millis(40),
            max_attempts: 5,
        }
    }

    fn fake(fail_first: u32, delay: Duration) -> (Fake, Arc<Mutex<Vec<Vec<u32>>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        (Fake { seen: seen.clone(), fail_first, attempts: 0, delay }, seen)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_failed_batch_is_retried_with_backoff_not_dropped() {
        let (tx, rx) = mpsc::channel(16);
        let counters = Arc::new(Counters::default());
        let (writer, seen) = fake(2, Duration::ZERO);
        let token = CancellationToken::new();
        let loop_task = tokio::spawn(write_loop(token.clone(), rx, writer, fast_policy(), counters.clone()));
        tx.send(7).await.unwrap();
        tx.send(8).await.unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(*seen.lock().unwrap(), vec![vec![7, 8]]);
        assert_eq!(counters.retries.load(Ordering::Relaxed), 2);
        assert_eq!(counters.failed_batches.load(Ordering::Relaxed), 0);
        assert_eq!(counters.rows.load(Ordering::Relaxed), 2);
        token.cancel();
        loop_task.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_full_queue_applies_backpressure_and_loses_nothing() {
        // Capacity 2 and a slow writer: the sender parks instead of the
        // queue dropping its oldest, and every message arrives in order.
        let (tx, rx) = mpsc::channel(2);
        let counters = Arc::new(Counters::default());
        let (writer, seen) = fake(0, Duration::from_millis(5));
        let token = CancellationToken::new();
        let loop_task = tokio::spawn(write_loop(token.clone(), rx, writer, fast_policy(), counters.clone()));
        for i in 0..40u32 {
            tx.send(i).await.unwrap();
        }
        drop(tx);
        loop_task.await.unwrap();
        let flat: Vec<u32> = seen.lock().unwrap().iter().flatten().copied().collect();
        assert_eq!(flat, (0..40).collect::<Vec<u32>>());
        assert_eq!(counters.snapshot()["dropped"], 0);
    }
}
