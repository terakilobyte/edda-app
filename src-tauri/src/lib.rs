#![recursion_limit = "512"]
mod ai;
mod app_update;
mod callouts;
mod carrier_follow;
mod capabilities;
mod commands;
mod contracts;
mod control;
mod eval;
mod events;
mod exchange;
mod feed;
mod follow;
mod heatmap;
mod telemetry;
mod mission_route;
mod capi_spike;
mod hold_sale;
mod phonetics;
mod remote_search;
mod remote_lookup;
mod remote_trade;
mod trap;
mod game;
mod helpers;
mod jobs;
mod speech_engines;
mod knowledge;
mod listen;
mod metrics;
mod time_fit;
mod trade_follow;
mod trade_timing;
mod overlay;
mod persona;
mod platform;
mod routing;
mod spansh;
mod status_flags;
mod state;
mod voice;
mod watcher;

use ed_store::Store;
use state::AppState;
use std::path::PathBuf;
use events::EmitExt as _;
use tauri::Manager;

fn data_root_under(parent: PathBuf) -> PathBuf {
    if parent
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case(env!("CARGO_PKG_NAME")))
    {
        parent
    } else {
        parent.join(env!("CARGO_PKG_NAME"))
    }
}

/// Where the database lives. `EDDA_DB` overrides it, which is how you
/// point the app at a galaxy database bootstrapped from the Spansh dumps
/// instead of building one from scratch.
fn installed_data_root() -> PathBuf {
    let bootstrap = platform::default_data_dir();
    let pointer = platform::pointer_file();
    if let Ok(saved) = std::fs::read_to_string(&pointer) {
        let saved = PathBuf::from(saved.trim());
        if !saved.as_os_str().is_empty() && std::fs::create_dir_all(&saved).is_ok() {
            return saved;
        }
    }

    // First paint immediately. Onboarding offers an explicit folder picker
    // before any optional large downloads begin.
    let root = bootstrap.clone();
    if let Err(e) = std::fs::create_dir_all(&root) {
        eprintln!(
            "[edda] could not create {}: {e}; using {}",
            root.display(),
            bootstrap.display()
        );
        let _ = std::fs::create_dir_all(&bootstrap);
        return bootstrap;
    }
    let _ = std::fs::create_dir_all(&bootstrap);
    if let Err(e) = std::fs::write(&pointer, root.to_string_lossy().as_bytes()) {
        eprintln!("[edda] could not remember data location: {e}");
    }
    root
}

fn db_path() -> PathBuf {
    if let Some(path) = std::env::var_os("EDDA_DB") {
        return PathBuf::from(path);
    }
    if cfg!(debug_assertions) && std::env::var_os("EDDA_TEST_FIRST_RUN").is_none() {
        Store::default_db_path_for(env!("CARGO_PKG_NAME"))
    } else {
        installed_data_root().join("edda.sqlite3")
    }
}

#[tauri::command]
async fn data_location_get(state: tauri::State<'_, AppState>) -> Result<String, String> {
    Ok(state.data_dir.display().to_string())
}

#[tauri::command]
async fn data_location_choose(app: tauri::AppHandle) -> Result<(), String> {
    let Some(folder) = rfd::AsyncFileDialog::new().set_title("Choose the parent folder for EDDA data").set_directory(platform::default_data_parent()).pick_folder().await else { return Ok(()) };
    let root = data_root_under(folder.path().to_path_buf());
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(platform::default_data_dir()).map_err(|e| e.to_string())?;
    std::fs::write(platform::pointer_file(), root.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    app.restart();
}

/// Where the journal is, or an empty stand-in when it is not.
#[derive(Debug)]
struct JournalLocation {
    dir: PathBuf,
    found: bool,
    /// Something to tell the commander when the folder was not found.
    note: Option<String>,
}

/// Resolve the journal folder: an explicit override wins, then the first
/// existing candidate. With none -- a Mac, a fresh Linux box, a Windows
/// machine that has never run the game -- the app still starts: it
/// watches an empty folder under its own data dir (so sync and the file
/// watcher have a real directory) and says so, once, instead of scanning
/// whatever the working directory happens to be.
fn journal_location(explicit: Option<&std::path::Path>, candidates: &[PathBuf], data_dir: &std::path::Path) -> JournalLocation {
    if let Some(p) = explicit {
        if p.is_dir() {
            return JournalLocation { dir: p.to_path_buf(), found: true, note: None };
        }
        let dir = empty_journal_stand_in(data_dir);
        return JournalLocation {
            dir,
            found: false,
            note: Some(format!("ED_JOURNAL_DIR points at {}, which is not a folder; no journal is being read.", p.display())),
        };
    }
    if let Some(dir) = candidates.iter().find(|p| p.is_dir()) {
        return JournalLocation { dir: dir.clone(), found: true, note: None };
    }
    let dir = empty_journal_stand_in(data_dir);
    let hint = match platform::name() {
        "macos" => "Elite Dangerous does not run on macOS; set ED_JOURNAL_DIR to a copy of a journal folder to work with real data.",
        "linux" => "Looked in the Steam/Proton prefix (compatdata/359320). Set ED_JOURNAL_DIR if the game lives elsewhere.",
        _ => "Set ED_JOURNAL_DIR to the folder holding Journal.*.log if it is not under Saved Games.",
    };
    JournalLocation { dir, found: false, note: Some(format!("Elite Dangerous journal folder not found. {hint}")) }
}

fn empty_journal_stand_in(data_dir: &std::path::Path) -> PathBuf {
    let dir = data_dir.join("journal-missing");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Linux graphics stopgaps, NVIDIA-GATED per the official Tauri
    // linux-graphics guidance (research 2026-09-06): webkit2gtk's
    // dmabuf renderer paints all-white on the NVIDIA proprietary
    // driver, and explicit-sync trips Wayland "Error 71" — but
    // disabling either unconditionally throws away the fast path on
    // healthy AMD/Intel setups (our 0.2.3 blanket set did exactly
    // that; narrowed here). A commander's own setting always wins.
    // NOTE: the distributed-AppImage white screen (EGL_BAD_PARAMETER)
    // is a PACKAGING defect — over-bundled libwayland vs Mesa 25+
    // hosts — fixed in scripts/appimage-fixup.sh, not by env vars.
    #[cfg(target_os = "linux")]
    if std::path::Path::new("/proc/driver/nvidia/version").exists() {
        if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        if std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none() {
            std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1");
        }
    }

    let db = db_path();
    let fresh_database = !db.exists();
    let data_dir = db
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".data"));

    let journal_override = std::env::var_os("ED_JOURNAL_DIR").map(PathBuf::from);
    let journal = journal_location(journal_override.as_deref(), &platform::journal_dir_candidates(), &data_dir);
    let journal_dir = journal.dir.clone();
    if let Some(note) = &journal.note {
        eprintln!("[edda] {note}");
    }

    // Logging goes beside the database; the guard must live as long as the
    // process or buffered lines are lost.
    let _log_guard = ed_store::observe::init(&db, true).ok();
    metrics::install();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        platform = platform::name(),
        journal = %journal_dir.display(),
        journal_found = journal.found,
        database = %db.display(),
        "starting"
    );

    let store = match Store::open(&db, &journal_dir) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[edda] fatal: could not open the database: {e:#}");
            std::process::exit(1);
        }
    };

    let app_state = AppState::new(store, data_dir, db.clone());
    let store_handle = app_state.store.clone();
    let routing_handle = app_state.routing.clone();

    // ONE INSTANCE, IN EVERY BUILD. This guard used to sit under
    // #[cfg(debug_assertions)] with the deep-link spike it was added
    // for, so every SHIPPED build had no guard at all -- and EDDA is a
    // tray app people launch again when they cannot see a window.
    //
    // Field case 2026-09-07 (maintainer, on 0.2.8): two edda.exe processes,
    // PIDs 60344 and 46292, both from Program Files. Two processes means
    // two SQLite writers on the same files and two in-memory Configs:
    //   - 1,591 "database is locked" in 50 minutes, each a 30 s wait
    //     (ed-store BUSY_TIMEOUT) that then failed;
    //   - the data-source choice never saved, because whichever process
    //     wrote config.json last clobbered it with ITS copy, where the
    //     field was still None. Same for notes_seen_version;
    //   - two tray icons, and "close EDDA" leaving the other instance's
    //     HUD on screen looking frozen.
    // The maintainer's ruling that day was "we need *one* writer". This is
    // that ruling at the process level, and it comes first: the plugin
    // must be registered before any other (Tauri's own requirement).
    let builder = tauri::Builder::default().plugin(tauri_plugin_single_instance::init(
        |app, _argv, _cwd| {
            use tauri::Manager as _;
            // The second launch is the commander asking to SEE EDDA --
            // it hides to tray, so focus alone is not enough.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        },
    ));
    // CAPI deep-link spike (dev builds only): a second launch via
    // edda:// must reach THIS instance, and the plugin delivers the URL.
    #[cfg(debug_assertions)]
    let builder = builder.plugin(tauri_plugin_deep_link::init());
    builder
        // Remembers where each window was left -- the HUD in particular, so
        // it does not have to be dragged into place after every launch.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // Source links under a ship-computer reply open in the default browser.
        .plugin(tauri_plugin_opener::init())
        // Signed self-updates from the community API.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        .manage(routing_handle)
        .manage(app_update::PendingUpdate::default())
        .manage(std::sync::Arc::new(heatmap::Heatmap::default()))
        .setup(move |app| {
            // Long plots run on every core; keep two for the app and put the
            // planner threads below normal priority, or Settings stops
            // answering while a route is being plotted.
            ed_galaxy::init_thread_pool(ed_input::lower_thread_priority);
            // CAPI spike: register the edda:// scheme at runtime (dev
            // builds have no installer to do it) and route callbacks.
            #[cfg(debug_assertions)]
            {
                use tauri_plugin_deep_link::DeepLinkExt as _;
                if let Err(error) = app.deep_link().register_all() {
                    tracing::warn!(%error, "capi spike: deep-link scheme registration failed");
                }
                let spike_handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        capi_spike::on_deep_link(&spike_handle, url.as_str());
                    }
                });
            }
            let handle = app.handle().clone();
            let state = handle.state::<AppState>();
            // From here on, events reach the windows.
            let emitter: std::sync::Arc<dyn events::Emitter> =
                std::sync::Arc::new(events::TauriEmitter(handle.clone()));
            state.events.install(emitter.clone());
            let jobs = state.jobs.clone();
            let announcer = state.announcer();

            // A managed engine receives a new port on every launch. Hold all
            // startup speech until that engine is ready so an early greeting
            // cannot fall through to a different local voice.
            let managed_voice = speech_engines::managed_kokoro_selected(&state);
            if managed_voice {
                state.voice.pause(true);
            }

            if let Err(e) = overlay::setup(&handle) {
                tracing::warn!(error = %e, "overlay setup failed; continuing without hotkeys");
            }

            // First run backfills the whole journal folder, which takes long
            // enough to be visible. Off the main thread so the window paints
            // immediately and reports progress rather than hanging.
            {
                let store = store_handle.clone();
                let announcer = announcer.clone();
                let events = state.events.clone();
                let _ = jobs.spawn_blocking(jobs::INITIAL_SYNC, move |_token| {
                    initial_sync(&store, &*events, &announcer, fresh_database);
                });
            }

            {
                let store = store_handle.clone();
                let app = handle.clone();
                let journal_dir = journal_dir.clone();
                let _ = jobs.spawn_blocking(jobs::JOURNAL_WATCHER, move |token| {
                    watcher::run(token, app, store, &journal_dir);
                });
            }
            if let Some(note) = journal.note.clone() {
                // Once, on screen and in the callout list: the app is up
                // with nothing to read, and the commander should know why.
                announcer.deliver(vec![(callouts::Callout::new("session", "", 2, false, note), None)]);
            }
            app_update::spawn_update_watch(handle.clone());
            {
                let running = state.game_running.clone();
                let announcer = announcer.clone();
                let _ = jobs.spawn(jobs::GAME_POLL, move |token| game::run(token, running, announcer, game::POLL_EVERY));
            }
            listen::setup(&handle);
            {
                let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if cfg.voice_server_enabled && !managed_voice {
                    state.voice.audio().set_server(cfg.voice_server.clone());
                }
                callouts::set_signal_watch(cfg.signal_watch.clone().unwrap_or_default());
            }
            // Managed speech runtimes may take several seconds to load. Give
            // each launch a fresh loopback port without delaying first paint.
            {
                let helper_app = handle.clone();
                let announcer = announcer.clone();
                let _ = jobs.spawn_blocking(jobs::SPEECH_ENGINE_START, move |_token| {
                    let state = helper_app.state::<state::AppState>();
                    let result = speech_engines::start_configured(&state);
                    state.voice.pause(false);
                    if let Err(e) = result {
                        tracing::warn!(error = %e, "managed Kokoro did not start; using local fallback, will retry");
                        announcer.deliver(vec![(callouts::Callout::new(
                            "session", "", 2, true,
                            "Kokoro is slow to wake. Using the local voice for now; I'll keep trying in the background.".into(),
                        ), None)]);
                        // Twice in one night (maintainer, 2026-09-05) the cold
                        // torch import lost its 60 s race against a
                        // first-launch disk storm — and a later start
                        // succeeded every time. Retry when the storm has
                        // had time to pass; bounded so a genuinely broken
                        // install stops after three tries.
                        for attempt in 2u32..=3 {
                            std::thread::sleep(std::time::Duration::from_secs(180));
                            match speech_engines::start_configured(&state) {
                                Ok(_) => {
                                    tracing::info!(attempt, "managed Kokoro started on retry");
                                    announcer.deliver(vec![(callouts::Callout::new(
                                        "session", "", 2, true,
                                        "Kokoro voice engine is up, Commander.".into(),
                                    ), None)]);
                                    break;
                                }
                                Err(e) => {
                                    tracing::warn!(attempt, error = %e, "Kokoro retry failed");
                                }
                            }
                        }
                    }
                });
            }
            {
                let app = handle.clone();
                let _ = jobs.spawn(jobs::EVAL_DEV_HOOK, move |token| eval::dev_hook(token, app));
            }
            {
                let app = handle.clone();
                let _ = jobs.spawn_blocking(jobs::STAR_BACKFILL, move |_token| {
                    spansh::backfill_journal_stars(&app.state::<AppState>());
                });
            }
            follow::watch_game_target_key(handle.clone());

            // A scripted exit for smoke tests of startup and shutdown on a
            // headless runner: goes through the same RunEvent::Exit path
            // as Quit, so every job is cancelled and joined.
            if let Some(secs) = std::env::var("EDDA_EXIT_AFTER_SECS").ok().and_then(|s| s.parse::<u64>().ok()) {
                let app = handle.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
                    tracing::info!(secs, "EDDA_EXIT_AFTER_SECS: exiting");
                    app.exit(0);
                });
            }

            // The live galaxy feed is opt-out: it is a continuous background
            // socket, and someone playing offline should not be paying for it.
            {
                // The heatmap places name-only EDDN events through the
                // galaxy index; a per-thread cache in the closure keeps
                // the lookup off the hot path.
                let heat = app.state::<std::sync::Arc<heatmap::Heatmap>>().inner().clone();
                let routing = app.state::<std::sync::Arc<routing::RoutingState>>().inner().clone();
                let data_dir = app.state::<state::AppState>().data_dir.clone();
                let cache = std::sync::Mutex::new(std::collections::HashMap::<String, Option<[f32; 3]>>::new());
                heat.set_resolver(move |name| {
                    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(hit) = cache.get(name) {
                        return *hit;
                    }
                    let looked = routing
                        .galaxy(&data_dir)
                        .and_then(|g| g.find(name).map(|idx| g.pos_of(idx)));
                    if cache.len() > 100_000 {
                        cache.clear();
                    }
                    cache.insert(name.to_owned(), looked);
                    looked
                });
            }
            {
                // Telemetry cadence: consent-gated per batch, fail-silent,
                // flushes once more on cancellation (shutdown).
                let state = app.state::<state::AppState>();
                let (http, config) = (state.http.clone(), state.config.clone());
                let _ = jobs.spawn(jobs::TELEMETRY, move |token| telemetry::run(token, http, config));
            }
            if std::env::var("EDDA_NO_EDDN").is_err() {
                let heat = app.state::<std::sync::Arc<heatmap::Heatmap>>().inner().clone();
                let _ = jobs.spawn(jobs::EDDN_FEED, move |token| {
                    feed::run(token, feed::FeedConfig::default(), heat)
                });
            } else {
                tracing::info!("EDDN feed disabled by EDDA_NO_EDDN");
            }

            // Minimize-to-tray (maintainer-approved 2026-09-05): closing the
            // main window keeps EDDA flying with the commander —
            // watcher, voice, overlay, EDDN — with a tray icon whose
            // Quit is the real shutdown. The rejected alternative was
            // an always-on background daemon; the ruling was "the
            // community server IS that daemon", so the tray only keeps
            // the app alive WHILE the commander plays.
            #[cfg(not(target_os = "linux"))]
            {
                use tauri::menu::{MenuBuilder, MenuItemBuilder};
                use tauri::tray::TrayIconBuilder;
                let show = MenuItemBuilder::with_id("show", "Show EDDA").build(app)?;
                let hud = MenuItemBuilder::with_id("hud", "Show / hide HUD").build(app)?;
                let quit = MenuItemBuilder::with_id("quit", "Quit EDDA").build(app)?;
                let menu = MenuBuilder::new(app).items(&[&show, &hud, &quit]).build()?;
                TrayIconBuilder::with_id("edda")
                    .icon(app.default_window_icon().cloned().ok_or("no window icon")?)
                    .tooltip("EDDA — flying with you")
                    .menu(&menu)
                    .show_menu_on_left_click(true)
                    .on_menu_event(|app, event| match event.id().as_ref() {
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                            // Put the HUD back if it was up when we went
                            // to the tray; leave it down if the commander
                            // had already hidden it.
                            if app
                                .state::<crate::state::AppState>()
                                .overlay_visible_before_tray
                                .load(std::sync::atomic::Ordering::Relaxed)
                            {
                                if let Some(hud) = app.get_webview_window("overlay") {
                                    let _ = hud.show();
                                }
                            }
                        }
                        // The overlay toggle, reachable while the main
                        // window is tucked away (maintainer, 2026-09-05) —
                        // the same show/hide the Ctrl+Shift+H shortcut
                        // and the Settings switch drive.
                        "hud" => {
                            if let Some(w) = app.get_webview_window("overlay") {
                                let visible = w.is_visible().unwrap_or(true);
                                let _ = if visible { w.hide() } else { w.show() };
                                // An explicit toggle is a fresh intent:
                                // record it so the next tray round-trip
                                // restores THIS, not what was up before.
                                app.state::<crate::state::AppState>()
                                    .overlay_visible_before_tray
                                    .store(!visible, std::sync::atomic::Ordering::Relaxed);
                                tracing::info!(now_visible = !visible, "HUD toggled from the tray");
                            }
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    })
                    .build(app)?;
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Close on the MAIN window hides to the tray; the overlay
            // has no close affordance and other windows behave normally.
            // NOT on Linux: tray indicators need a shell extension on
            // vanilla GNOME, so hide-to-tray can strand a running app
            // with no way back — there, close means close.
            if cfg!(target_os = "linux") {
                return;
            }
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                    // The HUD goes with it (maintainer, 2026-09-07). Closing
                    // to the tray means "get off my screen", and an
                    // overlay left floating over the desktop with no
                    // window behind it is the thing that reads as a
                    // half-dead app. Remember whether it was up, so
                    // opening from the tray restores what he had rather
                    // than overriding a deliberate Ctrl+Shift+H.
                    let app = window.app_handle();
                    if let Some(hud) = app.get_webview_window("overlay") {
                        let visible = hud.is_visible().unwrap_or(true);
                        app.state::<state::AppState>()
                            .overlay_visible_before_tray
                            .store(visible, std::sync::atomic::Ordering::Relaxed);
                        if visible {
                            let _ = hud.hide();
                        }
                        tracing::info!(hud_was_visible = visible, "main window hidden to tray; HUD follows");
                    }
                    tracing::info!("main window hidden to tray; EDDA keeps flying");
                    // Windows buries fresh tray icons in the overflow
                    // chevron, so a silent hide is indistinguishable
                    // from a crash (maintainer, first close: "didn't it
                    // close?"). Say so, once per session.
                    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        let state = window.app_handle().state::<state::AppState>();
                        state.voice.say(
                            "Still with you, Commander — EDDA is in the system tray, and the HUD stays up. Quit from the tray icon when you mean it.".to_string(),
                        );
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            data_location_get,
            data_location_choose,
            trade_follow::trade_follow_start,
            trade_follow::trade_follow_stop,
            trade_follow::trade_follow_status,
            carrier_follow::carrier_route_plot,
            carrier_follow::carrier_route_start,
            carrier_follow::carrier_route_status,
            carrier_follow::carrier_route_clear,
            carrier_follow::carrier_route_next,
            app_update::app_update_check,
            app_update::release_notes_get,
            app_update::release_notes_seen,
            app_update::app_update_install,
            app_update::app_restart,
            metrics::metrics_snapshot,
            commands::get_status,
            eval::ai_eval,
            commands::get_inventory,
            commands::list_commodities,
            commands::list_module_types,
            commands::list_blueprint_names,
            commands::search_blueprints,
            commands::check_blueprint,
            commands::blueprint_access,
            commands::check_experimental,
            commands::ship_modules,
            commands::ships_list,
            commands::carrier_status,
            commands::ship_slef,
            commands::ship_links,
            commands::material_shopping,
            commands::material_sources,
            commands::list_engineers,
            commands::find_system,
            commands::stations_in_system,
            commands::find_station,
            commands::nearest_service,
            commands::merit_model,
            commands::powerplay_seen,
            commands::sync_now,
            commands::ai_ask,
            commands::ai_reset,
            commands::combat_summary,
            commands::combat_timeline,
            commands::recent_kills,
            commands::merit_timeline,
            commands::mining_search,
        commands::mining_materials,
        commands::mark_add,
        commands::mark_remove,
        commands::mark_here,
        commands::station_market,
            commands::commodity_search,
            commands::outfitting_search,
            commands::shipyard_search,
            commands::systems_near,
            commands::profit_routes,
            commands::voice_status,
            commands::say,
            commands::say_now,
            commands::voice_interrupt,
            commands::set_muted,
            commands::recent_callouts,
            commands::set_overlay_interactive,
            commands::overlay_visible,
            commands::db_stats,
            commands::vacuum,
            commands::missions,
            commands::voice_models,
            commands::voice_use_windows,
            commands::voice_catalog,
            commands::voice_install,
            commands::voice_remove,
            commands::voice_install_default,
            commands::set_voice,
            commands::voice_server_get,
            commands::callouts_get,
            commands::callouts_set,
            commands::signal_watch_get,
            commands::signal_watch_set,
            commands::voice_server_probe,
            commands::voice_server_set,
            speech_engines::speech_engine_status,
            speech_engines::speech_engine_install,
            speech_engines::speech_engine_start,
            speech_engines::speech_engine_stop,
            speech_engines::speech_engine_remove,
            commands::get_ai_config,
            commands::set_ai_config,
            commands::personas,
            commands::set_persona,
            commands::cancel_search,
            commands::ranks,
            commands::current_route,
            commands::powerplay_options,
            capi_spike::capi_spike_start,
        exchange::dev_api_get,
        exchange::dev_api_set,
        hold_sale::sell_hold_search,
        routing::galaxy_status,
            commands::activity_heatmap,
            commands::feedback_send,
            commands::telemetry_prefs,
            commands::telemetry_prefs_set,
            routing::galaxy_complete,
            routing::galaxy_find,
            routing::edsm_system,
            routing::plot_route,
            routing::injections_available,
            routing::cancel_route,
            routing::galaxy_near,
            routing::name_complete,
            routing::ship_scoop_info,
            spansh::import_spansh_route,
            follow::route_activate,
            follow::route_clear,
            follow::route_clear_in_game,
            follow::route_follow_status,
            follow::route_advance,
            follow::route_target_next,
            follow::target_macro_get,
            follow::route_active_get,
            follow::target_macro_enabled_get,
            follow::target_macro_enabled_set,
            follow::game_route_max_get,
            follow::game_route_max_set,
            follow::target_macro_set,
            follow::target_macro_check,
            follow::target_macro_presets,
            follow::target_macro_test,
            follow::route_plot_test,
            follow::map_setup_test,
            follow::map_setup_say,
            follow::map_setup_cancel,
            follow::map_setup_target,
            follow::route_plot_in_game,
            knowledge::knowledge_status,
            follow::target_key_status,
            follow::target_trigger_capture,
            follow::target_trigger_clear,
            follow::map_point_capture,
            follow::map_points_get,
            follow::map_delay_set,
            follow::map_points_clear,
            follow::macro_record_start,
            follow::macro_record_stop,
            game::game_state,
            listen::listen_status,
            listen::listen_config_set,
            listen::listen_setup,
            listen::listen_models_remove,
            listen::listen_start,
            listen::listen_stop,
            listen::listen_ptt,
            listen::audio_devices,
            listen::joy_devices,
            listen::ptt_capture,
            control::game_controls,
            control::game_control,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            match event {
                // Tauri's default exits when it thinks the last window
                // is gone — which escalated hide-to-tray into a full
                // shutdown 235 ms after the hide (maintainer's second close,
                // 2026-09-05 18:10). A DELIBERATE quit (tray Quit,
                // updater restart) calls app.exit(code) and carries
                // Some(code); the heuristic carries None and is refused.
                // Diagnosis trace (2026-09-05, X-still-quits hunt): name
                // every lifecycle step so the next close tells us which
                // path kills us. Cheap; kept until the tray ships.
                tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::Destroyed, .. } => {
                    tracing::trace!(%label, "runevent: window DESTROYED");
                }
                tauri::RunEvent::WindowEvent { label, event: tauri::WindowEvent::CloseRequested { .. }, .. } => {
                    tracing::trace!(%label, "runevent: close requested");
                }
                tauri::RunEvent::ExitRequested { api, code, .. } => {
                    tracing::trace!(?code, "runevent: exit requested");
                    // Only where hide-to-tray is active (see the Linux
                    // note on the close handler): refusing exits on a
                    // platform where the window really closed would
                    // leave an unreachable process.
                    if code.is_none() && !cfg!(target_os = "linux") {
                        api.prevent_exit();
                        tracing::info!("window-close exit request refused; EDDA lives in the tray");
                    } else {
                        // A REAL quit. Start the jobs winding down NOW,
                        // while the windows are still closing, so the
                        // blocking join in RunEvent::Exit has almost
                        // nothing left to wait for. Non-blocking on
                        // purpose: this runs on the UI thread.
                        app.state::<AppState>().jobs.cancel_all();
                        tracing::info!("quit: background jobs cancelled");
                    }
                }
                tauri::RunEvent::Exit => {
                    // THIS RUNS ON THE UI THREAD, so whatever it waits
                    // for, the commander watches frozen. Field case
                    // 2026-09-07: the maintainer's HUD went "EDDA HUD (Not
                    // Responding)" on every quit, because this blocked
                    // the event loop for the full five seconds while a
                    // job sat inside a 30 s SQLite busy wait that cannot
                    // be cancelled. Jobs are now told to stop back in
                    // ExitRequested, and the wait here is short enough
                    // not to read as a hang: it is a courtesy to let a
                    // batch finish, not a guarantee.
                    let jobs = app.state::<AppState>().jobs.clone();
                    let report = tauri::async_runtime::block_on(jobs.shutdown(SHUTDOWN_TIMEOUT));
                    if !report.timed_out.is_empty() {
                        tracing::warn!(timed_out = ?report.timed_out, "jobs still running at exit");
                    }
                }
                _ => {}
            }
        });
}

/// How long app exit waits for background jobs to join, ON THE UI
/// THREAD -- so this is a "does the app feel dead?" budget, not a "did
/// the work finish?" one. Windows paints the not-responding title after
/// about five seconds, which is exactly what the old value bought.
/// Cancellation now starts earlier (RunEvent::ExitRequested), so this is
/// only the tail.
const SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(750);

/// The first-run backfill and the one-off market index build, then the
/// greeting. Reports progress through the emitter; a supervised job.
fn initial_sync(
    store: &std::sync::Mutex<Store>,
    events: &dyn events::Emitter,
    announcer: &watcher::Announcer,
    fresh_database: bool,
) {
    let result = {
        let guard = store.lock().unwrap_or_else(|e| e.into_inner());
        guard.sync_with_progress(|name, i, total| {
            if i % 10 == 0 || i == total {
                events.emit(events::SYNC_PROGRESS, serde_json::json!({ "file": name, "done": i, "total": total }));
            }
        })
    };
    match result {
        Ok(stats) => {
            tracing::info!(events = stats.ingest.events_inserted, ms = stats.elapsed_ms, "initial sync complete");
            events.emit(events::SYNC_COMPLETE, stats.ingest.events_inserted);
            // Jumps flown while the app was down reached the store but
            // never the watcher: align the followed route's cursor with
            // the commander's actual system, silently.
            {
                let guard = store.lock().unwrap_or_else(|e| e.into_inner());
                follow::reconcile(guard.conn(), events);
            }
            // A brand-new install is greeted by the setup UI, not by stale
            // journal state from a game that is not currently running.
            if !fresh_database {
                greet(announcer, store);
            }
            // The local market index build that used to run here (minutes
            // on a 100M-row table, "market index built" in the header) is
            // gone: searches run on the server (2026-09-08).
        }
        Err(e) => tracing::error!(error = %e, "initial sync failed"),
    }
}

/// Greet the commander once the store is current, by name and ship, and
/// with where they are. Spoken through the same path as every other
/// callout so the overlay shows it too.
fn greet(announcer: &watcher::Announcer, store: &std::sync::Mutex<Store>) {
    let (commander, ship, system) = {
        let guard = store.lock().unwrap_or_else(|e| e.into_inner());
        let conn = guard.conn();
        let commander = ed_store::session::commander_name(conn).ok().flatten();
        // The ship comes from the single current-ship source (the
        // swap-fed loadout row), never re-derived from raw events. The
        // custom name leads; otherwise the localised hull, never the
        // raw symbol.
        let ship: Option<String> = ed_store::session::current_ship(conn)
            .ok()
            .flatten()
            .and_then(|s| s.spoken(ed_route::ships::display_name));
        let system = ed_store::query::location(conn)
            .ok()
            .flatten()
            .and_then(|l| l.system_name);
        (commander, ship, system)
    };
    let text = ed_voice::greeting(commander.as_deref(), ship.as_deref(), system.as_deref());
    let callout = callouts::Callout {
        kind: "greeting",
        text,
        priority: 1,
        speak: true,
        ts: String::new(),
    };
    announcer.deliver(vec![(callout, None)]);
    let backend = announcer.voice.backend();
    // The dashboard flag (maintainer, 2026-09-05: which voice model is in
    // use). A managed Kokoro re-stamps itself on start; this covers
    // every other resolution.
    crate::telemetry::set_voice_engine(match backend {
        ed_voice::Backend::Server => "voice_server",
        ed_voice::Backend::Piper => "voice_piper",
        ed_voice::Backend::Sapi => "voice_windows",
        _ => "voice_none",
    });
    tracing::info!(backend = ?backend, model = ?announcer.voice.model(), "voice");
}

#[cfg(test)]
mod data_location_tests {
    use super::*;

    #[test]
    fn custom_parent_gets_one_edda_directory() {
        // Built with the host's separator: `D:\` is one opaque file name
        // on a POSIX host, so a literal Windows path proves nothing there.
        let parent = if cfg!(windows) { PathBuf::from(r"D:\") } else { PathBuf::from("/Volumes/Data") };
        assert_eq!(data_root_under(parent.clone()), parent.join("edda"));
        assert_eq!(data_root_under(parent.join("edda")), parent.join("edda"));
        assert_eq!(data_root_under(parent.join("EDDA")), parent.join("EDDA"));
    }

    #[test]
    fn startup_without_journal_dir_reports_missing_rather_than_scanning_cwd() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let found = journal_location(None, &[home.path().join("nowhere")], data.path());
        assert!(!found.found, "{found:?}");
        assert_ne!(found.dir, std::env::current_dir().unwrap());
        assert!(found.dir.starts_with(data.path()), "{found:?}");
        assert!(found.dir.is_dir(), "an empty stand-in folder so the watcher and sync have something to watch");
        assert!(found.note.as_deref().is_some_and(|n| n.contains("journal")), "{found:?}");

        let real = tempfile::tempdir().unwrap();
        let found = journal_location(None, &[real.path().to_path_buf()], data.path());
        assert!(found.found && found.dir == real.path() && found.note.is_none(), "{found:?}");
    }

}
