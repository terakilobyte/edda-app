//! Managed local speech engines.
//!
//! Engine adapters own downloads and command lines; [`crate::helpers`] owns
//! platform process-tree semantics. Nothing here assumes a developer tool,
//! PATH entry, fixed port, or application install-directory write access.

use crate::state::AppState;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

const KOKORO_COMMIT: &str = "5fb71ea6e75379f95dee0f4a42c12152f4ea0e1a";
const UV_VERSION: &str = "0.11.33";

/// Maintainer ruling 2026-09-04 (the Elite-crash sequencing ruling): voice setup
/// outranks heavy background work. The community sync that used to yield
/// to this flag went with the local data (B.4, 2026-09-09); the flag and
/// its guard stay so anything heavy added later has the same door.
static VOICE_INSTALLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static VOICE_INSTALL_DONE: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// RAII: the flag clears and waiters wake however the install ends —
/// success, error, or cancellation. Shared with the voice-input setup in
/// [`crate::listen`]: Parakeet is voice setup too, and voice goes first.
pub(crate) struct VoiceInstallGuard;
impl VoiceInstallGuard {
    pub(crate) fn begin() -> Self {
        VOICE_INSTALLING.store(true, std::sync::atomic::Ordering::SeqCst);
        VoiceInstallGuard
    }
}
impl Drop for VoiceInstallGuard {
    fn drop(&mut self) {
        VOICE_INSTALLING.store(false, std::sync::atomic::Ordering::SeqCst);
        VOICE_INSTALL_DONE.notify_waiters();
    }
}

#[cfg(windows)]
fn hide_console(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn hide_console(_command: &mut Command) {}

#[derive(Debug, Serialize)]
pub struct SpeechEngineStatus {
    engine: String,
    installed: bool,
    running: bool,
    url: Option<String>,
    approximate_mb: u32,
    available: bool,
    note: String,
}

fn engine_root(data_dir: &Path, engine: &str) -> PathBuf {
    data_dir.join("tools").join("speech").join(engine)
}

fn kokoro_status(state: &AppState) -> SpeechEngineStatus {
    let root = engine_root(&state.data_dir, "kokoro");
    let configured = state.config.lock().unwrap_or_else(|e| e.into_inner()).voice_server.clone();
    SpeechEngineStatus {
        engine: "kokoro".into(),
        installed: root.join("installed.ok").is_file(),
        running: state.helpers.running("speech:kokoro"),
        // Item 37: the reported URL is LIVE state — a configured-but-dead
        // engine advertises nothing, so no panel can claim it is active.
        url: if state.helpers.running("speech:kokoro") {
            configured.filter(|c| c.model == "kokoro").map(|c| c.url)
        } else {
            None
        },
        approximate_mb: 2800,
        available: true,
        note: "Natural local speech. EDDA downloads an isolated Python runtime, dependencies, and Kokoro model.".into(),
    }
}

/// Item 36's diagnosis line: an early death and a timeout are different
/// diseases, and a zero-byte log is itself a symptom worth naming (the
/// child never reached its own logging — the spawn path, not the app).
fn startup_failure_note(died_early: bool, waited_ms: u64, log_bytes: u64) -> String {
    if died_early {
        let log_note = if log_bytes == 0 {
            "kokoro.log is EMPTY — the process died before its own logging started, which points at the spawn path, not Kokoro"
        } else {
            "see logs/kokoro.log for its last words"
        };
        format!("Kokoro exited during startup after ~{waited_ms} ms; {log_note}. Retry with ED_KOKORO_CONSOLE=1 to watch it launch.")
    } else {
        format!("Kokoro did not answer on its port within {} s; see logs/kokoro.log", waited_ms / 1000)
    }
}

pub fn managed_kokoro_selected(state: &AppState) -> bool {
    let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
    // voice_server_enabled is the commander's off switch ("use the
    // Windows voice"): honoring only the config's presence made that
    // choice silently revert on every launch (field case 2026-09-05 —
    // start_configured re-enabled the server the user had turned off).
    engine_root(&state.data_dir, "kokoro").join("installed.ok").is_file()
        && cfg.voice_server_enabled
        && cfg.voice_server.as_ref().is_some_and(|c| c.model == "kokoro")
}

#[tauri::command]
pub async fn speech_engine_status(state: State<'_, AppState>) -> Result<Vec<SpeechEngineStatus>, String> {
    Ok(vec![kokoro_status(&state)])
}

fn emit(app: &AppHandle, engine: &str, phase: &str, detail: &str, fraction: f32) {
    let _ = app.emit(crate::events::SPEECH_ENGINE_PROGRESS, serde_json::json!({"engine":engine,"phase":phase,"detail":detail,"fraction":fraction}));
}

fn download(client: &reqwest::blocking::Client, url: &str, out: &Path) -> anyhow::Result<()> {
    let partial = out.with_extension(format!("{}partial", out.extension().and_then(|x| x.to_str()).map(|x| format!("{x}.")).unwrap_or_default()));
    let mut response = client.get(url).send()?.error_for_status()?;
    let mut file = std::fs::File::create(&partial)?;
    std::io::copy(&mut response, &mut file)?;
    file.flush()?;
    std::fs::rename(partial, out)?;
    Ok(())
}

fn unzip_flat(zip_path: &Path, out: &Path, strip_first: bool) -> anyhow::Result<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut zip = zip::ZipArchive::new(file)?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(enclosed) = entry.enclosed_name() else { continue };
        let relative = if strip_first { enclosed.components().skip(1).collect::<PathBuf>() } else { enclosed.to_path_buf() };
        if relative.as_os_str().is_empty() { continue }
        let target = out.join(relative);
        if entry.is_dir() { std::fs::create_dir_all(&target)?; continue }
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)?; }
        let mut file = std::fs::File::create(target)?;
        std::io::copy(&mut entry, &mut file)?;
    }
    Ok(())
}

fn install_kokoro(app: &AppHandle, data_dir: &Path) -> anyhow::Result<()> {
    let root = engine_root(data_dir, "kokoro");
    let downloads = root.join("downloads");
    let runtime = root.join("runtime");
    let source = root.join("app");
    std::fs::create_dir_all(&downloads)?;
    let client = app.state::<AppState>().http_blocking.clone();

    emit(app, "kokoro", "runtime", "Downloading runtime…", 0.05);
    let uv = runtime.join(crate::platform::exe("uv"));
    if !uv.is_file() {
        let url = crate::platform::uv_download_url(UV_VERSION)
            .ok_or_else(|| anyhow::anyhow!("no uv build is published for {}/{}", std::env::consts::OS, std::env::consts::ARCH))?;
        std::fs::create_dir_all(&runtime)?;
        if url.ends_with(".zip") {
            let uv_zip = downloads.join("uv.zip");
            download(&client, &url, &uv_zip)?;
            unzip_flat(&uv_zip, &runtime, false)?;
        } else {
            // The tarball wraps its binaries in one folder; strip it.
            let uv_tar = downloads.join("uv.tar.gz");
            download(&client, &url, &uv_tar)?;
            let gz = flate2::read::GzDecoder::new(std::fs::File::open(&uv_tar)?);
            for entry in tar::Archive::new(gz).entries()? {
                let mut entry = entry?;
                let rel: PathBuf = entry.path()?.components().skip(1).collect();
                if rel.as_os_str().is_empty() { continue }
                entry.unpack(runtime.join(rel))?;
            }
        }
    }

    emit(app, "kokoro", "application", "Downloading engine…", 0.15);
    let app_zip = downloads.join("kokoro.zip");
    if !source.join("pyproject.toml").is_file() {
        download(&client, &format!("https://github.com/remsky/Kokoro-FastAPI/archive/{KOKORO_COMMIT}.zip"), &app_zip)?;
        std::fs::create_dir_all(&source)?;
        unzip_flat(&app_zip, &source, true)?;
    }

    emit(app, "kokoro", "dependencies", "Installing speech dependencies…", 0.30);
    let mut command = Command::new(&uv);
    command.args(["sync", "--extra", "cpu", "--frozen"])
        .current_dir(&source)
        .env("UV_PYTHON_INSTALL_DIR", root.join("python"))
        .env("UV_CACHE_DIR", root.join("cache"));
    hide_console(&mut command);
    let status = command.status()?;
    anyhow::ensure!(status.success(), "Kokoro dependency installation failed ({status})");

    emit(app, "kokoro", "model", "Downloading voice model…", 0.75);
    let mut command = Command::new(&uv);
    command.args(["run", "--no-sync", "python", "docker/scripts/download_model.py", "--output", "api/src/models/v1_0"])
        .current_dir(&source)
        .env("UV_PYTHON_INSTALL_DIR", root.join("python"))
        .env("UV_CACHE_DIR", root.join("cache"));
    hide_console(&mut command);
    let status = command.status()?;
    anyhow::ensure!(status.success(), "Kokoro model download failed ({status})");
    std::fs::write(root.join("installed.ok"), format!("kokoro={KOKORO_COMMIT}\nuv={UV_VERSION}\n"))?;
    emit(app, "kokoro", "installed", "Kokoro files installed.", 0.90);
    Ok(())
}

#[tauri::command]
pub async fn speech_engine_install(app: AppHandle, state: State<'_, AppState>, engine: String) -> Result<SpeechEngineStatus, String> {
    if engine != "kokoro" { return Err(format!("Unknown managed speech engine: {engine}")) }
    // Held through install AND first start: the sync yields until the
    // engine is actually answering, not merely unpacked.
    let _priority = VoiceInstallGuard::begin();
    let data_dir = state.data_dir.clone();
    let app2 = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || install_kokoro(&app2, &data_dir)).await.map_err(|e| e.to_string())?;
    if let Err(e) = result {
        emit(&app, "kokoro", "error", &format!("Kokoro installation failed: {e}"), 0.0);
        return Err(e.to_string());
    }
    emit(&app, "kokoro", "starting", "Starting Kokoro…", 0.95);
    let status = speech_engine_start(state, engine).await?;
    emit(&app, "kokoro", "ready", "Kokoro is ready.", 1.0);
    Ok(status)
}

#[tauri::command]
pub async fn speech_engine_start(state: State<'_, AppState>, engine: String) -> Result<SpeechEngineStatus, String> {
    if engine != "kokoro" { return Err(format!("Unknown managed speech engine: {engine}")) }
    start_kokoro(&state)?;
    Ok(kokoro_status(&state))
}

fn start_kokoro(state: &AppState) -> Result<(), String> {
    let root = engine_root(&state.data_dir, "kokoro");
    if !root.join("installed.ok").is_file() { return Err("Kokoro is not installed".into()) }
    let port = crate::helpers::HelperManager::free_loopback_port().map_err(|e| e.to_string())?;
    let source = root.join("app");
    let log_dir = state.data_dir.join("logs");
    std::fs::create_dir_all(&log_dir).map_err(|e| e.to_string())?;
    let log = std::fs::File::create(log_dir.join("kokoro.log")).map_err(|e| e.to_string())?;
    let mut command = Command::new(root.join("runtime").join(crate::platform::exe("uv")));
    command.args(["run", "--no-sync", "uvicorn", "api.src.main:app", "--host", "127.0.0.1", "--port", &port.to_string()])
        .current_dir(&source)
        .env("UV_PYTHON_INSTALL_DIR", root.join("python"))
        .env("UV_CACHE_DIR", root.join("cache"))
        .env("PYTHONUTF8", "1").env("PROJECT_ROOT", &source).env("USE_GPU", "false")
        .env("PYTHONPATH", format!("{}{}{}", source.display(), if cfg!(windows) { ";" } else { ":" }, source.join("api").display()))
        .env("MODEL_DIR", "src/models").env("VOICES_DIR", "src/voices/v1_0").env("WEB_PLAYER_PATH", source.join("web"))
        .stdin(Stdio::null()).stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?)).stderr(Stdio::from(log));
    // Item 36 escape hatch: ED_KOKORO_CONSOLE=1 leaves the child's
    // console visible — the A/B for the hide_console+redirect suspect
    // in the evaporating-spawn report. Default hidden, as shipped.
    if std::env::var_os("ED_KOKORO_CONSOLE").is_none() {
        hide_console(&mut command);
    }
    state.helpers.spawn("speech:kokoro", &mut command).map_err(|e| e.to_string())?;
    let url = format!("http://127.0.0.1:{port}");
    // Item 36: a child that dies instantly must FAIL instantly with a
    // diagnosis, not poll a corpse for sixty seconds (the old `.any`
    // closure's `return false` skipped one iteration, not the loop —
    // the reported hang at 95%).
    let mut ready = false;
    let mut waited_ms = 0u64;
    let mut died = false;
    for _ in 0..240 {
        if !state.helpers.running("speech:kokoro") {
            died = true;
            break;
        }
        if std::net::TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_millis(100)).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
        waited_ms += 250;
    }
    if !ready {
        state.helpers.stop("speech:kokoro");
        let log_bytes = std::fs::metadata(log_dir.join("kokoro.log")).map(|m| m.len()).unwrap_or(0);
        return Err(startup_failure_note(died, waited_ms, log_bytes));
    }
    // Only the URL is ours to rewrite (fresh port every launch); the
    // VOICE is the commander's. Hardcoding af_heart here silently threw
    // away a chosen af_bella on every start (field case 2026-09-05).
    let voice = {
        let cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.voice_server
            .as_ref()
            .map(|c| c.voice.trim().to_string())
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "af_heart".into())
    };
    let config = ed_voice::ServerConfig { url: url.clone(), model: "kokoro".into(), voice, api_key: None };
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.voice_server = Some(config.clone());
        // An explicit start IS consent to use the server; start_configured
        // never reaches here when the commander has it switched off.
        cfg.voice_server_enabled = true;
        cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    }
    state.voice.audio().set_server(Some(config));
    crate::telemetry::set_voice_engine("voice_kokoro");
    Ok(())
}

/// Called from a background startup thread. A managed engine always gets a
/// newly allocated free port; a stale URL is never trusted across launches.
pub fn start_configured(state: &AppState) -> Result<bool, String> {
    if managed_kokoro_selected(state) {
        start_kokoro(state)?;
        return Ok(true);
    }
    Ok(false)
}

#[tauri::command]
pub async fn speech_engine_stop(state: State<'_, AppState>, engine: String) -> Result<SpeechEngineStatus, String> {
    if engine != "kokoro" { return Err(format!("Unknown managed speech engine: {engine}")) }
    state.helpers.stop(&format!("speech:{engine}"));
    Ok(kokoro_status(&state))
}

#[tauri::command]
pub async fn speech_engine_remove(state: State<'_, AppState>, engine: String) -> Result<SpeechEngineStatus, String> {
    if engine != "kokoro" { return Err(format!("Unknown managed speech engine: {engine}")) }
    state.helpers.stop("speech:kokoro");
    let root = engine_root(&state.data_dir, "kokoro");
    if root.is_dir() { std::fs::remove_dir_all(&root).map_err(|e| e.to_string())?; }
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        if cfg.voice_server.as_ref().is_some_and(|c| c.model == "kokoro") { cfg.voice_server = None; cfg.voice_server_enabled = false; cfg.save(&state.data_dir).map_err(|e| e.to_string())?; }
    }
    state.voice.audio().set_server(None);
    Ok(kokoro_status(&state))
}

#[cfg(test)]
mod tests {
    /// Item 36: the startup failure line names the disease — early death
    /// with an empty log indicts the spawn path and offers the console
    /// hatch; early death with content points at the log; a timeout is
    /// its own message.
    #[test]
    fn the_startup_diagnosis_separates_death_from_timeout() {
        let empty = super::startup_failure_note(true, 0, 0);
        assert!(empty.contains("EMPTY") && empty.contains("spawn path") && empty.contains("ED_KOKORO_CONSOLE"), "{empty}");
        let last_words = super::startup_failure_note(true, 250, 4096);
        assert!(last_words.contains("last words") && !last_words.contains("EMPTY"), "{last_words}");
        let slow = super::startup_failure_note(false, 60_000, 9999);
        assert!(slow.contains("60 s") && !slow.contains("exited"), "{slow}");
    }
}
