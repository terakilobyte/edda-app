//! Voice input: an activation word and/or a push-to-talk key, offline.
//!
//! Idle: a grammar recognizer listens for the wake phrase only (cheap and
//! near-perfect). On the wake phrase -- or while the PTT key is held --
//! a full-vocabulary recognizer takes the next utterance. Short, known
//! orders ("target next", "skip", "how many jumps left") are handled here
//! without the LLM; anything else goes to the ship computer and its answer
//! is spoken. Audio never leaves the machine.
//!
//! Setup downloads Vosk's small English model and the Windows library into
//! `.data/tools/vosk/` once (~55 MB).

use crate::state::AppState;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::MutexGuard;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

const MODEL_URL: &str = "https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip";
/// Vosk's full English model. Not allowable (maintainer-ruled 2026-09-03): its
/// graph idles at ~4.5 GB of RAM to do a job the small model does fine,
/// and Parakeet owns dictation. Never selected, never offered.
const BANNED_MODEL_DIR: &str = "vosk-model-en-us-0.22";
/// Parakeet (NVIDIA, TDT 0.6B v2, int8) through sherpa-onnx: state-of-the-art English dictation.
/// The models are platform-neutral; the RUNTIME LIBRARIES are not, and
/// were hardcoded to the Windows builds until the first Linux tester
/// (2026-09-05) — a Linux install would have downloaded unloadable DLLs.
/// Asset names verified against the upstream releases. macOS stays
/// unwired here (its lane opens with the Mac build).
#[cfg(target_os = "linux")]
const SHERPA_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.6/sherpa-onnx-v1.13.6-linux-x64-shared-no-tts.tar.bz2";
#[cfg(not(target_os = "linux"))]
const SHERPA_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.6/sherpa-onnx-v1.13.6-win-x64-shared-MD-Release-no-tts.tar.bz2";
const PARAKEET_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2";
const PARAKEET_DIR: &str = "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8";
#[cfg(target_os = "linux")]
const LIB_URL: &str = "https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-linux-x86_64-0.3.45.zip";
#[cfg(not(target_os = "linux"))]
const LIB_URL: &str = "https://github.com/alphacep/vosk-api/releases/download/v0.3.45/vosk-win64-0.3.45.zip";

/// Where push-to-talk comes from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[derive(Default)]
pub enum PttSource {
    #[default]
    None,
    /// A global hotkey, e.g. "Ctrl+Alt+Space" (never Shift: the game's UIFocus key).
    Keyboard { hotkey: String },
    /// A joystick button (WinMM device id, 1-based button), vJoy included.
    Joystick { device: u32, button: u32, name: String },
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ListenConfig {
    /// Listen at all (mic open).
    pub enabled: bool,
    /// Activation phrase, lower-case words; empty = wake word off.
    pub wake_word: String,
    /// Global push-to-talk hotkey, e.g. "Ctrl+Alt+Space"; empty = off.
    /// (Legacy field; `ptt` wins when present.)
    #[serde(default)]
    pub ptt_hotkey: String,
    #[serde(default)]
    pub ptt: Option<PttSource>,
    /// Microphone device name (None = system default).
    #[serde(default)]
    pub mic_device: Option<String>,
    /// Speech output device name (None = system default).
    #[serde(default)]
    pub output_device: Option<String>,
    /// "small" (40 MB) or "parakeet" (480 MB, best;
    /// Vosk small stays for the wake word).
    #[serde(default)]
    pub model: Option<String>,
    /// Seconds to wait for an order after the wake word.
    pub window_secs: u64,
}

impl Default for ListenConfig {
    fn default() -> Self {
        ListenConfig { enabled: false, wake_word: "hey edda".into(), ptt_hotkey: String::new(), ptt: Some(PttSource::default()), mic_device: None, output_device: None, model: None, window_secs: 6 }
    }
}

/// The listener's state, owned by `AppState` behind one mutex.
///
/// The wake-word state machine itself is unchanged; this only gives its
/// eight former process globals one home, so two `AppState`s (a test and
/// the app, or two tests) cannot see each other's listener.
#[derive(Default)]
pub struct ListenState {
    /// 0 off, 1 idle (waiting for wake word), 2 listening (order), 3 thinking
    pub phase: u8,
    pub run: bool,
    /// Generation of the current listener loop; bumping it retires every
    /// older loop on its next chunk, so a restart never leaves two running.
    pub gen: u64,
    pub ptt: bool,
    pub setup_running: bool,
    /// Parakeet, loaded once (a few seconds) and kept across listener restarts.
    pub parakeet: Option<std::sync::Arc<ed_listen::parakeet::Parakeet>>,
    pub ptt_shortcut: Option<Shortcut>,
    /// (device id, 1-based button) polled by the joystick thread.
    pub joy_src: Option<(u32, u32)>,
    pub joy_thread: bool,
}

impl ListenState {
    /// Retire any running loop; the new one owns the returned generation.
    pub fn begin_loop(&mut self) -> u64 {
        self.gen += 1;
        self.run = true;
        self.gen
    }

    /// Stop the current loop on its next chunk.
    pub fn stop(&mut self) {
        self.gen += 1;
        self.run = false;
    }

    /// Does the loop that owns `gen` still own the microphone?
    pub fn is_current(&self, gen: u64) -> bool {
        self.gen == gen
    }

    /// A loop exited. Only the newest loop reports the listener as off; an
    /// older one that was retired by a restart must not.
    pub fn loop_ended(&mut self, gen: u64) {
        if self.gen == gen {
            self.run = false;
            self.phase = 0;
        }
    }
}

fn listen(state: &AppState) -> MutexGuard<'_, ListenState> {
    state.listen.lock().unwrap_or_else(|e| e.into_inner())
}

impl ListenConfig {
    pub fn ptt_source(&self) -> PttSource {
        match &self.ptt {
            Some(p) => p.clone(),
            None if self.ptt_hotkey.trim().is_empty() => PttSource::None,
            None => PttSource::Keyboard { hotkey: self.ptt_hotkey.clone() },
        }
    }
}

fn tools_dir(state: &AppState) -> PathBuf {
    state.data_dir.join("tools").join("vosk")
}

fn is_model(p: &Path) -> bool {
    p.is_dir() && p.join("conf").is_dir() && p.join("am").is_dir()
}

/// The small model if installed, else any other model present — except
/// [`BANNED_MODEL_DIR`], which is never selected. The Vosk engine's job
/// is the wake grammar (and fallback dictation); small does both.
fn find_model(dir: &Path) -> Option<PathBuf> {
    let candidates = |f: fn(&str) -> bool| {
        std::fs::read_dir(dir).ok().into_iter().flatten().flatten().map(|e| e.path()).find(move |p| {
            is_model(p) && p.file_name().is_some_and(|n| { let n = n.to_string_lossy(); n != BANNED_MODEL_DIR && f(&n) })
        })
    };
    candidates(|n| n.contains("small")).or_else(|| candidates(|_| true))
}

#[derive(Debug, Serialize)]
pub struct AudioDevices {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub mic_device: Option<String>,
    pub output_device: Option<String>,
    pub models_installed: Vec<String>,
}

#[tauri::command]
pub async fn audio_devices(state: State<'_, AppState>) -> Result<AudioDevices, String> {
    Ok({
    let cfg = config_of(&state);
    let dir = tools_dir(&state);
    let mut models: Vec<String> = std::fs::read_dir(&dir).ok().map(|r| r.flatten().map(|e| e.path()).filter(|p| is_model(p)).filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect()).unwrap_or_default();
    if parakeet_installed(&dir) {
        models.push(PARAKEET_DIR.into());
    }
    AudioDevices { inputs: ed_listen::list_inputs(), outputs: ed_voice::list_outputs(), mic_device: cfg.mic_device, output_device: cfg.output_device, models_installed: models }
    })
}

fn parakeet_installed(dir: &Path) -> bool {
    ed_listen::parakeet::is_model_dir(&dir.join(PARAKEET_DIR)) && ed_listen::parakeet::find_lib(dir).is_some()
}

fn find_lib(dir: &Path) -> Option<PathBuf> {
    let lib = crate::platform::dylib("vosk");
    if dir.join(&lib).is_file() {
        return Some(dir.to_path_buf());
    }
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.join(&lib).is_file())
}

#[derive(Debug, Serialize)]
pub struct ListenStatus {
    pub ready: bool,
    /// Parakeet model + sherpa-onnx library present.
    pub parakeet_ready: bool,
    /// Which engine takes dictation right now.
    pub dictation: &'static str,
    pub running: bool,
    pub phase: &'static str,
    pub model_dir: Option<String>,
    pub lib_dir: Option<String>,
    pub setup_running: bool,
    pub config: ListenConfig,
}

fn phase_name(p: u8) -> &'static str {
    match p {
        1 => "idle",
        2 => "listening",
        3 => "thinking",
        _ => "off",
    }
}

pub fn config_of(state: &AppState) -> ListenConfig {
    state.config.lock().unwrap_or_else(|e| e.into_inner()).listen.clone().unwrap_or_default()
}

#[tauri::command]
pub async fn listen_status(state: State<'_, AppState>) -> Result<ListenStatus, String> {
    Ok({
    let dir = tools_dir(&state);
    let model = find_model(&dir);
    let lib = find_lib(&dir);
    let parakeet_ready = parakeet_installed(&dir);
    let (running, phase, setup_running) = {
        let l = listen(&state);
        (l.run, l.phase, l.setup_running)
    };
    ListenStatus {
        ready: model.is_some() && lib.is_some(),
        parakeet_ready,
        dictation: if parakeet_ready && config_of(&state).model.as_deref() == Some("parakeet") { "parakeet" } else { "vosk" },
        running,
        phase: phase_name(phase),
        model_dir: model.map(|p| p.display().to_string()),
        lib_dir: lib.map(|p| p.display().to_string()),
        setup_running,
        config: config_of(&state),
    }
    })
}

#[tauri::command]
pub async fn listen_config_set(app: AppHandle, state: State<'_, AppState>, config: ListenConfig) -> Result<ListenConfig, String> {
    {
        let mut cfg = state.config.lock().unwrap_or_else(|e| e.into_inner());
        cfg.listen = Some(config.clone());
        cfg.save(&state.data_dir).map_err(|e| e.to_string())?;
    }
    state.voice.audio().set_output_device(config.output_device.clone());
    // Re-register the PTT source; restart the listener if it is running.
    register_ptt_source(&app, &config.ptt_source())?;
    if listen(&state).run {
        stop_listening(&state);
        start_listening(app.clone())?;
    } else if config.enabled {
        start_listening(app.clone())?;
    }
    Ok(config)
}

/// Download and unpack the model and library. Progress on `listen-setup`.
#[tauri::command]
pub async fn listen_setup(app: AppHandle, state: State<'_, AppState>, model: Option<String>, wake_word: Option<bool>) -> Result<(), String> {
    let _ = wake_word;
    {
        let mut l = listen(&state);
        if l.setup_running {
            return Err("setup already running".into());
        }
        l.setup_running = true;
    }
    let dir = tools_dir(&state);
    let http = state.http.clone();
    let app2 = app.clone();
    let mut jobs: Vec<(&str, &str)> = Vec::new();
    match model.as_deref() {
        Some("parakeet") => {
            // Vosk small still does the wake word; Parakeet takes dictation.
            if find_model(&dir).is_none() {
                jobs.push(("wake-word model", MODEL_URL));
            }
            if ed_listen::parakeet::find_lib(&dir).is_none() {
                jobs.push(("sherpa-onnx library", SHERPA_URL));
            }
            if !ed_listen::parakeet::is_model_dir(&dir.join(PARAKEET_DIR)) {
                jobs.push(("Parakeet model (460 MB)", PARAKEET_URL));
            }
        }
        _ => jobs.push(("model", MODEL_URL)),
    }
    if find_lib(&dir).is_none() {
        jobs.push(("Vosk library", LIB_URL));
    }
    let jobs: Vec<(String, String)> = jobs.into_iter().map(|(a, b)| (a.into(), b.into())).collect();
    tauri::async_runtime::spawn(async move {
        // Voice setup outranks the heavy data sync (sequencing ruling
        // 2026-09-04) — the community sync pauses at its phase
        // boundaries while this guard is held.
        let _priority = crate::speech_engines::VoiceInstallGuard::begin();
        let r: anyhow::Result<()> = async {
            std::fs::create_dir_all(&dir)?;
            use futures_util::{stream, StreamExt as _, TryStreamExt as _};
            stream::iter(jobs.into_iter().map(|(label, url)| {
                let app2 = app2.clone(); let http = http.clone(); let dir = dir.clone();
                async move {
                let _ = app2.emit(crate::events::LISTEN_SETUP, serde_json::json!({ "phase": &label, "status": "downloading" }));
                // Streamed to a temp file: the Parakeet and large-Vosk archives are too big to hold in memory.
                let tmp = dir.join(format!(".download-{}", url.rsplit('/').next().unwrap_or("archive")));
                {
                    use tokio::io::AsyncWriteExt;
                    let resp = http.get(&url).header(reqwest::header::USER_AGENT, "EDDA").send().await?.error_for_status()?;
                    let total = resp.content_length();
                    let mut f = tokio::fs::File::create(&tmp).await?;
                    let mut stream = resp.bytes_stream();
                    let mut got: u64 = 0;
                    let mut last = std::time::Instant::now();
                    while let Some(chunk) = stream.next().await {
                        let chunk = chunk?;
                        f.write_all(&chunk).await?;
                        got += chunk.len() as u64;
                        if last.elapsed().as_millis() > 500 {
                            last = std::time::Instant::now();
                            let _ = app2.emit(crate::events::LISTEN_SETUP, serde_json::json!({ "phase": &label, "status": "downloading", "bytes": got, "total": total }));
                        }
                    }
                    f.flush().await?;
                }
                let _ = app2.emit(crate::events::LISTEN_SETUP, serde_json::json!({ "phase": &label, "status": "unpacking" }));
                let dir2 = dir.clone();
                let tmp2 = tmp.clone();
                let is_tbz = url.ends_with(".tar.bz2");
                tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
                    let r = if is_tbz { untar_bz2_to(&tmp2, &dir2) } else { unzip_file_to(&tmp2, &dir2) };
                    let _ = std::fs::remove_file(&tmp2);
                    r
                })
                .await??;
                Ok::<(), anyhow::Error>(())
                }
            // One archive at a time, unpacked before the next downloads
            // (sequencing ruling: never fetch the next piece before the
            // last is written).
            })).buffer_unordered(1).try_collect::<Vec<_>>().await?;
            Ok(())
        }
        .await;
        listen(&app2.state::<AppState>()).setup_running = false;
        match r {
            Ok(()) => {
                let _ = app2.emit(crate::events::LISTEN_SETUP, serde_json::json!({ "phase": "done", "status": "ok" }));
            }
            Err(e) => {
                let _ = app2.emit(crate::events::LISTEN_SETUP, serde_json::json!({ "phase": "done", "status": "error", "error": e.to_string() }));
            }
        }
    });
    Ok(())
}

/// Remove only EDDA-managed speech-recognition runtimes and models. Voice
/// output models live elsewhere and are deliberately unaffected.
#[tauri::command]
pub async fn listen_models_remove(state: State<'_, AppState>) -> Result<(), String> {
    if listen(&state).setup_running {
        return Err("wait for the current voice-model setup to finish".into());
    }
    stop_listening(&state);
    listen(&state).parakeet = None;
    let dir = tools_dir(&state);
    tauri::async_runtime::spawn_blocking(move || {
        if dir.exists() { std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?; }
        Ok::<(), String>(())
    }).await.map_err(|e| e.to_string())?
}

fn untar_bz2_to(file: &Path, dir: &Path) -> anyhow::Result<()> {
    let f = std::fs::File::open(file)?;
    let bz = bzip2::read::MultiBzDecoder::new(std::io::BufReader::new(f));
    let mut ar = tar::Archive::new(bz);
    for entry in ar.entries()? {
        let mut e = entry?;
        let Ok(rel) = e.path().map(|p| p.into_owned()) else { continue };
        // Test wavs are dead weight; everything else lands as-is.
        if rel.components().any(|c| c.as_os_str() == "test_wavs") {
            continue;
        }
        let out = dir.join(&rel);
        if e.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            e.unpack(&out)?;
        }
    }
    Ok(())
}

fn unzip_file_to(file: &Path, dir: &Path) -> anyhow::Result<()> {
    let f = std::fs::File::open(file)?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f))?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let Some(rel) = f.enclosed_name() else { continue };
        let out = dir.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut w = std::fs::File::create(&out)?;
            std::io::copy(&mut f, &mut w)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn listen_start(app: AppHandle) -> Result<(), String> {
    start_listening(app)
}

#[tauri::command]
pub async fn listen_stop(state: State<'_, AppState>) -> Result<(), String> {
    stop_listening(&state);
    Ok(())
}

/// Push-to-talk from the UI (mouse down/up on a button).
#[tauri::command]
pub async fn listen_ptt(app: AppHandle, down: bool) {
    ptt_set(&app, down);
}

pub fn ptt_set(app: &AppHandle, down: bool) {
    let state = app.state::<AppState>();
    if down {
        // Barge-in: the commander wants the floor.
        state.voice.interrupt();
    }
    let phase = {
        let mut l = listen(&state);
        l.ptt = down;
        l.phase
    };
    let _ = app.emit(crate::events::LISTEN_STATE, serde_json::json!({ "phase": if down { "listening" } else { phase_name(phase) }, "ptt": down }));
}

/// Called by the global-shortcut handler for shortcuts it does not own.
pub fn hotkey_event(app: &AppHandle, shortcut: &Shortcut, st: ShortcutState) -> bool {
    let is_ptt = listen(&app.state::<AppState>()).ptt_shortcut.as_ref() == Some(shortcut);
    if !is_ptt {
        return false;
    }
    ptt_set(app, st == ShortcutState::Pressed);
    true
}

fn register_ptt_source(app: &AppHandle, src: &PttSource) -> Result<(), String> {
    match src {
        PttSource::None => {
            register_ptt(app, "")?;
            listen(&app.state::<AppState>()).joy_src = None;
        }
        PttSource::Keyboard { hotkey } => {
            if hotkey.to_lowercase().contains("shift") {
                return Err("Shift is the game's UIFocus key; pick a hotkey without Shift".into());
            }
            register_ptt(app, hotkey)?;
            listen(&app.state::<AppState>()).joy_src = None;
        }
        PttSource::Joystick { device, button, .. } => {
            register_ptt(app, "")?;
            listen(&app.state::<AppState>()).joy_src = Some((*device, *button));
            start_joy_thread(app.clone());
        }
    }
    Ok(())
}

/// Polls the chosen joystick button at 50 Hz; one thread for the app's life.
fn start_joy_thread(app: AppHandle) {
    {
        let state = app.state::<AppState>();
        let mut l = listen(&state);
        if l.joy_thread {
            return;
        }
        l.joy_thread = true;
    }
    std::thread::spawn(move || {
        let mut was = false;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(20));
            let src = listen(&app.state::<AppState>()).joy_src;
            let Some((dev, btn)) = src else {
                was = false;
                continue;
            };
            let now = ed_input::joy::is_down(dev, btn);
            if now != was {
                was = now;
                ptt_set(&app, now);
            }
        }
    });
}

#[tauri::command]
pub async fn joy_devices() -> Vec<ed_input::joy::JoyDevice> {
    ed_input::joy::devices()
}

/// Wait up to `secs` for the commander to press a key or a joystick
/// button, and return it as a PTT source.
#[tauri::command]
pub async fn ptt_capture(secs: Option<u64>) -> Result<PttSource, String> {
    let secs = secs.unwrap_or(8).clamp(2, 30);
    tauri::async_runtime::spawn_blocking(move || {
        let devices = ed_input::joy::devices();
        let baseline: Vec<(u32, u32)> = devices.iter().map(|d| (d.id, ed_input::joy::buttons(d.id).unwrap_or(0))).collect();
        ed_input::record::start()?;
        let started = std::time::Instant::now();
        let result: Option<PttSource> = loop {
            if started.elapsed().as_secs() >= secs {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            // Joystick: any button newly down.
            let joy_hit = baseline.iter().find_map(|(id, base)| {
                let now = ed_input::joy::buttons(*id).unwrap_or(0);
                let fresh = now & !base;
                (fresh != 0).then(|| {
                    let button = fresh.trailing_zeros() + 1;
                    let name = devices.iter().find(|d| d.id == *id).map(|d| d.name.clone()).unwrap_or_default();
                    PttSource::Joystick { device: *id, button, name: format!("{name} button {button}") }
                })
            });
            if let Some(hit) = joy_hit {
                break Some(hit);
            }
            // Hats: a direction pushed from centre.
            let hat_hit = devices.iter().find_map(|d| {
                let p = ed_input::joy::pov(d.id)?;
                let dir = ed_input::joy::hat_dir(p);
                let button = ed_input::joy::HAT_BASE + dir;
                Some(PttSource::Joystick { device: d.id, button, name: format!("{} {}", d.name, ed_input::joy::hat_name(button)) })
            });
            if let Some(hit) = hat_hit {
                break Some(hit);
            }
            // Keyboard: first non-modifier key down, with the modifiers held.
            let events = ed_input::record::peek();
            let mut held: Vec<&str> = Vec::new();
            let mut found: Option<String> = None;
            for e in &events {
                let Some(name) = ed_input::record::key_name(e.sc) else { continue };
                let is_mod = matches!(name, "LeftShift" | "RightShift" | "LeftControl" | "RightControl" | "LeftAlt" | "RightAlt");
                if is_mod {
                    if e.down { if !held.contains(&name) { held.push(name) } } else { held.retain(|h| *h != name) }
                } else if e.down {
                    let mut parts: Vec<String> = Vec::new();
                    if held.iter().any(|m| m.contains("Control")) { parts.push("Ctrl".into()) }
                    if held.iter().any(|m| m.contains("Alt")) { parts.push("Alt".into()) }
                    if held.iter().any(|m| m.contains("Shift")) { parts.push("Shift".into()) }
                    parts.push(shortcut_code(name));
                    found = Some(parts.join("+"));
                    break;
                }
            }
            if let Some(hotkey) = found {
                break Some(PttSource::Keyboard { hotkey });
            }
        };
        let _ = ed_input::record::stop();
        result.ok_or_else(|| "nothing pressed".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Binds-style key name -> global-shortcut `Code` name.
fn shortcut_code(name: &str) -> String {
    match name {
        n if n.len() == 1 && n.chars().all(|c| c.is_ascii_uppercase()) => format!("Key{n}"),
        n if n.len() == 1 && n.chars().all(|c| c.is_ascii_digit()) => format!("Digit{n}"),
        "UpArrow" => "ArrowUp".into(),
        "DownArrow" => "ArrowDown".into(),
        "LeftArrow" => "ArrowLeft".into(),
        "RightArrow" => "ArrowRight".into(),
        "Grave" => "Backquote".into(),
        "Equals" => "Equal".into(),
        "LeftBracket" => "BracketLeft".into(),
        "RightBracket" => "BracketRight".into(),
        "SemiColon" => "Semicolon".into(),
        "Apostrophe" => "Quote".into(),
        n if n.starts_with("Numpad_") => n.replace("Numpad_", "Numpad").replace("NumpadAdd", "NumpadAdd"),
        other => other.to_string(),
    }
}

fn register_ptt(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut cur = listen(&state);
    if let Some(old) = cur.ptt_shortcut.take() {
        let _ = app.global_shortcut().unregister(old);
    }
    if hotkey.trim().is_empty() {
        return Ok(());
    }
    let sc: Shortcut = hotkey.parse().map_err(|e| format!("bad hotkey {hotkey:?}: {e}"))?;
    app.global_shortcut().register(sc).map_err(|e| format!("could not register {hotkey}: {e}"))?;
    cur.ptt_shortcut = Some(sc);
    Ok(())
}

pub fn setup(app: &AppHandle) {
    let state = app.state::<AppState>();
    let cfg = config_of(&state);
    state.voice.audio().set_output_device(cfg.output_device.clone());
    if let Err(e) = register_ptt_source(app, &cfg.ptt_source()) {
        tracing::warn!(error = %e, "push-to-talk source");
    }
    if cfg.enabled {
        if let Err(e) = start_listening(app.clone()) {
            tracing::warn!(error = %e, "listening not started");
        }
    }
}

fn stop_listening(state: &AppState) {
    listen(state).stop();
}

fn start_listening(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    // Retire any running loop first; the new one owns the next generation.
    let my_gen = listen(&state).begin_loop();
    let dir = tools_dir(&state);
    let (Some(model), Some(lib)) = (find_model(&dir), find_lib(&dir)) else {
        listen(&state).run = false;
        return Err("voice model not installed -- run Setup in Settings → Voice input".into());
    };
    let cfg = config_of(&state);
    std::thread::spawn(move || {
        if let Err(e) = listen_loop(&app, &lib, &model, cfg, my_gen) {
            tracing::warn!(error = %e, "listening stopped");
            let _ = app.emit(crate::events::LISTEN_STATE, serde_json::json!({ "phase": "off", "error": e.to_string() }));
        }
        // Only the newest loop reports the listener as off.
        listen(&app.state::<AppState>()).loop_ended(my_gen);
    });
    Ok(())
}

fn listen_loop(app: &AppHandle, lib: &Path, model: &Path, cfg: ListenConfig, my_gen: u64) -> anyhow::Result<()> {
    let engine = ed_listen::Engine::load(lib, model)?;
    let mic = ed_listen::open_mic_named(cfg.mic_device.as_deref())?;
    tracing::info!(model = %model.display(), "speech model");
    let wake = cfg.wake_word.trim().to_lowercase();
    let mut wake_rec = if wake.is_empty() { None } else { Some(engine.grammar_recognizer(&[wake.as_str()])?) };
    let mut free_rec = engine.recognizer()?;
    let st = app.state::<AppState>();
    let parakeet = if cfg.model.as_deref() == Some("parakeet") {
        let dir = model.parent().map(Path::to_path_buf).unwrap_or_default();
        let loaded = listen(&st).parakeet.clone();
        match loaded {
            Some(p) => Some(p),
            // Loaded outside the lock: it takes seconds, and push-to-talk
            // must not wait on it.
            None => match ed_listen::parakeet::find_lib(&dir) {
                Some(lib_dir) => match ed_listen::parakeet::Parakeet::load(&lib_dir, &dir.join(PARAKEET_DIR), std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(4)) {
                    Ok(p) => {
                        let p = std::sync::Arc::new(p);
                        listen(&st).parakeet = Some(p.clone());
                        Some(p)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Parakeet not loaded; dictation falls back to Vosk");
                        None
                    }
                },
                None => {
                    tracing::warn!("sherpa-onnx library missing; dictation falls back to Vosk");
                    None
                }
            },
        }
    } else {
        None
    };
    tracing::info!(wake = %wake, ptt = %cfg.ptt_hotkey, mic = %mic.device, parakeet = parakeet.is_some(), "listening");
    // Everything said during an order window, for Parakeet (30 s cap).
    let mut order_pcm: Vec<i16> = Vec::new();
    // Final text is Parakeet's when it is loaded and Vosk did not already hear
    // a known short order (those stay instant).
    let finish = |vosk_text: String, pcm: &mut Vec<i16>| -> String {
        let t = vosk_text.trim().to_lowercase();
        let out = match parakeet.as_ref() {
            Some(p) if !pcm.is_empty() && direct_order(&t).is_none() && cockpit_order(&t).is_none() => {
                let started = std::time::Instant::now();
                match p.transcribe(pcm) {
                    Ok(s) if !s.trim().is_empty() => {
                        tracing::info!(vosk = %t, parakeet = %s, ms = started.elapsed().as_millis(), "dictation");
                        s
                    }
                    Ok(_) => t,
                    Err(e) => {
                        tracing::warn!(error = %e, "Parakeet failed; using Vosk");
                        t
                    }
                }
            }
            _ => t,
        };
        pcm.clear();
        out
    };
    let set_phase = |p: u8| {
        let ptt = {
            let mut l = listen(&st);
            l.phase = p;
            l.ptt
        };
        let _ = app.emit(crate::events::LISTEN_STATE, serde_json::json!({ "phase": phase_name(p), "ptt": ptt }));
    };
    set_phase(1);
    let mut order_until: Option<std::time::Instant> = None;
    let mut was_ptt = false;
    let mut chunks: u64 = 0;
    let mut peak: i16 = 0;
    let mut last_partial = std::time::Instant::now();
    while listen(&st).is_current(my_gen) {
        let chunk = match mic.rx.recv_timeout(std::time::Duration::from_millis(300)) {
            Ok(c) => c,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        chunks += 1;
        peak = peak.max(chunk.iter().map(|s| s.saturating_abs()).max().unwrap_or(0));
        let ptt = listen(&st).ptt;
        if ptt && !was_ptt {
            chunks = 0;
            peak = 0;
            free_rec.reset();
            order_pcm.clear();
            set_phase(2);
        }
        if !ptt && was_ptt {
            // Key released: whatever was said is the order.
            let text = finish(free_rec.final_result(), &mut order_pcm);
            was_ptt = false;
            tracing::info!(chunks, peak, text = %text, "push-to-talk released");
            if text.trim().is_empty() {
                let note = if peak < 50 {
                    "microphone is silent — check the selected input or hardware mute"
                } else {
                    "nothing recognised"
                };
                let _ = app.emit(crate::events::LISTEN_HEARD, serde_json::json!({ "text": "", "note": note, "peak": peak }));
            } else {
                handle_heard(app, &text);
            }
            set_phase(1);
            continue;
        }
        was_ptt = ptt;
        if ptt || order_until.is_some() {
            if order_pcm.len() < 16000 * 30 {
                order_pcm.extend_from_slice(&chunk);
            }
            let ended = free_rec.accept(&chunk);
            if last_partial.elapsed().as_millis() >= 700 {
                last_partial = std::time::Instant::now();
                let p = free_rec.partial();
                tracing::debug!(partial = %p, chunks, peak, "listening");
                let _ = app.emit(crate::events::LISTEN_PARTIAL, serde_json::json!({ "text": p, "peak": peak }));
            }
            if !ptt && (ended || order_until.is_some_and(|t| std::time::Instant::now() > t)) {
                let text = finish(if ended { free_rec.result() } else { free_rec.final_result() }, &mut order_pcm);
                order_until = None;
                if !text.trim().is_empty() {
                    handle_heard(app, &text);
                } else {
                    set_phase(1);
                }
            }
            continue;
        }
        if let Some(w) = wake_rec.as_mut() {
            if w.accept(&chunk) {
                let heard = w.result();
                if heard.contains(&wake) {
                    // Wake word: cut anything being said, acknowledge, open the window.
                    let st = app.state::<AppState>();
                    st.voice.interrupt();
                    st.voice.say("Yes, Commander?");
                    free_rec.reset();
                    order_pcm.clear();
                    order_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(cfg.window_secs.max(2)));
                    set_phase(2);
                }
            }
        }
    }
    Ok(())
}

/// Route what was heard: direct verbs first, then the ship computer.
fn handle_heard(app: &AppHandle, text: &str) {
    let t = text.trim().to_lowercase();
    if t.is_empty() {
        let _ = app.emit(crate::events::LISTEN_HEARD, serde_json::json!({ "text": "", "note": "nothing recognised" }));
        return;
    }
    let _ = app.emit(crate::events::LISTEN_HEARD, serde_json::json!({ "text": t }));
    let state = app.state::<AppState>();
    listen(&state).phase = 3;
    let _ = app.emit(crate::events::LISTEN_STATE, serde_json::json!({ "phase": "thinking" }));
    // Route/profit the ship computer produced this turn, for the tabs.
    let mut artefacts: (Option<serde_json::Value>, Option<serde_json::Value>) = (None, None);
    let reply: String = match direct_order(&t) {
        Some(Order::TargetNext) => match crate::follow::target_next(app) {
            Ok(m) => m,
            Err(e) => format!("Couldn't target: {e}"),
        },
        Some(Order::Skip) => {
            let mut ar = state.with_read(|s| crate::follow::load(s.conn()));
            match ar.as_mut() {
                Some(a) => {
                    a.next = (a.next + 1).min(a.route.hops.len());
                    let _ = state.with_store(|s| crate::follow::save_pub(s.conn(), a));
                    let _ = app.emit(crate::events::ROUTE_FOLLOW, crate::follow::view(Some(a)));
                    crate::follow::advance_text(a)
                }
                None => "No route is being followed.".into(),
            }
        }
        Some(Order::StopFollowing) => {
            let _ = state.with_store(|s| s.conn().execute("DELETE FROM active_route WHERE id = 1", []).map(|_| ()).map_err(|e| e.to_string()));
            let _ = app.emit(crate::events::ROUTE_FOLLOW, crate::follow::view(None));
            match crate::follow::clear_in_game(&state) {
                Ok(m) if m.starts_with("cleared") => "Route cleared, in the app and in the game.".into(),
                Ok(_) => "Route cleared.".into(),
                Err(e) => format!("Route cleared in the app; couldn't clear the game's: {e}"),
            }
        }
        Some(Order::JumpsLeft) | Some(Order::NextSystem) => match state.with_read(|s| crate::follow::load(s.conn())) {
            Some(a) => crate::follow::advance_text(&a),
            None => "No route is being followed.".into(),
        },
        Some(Order::Repeat) => state.voice.last_spoken().unwrap_or_else(|| "Nothing to repeat.".into()),
        Some(Order::Replan) => match tauri::async_runtime::block_on(crate::follow::replan_now(app.clone())) {
            Ok(m) => m,
            Err(e) => format!("Couldn't re-plan: {e}"),
        },
        Some(Order::Watch(id, on)) => {
            let mut ids = crate::callouts::signal_watch();
            if on { ids.push(id.to_string()); } else { ids.retain(|w| w != id); }
            let label = crate::callouts::SIGNALS.iter().find(|(i, _, _)| *i == id).map(|(_, l, _)| *l).unwrap_or(id);
            match crate::commands::set_signal_watch(&state, ids) {
                Ok(()) => if on { format!("Watching for {}.", label.to_lowercase()) } else { format!("No longer watching for {}.", label.to_lowercase()) },
                Err(e) => format!("Couldn't change the watch list: {e}"),
            }
        }
        Some(Order::WatchList) => {
            let on = crate::callouts::signal_watch();
            if on.is_empty() {
                "Not watching for any signals.".into()
            } else {
                let labels: Vec<String> = crate::callouts::SIGNALS.iter().filter(|(i, _, _)| on.iter().any(|w| w == i)).map(|(_, l, _)| l.to_lowercase()).collect();
                format!("Watching for {}.", labels.join(", "))
            }
        }
        None if cockpit_order(&t).is_some() => {
            // Cockpit orders: pressed straight from the binds, no model round trip.
            let (presses, ack) = cockpit_order(&t).unwrap();
            let mut err: Option<String> = None;
            for (name, times) in presses {
                if let Err(e) = crate::control::press(name, times) {
                    err = Some(e);
                    break;
                }
            }
            match err {
                None => ack,
                Some(e) => format!("Couldn't do that: {e}"),
            }
        }
        None => {
            // Ship computer. Runs the tool loop; can take a while. The
            // route/profit artefacts ride the reply event: a route
            // plotted by voice used to reach the HUD but never the
            // Route/Trade tabs, because only the chat panel applied
            // Answer.route/.profit (field case 2026-09-05).
            match tauri::async_runtime::block_on(crate::ai::ask(&state, &t)) {
                Ok(a) => {
                    artefacts = (a.route, a.profit);
                    a.text
                }
                Err(e) => format!("Ship computer error: {e}"),
            }
        }
    };
    let _ = app.emit(
        crate::events::LISTEN_REPLY,
        serde_json::json!({ "text": reply, "route": artefacts.0, "profit": artefacts.1 }),
    );
    state.voice.say(&reply);
    listen(&state).phase = 1;
    let _ = app.emit(crate::events::LISTEN_STATE, serde_json::json!({ "phase": "idle" }));
}

enum Order {
    /// "I'm looking for high grade emissions" / "stop looking for ...".
    Watch(&'static str, bool),
    WatchList,
    /// "re-plan the route" from here with the real tank.
    Replan,
    TargetNext,
    Skip,
    StopFollowing,
    JumpsLeft,
    NextSystem,
    Repeat,
}

/// Cockpit orders that map straight to bindings: pips, gear, scoop, lights,
/// hardpoints, countermeasures, drive. Returns the presses and what to say.
fn cockpit_order(t: &str) -> Option<(Vec<(&'static str, u32)>, String)> {
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    // Pips: "full pips to systems" / "four pips to engines" / "two pips to weapons" / "reset pips".
    if t.contains("pip") {
        let target = if has(&["sys", "shield"]) { Some(("pips_systems", "systems")) } else if has(&["eng", "engine"]) { Some(("pips_engines", "engines")) } else if has(&["wep", "weapon"]) { Some(("pips_weapons", "weapons")) } else { None };
        if has(&["reset", "balance", "even"]) || target.is_none() {
            return Some((vec![("pips_reset", 1)], "Pips reset.".into()));
        }
        let (name, label) = target.unwrap();
        let n: u32 = if has(&["full", "max", "all", "four", "4"]) { 4 } else if has(&["three", "3"]) { 3 } else if has(&["two", "2"]) { 2 } else if has(&["one", "1"]) { 1 } else { 4 };
        // From a 2/2/2 reset each press adds one pip to the target (up to 4).
        let presses = n.saturating_sub(2);
        let mut v = vec![("pips_reset", 1)];
        if presses > 0 {
            v.push((name, presses));
        }
        return Some((v, format!("{n} pips to {label}.")));
    }
    let simple: &[(&[&str], &str, &str)] = &[
        (&["landing gear", "gear down", "gear up", "lower the gear", "raise the gear", "deploy gear", "retract gear"], "landing_gear", "Landing gear."),
        (&["cargo scoop", "open the scoop", "close the scoop"], "cargo_scoop", "Cargo scoop."),
        (&["lights on", "lights off", "toggle lights", "ship lights", "headlights"], "lights", "Lights."),
        (&["night vision"], "night_vision", "Night vision."),
        (&["hardpoints", "deploy weapons", "retract weapons", "weapons out", "weapons away"], "hardpoints", "Hardpoints."),
        (&["flight assist"], "flight_assist", "Flight assist."),
        (&["heat sink", "heatsink"], "heat_sink", "Heat sink away."),
        (&["chaff"], "chaff", "Chaff."),
        (&["shield cell"], "shield_cell", "Shield cell."),
        (&["silent running"], "silent_running", "Silent running."),
        (&["boost"], "boost", "Boosting."),
        (&["supercruise", "super cruise"], "supercruise", "Supercruise."),
        (&["jump", "hyperspace", "engage"], "jump_or_supercruise", "Engaging."),
        (&["highest threat"], "highest_threat", "Targeting the highest threat."),
        (&["next hostile", "target next hostile"], "next_hostile", "Next hostile."),
        (&["next target", "cycle target", "next ship", "target next ship"], "next_target", "Next target."),
        (&["next subsystem", "cycle subsystem", "power plant", "powerplant", "target the drives", "target drives", "target fsd", "target the fsd"], "next_subsystem", "Next subsystem."),
        (&["discovery scan", "honk"], "discovery_scan", "Scanning."),
        (&["galaxy map"], "galaxy_map", "Galaxy map."),
        (&["system map"], "system_map", "System map."),
        (&["throttle zero", "all stop", "full stop", "cut throttle"], "throttle_zero", "Throttle zero."),
        (&["full throttle", "throttle full", "throttle 100"], "throttle_100", "Full throttle."),
        (&["throttle 75", "three quarters"], "throttle_75", "Throttle 75."),
        (&["throttle 50", "half throttle"], "throttle_50", "Throttle 50."),
    ];
    for (phrases, name, ack) in simple {
        if has(phrases) {
            return Some((vec![(name, 1)], (*ack).to_string()));
        }
    }
    None
}

fn direct_order(t: &str) -> Option<Order> {
    // "guidance: ..." is the routing namespace; the word is dropped and the
    // rest matched as an order.
    let t = t.trim_start_matches(|c: char| !c.is_alphanumeric());
    let guided = t.starts_with("guidance");
    let t = if let Some(rest) = t.strip_prefix("guidance") { rest.trim_start_matches(|c: char| c == ':' || c == ',' || c.is_whitespace()) } else { t };
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    // Combat targeting is not routing: "target next hostile", "next target",
    // "next subsystem", "target the power plant" go to the cockpit keys.
    if !guided && has(&["hostile", "subsystem", "power plant", "powerplant", "drives", "fsd", "next target", "next ship", "target ahead", "highest threat"]) {
        return None;
    }
    // Signal watch: "(I'm) looking for X", "watch for X", "stop looking for X", "what are we watching for".
    if has(&["what are we watching", "what am i watching", "what are you watching", "watch list"]) {
        return Some(Order::WatchList);
    }
    let stop = has(&["stop looking", "stop watching", "no longer looking", "forget about"]);
    if stop || has(&["looking for", "look for", "watch for", "keep an eye out", "keep an eye on", "let me know if you see", "tell me if you see", "tell me when you see"]) {
        if let Some((id, _, _)) = crate::callouts::signal_by_words(t) {
            return Some(Order::Watch(id, !stop));
        }
    }
    if has(&["re-plan", "replan", "re plan", "plan again", "recalculate"]) {
        return Some(Order::Replan);
    }
    // Route targeting: needs "guidance" or a route word, so "target next" alone in a fight is not a route order.
    let route_word = has(&["system", "star", "waypoint", "route", "jump"]);
    if (guided || route_word) && has(&["target next", "target the next", "next waypoint", "target waypoint", "plot next", "next system", "next star"]) {
        Some(Order::TargetNext)
    } else if has(&["skip", "skip this", "skip that"]) {
        Some(Order::Skip)
    } else if has(&["stop following", "stop the route"])
        // "clear/cancel my route", "clear the route", "clear route": any
        // clear/cancel verb next to a route word is the deterministic stop
        // (field case 2026-09-05 — "clear my route" fell through to the
        // model, whose tool cleared state but not, back then, the HUD).
        || (route_word && has(&["clear", "cancel"]))
    {
        Some(Order::StopFollowing)
    } else if has(&["jumps left", "how many jumps", "how far to go", "how far left", "how far is it", "how much further", "how much farther", "remaining jumps"]) {
        Some(Order::JumpsLeft)
    } else if has(&["next system", "what's next", "whats next", "where next"]) {
        Some(Order::NextSystem)
    } else if has(&["repeat", "say again", "say that again"]) {
        Some(Order::Repeat)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing is bound until the commander provides a control: a fresh
    /// config must not invent a push-to-talk hotkey (a fabricated
    /// "Ctrl+Alt+Space" showed up in Settings as if the user had set it).
    #[test]
    fn a_fresh_config_has_no_push_to_talk_until_the_commander_sets_one() {
        let config = ListenConfig::default();
        assert_eq!(config.ptt_source(), PttSource::None);
        assert!(config.ptt_hotkey.is_empty(), "no invented hotkey text: {:?}", config.ptt_hotkey);
    }

    #[test]
    fn direct_orders_are_recognised_and_questions_fall_through() {
        assert!(matches!(direct_order("edda target next system"), Some(Order::TargetNext)));
        assert!(matches!(direct_order("guidance: target next system in route"), Some(Order::TargetNext)));
        assert!(matches!(direct_order("target next star"), Some(Order::TargetNext)));
        assert!(direct_order("target next hostile").is_none());
        assert!(direct_order("target the power plant").is_none());
        assert!(direct_order("target next").is_none(), "bare 'target next' in a fight is not a route order");
        assert!(matches!(direct_order("guidance: target next"), Some(Order::TargetNext)));
        assert!(matches!(direct_order("guidance, how many jumps left"), Some(Order::JumpsLeft)));
        assert!(matches!(direct_order("how many jumps left"), Some(Order::JumpsLeft)));
        assert!(matches!(direct_order("stop following the route"), Some(Order::StopFollowing)));
        assert!(direct_order("what's the best trade from here").is_none());
        assert!(direct_order("how far away from elite combat rank am i").is_none());
        assert!(matches!(direct_order("how far to go"), Some(Order::JumpsLeft)));
        assert!(matches!(direct_order("i'm looking for high grade emissions"), Some(Order::Watch("hge", true))));
        assert!(matches!(direct_order("stop looking for pirates"), Some(Order::Watch("pirates", false))));
        assert!(matches!(direct_order("tell me when you see a power convoy distress signal"), Some(Order::Watch("power_convoy", true))));
        assert!(matches!(direct_order("what are we watching for"), Some(Order::WatchList)));
    }
}

#[cfg(test)]
mod listen_state_tests {
    use super::ListenState;

    /// A restart bumps the generation: the old loop sees it is retired on
    /// its next chunk, and only the newest loop may report "off" when it
    /// exits -- an old loop finishing late must not switch off the new one.
    #[test]
    fn generation_bump_restarts_the_listener_without_two_loops() {
        let mut l = ListenState::default();
        let first = l.begin_loop();
        assert!(l.run && l.is_current(first));
        l.phase = 1;

        let second = l.begin_loop();
        assert_ne!(first, second);
        assert!(!l.is_current(first), "the old loop must stop on its next chunk");
        assert!(l.is_current(second));

        l.loop_ended(first);
        assert!(l.run, "an old loop exiting late must not report the listener off");
        assert_eq!(l.phase, 1);

        l.loop_ended(second);
        assert!(!l.run);
        assert_eq!(l.phase, 0);
    }

    #[test]
    fn stop_retires_the_current_loop() {
        let mut l = ListenState::default();
        let gen = l.begin_loop();
        l.stop();
        assert!(!l.run);
        assert!(!l.is_current(gen));
        // Two states are two listeners: no shared globals.
        let other = ListenState::default();
        assert!(!other.run && other.gen == 0);
    }
}
