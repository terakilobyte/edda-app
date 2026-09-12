//! The voice thread.
//!
//! Synthesis and playback block for a second or two, and the watcher must
//! never wait on them. So callouts are queued to one dedicated thread that
//! owns the [`ed_voice::Voice`], speaks in order, and drops the oldest
//! backlog if the game outruns it -- a warning about a star three jumps ago
//! is worse than silence. Model changes travel down the same queue so they
//! land between utterances, never mid-word.
//!
//! The interrupt generation has one home: the [`ed_voice::Audio`] value
//! shared by this handle and the voice it speaks through. A queued line
//! carries the generation it was queued under and is dropped if a barge-in
//! has moved it on.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

/// Bounded so a RES full of kills cannot build a ten-minute speech backlog.
const QUEUE: usize = 6;

enum Msg {
    /// (text, generation): a line queued before an interrupt is dropped.
    Say(String, u64),
    SayWait(String, u64, std::sync::mpsc::SyncSender<()>),
    SetModel(String),
    UseWindows,
    Reload(std::path::PathBuf),
    Pause(bool),
}

pub struct VoiceHandle {
    tx: Mutex<Option<SyncSender<Msg>>>,
    backend: Mutex<ed_voice::Backend>,
    model: Mutex<Option<String>>,
    muted: std::sync::atomic::AtomicBool,
    /// What was last said, for "repeat".
    last: Mutex<Option<String>>,
    /// Speech server, output device and the interrupt generation, shared
    /// with the voice thread.
    audio: Arc<ed_voice::Audio>,
}

impl VoiceHandle {
    pub fn spawn(data_dir: &std::path::Path, user_agent: &str) -> Self {
        let audio = Arc::new(ed_voice::Audio::new(user_agent));
        let voice = ed_voice::Voice::discover_with(data_dir, audio.clone());
        let backend = voice.backend();
        let model = voice.model_name();
        let (tx, rx) = sync_channel::<Msg>(QUEUE);
        let thread_audio = audio.clone();
        std::thread::Builder::new()
            .name("voice".into())
            .spawn(move || run(voice, rx, thread_audio))
            .expect("spawn voice thread");
        VoiceHandle {
            tx: Mutex::new(Some(tx)),
            backend: Mutex::new(backend),
            model: Mutex::new(model),
            muted: std::sync::atomic::AtomicBool::new(false),
            last: Mutex::new(None),
            audio,
        }
    }

    /// The app's audio settings: speech server, output device, barge-in.
    pub fn audio(&self) -> &Arc<ed_voice::Audio> {
        &self.audio
    }

    fn send(&self, msg: Msg) {
        let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = guard.as_ref() {
            match tx.try_send(msg) {
                Ok(()) => {}
                Err(TrySendError::Full(Msg::Say(t, _))) => {
                    tracing::warn!(text = t, "voice queue full; dropped")
                }
                Err(TrySendError::Full(Msg::SayWait(t, _, done))) => {
                    tracing::warn!(text = t, "voice queue full; dropped");
                    let _ = done.send(());
                }
                Err(TrySendError::Full(_)) => {
                    tracing::warn!("voice queue full; model change dropped")
                }
                Err(TrySendError::Disconnected(_)) => tracing::error!("voice thread is gone"),
            }
        }
    }

    /// Queue a line. Never blocks; a full queue drops this line with a log.
    pub fn say(&self, text: impl Into<String>) {
        let text: String = text.into();
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
        if self.muted.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        self.send(Msg::Say(text, self.audio.generation()));
    }

    /// Speak a setup instruction and return after playback completes.
    pub fn say_wait(&self, text: impl Into<String>) {
        let text = text.into();
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
        if self.muted.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let gen = self.audio.generation();
        let (done_tx, done_rx) = sync_channel(0);
        let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = guard.as_ref() {
            if tx.send(Msg::SayWait(text, gen, done_tx)).is_ok() {
                let _ = done_rx.recv();
            }
        }
    }

    /// Switch model. Takes effect after whatever is currently being said.
    pub fn set_model(&self, file_name: &str) {
        *self.model.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(file_name.trim_end_matches(".onnx").to_string());
        self.send(Msg::SetModel(file_name.to_string()));
    }

    pub fn use_windows(&self) {
        *self.model.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.backend.lock().unwrap_or_else(|e| e.into_inner()) = ed_voice::Backend::Sapi;
        self.send(Msg::UseWindows);
    }

    pub fn model(&self) -> Option<String> {
        self.model.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The engine that will speak: the server when one is configured,
    /// else what discovery found on disk.
    pub fn backend(&self) -> ed_voice::Backend {
        if self.audio.server_config().is_some() {
            return ed_voice::Backend::Server;
        }
        *self.backend.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Re-scan installed sidecars and models without restarting the app.
    pub fn reload(&self, data_dir: &std::path::Path) {
        let discovered = ed_voice::Voice::discover_with(data_dir, self.audio.clone());
        *self.backend.lock().unwrap_or_else(|e| e.into_inner()) = discovered.backend();
        *self.model.lock().unwrap_or_else(|e| e.into_inner()) = discovered.model_name();
        self.send(Msg::Reload(data_dir.to_path_buf()));
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted
            .store(muted, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Hold queued speech while a selected managed engine starts. The voice
    /// worker keeps draining its bounded channel, so callers never block.
    pub fn pause(&self, paused: bool) {
        self.send(Msg::Pause(paused));
    }

    /// Stop speaking now and drop anything queued (barge-in).
    pub fn interrupt(&self) {
        self.audio.interrupt();
    }

    pub fn last_spoken(&self) -> Option<String> {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn run(mut voice: ed_voice::Voice, rx: Receiver<Msg>, audio: Arc<ed_voice::Audio>) {
    let mut paused = false;
    let mut held = std::collections::VecDeque::with_capacity(QUEUE);
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Say(text, gen) => {
                if paused {
                    if held.len() == QUEUE {
                        held.pop_front();
                    }
                    held.push_back((text, gen));
                    continue;
                }
                if gen < audio.generation() {
                    tracing::debug!(text, "dropped: interrupted before it was spoken");
                    continue;
                }
                tracing::debug!(text, "speaking");
                voice.speak(&text);
            }
            Msg::SayWait(text, gen, done) => {
                if gen >= audio.generation() {
                    tracing::debug!(text, "speaking and waiting");
                    voice.speak(&text);
                }
                let _ = done.send(());
            }
            Msg::SetModel(name) => {
                if let Err(e) = voice.set_model(&name) {
                    tracing::warn!(error = %e, "voice model change failed");
                }
            }
            Msg::UseWindows => voice.use_windows_voice(),
            Msg::Reload(data_dir) => {
                voice = ed_voice::Voice::discover_with(&data_dir, audio.clone())
            }
            Msg::Pause(value) => {
                paused = value;
                if !paused {
                    while let Some((text, gen)) = held.pop_front() {
                        if gen >= audio.generation() {
                            tracing::debug!(text, "speaking held startup line");
                            voice.speak(&text);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_has_one_source_of_truth_shared_with_the_voice_thread() {
        let dir = tempfile::tempdir().unwrap();
        let handle = VoiceHandle::spawn(dir.path(), "EDDA/test");
        let before = handle.audio().generation();
        handle.interrupt();
        handle.interrupt();
        // The handle, the audio value the thread holds, and a voice
        // re-discovered through it all read the same counter.
        assert_eq!(handle.audio().generation(), before + 2);
        let rediscovered = ed_voice::Voice::discover_with(dir.path(), handle.audio().clone());
        assert_eq!(rediscovered.audio().generation(), before + 2);
        assert!(Arc::ptr_eq(rediscovered.audio(), handle.audio()));
    }

    #[test]
    fn backend_reports_the_server_configured_on_this_handle() {
        let dir = tempfile::tempdir().unwrap();
        let a = VoiceHandle::spawn(dir.path(), "EDDA/test");
        let b = VoiceHandle::spawn(dir.path(), "EDDA/test");
        a.audio().set_server(Some(ed_voice::ServerConfig {
            url: "http://127.0.0.1:1".into(),
            ..Default::default()
        }));
        assert_eq!(a.backend(), ed_voice::Backend::Server);
        assert_ne!(
            b.backend(),
            ed_voice::Backend::Server,
            "a second handle must not inherit it"
        );
    }
}
