//! Source-level contracts for the background-work seams: where event names
//! live, who may own a thread or an HTTP client, and which modules may keep
//! process-global state. Cheap to run, and they fail the moment somebody
//! reintroduces a second adapter for a seam that is meant to have one.

#![cfg(test)]

use std::path::{Path, PathBuf};

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(src_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            out.push((name, std::fs::read_to_string(&path).unwrap()));
        }
    }
    out.sort();
    out
}

/// Files (other than `except` and this one) whose text contains `needle`.
fn files_containing(needle: &str, except: &[&str]) -> Vec<String> {
    sources()
        .into_iter()
        .filter(|(name, text)| {
            name != "contracts.rs" && !except.contains(&name.as_str()) && text.contains(needle)
        })
        .map(|(name, _)| name)
        .collect()
}

/// A literal channel name in an emit call outside events.rs is one nobody can
/// find from the table. Every event is a `pub const` in one file, and the
/// frontend contract test greps that file.
#[test]
fn event_names_are_declared_only_in_events_rs() {
    let offenders: Vec<String> = sources()
        .into_iter()
        .filter(|(name, text)| {
            let squeezed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            name != "events.rs" && name != "contracts.rs" && squeezed.contains(".emit(\"")
        })
        .map(|(name, _)| name)
        .collect();
    assert_eq!(
        offenders,
        Vec::<String>::new(),
        "literal event names outside events.rs"
    );
    let events = std::fs::read_to_string(src_dir().join("events.rs")).unwrap();
    let declared = events
        .lines()
        .filter(|l| l.contains("pub const") && l.contains("&str = \""))
        .count();
    // 21 since B.4 (community-sync, maintenance and search-progress went
    // with the local data); the bound catches a table that lost a row by
    // accident, not one trimmed on purpose.
    assert!(
        declared >= 21,
        "events.rs declares {declared} events; expected the whole table"
    );
}

/// Background work runs on the one runtime under the supervisor: no job
/// module spawns its own thread loop or builds a private Tokio runtime,
/// and app exit shuts the supervisor down.
#[test]
fn background_tasks_run_on_the_supervised_runtime() {
    assert_eq!(
        files_containing("tokio::runtime::Builder", &[]),
        Vec::<String>::new()
    );
    let job_modules = [
        "lib.rs",
        "feed.rs",
        "game.rs",
        "watcher.rs",
        "eval.rs",
        "spansh.rs",
        "exchange.rs",
    ];
    for needle in ["std::thread::spawn", "std::thread::Builder"] {
        let offenders: Vec<String> = files_containing(needle, &[])
            .into_iter()
            .filter(|name| job_modules.contains(&name.as_str()))
            .collect();
        assert_eq!(
            offenders,
            Vec::<String>::new(),
            "job modules that still own a thread ({needle})"
        );
    }
    let lib = std::fs::read_to_string(src_dir().join("lib.rs")).unwrap();
    assert!(
        lib.contains("RunEvent::Exit"),
        "app exit must shut the supervisor down"
    );
}

/// One HTTP client per flavour, built once with the app's user agent.
#[test]
fn http_clients_are_built_once_on_app_state() {
    for needle in [
        "reqwest::Client::builder()",
        "reqwest::Client::new()",
        "reqwest::blocking::Client::builder()",
        "reqwest::blocking::Client::new()",
    ] {
        assert_eq!(
            files_containing(needle, &["state.rs"]),
            Vec::<String>::new(),
            "{needle} outside state.rs"
        );
    }
    assert_eq!(
        files_containing("\"EDDA/0.1", &[]),
        Vec::<String>::new(),
        "hardcoded version in a user agent"
    );
}

/// Writer ownership, cancellation, voice generation and listener state are
/// values owned by `AppState`, not process globals.
#[test]
fn coordination_state_is_owned_not_global() {
    for file in ["exchange.rs", "voice.rs", "listen.rs", "game.rs"] {
        let text = std::fs::read_to_string(src_dir().join(file)).unwrap();
        let statics: Vec<&str> = text
            .lines()
            .filter(|l| l.trim_start().starts_with("static "))
            .collect();
        assert_eq!(
            statics,
            Vec::<&str>::new(),
            "{file} still has process-global state"
        );
    }
    // The old flag fields (`x.search_cancel`, `import_cancel: Arc<AtomicBool>`),
    // not the `*_cancel` commands that now cancel a supervised job.
    for needle in [
        ".search_cancel",
        "search_cancel:",
        ".import_cancel",
        "import_cancel:",
        "::CANCEL",
        "CANCEL.store",
    ] {
        assert_eq!(
            files_containing(needle, &[]),
            Vec::<String>::new(),
            "{needle}"
        );
    }
    assert_eq!(
        files_containing("Mutex<Option<tauri::AppHandle>>", &[]),
        Vec::<String>::new(),
        "AppState still holds an AppHandle back-reference"
    );
}

/// Reading the data-source status must RETURN. It once could not: the
/// MutexGuard temporary from a struct-literal field lived to the end of
/// the literal, and the next field locked the same non-reentrant mutex
/// on the same thread, so the call deadlocked against itself and held
/// the config lock for the life of the process. Everything that reads
/// config queued behind it -- the data-source radios, the release-notes
/// splash (which then could not be dismissed and covered the whole app),
/// ships, trade, market search, plotting, and the job join at exit,
/// which froze the UI thread on every quit.
///
/// A deadlock does not fail a test, it HANGS one and takes the suite
/// with it, so the call runs on its own thread and the assertion is on
/// the clock. It lives HERE rather than beside the code because
/// exchange.rs is a scanned job module and
/// `background_tasks_run_on_the_supervised_runtime` rightly objects to a
/// thread there; contracts.rs is already excluded from that scan, and
/// "a config read returns" is exactly the kind of whole-app invariant
/// this module exists to pin.
#[test]
fn reading_config_backed_status_never_deadlocks() {
    let dir = tempfile::tempdir().unwrap();
    let state = crate::state::test_state(dir.path());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let status = crate::exchange::dev_api_status(&state);
        // Touch every field that needs the lock, so a future rewrite
        // cannot pass by not reading config at all.
        let _ = (status.available, status.local, status.effective);
        // Twice: proves the guard was released, not merely survived once.
        let _ = crate::exchange::dev_api_status(&state);
        let _ = tx.send(());
    });
    assert!(
        rx.recv_timeout(// Sixty seconds, not ten: the failure this guards is an infinite
            // hang, and the suite runs 200 tests in parallel on a box that may
            // also be compiling the dev app (2026-09-07: a 10 s budget tripped
            // under exactly that load while the test passed alone in 30 ms).
            std::time::Duration::from_secs(60)).is_ok(),
        "dev_api_status did not return in time - it is holding the config mutex and          waiting for itself again (see the comment on that function)"
    );
}
