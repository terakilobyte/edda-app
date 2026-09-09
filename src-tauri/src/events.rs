//! Every event the backend sends to the windows, and the seam it sends
//! them through.
//!
//! One `pub const` per channel, string literal on the same line: the
//! frontend contract test (`frontend/src/test/contracts.test.js`) greps
//! this file, so a renamed or orphaned event fails a test instead of
//! silently reaching nobody.
//!
//! [`Emitter`] is the seam. Background jobs and tool executors take
//! `&dyn Emitter` (or an `Arc`), never an `AppHandle`, so they run under a
//! test with [`Recording`] and under Tauri with [`TauriEmitter`].

use serde::Serialize;
use serde_json::Value;
use std::sync::{Arc, Mutex, RwLock};

pub const SYNC_PROGRESS: &str = "sync-progress";
pub const SYNC_COMPLETE: &str = "sync-complete";
pub const JOURNAL_CHANGED: &str = "journal-changed";
pub const CALLOUT: &str = "callout";
pub const GAME_STATE: &str = "game-state";
pub const OVERLAY_INTERACTIVE: &str = "overlay-interactive";
/// Item 43: fired on a real JetConeBoost so the HUD can light the
/// next-target box the moment the drive is actually supercharged.
pub const SUPERCHARGE: &str = "supercharge";
pub const KNOWLEDGE_PROGRESS: &str = "knowledge-progress";
pub const ROUTE_PROGRESS: &str = "route-progress";
pub const ROUTE_CANDIDATE: &str = "route-candidate";
pub const ROUTE_REPLANNED: &str = "route-replanned";
pub const ROUTE_FOLLOW: &str = "route-follow";
pub const LISTEN_STATE: &str = "listen-state";
pub const LISTEN_HEARD: &str = "listen-heard";
pub const LISTEN_PARTIAL: &str = "listen-partial";
pub const LISTEN_REPLY: &str = "listen-reply";
pub const TRADE_FOLLOW: &str = "trade-follow";
pub const CARRIER_ROUTE: &str = "carrier-route";
pub const LISTEN_SETUP: &str = "listen-setup";
pub const SPEECH_ENGINE_PROGRESS: &str = "speech-engine-progress";
pub const APP_UPDATE: &str = "app-update";

/// Where events go. Object-safe on purpose: jobs hold `Arc<dyn Emitter>`.
pub trait Emitter: Send + Sync {
    fn emit_value(&self, event: &'static str, payload: Value);
}

/// `emit` with any serialisable payload, for every `Emitter`.
pub trait EmitExt {
    fn emit<T: Serialize>(&self, event: &'static str, payload: T);
}

impl<E: Emitter + ?Sized> EmitExt for E {
    fn emit<T: Serialize>(&self, event: &'static str, payload: T) {
        match serde_json::to_value(payload) {
            Ok(v) => self.emit_value(event, v),
            Err(e) => tracing::warn!(event, error = %e, "event payload not serialisable"),
        }
    }
}

/// The live adapter: every window, through Tauri.
pub struct TauriEmitter(pub tauri::AppHandle);

impl Emitter for TauriEmitter {
    fn emit_value(&self, event: &'static str, payload: Value) {
        use tauri::Emitter as _;
        if let Err(e) = self.0.emit(event, payload) {
            tracing::debug!(event, error = %e, "emit failed");
        }
    }
}

/// The test adapter: remembers what would have reached the windows.
#[derive(Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct Recording {
    events: Mutex<Vec<(&'static str, Value)>>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl Recording {
    pub fn take(&self) -> Vec<(&'static str, Value)> {
        std::mem::take(&mut *self.events.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(n, _)| *n).collect()
    }

    pub fn last(&self, event: &str) -> Option<Value> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .rev()
            .find(|(n, _)| *n == event)
            .map(|(_, v)| v.clone())
    }

}

impl Emitter for Recording {
    fn emit_value(&self, event: &'static str, payload: Value) {
        self.events.lock().unwrap_or_else(|e| e.into_inner()).push((event, payload));
    }
}

/// The app's outbox, owned by `AppState` from the first line of `run()`.
///
/// `AppState` exists before Tauri does, so code that only has the state
/// (the AI tool executor, callout delivery) emits through this and the
/// composition root installs the live adapter once the app is up. Until
/// then events are logged and dropped: no window exists to receive them.
#[derive(Default)]
pub struct EventBus {
    inner: RwLock<Option<Arc<dyn Emitter>>>,
}

impl EventBus {
    pub fn install(&self, emitter: Arc<dyn Emitter>) {
        *self.inner.write().unwrap_or_else(|e| e.into_inner()) = Some(emitter);
    }
}

impl Emitter for EventBus {
    fn emit_value(&self, event: &'static str, payload: Value) {
        let target = self.inner.read().unwrap_or_else(|e| e.into_inner()).clone();
        match target {
            Some(e) => e.emit_value(event, payload),
            None => tracing::debug!(event, "event before any window exists; dropped"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_emitter_keeps_events_in_order_with_payloads() {
        let rec = Recording::default();
        rec.emit(SYNC_PROGRESS, serde_json::json!({ "done": 1 }));
        rec.emit(SYNC_COMPLETE, 42u64);
        assert_eq!(rec.names(), vec![SYNC_PROGRESS, SYNC_COMPLETE]);
        assert_eq!(rec.last(SYNC_COMPLETE), Some(serde_json::json!(42)));
        assert_eq!(rec.take().len(), 2);
        assert!(rec.take().is_empty());
    }

    #[test]
    fn event_bus_forwards_once_an_adapter_is_installed() {
        let bus = EventBus::default();
        bus.emit(GAME_STATE, serde_json::json!({ "running": false }));
        let rec = Arc::new(Recording::default());
        bus.install(rec.clone());
        bus.emit(GAME_STATE, serde_json::json!({ "running": true }));
        assert_eq!(rec.names(), vec![GAME_STATE]);
        assert_eq!(rec.last(GAME_STATE), Some(serde_json::json!({ "running": true })));
    }
}
