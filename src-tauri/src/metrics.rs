//! Wired-but-silent telemetry for the desktop app (ROUTING-NEXT item 15).
//!
//! The app emits through the same `metrics` facade as the server, with
//! the same series names (`edda_route_wall_seconds` here is the very
//! series a future server-side routing endpoint exports), and the
//! recorder collects **locally only**: nothing leaves the machine.
//! `metrics_snapshot` hands the Prometheus text to the frontend for a
//! diagnostics view; if telemetry ever ships, it ships as an explicit
//! opt-in on top of this, never as a default.

use std::sync::OnceLock;

use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Same duration buckets as the server, so a dashboard built against
/// one reads the other.
const SECONDS_BUCKETS: &[f64] = &[
    0.001, 0.003, 0.01, 0.03, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0, 60.0,
];

/// Install the process-local recorder. Idempotent-by-outcome: a second
/// call (dev-server reloads) leaves the first recorder standing and
/// logs instead of panicking.
pub fn install() {
    let recorder = PrometheusBuilder::new().set_buckets_for_metric(Matcher::Suffix("_seconds".into()), SECONDS_BUCKETS);
    match recorder.and_then(|b| b.install_recorder()) {
        Ok(handle) => {
            // Stale histogram samples age out even if nobody ever opens
            // the diagnostics view.
            let upkeep = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
                upkeep.run_upkeep();
            });
            let _ = HANDLE.set(handle);
            metrics::gauge!("edda_build_info", "version" => env!("CARGO_PKG_VERSION")).set(1.0);
        }
        Err(error) => tracing::warn!(%error, "metrics recorder not installed"),
    }
}

/// Everything gathered so far, as Prometheus text. Local diagnostics
/// only — this string is rendered for the user, never transmitted.
#[tauri::command]
pub fn metrics_snapshot() -> Result<String, String> {
    HANDLE.get().map(|h| h.render()).ok_or_else(|| "metrics not initialized".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_collects_and_snapshot_renders() {
        install();
        metrics::counter!("edda_route_requests_total", "outcome" => "ok").increment(1);
        metrics::histogram!("edda_route_wall_seconds").record(0.5);
        let text = metrics_snapshot().expect("installed");
        assert!(text.contains(r#"edda_route_requests_total{outcome="ok"} 1"#), "{text}");
        assert!(text.contains(r#"edda_route_wall_seconds_bucket{le="1"} 1"#), "{text}");
        // A second install must not panic or wipe what's collected.
        install();
        assert!(metrics_snapshot().expect("still installed").contains("edda_route_requests_total"));
    }
}
