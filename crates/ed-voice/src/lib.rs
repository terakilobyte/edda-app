//! Local text-to-speech for the ship computer.
//!
//! Decision 4 in `docs/PLAN.md` chose local neural TTS. Building it
//! in-process turned out to need LLVM (`piper-rs` and `kokoro-en` both go
//! through `espeak-rs-sys` → bindgen → libclang), so this uses the route the
//! module docs recommended: Piper's standalone Windows binary as a
//! **sidecar**, staged under `.data/voices/piper/`. Text goes in on stdin, a
//! WAV comes out, and `rodio` plays it. No toolchain, same neural voice.
//!
//! When the sidecar is missing the module degrades rather than failing:
//! Windows' built-in SAPI synthesizer (via PowerShell) speaks instead, and
//! if even that is unavailable, `speak` logs and returns. Nothing about
//! voice may ever take the app down.
//!
//! The `backend()` accessor exists so the UI can *say* which voice is live
//! instead of letting a silent fallback masquerade as the chosen design.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

/// Which engine will actually produce audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// A speech server speaking the OpenAI audio API (Kokoro, OpenAI, and compatible servers).
    Server,
    /// Piper neural TTS via the staged sidecar binary.
    Piper,
    /// Windows Speech API through PowerShell. Robotic, zero dependencies.
    Sapi,
    /// Nothing available; `speak` is a no-op that logs.
    Silent,
}

#[derive(Debug, Clone)]
pub struct Voice {
    piper: Option<PathBuf>,
    model: Option<PathBuf>,
    espeak_data: Option<PathBuf>,
    out_dir: PathBuf,
    /// Speech server, output device, interrupt generation: shared with
    /// whoever owns this voice, never process-global.
    audio: Arc<Audio>,
}

/// The audio settings a voice speaks through, and the barge-in counter.
///
/// Two voices with two `Audio` values answer `backend()` differently for
/// the same files on disk -- which is the point: a speech server is a
/// property of the app's voice, not of the process.
#[derive(Debug)]
pub struct Audio {
    server: std::sync::Mutex<Option<ServerConfig>>,
    output_device: std::sync::Mutex<Option<String>>,
    /// Bumped by `interrupt()`; playback that started under an older value stops.
    interrupt: std::sync::atomic::AtomicU64,
    http: reqwest::blocking::Client,
}

impl Default for Audio {
    fn default() -> Self {
        Audio::new(concat!("ed-voice/", env!("CARGO_PKG_VERSION")))
    }
}

impl Audio {
    /// `user_agent` is the caller's: the app owns its HTTP identity.
    pub fn new(user_agent: &str) -> Self {
        Audio {
            server: std::sync::Mutex::new(None),
            output_device: std::sync::Mutex::new(None),
            interrupt: std::sync::atomic::AtomicU64::new(0),
            http: reqwest::blocking::Client::builder()
                .user_agent(user_agent)
                .build()
                .unwrap_or_default(),
        }
    }

    /// Route speech through a server (None = back to the local voice).
    pub fn set_server(&self, cfg: Option<ServerConfig>) {
        *self.server.lock().unwrap_or_else(|e| e.into_inner()) =
            cfg.filter(|c| !c.url.trim().is_empty());
    }

    pub fn server_config(&self) -> Option<ServerConfig> {
        self.server
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Chosen output device name (None = system default). Set from Settings.
    pub fn set_output_device(&self, name: Option<String>) {
        *self.output_device.lock().unwrap_or_else(|e| e.into_inner()) =
            name.filter(|n| !n.trim().is_empty());
    }

    pub fn output_device(&self) -> Option<String> {
        self.output_device
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Stop whatever is being said right now, and retire every line queued
    /// under the previous generation. Returns the new generation.
    pub fn interrupt(&self) -> u64 {
        self.interrupt
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1
    }

    /// The current interrupt generation; a line reads it when queued or
    /// before synthesis and is dropped if it has moved on.
    pub fn generation(&self) -> u64 {
        self.interrupt.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Voice names the server offers (`GET /v1/audio/voices`, as Kokoro-FastAPI
    /// serves it); empty when the server has no such listing.
    pub fn server_voices(&self, cfg: &ServerConfig) -> Vec<String> {
        server_voices_with(&self.http, cfg)
    }

    /// Is anything listening at the server's root? For a server without a
    /// voice listing.
    pub fn server_reachable(&self, cfg: &ServerConfig) -> Result<()> {
        self.http
            .get(format!("{}/", cfg.url.trim_end_matches('/')))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .map(|_| ())
            .with_context(|| format!("no speech server at {}", cfg.url))
    }
}

/// Preferred voices, first found wins. Lessac is the clearer of the two
/// staged models; Alan is the British one.
const PREFERRED_MODELS: &[&str] = &["en_US-lessac-high.onnx", "en_GB-alan-medium.onnx"];

/// Where the Settings choice is remembered, inside the voices directory.
const SELECTED_FILE: &str = "selected.txt";

/// Model files available under `data_dir/voices/`, by file name, sorted.
pub fn list_models(data_dir: &Path) -> Vec<String> {
    let voices = data_dir.join("voices");
    let mut out: Vec<String> = std::fs::read_dir(&voices)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    // A model is usable only with its .onnx.json beside it.
                    (name.ends_with(".onnx") && voices.join(format!("{name}.json")).is_file())
                        .then_some(name)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

impl Voice {
    /// Switch to another model file under the voices directory and remember
    /// the choice for next launch. Cached WAVs are keyed by text only, so
    /// the cache is cleared or the old voice would keep answering.
    pub fn set_model(&mut self, file_name: &str) -> Result<()> {
        let voices = self
            .out_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let path = voices.join(file_name);
        if !path.is_file() || !voices.join(format!("{file_name}.json")).is_file() {
            bail!(
                "no voice model named {file_name} (with its .json) in {}",
                voices.display()
            );
        }
        self.model = Some(path);
        let _ = std::fs::write(voices.join(SELECTED_FILE), file_name);
        let _ = std::fs::remove_dir_all(&self.out_dir);
        tracing::info!(model = file_name, "voice model changed");
        Ok(())
    }

    /// Explicitly use the zero-download Windows voice even when Piper models
    /// are installed. The sentinel is remembered across launches.
    pub fn use_windows_voice(&mut self) {
        let voices = self
            .out_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        self.model = None;
        let _ = std::fs::write(voices.join(SELECTED_FILE), "windows");
        let _ = std::fs::remove_dir_all(&self.out_dir);
    }
}

impl Voice {
    /// Locate the sidecar and a model under `data_dir/voices/`.
    ///
    /// Missing pieces are not errors here -- `backend()` reports what was
    /// found, and `speak` falls back accordingly.
    pub fn discover(data_dir: &Path) -> Voice {
        Voice::discover_with(data_dir, Arc::new(Audio::default()))
    }

    /// Discover, speaking through `audio` -- the app's settings and
    /// interrupt counter, shared across re-discoveries.
    pub fn discover_with(data_dir: &Path, audio: Arc<Audio>) -> Voice {
        let voices = data_dir.join("voices");
        let piper_dir = voices.join("piper");
        let exe = piper_dir.join(if cfg!(windows) { "piper.exe" } else { "piper" });
        let piper = exe.is_file().then_some(exe);

        // Precedence: explicit env override, then the voice chosen in
        // Settings last time, then the first preferred model present.
        let model = std::env::var_os("EDDA_VOICE")
            .map(PathBuf::from)
            .filter(|p| p.is_file())
            .or_else(|| {
                std::fs::read_to_string(voices.join(SELECTED_FILE))
                    .ok()
                    .filter(|s| s.trim() != "windows")
                    .map(|s| voices.join(s.trim()))
                    .filter(|p| p.is_file())
            })
            .or_else(|| {
                PREFERRED_MODELS
                    .iter()
                    .map(|m| voices.join(m))
                    .find(|p| p.is_file())
            });

        let espeak = piper_dir.join("espeak-ng-data");
        let espeak_data = espeak.is_dir().then_some(espeak);

        let out_dir = data_dir.join("voices").join("cache");

        let v = Voice {
            piper,
            model,
            espeak_data,
            out_dir,
            audio,
        };
        tracing::info!(backend = ?v.backend(), model = ?v.model, "voice discovered");
        v
    }

    pub fn backend(&self) -> Backend {
        if self.audio.server_config().is_some() {
            return Backend::Server;
        }
        self.local_backend()
    }

    /// The engine on this machine, ignoring any speech server.
    fn local_backend(&self) -> Backend {
        match (&self.piper, &self.model) {
            (Some(_), Some(_)) => Backend::Piper,
            _ if cfg!(windows) => Backend::Sapi,
            _ => Backend::Silent,
        }
    }

    pub fn model_name(&self) -> Option<String> {
        self.model
            .as_ref()
            .and_then(|m| m.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
    }

    /// Synthesize `text` with the speech server to a cached WAV file.
    pub fn synth_server_wav(&self, text: &str, cfg: &ServerConfig) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.out_dir)?;
        let out = self.out_dir.join(format!(
            "srv-{}.wav",
            stable_hash(&format!("{}|{}|{}|{text}", cfg.url, cfg.model, cfg.voice))
        ));
        if out.is_file() {
            return Ok(out);
        }
        let bytes = server_speech(&self.audio.http, cfg, text)?;
        std::fs::write(&out, bytes)?;
        Ok(out)
    }

    /// Synthesize `text` to a WAV file with Piper and return its path.
    pub fn synth_wav(&self, text: &str) -> Result<PathBuf> {
        let (Some(piper), Some(model)) = (&self.piper, &self.model) else {
            bail!("piper sidecar or voice model not available");
        };
        std::fs::create_dir_all(&self.out_dir)?;
        let out = self.out_dir.join(format!("{}.wav", stable_hash(text)));
        if out.is_file() {
            return Ok(out);
        }

        let mut cmd = Command::new(piper);
        cmd.arg("--model")
            .arg(model)
            .arg("--output_file")
            .arg(&out)
            .arg("--quiet")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(data) = &self.espeak_data {
            cmd.arg("--espeak_data").arg(data);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: a console flashing up mid-flight is worse
            // than no voice at all.
            cmd.creation_flags(0x0800_0000);
        }

        let mut child = cmd.spawn().context("spawning piper sidecar")?;
        {
            let mut stdin = child.stdin.take().context("piper stdin")?;
            // One line = one utterance for Piper.
            writeln!(stdin, "{}", text.replace(['\r', '\n'], " "))?;
        }
        let output = child.wait_with_output().context("waiting for piper")?;
        if !output.status.success() || !out.is_file() {
            bail!(
                "piper failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(out)
    }

    /// Say `text`, blocking until playback ends. Never panics or propagates
    /// audio failures beyond a log line -- callers run this on a background
    /// thread and care only that the app keeps working.
    pub fn speak(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        // Read once, before synthesis: an interrupt at any point after this
        // cancels the line.
        let gen = self.audio.generation();
        let audio = &*self.audio;
        let result = match self.backend() {
            Backend::Server => {
                let cfg = self.audio.server_config().unwrap_or_default();
                self.synth_server_wav(text, &cfg)
                    .and_then(|wav| play_wav_from(audio, &wav, gen))
                    .or_else(|e| {
                        tracing::warn!(error = %e, "speech server failed; using the local voice");
                        match self.local_backend() {
                            Backend::Piper => self
                                .synth_wav(text)
                                .and_then(|wav| play_wav_from(audio, &wav, gen)),
                            _ => speak_sapi(text),
                        }
                    })
            }
            Backend::Piper => self
                .synth_wav(text)
                .and_then(|wav| play_wav_from(audio, &wav, gen))
                .or_else(|e| {
                    tracing::warn!(error = %e, "piper failed; falling back to SAPI");
                    speak_sapi(text)
                }),
            Backend::Sapi => speak_sapi(text),
            Backend::Silent => {
                tracing::info!(text, "no voice backend; would have said");
                Ok(())
            }
        };
        if let Err(e) = result {
            tracing::error!(error = %e, text, "voice failed");
        }
    }
}

/// A speech server speaking the OpenAI audio API: `POST {url}/v1/audio/speech`
/// with `{model, input, voice, response_format: "wav"}`. Kokoro-FastAPI,
/// Kokoro, OpenAI, and most local TTS servers speak it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServerConfig {
    pub url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub voice: String,
    #[serde(default)]
    pub api_key: Option<String>,
}

impl Voice {
    /// The audio settings this voice speaks through.
    pub fn audio(&self) -> &Arc<Audio> {
        &self.audio
    }
}

/// Deadline for one synthesis request. CPU synthesis time tracks text
/// length loosely but with wild variance (field case 2026-09-05: a
/// 92-char line took Kokoro 60.87 s on a contended CPU against the old
/// flat 60 s timeout — lost by 0.6 s, so SAPI spoke the fallback a
/// minute after the event while the finished server audio died on a
/// closed socket). The budget scales with the text and keeps a generous
/// floor; the reqwest builder timeout covers connect + send + the whole
/// body read, which for a streaming server IS the synthesis.
fn server_speech_budget(text: &str) -> std::time::Duration {
    std::time::Duration::from_secs(120 + text.len() as u64 / 2)
}

fn server_speech(
    client: &reqwest::blocking::Client,
    cfg: &ServerConfig,
    text: &str,
) -> Result<Vec<u8>> {
    let url = format!("{}/v1/audio/speech", cfg.url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": if cfg.model.trim().is_empty() { "kokoro" } else { cfg.model.trim() },
        "input": text,
        "voice": if cfg.voice.trim().is_empty() { "af_heart" } else { cfg.voice.trim() },
        "response_format": "wav",
    });
    let budget = server_speech_budget(text);
    let mut req = client.post(&url).timeout(budget).json(&body);
    if let Some(k) = cfg.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        req = req.bearer_auth(k.trim());
    }
    let started = std::time::Instant::now();
    let resp = req.send().with_context(|| format!("speech server {url}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        bail!(
            "speech server returned {status}: {}",
            text.chars().take(300).collect::<String>()
        );
    }
    // The body read is where a slow synthesis actually times out; name
    // everything a field diagnosis needs (the old bare `?` produced an
    // unactionable "error decoding response body").
    let bytes = resp
        .bytes()
        .with_context(|| {
            format!(
                "speech server {url}: body after {:.1}s of {:.0}s budget ({} chars)",
                started.elapsed().as_secs_f64(),
                budget.as_secs_f64(),
                text.len()
            )
        })?
        .to_vec();
    if bytes.len() < 44 {
        bail!("speech server returned no audio");
    }
    Ok(bytes)
}

fn server_voices_with(client: &reqwest::blocking::Client, cfg: &ServerConfig) -> Vec<String> {
    let url = format!("{}/v1/audio/voices", cfg.url.trim_end_matches('/'));
    let mut req = client.get(&url).timeout(std::time::Duration::from_secs(60));
    if let Some(k) = cfg.api_key.as_deref().filter(|k| !k.trim().is_empty()) {
        req = req.bearer_auth(k.trim());
    }
    let Ok(resp) = req.send() else {
        return Vec::new();
    };
    let Ok(v) = resp.json::<serde_json::Value>() else {
        return Vec::new();
    };
    let list = v
        .get("voices")
        .and_then(|x| x.as_array())
        .or_else(|| v.as_array())
        .cloned()
        .unwrap_or_default();
    list.iter()
        .filter_map(|x| {
            x.as_str().map(str::to_string).or_else(|| {
                x.get("id")
                    .or_else(|| x.get("name"))
                    .and_then(|n| n.as_str())
                    .map(str::to_string)
            })
        })
        .collect()
}

/// Names of the output devices cpal can see.
pub fn list_outputs() -> Vec<String> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    rodio::cpal::default_host()
        .output_devices()
        .map(|d| {
            d.filter_map(|d| d.description().ok().map(|d| d.name().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

fn open_sink(audio: &Audio) -> Result<rodio::MixerDeviceSink> {
    use rodio::cpal::traits::{DeviceTrait, HostTrait};
    let wanted = audio.output_device();
    if let Some(name) = wanted {
        let dev = rodio::cpal::default_host()
            .output_devices()
            .ok()
            .and_then(|mut d| d.find(|d| d.description().ok().is_some_and(|d| d.name() == name)));
        if let Some(dev) = dev {
            if let Ok(sink) =
                rodio::DeviceSinkBuilder::from_device(dev).and_then(|b| b.open_sink_or_fallback())
            {
                return Ok(sink);
            }
            tracing::warn!(device = %name, "could not open the chosen output device; using the default");
        } else {
            tracing::warn!(device = %name, "chosen output device not found; using the default");
        }
    }
    rodio::DeviceSinkBuilder::open_default_sink().context("opening audio output")
}

/// Play unless an interrupt has happened since `gen` was read -- so a
/// barge-in during synthesis (a second or more with a speech server)
/// cancels the line instead of being missed.
fn play_wav_from(audio: &Audio, path: &Path, gen: u64) -> Result<()> {
    if audio.generation() != gen {
        return Ok(());
    }
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut sink = open_sink(audio)?;
    // The sink is dropped after every line by design; rodio's reminder
    // about that is noise here.
    sink.log_on_drop(false);
    // `play` takes the raw reader and decodes internally.
    let player =
        rodio::play(sink.mixer(), std::io::BufReader::new(file)).context("starting playback")?;
    // Poll rather than block, so a barge-in can cut the line short.
    while !player.empty() {
        if audio.generation() != gen {
            player.stop();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    Ok(())
}

/// Windows Speech API through PowerShell -- present on every Windows install.
fn speak_sapi(text: &str) -> Result<()> {
    if !cfg!(windows) {
        bail!("SAPI is Windows-only");
    }
    // Single-quoted PowerShell string: only the quote itself needs escaping.
    let escaped = text.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName System.Speech; \
         $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
         $s.Speak('{escaped}')"
    );
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let output = cmd.output().context("running powershell for SAPI")?;
    if !output.status.success() {
        bail!(
            "SAPI failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// Content-addressed cache key so a repeated callout ("fuel low") is
/// synthesized once.
fn stable_hash(text: &str) -> u64 {
    // FNV-1a: tiny, dependency-free, and collisions only cost a wrong
    // cached phrase -- acceptable for a cache, never for anything else.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// What the ship computer says on connecting to a live session.
///
/// Pure and testable so the greeting is settled independently of how it
/// gets spoken.
pub fn greeting(commander: Option<&str>, ship: Option<&str>, system: Option<&str>) -> String {
    // "Commander" is the address, not a fallback name -- appending an unknown
    // name to it produces "Commander Commander".
    let address = match commander {
        Some(name) if !name.trim().is_empty() => format!("Commander {}", name.trim()),
        _ => "Commander".to_string(),
    };
    match (ship, system) {
        (Some(ship), Some(system)) => {
            format!("Welcome back, {address}. {ship} systems online. We're in {system}.")
        }
        (Some(ship), None) => format!("Welcome back, {address}. {ship} systems online."),
        (None, Some(system)) => format!("Welcome back, {address}. We're in {system}."),
        (None, None) => format!("Welcome back, {address}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_uses_what_it_knows_and_no_more() {
        assert_eq!(
            greeting(Some("Jameson"), Some("Kestrel Mk II"), Some("Wongi")),
            "Welcome back, Commander Jameson. Kestrel Mk II systems online. We're in Wongi."
        );
        assert_eq!(
            greeting(Some("Jameson"), None, None),
            "Welcome back, Commander Jameson."
        );
    }

    #[test]
    fn an_unknown_commander_is_still_addressed_properly() {
        assert_eq!(greeting(None, None, None), "Welcome back, Commander.");
        assert!(!greeting(None, None, None).contains("None"));
    }

    #[test]
    fn discovery_without_assets_degrades_instead_of_failing() {
        let v = Voice::discover(Path::new("/definitely/not/here"));
        assert_ne!(v.backend(), Backend::Piper);
        assert!(v.synth_wav("hello").is_err());
    }

    #[test]
    fn a_speech_server_belongs_to_the_voice_not_the_process() {
        let dir = Path::new("/definitely/not/here");
        let with_server = Voice::discover(dir);
        let without = Voice::discover(dir);
        with_server.audio().set_server(Some(ServerConfig {
            url: "http://127.0.0.1:1".into(),
            ..Default::default()
        }));
        assert_eq!(with_server.backend(), Backend::Server);
        assert_ne!(
            without.backend(),
            Backend::Server,
            "the other voice must not see it"
        );
        with_server.audio().set_server(None);
        assert_ne!(with_server.backend(), Backend::Server);
    }

    #[test]
    fn interrupt_generation_is_one_shared_counter() {
        let audio = Arc::new(Audio::default());
        let a = Voice::discover_with(Path::new("/nope"), audio.clone());
        let b = Voice::discover_with(Path::new("/nope"), audio.clone());
        let queued_at = audio.generation();
        assert_eq!(a.audio().generation(), b.audio().generation());
        let bumped = audio.interrupt();
        assert_eq!(bumped, queued_at + 1);
        assert_eq!(
            a.audio().generation(),
            bumped,
            "every holder sees the same generation"
        );
        assert!(
            queued_at < b.audio().generation(),
            "a line queued before the interrupt is stale"
        );
    }

    #[test]
    fn cache_key_is_stable_and_distinguishes_phrases() {
        assert_eq!(stable_hash("fuel low"), stable_hash("fuel low"));
        assert_ne!(stable_hash("fuel low"), stable_hash("fuel high"));
    }
}
