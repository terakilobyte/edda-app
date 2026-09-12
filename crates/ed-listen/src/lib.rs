//! Listening: microphone capture and offline speech recognition.
//!
//! `parakeet` adds NVIDIA Parakeet via sherpa-onnx for dictation.
//!
//! Recognition is Vosk (Kaldi under the hood), loaded **at runtime** from
//! `libvosk.dll` with a hand-written binding for its tiny C API, so the
//! build has no dependency on a downloaded library. The small English
//! model (~40 MB) is enough for a wake word and short commands; a grammar
//! restricted to the phrases we expect makes those near-perfect, and a
//! free recognizer handles questions for the ship computer.
//!
//! Audio comes from the default input device via cpal, converted to the
//! 16 kHz mono 16-bit PCM Vosk wants. Nothing is stored or sent anywhere.

pub mod parakeet;

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_float, c_int, c_void, CStr, CString};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};

pub const SAMPLE_RATE: u32 = 16_000;

type ModelNew = unsafe extern "C" fn(*const c_char) -> *mut c_void;
type ModelFree = unsafe extern "C" fn(*mut c_void);
type RecNew = unsafe extern "C" fn(*mut c_void, c_float) -> *mut c_void;
type RecNewGrm = unsafe extern "C" fn(*mut c_void, c_float, *const c_char) -> *mut c_void;
type RecFree = unsafe extern "C" fn(*mut c_void);
type RecAccept = unsafe extern "C" fn(*mut c_void, *const i16, c_int) -> c_int;
type RecStr = unsafe extern "C" fn(*mut c_void) -> *const c_char;
type RecReset = unsafe extern "C" fn(*mut c_void);
type SetLog = unsafe extern "C" fn(c_int);

/// The loaded library plus one model. Recognizers borrow it.
pub struct Engine {
    _lib: Library,
    model: *mut c_void,
    model_new: ModelNew,
    model_free: ModelFree,
    rec_new: RecNew,
    rec_new_grm: RecNewGrm,
    rec_free: RecFree,
    rec_accept: RecAccept,
    rec_result: RecStr,
    rec_partial: RecStr,
    rec_final: RecStr,
    rec_reset: RecReset,
}

// SAFETY: Vosk models are immutable after creation and documented as safe
// to share between recognizers on different threads.
unsafe impl Send for Engine {}
unsafe impl Sync for Engine {}

impl Engine {
    /// `lib_dir` holds `libvosk.dll` (and its runtime DLLs); `model_dir` is
    /// the unpacked model folder.
    pub fn load(lib_dir: &Path, model_dir: &Path) -> Result<Engine> {
        let dll = lib_dir.join(if cfg!(windows) {
            "libvosk.dll"
        } else {
            "libvosk.so"
        });
        if !dll.is_file() {
            return Err(anyhow!("{} not found", dll.display()));
        }
        if !model_dir.join("conf").is_dir() && !model_dir.join("am").is_dir() {
            return Err(anyhow!(
                "{} does not look like a Vosk model",
                model_dir.display()
            ));
        }
        // Sibling DLLs (libstdc++, winpthread) resolve from the DLL's own dir.
        #[cfg(windows)]
        let lib = unsafe {
            use libloading::os::windows::{
                Library as WinLib, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
            };
            Library::from(
                WinLib::load_with_flags(
                    &dll,
                    LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
                )
                .with_context(|| format!("loading {}", dll.display()))?,
            )
        };
        #[cfg(not(windows))]
        let lib =
            unsafe { Library::new(&dll).with_context(|| format!("loading {}", dll.display()))? };

        // SAFETY: symbol names and signatures follow vosk_api.h for 0.3.x.
        unsafe {
            let set_log: Symbol<SetLog> = lib.get(b"vosk_set_log_level\0")?;
            set_log(-1);
            let model_new: Symbol<ModelNew> = lib.get(b"vosk_model_new\0")?;
            let model_free: Symbol<ModelFree> = lib.get(b"vosk_model_free\0")?;
            let rec_new: Symbol<RecNew> = lib.get(b"vosk_recognizer_new\0")?;
            let rec_new_grm: Symbol<RecNewGrm> = lib.get(b"vosk_recognizer_new_grm\0")?;
            let rec_free: Symbol<RecFree> = lib.get(b"vosk_recognizer_free\0")?;
            let rec_accept: Symbol<RecAccept> = lib.get(b"vosk_recognizer_accept_waveform_s\0")?;
            let rec_result: Symbol<RecStr> = lib.get(b"vosk_recognizer_result\0")?;
            let rec_partial: Symbol<RecStr> = lib.get(b"vosk_recognizer_partial_result\0")?;
            let rec_final: Symbol<RecStr> = lib.get(b"vosk_recognizer_final_result\0")?;
            let rec_reset: Symbol<RecReset> = lib.get(b"vosk_recognizer_reset\0")?;
            let (
                model_new,
                model_free,
                rec_new,
                rec_new_grm,
                rec_free,
                rec_accept,
                rec_result,
                rec_partial,
                rec_final,
                rec_reset,
            ) = (
                *model_new,
                *model_free,
                *rec_new,
                *rec_new_grm,
                *rec_free,
                *rec_accept,
                *rec_result,
                *rec_partial,
                *rec_final,
                *rec_reset,
            );
            let path = CString::new(model_dir.to_string_lossy().as_bytes())?;
            let model = model_new(path.as_ptr());
            if model.is_null() {
                return Err(anyhow!(
                    "Vosk could not load the model at {}",
                    model_dir.display()
                ));
            }
            Ok(Engine {
                _lib: lib,
                model,
                model_new,
                model_free,
                rec_new,
                rec_new_grm,
                rec_free,
                rec_accept,
                rec_result,
                rec_partial,
                rec_final,
                rec_reset,
            })
        }
    }

    /// A recognizer over the whole vocabulary.
    pub fn recognizer(&self) -> Result<Recognizer<'_>> {
        // SAFETY: model is valid for the engine's lifetime.
        let rec = unsafe { (self.rec_new)(self.model, SAMPLE_RATE as c_float) };
        if rec.is_null() {
            return Err(anyhow!("could not create a recognizer"));
        }
        Ok(Recognizer { engine: self, rec })
    }

    /// A recognizer limited to `phrases` (plus `[unk]` for anything else),
    /// which is what makes a wake word and short commands reliable.
    pub fn grammar_recognizer(&self, phrases: &[&str]) -> Result<Recognizer<'_>> {
        let mut list: Vec<&str> = phrases.to_vec();
        if !list.contains(&"[unk]") {
            list.push("[unk]");
        }
        let grammar = serde_json::to_string(&list)?;
        let c = CString::new(grammar)?;
        // SAFETY: as above; the grammar string outlives the call.
        let rec = unsafe { (self.rec_new_grm)(self.model, SAMPLE_RATE as c_float, c.as_ptr()) };
        if rec.is_null() {
            return Err(anyhow!("could not create a grammar recognizer"));
        }
        Ok(Recognizer { engine: self, rec })
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // SAFETY: created by model_new; recognizers hold a borrow so are gone.
        unsafe { (self.model_free)(self.model) };
        let _ = self.model_new;
    }
}

pub struct Recognizer<'e> {
    engine: &'e Engine,
    rec: *mut c_void,
}

unsafe impl Send for Recognizer<'_> {}

impl Recognizer<'_> {
    /// Feed 16 kHz mono PCM. Returns true when Vosk saw the end of an
    /// utterance (silence), i.e. `result()` has a complete phrase.
    pub fn accept(&mut self, pcm: &[i16]) -> bool {
        // SAFETY: slice pointer and length are valid for the call.
        unsafe { (self.engine.rec_accept)(self.rec, pcm.as_ptr(), pcm.len() as c_int) == 1 }
    }
    fn text_of(&self, f: RecStr) -> String {
        // SAFETY: Vosk returns a NUL-terminated string owned by the recognizer,
        // valid until the next call; we copy it immediately.
        let s = unsafe { CStr::from_ptr(f(self.rec)) }
            .to_string_lossy()
            .into_owned();
        serde_json::from_str::<serde_json::Value>(&s)
            .ok()
            .and_then(|v| {
                v.get("text")
                    .or_else(|| v.get("partial"))
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }
    pub fn result(&mut self) -> String {
        self.text_of(self.engine.rec_result)
    }
    pub fn partial(&mut self) -> String {
        self.text_of(self.engine.rec_partial)
    }
    pub fn final_result(&mut self) -> String {
        self.text_of(self.engine.rec_final)
    }
    pub fn reset(&mut self) {
        // SAFETY: valid recognizer.
        unsafe { (self.engine.rec_reset)(self.rec) }
    }
}

impl Drop for Recognizer<'_> {
    fn drop(&mut self) {
        // SAFETY: created by rec_new*.
        unsafe { (self.engine.rec_free)(self.rec) }
    }
}

/// The default input device, delivering 16 kHz mono i16 chunks.
pub struct Mic {
    _stream: cpal::Stream,
    pub rx: Receiver<Vec<i16>>,
    pub device: String,
}

/// Names of the input devices cpal can see.
pub fn list_inputs() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|d| d.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

pub fn open_mic() -> Result<Mic> {
    open_mic_named(None)
}

/// Open a specific input device by name, or the default.
pub fn open_mic_named(name: Option<&str>) -> Result<Mic> {
    let host = cpal::default_host();
    let device = match name.filter(|n| !n.trim().is_empty()) {
        Some(n) => host
            .input_devices()
            .ok()
            .and_then(|mut d| d.find(|d| d.name().ok().as_deref() == Some(n)))
            .ok_or_else(|| anyhow!("microphone {n:?} not found"))?,
        None => host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default microphone"))?,
    };
    let name = device.name().unwrap_or_else(|_| "microphone".into());
    let cfg = device.default_input_config().context("microphone config")?;
    let in_rate = cfg.sample_rate().0;
    let channels = cfg.channels() as usize;
    let (tx, rx): (Sender<Vec<i16>>, Receiver<Vec<i16>>) = channel();
    let err = |e| tracing::warn!(error = %e, "microphone stream error");
    let mut acc = Resampler::new(in_rate, channels);
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &cfg.into(),
            move |data: &[f32], _| {
                let out = acc.push_f32(data);
                if !out.is_empty() {
                    let _ = tx.send(out);
                }
            },
            err,
            None,
        )?,
        cpal::SampleFormat::I16 => device.build_input_stream(
            &cfg.into(),
            move |data: &[i16], _| {
                let f: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                let out = acc.push_f32(&f);
                if !out.is_empty() {
                    let _ = tx.send(out);
                }
            },
            err,
            None,
        )?,
        other => return Err(anyhow!("unsupported microphone sample format {other:?}")),
    };
    stream.play().context("starting microphone")?;
    tracing::info!(device = %name, in_rate, channels, "microphone open");
    Ok(Mic {
        _stream: stream,
        rx,
        device: name,
    })
}

/// Channel-average + linear resample to 16 kHz, streaming.
struct Resampler {
    in_rate: u32,
    channels: usize,
    pos: f64,
    last: f32,
}

impl Resampler {
    fn new(in_rate: u32, channels: usize) -> Self {
        Resampler {
            in_rate,
            channels: channels.max(1),
            pos: 0.0,
            last: 0.0,
        }
    }
    fn push_f32(&mut self, data: &[f32]) -> Vec<i16> {
        let frames = data.len() / self.channels;
        let mono: Vec<f32> = (0..frames)
            .map(|i| {
                data[i * self.channels..(i + 1) * self.channels]
                    .iter()
                    .sum::<f32>()
                    / self.channels as f32
            })
            .collect();
        let step = self.in_rate as f64 / SAMPLE_RATE as f64;
        let mut out = Vec::with_capacity((frames as f64 / step) as usize + 1);
        // `pos` is the fractional read position into [last, mono...].
        while self.pos < frames as f64 {
            let i = self.pos.floor();
            let frac = (self.pos - i) as f32;
            let idx = i as isize;
            let a = if idx < 0 {
                self.last
            } else {
                mono[idx as usize]
            };
            let b = if (idx + 1) < frames as isize {
                mono[(idx + 1) as usize]
            } else {
                a
            };
            let s = a + (b - a) * frac;
            out.push((s.clamp(-1.0, 1.0) * 32767.0) as i16);
            self.pos += step;
        }
        self.pos -= frames as f64;
        self.last = *mono.last().unwrap_or(&self.last);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_halves_48k_stereo_to_16k_mono() {
        let mut r = Resampler::new(48_000, 2);
        let data: Vec<f32> = (0..4800)
            .map(|i| if i % 2 == 0 { 0.5 } else { 0.5 })
            .collect();
        let out = r.push_f32(&data);
        assert!((out.len() as i64 - 800).abs() <= 1, "{} samples", out.len());
        assert!(out.iter().all(|&s| (s - 16383).abs() < 3));
    }
}
