//! Is the game running? Polled from the process list, so it is true from
//! the launcher handing over to the client until the client exits, without
//! waiting for a journal line. Transitions are announced and broadcast as
//! `game-state` events for the header pill.

use crate::callouts::Callout;
use crate::events::{EmitExt, GAME_STATE};
use crate::jobs::sleep_unless_cancelled;
use crate::state::AppState;
use crate::watcher::Announcer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};
use tauri::State;
use tokio_util::sync::CancellationToken;

pub const POLL_EVERY: Duration = Duration::from_secs(3);

fn game_process_present(sys: &mut System) -> bool {
    let names = crate::platform::game_process_names();
    if names.is_empty() {
        return false; // no client on this host; nothing to scan for
    }
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Never),
    );
    sys.processes().values().any(|p| {
        let name = p.name().to_string_lossy();
        names.iter().any(|n| name.eq_ignore_ascii_case(n))
    })
}

/// One observation of the process list, applied to `running`: emits the
/// state on the first call and on every change, and speaks the change.
pub fn observe(now: bool, first: bool, running: &AtomicBool, announcer: &Announcer) {
    let was = running.swap(now, Ordering::SeqCst);
    if first || now != was {
        announcer
            .events
            .emit(GAME_STATE, serde_json::json!({ "running": now }));
        if !first {
            let ts = chrono::Utc::now().to_rfc3339();
            let text = if now {
                "Game detected. Ship computer online and listening.".to_string()
            } else {
                "Game closed. Ship computer standing by.".to_string()
            };
            announcer.deliver(vec![(
                Callout {
                    kind: "game",
                    text,
                    priority: 3,
                    speak: true,
                    ts,
                },
                None,
            )]);
        }
    }
}

/// The game-poll job.
pub async fn run(
    token: CancellationToken,
    running: Arc<AtomicBool>,
    announcer: Announcer,
    every: Duration,
) {
    let mut sys = System::new_with_specifics(RefreshKind::nothing());
    let mut first = true;
    loop {
        let now = game_process_present(&mut sys);
        observe(now, first, &running, &announcer);
        first = false;
        if !sleep_unless_cancelled(&token, every).await {
            return;
        }
    }
}

#[tauri::command]
pub fn game_state(state: State<'_, AppState>) -> serde_json::Value {
    serde_json::json!({ "running": state.game_running.load(Ordering::SeqCst) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Recording;

    #[test]
    fn transitions_are_broadcast_and_spoken_once_each() {
        let dir = tempfile::tempdir().unwrap();
        let state = crate::state::test_state(dir.path());
        let rec = Arc::new(Recording::default());
        state.events.install(rec.clone());
        let announcer = state.announcer();
        let running = AtomicBool::new(false);

        observe(false, true, &running, &announcer);
        assert_eq!(
            rec.names(),
            vec![GAME_STATE],
            "first poll reports the state, silently"
        );
        observe(false, false, &running, &announcer);
        assert_eq!(rec.names().len(), 1, "no change, no event");
        observe(true, false, &running, &announcer);
        assert!(running.load(Ordering::SeqCst));
        let names = rec.names();
        assert_eq!(names, vec![GAME_STATE, GAME_STATE, crate::events::CALLOUT]);
        assert_eq!(
            rec.last(GAME_STATE),
            Some(serde_json::json!({ "running": true }))
        );
        let said = rec.last(crate::events::CALLOUT).unwrap();
        assert!(
            said["text"].as_str().unwrap().contains("Game detected"),
            "{said}"
        );
    }
}
