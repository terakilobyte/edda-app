//! One runtime, one supervisor, every background task a named job.
//!
//! Before this the app ran nine background tasks under four runtime
//! models -- bare threads with sleep loops, a private single-thread Tokio
//! runtime, `tauri::async_runtime::spawn`, named threads -- with no
//! shutdown path and five unrelated cancel flags. Now every task is a
//! [`Job`] on the shared Tokio runtime with a `CancellationToken` and a
//! join handle, and app exit cancels and joins them all with a timeout.
//!
//! Command-facing cancellation ("cancel search", "cancel import") is the
//! same token: a command registers a job name with [`Supervisor::begin`]
//! and the matching cancel command calls [`Supervisor::cancel`].

use std::collections::HashMap;
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// Job names. One place, so a cancel command and the job it cancels agree.
pub const INITIAL_SYNC: &str = "initial-sync";
pub const JOURNAL_WATCHER: &str = "journal-watcher";
pub const GAME_POLL: &str = "game-poll";
pub const EDDN_FEED: &str = "eddn-feed";
pub const TELEMETRY: &str = "telemetry";
pub const SPEECH_ENGINE_START: &str = "speech-engine-start";
pub const EVAL_DEV_HOOK: &str = "eval-dev-hook";
pub const STAR_BACKFILL: &str = "star-backfill";
pub const ROUTE_PLOT: &str = "route-plot";
pub const HIGHWAY_REBUILD: &str = "highway-rebuild";
pub const PROFIT_SEARCH: &str = "profit-search";

struct Job {
    token: CancellationToken,
    /// `None` for a token-only registration (a command's own blocking
    /// task that only needs a cancel handle, not supervision).
    handle: Option<JoinHandle<()>>,
}

/// What shutdown found: which jobs joined in time and which did not.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ShutdownReport {
    pub joined: Vec<&'static str>,
    pub timed_out: Vec<&'static str>,
}

pub struct Supervisor {
    runtime: Handle,
    root: CancellationToken,
    jobs: Mutex<HashMap<&'static str, Job>>,
}

impl Supervisor {
    pub fn new(runtime: Handle) -> Self {
        Supervisor {
            runtime,
            root: CancellationToken::new(),
            jobs: Mutex::new(HashMap::new()),
        }
    }

    fn jobs(&self) -> std::sync::MutexGuard<'_, HashMap<&'static str, Job>> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Register `name` and hand back its token. A previous job under the
    /// same name keeps its own token: a new search does not un-cancel an
    /// old one, and cancelling the new one leaves the old one alone.
    pub fn begin(&self, name: &'static str) -> CancellationToken {
        let token = self.root.child_token();
        self.jobs().insert(
            name,
            Job {
                token: token.clone(),
                handle: None,
            },
        );
        token
    }

    /// Spawn an async job. `Err` if a job of that name is still running.
    pub fn spawn<F, Fut>(&self, name: &'static str, f: F) -> Result<(), AlreadyRunning>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut jobs = self.jobs();
        if jobs
            .get(name)
            .is_some_and(|j| j.handle.as_ref().is_some_and(|h| !h.is_finished()))
        {
            return Err(AlreadyRunning(name));
        }
        let token = self.root.child_token();
        let handle = self.runtime.spawn(f(token.clone()));
        jobs.insert(
            name,
            Job {
                token,
                handle: Some(handle),
            },
        );
        Ok(())
    }

    /// Spawn a blocking job on the runtime's blocking pool. The body must
    /// poll `token.is_cancelled()`; a blocking thread cannot be aborted.
    pub fn spawn_blocking<F>(&self, name: &'static str, f: F) -> Result<(), AlreadyRunning>
    where
        F: FnOnce(CancellationToken) + Send + 'static,
    {
        let mut jobs = self.jobs();
        if jobs
            .get(name)
            .is_some_and(|j| j.handle.as_ref().is_some_and(|h| !h.is_finished()))
        {
            return Err(AlreadyRunning(name));
        }
        let token = self.root.child_token();
        let t = token.clone();
        let handle = self.runtime.spawn_blocking(move || f(t));
        jobs.insert(
            name,
            Job {
                token,
                handle: Some(handle),
            },
        );
        Ok(())
    }

    pub fn cancel(&self, name: &'static str) {
        if let Some(job) = self.jobs().get(name) {
            job.token.cancel();
        }
    }

    /// Names of the jobs currently running, sorted.
    pub fn running(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self
            .jobs()
            .iter()
            .filter(|(_, j)| j.handle.as_ref().is_some_and(|h| !h.is_finished()))
            .map(|(n, _)| *n)
            .collect();
        names.sort();
        names
    }

    /// Cancel everything and wait up to `timeout` for each job to join.
    /// Tell every job to stop, without waiting for any of it. Called on
    /// a DELIBERATE quit while the windows are still tearing down, so
    /// that by the time [`Self::shutdown`] runs there is little left to
    /// join -- see the comment on `RunEvent::Exit` in lib.rs for why the
    /// waiting must not happen on the UI thread.
    pub fn cancel_all(&self) {
        self.root.cancel();
    }

    pub async fn shutdown(&self, timeout: Duration) -> ShutdownReport {
        self.root.cancel();
        let handles: Vec<(&'static str, JoinHandle<()>)> = self
            .jobs()
            .iter_mut()
            .filter_map(|(name, job)| job.handle.take().map(|h| (*name, h)))
            .collect();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut report = ShutdownReport::default();
        for (name, handle) in handles {
            match tokio::time::timeout_at(deadline, handle).await {
                Ok(_) => report.joined.push(name),
                Err(_) => report.timed_out.push(name),
            }
        }
        report.joined.sort();
        report.timed_out.sort();
        tracing::info!(joined = ?report.joined, timed_out = ?report.timed_out, "background jobs stopped");
        report
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadyRunning(pub &'static str);

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is already running", self.0)
    }
}

/// Sleep that ends early when the job is cancelled. `true` if it slept
/// the whole way; `false` means stop.
pub async fn sleep_unless_cancelled(token: &CancellationToken, dur: Duration) -> bool {
    tokio::select! {
        _ = token.cancelled() => false,
        _ = tokio::time::sleep(dur) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn shutdown_cancels_and_joins_every_job() {
        let sup = Supervisor::new(Handle::current());
        let ticks = Arc::new(AtomicU32::new(0));
        let stopped = Arc::new(AtomicBool::new(false));
        let (t, s) = (ticks.clone(), stopped.clone());
        sup.spawn(GAME_POLL, move |token| async move {
            while sleep_unless_cancelled(&token, Duration::from_millis(5)).await {
                t.fetch_add(1, Ordering::SeqCst);
            }
            s.store(true, Ordering::SeqCst);
        })
        .unwrap();
        let s2 = Arc::new(AtomicBool::new(false));
        let s3 = s2.clone();
        sup.spawn_blocking(JOURNAL_WATCHER, move |token| {
            while !token.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
            s3.store(true, Ordering::SeqCst);
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(sup.running(), vec![GAME_POLL, JOURNAL_WATCHER]);

        let report = sup.shutdown(Duration::from_secs(2)).await;
        assert_eq!(
            report,
            ShutdownReport {
                joined: vec![GAME_POLL, JOURNAL_WATCHER],
                timed_out: vec![]
            }
        );
        assert!(stopped.load(Ordering::SeqCst) && s2.load(Ordering::SeqCst));
        assert!(ticks.load(Ordering::SeqCst) > 0);
        assert!(sup.running().is_empty());
    }

    #[tokio::test]
    async fn a_job_that_ignores_cancellation_is_reported_not_waited_on_forever() {
        let sup = Supervisor::new(Handle::current());
        sup.spawn(EDDN_FEED, |_token| async {
            tokio::time::sleep(Duration::from_secs(60)).await;
        })
        .unwrap();
        let report = sup.shutdown(Duration::from_millis(50)).await;
        assert_eq!(report.timed_out, vec![EDDN_FEED]);
        assert!(report.joined.is_empty());
    }

    #[tokio::test]
    async fn one_job_per_name_and_cancel_targets_the_current_registration() {
        const JOB: &str = "test-job";
        let sup = Supervisor::new(Handle::current());
        sup.spawn(JOB, |token| async move { token.cancelled().await })
            .unwrap();
        assert_eq!(sup.spawn(JOB, |_| async {}), Err(AlreadyRunning(JOB)));
        assert_eq!(sup.running(), vec![JOB]);

        // Command-style: a token per search, cancel hits only the newest.
        let first = sup.begin(PROFIT_SEARCH);
        let second = sup.begin(PROFIT_SEARCH);
        sup.cancel(PROFIT_SEARCH);
        assert!(second.is_cancelled());
        assert!(!first.is_cancelled());

        sup.cancel(JOB);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(sup.running().is_empty());
        sup.spawn(JOB, |_| async {}).unwrap();
    }
}
