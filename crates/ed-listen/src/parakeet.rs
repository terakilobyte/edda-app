//! NVIDIA Parakeet (TDT 0.6B v2) through sherpa-onnx's C API, loaded at
//! run time like Vosk. Offline (whole-utterance) decoding: hand it the PCM
//! of one order, get the text back. Far better dictation than Vosk; no
//! grammar mode, so the wake word stays with Vosk.
//!
//! Struct layouts are transcribed from sherpa-onnx v1.13.6 `c-api.h`; the
//! library is pinned to that release, so a newer header never silently
//! changes the ABI under us.

use anyhow::{anyhow, Context, Result};
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_float, c_int, c_void, CStr, CString};
use std::path::Path;
use std::ptr;

/// The sherpa-onnx release whose ABI this module was written against.
pub const SHERPA_VERSION: &str = "1.13.6";

#[repr(C)]
struct FeatureConfig {
    sample_rate: c_int,
    feature_dim: c_int,
}
#[repr(C)]
struct TransducerModelConfig {
    encoder: *const c_char,
    decoder: *const c_char,
    joiner: *const c_char,
}
#[repr(C)]
struct OneModel {
    model: *const c_char,
}
#[repr(C)]
struct WhisperModelConfig {
    encoder: *const c_char,
    decoder: *const c_char,
    language: *const c_char,
    task: *const c_char,
    tail_paddings: c_int,
    enable_token_timestamps: c_int,
    enable_segment_timestamps: c_int,
}
#[repr(C)]
struct CanaryModelConfig {
    encoder: *const c_char,
    decoder: *const c_char,
    src_lang: *const c_char,
    tgt_lang: *const c_char,
    use_pnc: c_int,
}
#[repr(C)]
struct CohereTranscribeModelConfig {
    encoder: *const c_char,
    decoder: *const c_char,
    language: *const c_char,
    use_punct: c_int,
    use_itn: c_int,
}
#[repr(C)]
struct FireRedAsrModelConfig {
    encoder: *const c_char,
    decoder: *const c_char,
}
#[repr(C)]
struct MoonshineModelConfig {
    preprocessor: *const c_char,
    encoder: *const c_char,
    uncached_decoder: *const c_char,
    cached_decoder: *const c_char,
    merged_decoder: *const c_char,
}
#[repr(C)]
struct LmConfig {
    model: *const c_char,
    scale: c_float,
}
#[repr(C)]
struct SenseVoiceModelConfig {
    model: *const c_char,
    language: *const c_char,
    use_itn: c_int,
}
#[repr(C)]
struct FunAsrNanoModelConfig {
    encoder_adaptor: *const c_char,
    llm: *const c_char,
    embedding: *const c_char,
    tokenizer: *const c_char,
    system_prompt: *const c_char,
    user_prompt: *const c_char,
    max_new_tokens: c_int,
    temperature: c_float,
    top_p: c_float,
    seed: c_int,
    language: *const c_char,
    itn: c_int,
    hotwords: *const c_char,
}
#[repr(C)]
struct Qwen3AsrModelConfig {
    conv_frontend: *const c_char,
    encoder: *const c_char,
    decoder: *const c_char,
    tokenizer: *const c_char,
    max_total_len: c_int,
    max_new_tokens: c_int,
    temperature: c_float,
    top_p: c_float,
    seed: c_int,
    hotwords: *const c_char,
}
#[repr(C)]
struct OfflineModelConfig {
    transducer: TransducerModelConfig,
    paraformer: OneModel,
    nemo_ctc: OneModel,
    whisper: WhisperModelConfig,
    tdnn: OneModel,
    tokens: *const c_char,
    num_threads: c_int,
    debug: c_int,
    provider: *const c_char,
    model_type: *const c_char,
    modeling_unit: *const c_char,
    bpe_vocab: *const c_char,
    telespeech_ctc: *const c_char,
    sense_voice: SenseVoiceModelConfig,
    moonshine: MoonshineModelConfig,
    fire_red_asr: FireRedAsrModelConfig,
    dolphin: OneModel,
    zipformer_ctc: OneModel,
    canary: CanaryModelConfig,
    wenet_ctc: OneModel,
    omnilingual: OneModel,
    medasr: OneModel,
    funasr_nano: FunAsrNanoModelConfig,
    fire_red_asr_ctc: OneModel,
    qwen3_asr: Qwen3AsrModelConfig,
    cohere_transcribe: CohereTranscribeModelConfig,
}
#[repr(C)]
struct HomophoneReplacerConfig {
    dict_dir: *const c_char,
    lexicon: *const c_char,
    rule_fsts: *const c_char,
}
#[repr(C)]
struct OfflineRecognizerConfig {
    feat_config: FeatureConfig,
    model_config: OfflineModelConfig,
    lm_config: LmConfig,
    decoding_method: *const c_char,
    max_active_paths: c_int,
    hotwords_file: *const c_char,
    hotwords_score: c_float,
    rule_fsts: *const c_char,
    rule_fars: *const c_char,
    blank_penalty: c_float,
    hr: HomophoneReplacerConfig,
}
#[repr(C)]
struct OfflineRecognizerResult {
    text: *const c_char,
    timestamps: *mut c_float,
    count: c_int,
    tokens: *const c_char,
    tokens_arr: *const *const c_char,
    json: *const c_char,
    lang: *const c_char,
    emotion: *const c_char,
    event: *const c_char,
    durations: *mut c_float,
}

type CreateRec = unsafe extern "C" fn(*const OfflineRecognizerConfig) -> *const c_void;
type DestroyRec = unsafe extern "C" fn(*const c_void);
type CreateStream = unsafe extern "C" fn(*const c_void) -> *const c_void;
type DestroyStream = unsafe extern "C" fn(*const c_void);
type Accept = unsafe extern "C" fn(*const c_void, c_int, *const c_float, c_int);
type Decode = unsafe extern "C" fn(*const c_void, *const c_void);
type GetResult = unsafe extern "C" fn(*const c_void) -> *const OfflineRecognizerResult;
type DestroyResult = unsafe extern "C" fn(*const OfflineRecognizerResult);

/// A loaded Parakeet recognizer. One per app; `transcribe` may be called
/// from any thread, one at a time.
pub struct Parakeet {
    _ort: Option<Library>,
    _lib: Library,
    rec: *const c_void,
    destroy_rec: DestroyRec,
    create_stream: CreateStream,
    destroy_stream: DestroyStream,
    accept: Accept,
    decode: Decode,
    get_result: GetResult,
    destroy_result: DestroyResult,
}

// SAFETY: the recognizer is immutable after creation; sherpa documents it
// as shareable across threads, and each call makes its own stream.
unsafe impl Send for Parakeet {}
unsafe impl Sync for Parakeet {}

/// A model folder: the four files sherpa needs for a NeMo transducer.
pub fn is_model_dir(p: &Path) -> bool {
    model_files(p).is_some()
}

fn model_files(
    p: &Path,
) -> Option<(
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
)> {
    let tokens = p.join("tokens.txt");
    if !tokens.is_file() {
        return None;
    }
    let pick = |stem: &str| {
        for name in [
            format!("{stem}.int8.onnx"),
            format!("{stem}.onnx"),
            format!("{stem}.fp16.onnx"),
        ] {
            let f = p.join(&name);
            if f.is_file() {
                return Some(f);
            }
        }
        None
    };
    Some((pick("encoder")?, pick("decoder")?, pick("joiner")?, tokens))
}

/// The sherpa-onnx C API DLL if present under `lib_dir` (or its `bin`/`lib`).
pub fn find_lib(lib_dir: &Path) -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) {
        "sherpa-onnx-c-api.dll"
    } else {
        "libsherpa-onnx-c-api.so"
    };
    for d in [
        lib_dir.to_path_buf(),
        lib_dir.join("bin"),
        lib_dir.join("lib"),
    ] {
        if d.join(name).is_file() {
            return Some(d);
        }
    }
    // One level of release folder, e.g. sherpa-onnx-v1.13.6-win-x64-shared-.../bin
    for e in std::fs::read_dir(lib_dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            for d in [p.clone(), p.join("bin"), p.join("lib")] {
                if d.join(name).is_file() {
                    return Some(d);
                }
            }
        }
    }
    None
}

impl Parakeet {
    /// `lib_dir` holds `sherpa-onnx-c-api.dll` + `onnxruntime.dll`;
    /// `model_dir` the unpacked `sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8`.
    pub fn load(lib_dir: &Path, model_dir: &Path, threads: usize) -> Result<Parakeet> {
        let (encoder, decoder, joiner, tokens) = model_files(model_dir).ok_or_else(|| {
            anyhow!(
                "{} does not look like a Parakeet model",
                model_dir.display()
            )
        })?;
        let dll = lib_dir.join(if cfg!(windows) {
            "sherpa-onnx-c-api.dll"
        } else {
            "libsherpa-onnx-c-api.so"
        });
        if !dll.is_file() {
            return Err(anyhow!("{} not found", dll.display()));
        }
        // onnxruntime.dll is loaded first by full path so the C API DLL's
        // import of it resolves to this copy regardless of the search path.
        let ort_path = lib_dir.join(if cfg!(windows) {
            "onnxruntime.dll"
        } else {
            "libonnxruntime.so"
        });
        let ort = if ort_path.is_file() {
            Some(unsafe { load_lib(&ort_path)? })
        } else {
            None
        };
        let lib = unsafe { load_lib(&dll)? };

        let c = |s: &str| CString::new(s).unwrap();
        let s_encoder = c(&encoder.to_string_lossy());
        let s_decoder = c(&decoder.to_string_lossy());
        let s_joiner = c(&joiner.to_string_lossy());
        let s_tokens = c(&tokens.to_string_lossy());
        let s_provider = c("cpu");
        let s_type = c("nemo_transducer");
        let s_greedy = c("greedy_search");
        let empty = c("");
        let e = empty.as_ptr();
        let one = || OneModel { model: e };
        let cfg = OfflineRecognizerConfig {
            feat_config: FeatureConfig {
                sample_rate: 16000,
                feature_dim: 80,
            },
            model_config: OfflineModelConfig {
                transducer: TransducerModelConfig {
                    encoder: s_encoder.as_ptr(),
                    decoder: s_decoder.as_ptr(),
                    joiner: s_joiner.as_ptr(),
                },
                paraformer: one(),
                nemo_ctc: one(),
                whisper: WhisperModelConfig {
                    encoder: e,
                    decoder: e,
                    language: e,
                    task: e,
                    tail_paddings: -1,
                    enable_token_timestamps: 0,
                    enable_segment_timestamps: 0,
                },
                tdnn: one(),
                tokens: s_tokens.as_ptr(),
                num_threads: threads.clamp(1, 16) as c_int,
                debug: 0,
                provider: s_provider.as_ptr(),
                model_type: s_type.as_ptr(),
                modeling_unit: e,
                bpe_vocab: e,
                telespeech_ctc: e,
                sense_voice: SenseVoiceModelConfig {
                    model: e,
                    language: e,
                    use_itn: 0,
                },
                moonshine: MoonshineModelConfig {
                    preprocessor: e,
                    encoder: e,
                    uncached_decoder: e,
                    cached_decoder: e,
                    merged_decoder: e,
                },
                fire_red_asr: FireRedAsrModelConfig {
                    encoder: e,
                    decoder: e,
                },
                dolphin: one(),
                zipformer_ctc: one(),
                canary: CanaryModelConfig {
                    encoder: e,
                    decoder: e,
                    src_lang: e,
                    tgt_lang: e,
                    use_pnc: 0,
                },
                wenet_ctc: one(),
                omnilingual: one(),
                medasr: one(),
                funasr_nano: FunAsrNanoModelConfig {
                    encoder_adaptor: e,
                    llm: e,
                    embedding: e,
                    tokenizer: e,
                    system_prompt: e,
                    user_prompt: e,
                    max_new_tokens: 0,
                    temperature: 0.0,
                    top_p: 0.0,
                    seed: 0,
                    language: e,
                    itn: 0,
                    hotwords: e,
                },
                fire_red_asr_ctc: one(),
                qwen3_asr: Qwen3AsrModelConfig {
                    conv_frontend: e,
                    encoder: e,
                    decoder: e,
                    tokenizer: e,
                    max_total_len: 0,
                    max_new_tokens: 0,
                    temperature: 0.0,
                    top_p: 0.0,
                    seed: 0,
                    hotwords: e,
                },
                cohere_transcribe: CohereTranscribeModelConfig {
                    encoder: e,
                    decoder: e,
                    language: e,
                    use_punct: 0,
                    use_itn: 0,
                },
            },
            lm_config: LmConfig {
                model: e,
                scale: 1.0,
            },
            decoding_method: s_greedy.as_ptr(),
            max_active_paths: 4,
            hotwords_file: e,
            hotwords_score: 1.5,
            rule_fsts: e,
            rule_fars: e,
            blank_penalty: 0.0,
            hr: HomophoneReplacerConfig {
                dict_dir: e,
                lexicon: e,
                rule_fsts: e,
            },
        };

        // SAFETY: symbol names and signatures follow c-api.h for the pinned release.
        unsafe {
            let create_rec: Symbol<CreateRec> = lib.get(b"SherpaOnnxCreateOfflineRecognizer\0")?;
            let destroy_rec: Symbol<DestroyRec> =
                lib.get(b"SherpaOnnxDestroyOfflineRecognizer\0")?;
            let create_stream: Symbol<CreateStream> =
                lib.get(b"SherpaOnnxCreateOfflineStream\0")?;
            let destroy_stream: Symbol<DestroyStream> =
                lib.get(b"SherpaOnnxDestroyOfflineStream\0")?;
            let accept: Symbol<Accept> = lib.get(b"SherpaOnnxAcceptWaveformOffline\0")?;
            let decode: Symbol<Decode> = lib.get(b"SherpaOnnxDecodeOfflineStream\0")?;
            let get_result: Symbol<GetResult> = lib.get(b"SherpaOnnxGetOfflineStreamResult\0")?;
            let destroy_result: Symbol<DestroyResult> =
                lib.get(b"SherpaOnnxDestroyOfflineRecognizerResult\0")?;
            let (
                create_rec,
                destroy_rec,
                create_stream,
                destroy_stream,
                accept,
                decode,
                get_result,
                destroy_result,
            ) = (
                *create_rec,
                *destroy_rec,
                *create_stream,
                *destroy_stream,
                *accept,
                *decode,
                *get_result,
                *destroy_result,
            );
            let rec = create_rec(&cfg);
            if rec.is_null() {
                return Err(anyhow!(
                    "sherpa-onnx could not load the Parakeet model at {}",
                    model_dir.display()
                ));
            }
            Ok(Parakeet {
                _ort: ort,
                _lib: lib,
                rec,
                destroy_rec,
                create_stream,
                destroy_stream,
                accept,
                decode,
                get_result,
                destroy_result,
            })
        }
    }

    /// Transcribe one utterance of 16 kHz mono PCM.
    pub fn transcribe(&self, pcm: &[i16]) -> Result<String> {
        if pcm.is_empty() {
            return Ok(String::new());
        }
        let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
        // SAFETY: stream and result are created and destroyed here, in order.
        unsafe {
            let stream = (self.create_stream)(self.rec);
            if stream.is_null() {
                return Err(anyhow!("sherpa-onnx could not create a stream"));
            }
            (self.accept)(stream, 16000, samples.as_ptr(), samples.len() as c_int);
            (self.decode)(self.rec, stream);
            let r = (self.get_result)(stream);
            let text = if r.is_null() || (*r).text.is_null() {
                String::new()
            } else {
                CStr::from_ptr((*r).text)
                    .to_string_lossy()
                    .trim()
                    .to_string()
            };
            if !r.is_null() {
                (self.destroy_result)(r);
            }
            (self.destroy_stream)(stream);
            Ok(text)
        }
    }
}

impl Drop for Parakeet {
    fn drop(&mut self) {
        // SAFETY: created by create_rec; no stream outlives a transcribe call.
        unsafe { (self.destroy_rec)(self.rec) };
        self.rec = ptr::null();
    }
}

unsafe fn load_lib(path: &Path) -> Result<Library> {
    #[cfg(windows)]
    {
        use libloading::os::windows::{
            Library as WinLib, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        };
        Ok(Library::from(
            WinLib::load_with_flags(
                path,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
            .with_context(|| format!("loading {}", path.display()))?,
        ))
    }
    #[cfg(not(windows))]
    {
        Library::new(path).with_context(|| format!("loading {}", path.display()))
    }
}
