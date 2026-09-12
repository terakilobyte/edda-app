//! Application self-update (maintainer-claimed 2026-09-04: "the auto update
//! feature... that would be great").
//!
//! The updater plugin checks OUR community API for a signed manifest
//! (`/v1/app/latest.json`), so app releases travel the same pipe as data
//! publishes: build, sign, drop in the artifact directory. The endpoint
//! follows the configured community API URL, so a localhost server tests
//! the whole flow end to end.
//!
//! Flow: a check (manual button or the startup/periodic task) finds an
//! update and holds it in state; the frontend shows the pill and asks for
//! the install; download+install streams progress events; the commander
//! chooses when to restart. Nothing installs without an explicit ask.

use crate::state::AppState;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::events::APP_UPDATE as APP_UPDATE_EVENT;

/// The update a check found, awaiting the commander's install decision.
#[derive(Default)]
pub struct PendingUpdate(Mutex<Option<tauri_plugin_updater::Update>>);

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpdateCheck {
    pub current: String,
    pub available: Option<String>,
    pub notes: Option<String>,
}

/// Where the updater looks — CANONICAL ONLY in a release build.
///
/// This used to follow `community_api_url`, the address a commander
/// could type into Settings. That meant anyone who pointed EDDA at a
/// self-hosted or development server was also pointing the AUTO-UPDATER
/// at it: that server could hand back a `latest.json` naming any signed
/// build it liked, and EDDA would download and install it. Data and
/// executables do not deserve the same trust, and a field meant for
/// "fetch market data from my box" should never have decided which
/// binary runs on the commander's machine.
///
/// Maintainer, 2026-09-06: "if someone wants to use the api, we should no
/// longer check for and apply automatic updates", then "we don't need
/// the endpoint to be editable though". The endpoint is no longer
/// user-editable in release builds, and this pins updates regardless —
/// belt and braces, because the UI's absence is not a guarantee.
///
/// Debug builds still honour the override so a dev server can serve
/// test updates.
fn endpoint(state: &AppState) -> String {
    let base = if cfg!(debug_assertions) {
        state
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .community_api_url
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| crate::exchange::DEFAULT_COMMUNITY_API.to_string())
    } else {
        crate::exchange::DEFAULT_COMMUNITY_API.to_string()
    };
    format!("{}/v1/app/latest.json", base.trim_end_matches('/'))
}

/// The manifest fetch's whole budget. Field case 2026-09-07 (maintainer, 0.2.7
/// prod): "check for update — the button just goes gray": the check
/// hung with no timeout on its request while every other call in the
/// app carries one, so a stalled connection was indistinguishable from
/// a check that never ran. A bounded wait turns that into an error the
/// Settings line can show and the log can explain.
const CHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

async fn check_inner(app: &AppHandle) -> Result<UpdateCheck, String> {
    let state = app.state::<AppState>();
    let url = endpoint(&state);
    let current = app.package_info().version.to_string();
    let started = std::time::Instant::now();
    tracing::debug!(%url, %current, "app update check: starting");
    let updater = app
        .updater_builder()
        .endpoints(vec![url
            .parse()
            .map_err(|e| format!("bad update endpoint {url}: {e}"))?])
        .map_err(|e| e.to_string())?
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    // Belt and braces: the plugin's timeout covers the request; this one
    // covers everything else in check() so the command always returns.
    let found = match tokio::time::timeout(CHECK_TIMEOUT + std::time::Duration::from_secs(5), updater.check()).await {
        Ok(result) => result.map_err(|e| {
            tracing::warn!(error = %e, ms = started.elapsed().as_millis() as u64, "app update check: failed");
            format!("update check failed: {e}")
        })?,
        Err(_) => {
            tracing::warn!(ms = started.elapsed().as_millis() as u64, "app update check: timed out");
            return Err(format!("update check timed out after {} s reaching {url}", CHECK_TIMEOUT.as_secs()));
        }
    };
    tracing::debug!(
        ms = started.elapsed().as_millis() as u64,
        available = found.as_ref().map(|u| u.version.as_str()).unwrap_or("none"),
        "app update check: done"
    );
    let result = UpdateCheck {
        current,
        available: found.as_ref().map(|u| u.version.clone()),
        notes: found.as_ref().and_then(|u| u.body.clone()),
    };
    *app.state::<PendingUpdate>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = found;
    Ok(result)
}

#[tauri::command]
pub async fn app_update_check(app: AppHandle) -> Result<UpdateCheck, String> {
    let check = check_inner(&app).await?;
    if check.available.is_some() {
        let _ = app.emit(
            APP_UPDATE_EVENT,
            serde_json::json!({"phase": "available", "version": check.available}),
        );
    }
    Ok(check)
}

/// Download and install the pending update. The NSIS package applies on
/// the spot; the app keeps running until `app_restart`.
#[tauri::command]
pub async fn app_update_install(app: AppHandle) -> Result<(), String> {
    let update = app
        .state::<PendingUpdate>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or("no update is pending; check for updates first")?;
    let version = update.version.clone();

    // STOP THE DATA WORK FIRST (maintainer, 2026-09-07: "if someone clicks
    // install we need to immediately stop any data downloads/writes we
    // have going on").
    //
    // An NSIS install replaces the running binary and the commander
    // restarts into it. Anything still streaming EBEX into SQLite while
    // that happens is a large write racing a process swap — at best it
    // wastes a commander's bandwidth on a download the restart discards,
    // at worst it is a half-applied hydration in a database the new
    // build then opens. Cancelling is cheap and every one of these jobs
    // is resumable: the market sync restarts from its watermark, the
    // backfill from its cursor.
    {
        let state = app.state::<AppState>();
        for job in [
            crate::jobs::STAR_BACKFILL,
            crate::jobs::HIGHWAY_REBUILD,
            crate::jobs::INITIAL_SYNC,
        ] {
            state.jobs.cancel(job);
        }
        tracing::info!(%version, "app update: data jobs cancelled before install");
    }

    // WINDOWS EXITS INSIDE download_and_install AND NEVER COMES BACK
    // HERE. tauri-plugin-updater 2.11's Windows path ShellExecuteW's the
    // NSIS installer with /UPDATE plus the restart flags
    // (restart_after_install defaults to true) and then calls
    // std::process::exit(0) itself; NSIS waits for us to die, swaps the
    // binary and relaunches it. So on Windows the "ready" phase below,
    // the log line after it, and app_restart are all unreachable -- the
    // commander clicks Install and the app simply VANISHES and comes
    // back, which after this evening reads as a crash.
    //
    // (It is not unreachable on Linux: the AppImage path writes the new
    // image and returns, so "ready" and the Restart button are correct
    // there. The code was not wrong, it was Windows-blind.)
    //
    // Say so BEFORE it happens, because afterwards there is no process
    // left to say anything.
    let _ = app.emit(
        APP_UPDATE_EVENT,
        serde_json::json!({
            "phase": "downloading",
            "downloaded": 0,
            "total": serde_json::Value::Null,
            "restarts_itself": cfg!(windows),
        }),
    );
    let emitter = app.clone();
    let mut downloaded: u64 = 0;
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = emitter.emit(
                    APP_UPDATE_EVENT,
                    serde_json::json!({
                        "phase": "downloading",
                        "downloaded": downloaded,
                        "total": total,
                        "restarts_itself": cfg!(windows),
                    }),
                );
            },
            || {},
        )
        .await
        .map_err(|e| e.to_string())?;
    // Reached on Linux only; see the note above.
    tracing::info!(%version, "app update installed; restart applies it");
    let _ = app.emit(
        APP_UPDATE_EVENT,
        serde_json::json!({"phase": "ready", "version": version}),
    );
    Ok(())
}

/// Linux's half of the install: the AppImage path returns from
/// `download_and_install`, so something must actually restart us. On
/// Windows this is never called -- the NSIS installer has already done
/// it (see the note in `app_update_install`).
#[tauri::command]
pub async fn app_restart(app: AppHandle) {
    app.restart();
}

/// Startup + periodic check, sharing the data heartbeat's consent flag:
/// auto_update == false means the Settings button only. A found update
/// only ever NOTIFIES (the pill); download still waits for the commander.
pub fn spawn_update_watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // CHECK IMMEDIATELY ON LAUNCH, then every five minutes (maintainer,
        // 2026-09-07). There used to be a two-minute delay before the
        // first check, and his own log showed why that was wrong: four
        // launches after 0.2.7 went live (04:40:53, 04:46:20, 04:47:13,
        // 04:48:02), none lasting two minutes, so the check NEVER RAN.
        // He reported "no update yet" and was exactly right — nothing
        // had looked. The check is one small bounded request; there is
        // no reason to make a commander wait for it.
        // The five-minute beat logs at INFO only when the answer changes:
        // 27 "available 0.3.1" lines in two hours told the maintainer
        // nothing the first one had not (2026-09-09).
        let mut announced: Option<String> = None;
        loop {
            let auto = app
                .state::<AppState>()
                .config
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .auto_update;
            if auto != Some(false) {
                match check_inner(&app).await {
                    Ok(check) => match &check.available {
                        Some(version) => {
                            if announced.as_deref() != Some(version.as_str()) {
                                tracing::info!(%version, "app update available");
                                announced = Some(version.clone());
                            } else {
                                tracing::debug!(%version, "app update still available");
                            }
                            let _ = app.emit(
                                APP_UPDATE_EVENT,
                                serde_json::json!({"phase": "available", "version": version}),
                            );
                        }
                        // Say the boring case out loud too. Silence used
                        // to cover three different states — checked and
                        // up to date, checked and errored, never ran —
                        // and "no update yet" could be any of them.
                        None => {
                            announced = None;
                            tracing::debug!(current = %check.current, "app update check: already current");
                        }
                    },
                    // WARN, not debug: a release build filters debug out,
                    // so a check that failed every time looked exactly
                    // like a check that found nothing (field case
                    // 2026-09-06, 0.2.7 not offered to a 0.2.6 install).
                    Err(error) => {
                        tracing::warn!(error, "app update check failed; next beat retries")
                    }
                }
            }
            // Five minutes, not six hours: a fix that ships at 02:00
            // is no use to someone flying at 02:05 (maintainer, 2026-09-07).
            tokio::time::sleep(std::time::Duration::from_secs(5 * 60)).await;
        }
    });
}

// ── Release notes (maintainer, 2026-09-05: "we need release notes in the
// app!") ─────────────────────────────────────────────────────────────
// Compiled into the binary, so the what's-new splash needs no server
// and can never describe a different build than the one running — the
// version law's spirit applied to prose. The release script refuses to
// cut a version without a section here.

const RELEASE_NOTES: &str = include_str!("../RELEASE-NOTES.md");

#[derive(serde::Serialize)]
pub struct ReleaseNotes {
    pub version: String,
    /// The full bundled markdown, newest section first.
    pub markdown: &'static str,
    /// True until the commander dismisses the splash for this version.
    pub unseen: bool,
}

/// The running version's own section of the bundled notes, for tests
/// and the splash alike.
// Exercised by notes_tests (the bump-with-notes contract) — dead only
// in non-test builds, hence the allow rather than a deletion.
#[cfg_attr(not(test), allow(dead_code))]
pub fn section_for(version: &str) -> Option<&'static str> {
    let header = format!("## {version}");
    let start = RELEASE_NOTES.find(&header)?;
    let body = &RELEASE_NOTES[start..];
    let end = body[header.len()..]
        .find("\n## ")
        .map(|i| i + header.len())
        .unwrap_or(body.len());
    Some(body[..end].trim())
}

#[tauri::command]
pub async fn release_notes_get(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<ReleaseNotes, String> {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let seen = state
        .config
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .notes_seen_version
        .clone();
    Ok(ReleaseNotes {
        unseen: seen.as_deref() != Some(version.as_str()),
        version,
        markdown: RELEASE_NOTES,
    })
}

/// The commander dismissed the splash (or opened the notes): this
/// version counts as seen.
#[tauri::command]
pub async fn release_notes_seen(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    cfg.notes_seen_version = Some(env!("CARGO_PKG_VERSION").to_string());
    cfg.save(&state.data_dir).map_err(|e| e.to_string())
}

#[cfg(test)]
mod notes_tests {
    /// The contract the release script also enforces: the running
    /// version must have a section, and it must be the newest one.
    #[test]
    fn the_running_version_has_the_newest_notes_section() {
        let version = env!("CARGO_PKG_VERSION");
        let section = super::section_for(version)
            .unwrap_or_else(|| panic!("RELEASE-NOTES.md has no '## {version}' section"));
        // Non-empty is the rule; a fix-only release may be one honest line
        // (0.2.7: "Fixing our own mistake." — the maintainer's exact words).
        assert!(
            section.lines().skip(1).any(|l| !l.trim().is_empty()),
            "a release says something"
        );
        let first_header = super::RELEASE_NOTES
            .lines()
            .find(|l| l.starts_with("## "))
            .unwrap();
        assert_eq!(
            first_header.trim(),
            &format!("## {version}"),
            "the newest section must belong to the running version"
        );
    }
}
