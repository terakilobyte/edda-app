//! Everything the app knows about the host operating system, in one place.
//!
//! The rest of `src-tauri` asks this module for locations and names and
//! never spells `LOCALAPPDATA`, `.exe` or `.dll` itself. Every `cfg` that
//! chooses between hosts lives here, so adding a platform is editing one
//! file, and the choice is visible in one read.
//!
//! Three adapters (Windows, Linux, macOS) sit behind each function; where
//! a host has no answer (no journal on macOS, the game does not run there)
//! the function says so with `None`/empty rather than a plausible-looking
//! path.

use std::path::{Path, PathBuf};

/// The app's directory name under the host's per-user data root.
pub const APP_DIR: &str = env!("CARGO_PKG_NAME");

/// "windows", "linux" or "macos": for status displays and log lines.
pub fn name() -> &'static str {
    std::env::consts::OS
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// The per-user data directory, from the pieces of the environment each
/// host consults. Pure, so every host can test its own rule.
///
/// * Windows: `%LOCALAPPDATA%\edda`
/// * macOS: `~/Library/Application Support/edda`
/// * Linux: `$XDG_DATA_HOME/edda`, else `~/.local/share/edda`
pub fn data_dir_under(home: &Path, local_app_data: Option<&Path>, xdg_data_home: Option<&Path>) -> PathBuf {
    if cfg!(windows) {
        local_app_data.map(Path::to_path_buf).unwrap_or_else(|| home.join("AppData/Local")).join(APP_DIR)
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support").join(APP_DIR)
    } else {
        xdg_data_home.map(Path::to_path_buf).unwrap_or_else(|| home.join(".local/share")).join(APP_DIR)
    }
}

/// Where an installed build keeps its data unless a pointer file says
/// otherwise. Falls back to the working directory when the environment
/// gives no home at all (a bare container); never panics.
pub fn default_data_dir() -> PathBuf {
    let Some(home) = home() else { return PathBuf::from(".").join(APP_DIR) };
    data_dir_under(
        &home,
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).as_deref(),
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).as_deref(),
    )
}

/// The parent of [`default_data_dir`]: where the folder picker starts.
pub fn default_data_parent() -> PathBuf {
    default_data_dir().parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}

/// The file that remembers a custom data location. It lives in the
/// default data dir, the one place the app can always find.
pub fn pointer_file() -> PathBuf {
    default_data_dir().join("data-location.txt")
}

/// Executable file name for a sidecar (`piper` -> `piper.exe` on Windows).
pub fn exe(stem: &str) -> String {
    if cfg!(windows) { format!("{stem}.exe") } else { stem.to_string() }
}

/// Dynamic library file name for a stem: `onnxruntime.dll`,
/// `libonnxruntime.dylib`, `libonnxruntime.so`.
pub fn dylib(stem: &str) -> String {
    if cfg!(windows) {
        // Windows builds drop the `lib` prefix as a rule (onnxruntime.dll),
        // but Vosk ships its Windows library as libvosk.dll, the name
        // ed-listen loads.
        if stem == "vosk" {
            return format!("lib{stem}.dll");
        }
        format!("{stem}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else {
        format!("lib{stem}.so")
    }
}

/// Whether a Piper sidecar folder is complete for this host.
pub fn piper_installed(dir: &Path) -> bool {
    dir.join(exe("piper")).is_file() && dir.join(dylib("onnxruntime")).is_file() && dir.join("espeak-ng-data").is_dir()
}

/// Piper release archive for this host, if one is published.
pub fn piper_download_url() -> Option<String> {
    const BASE: &str = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2";
    let file = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "piper_windows_amd64.zip",
        ("linux", "x86_64") => "piper_linux_x86_64.tar.gz",
        ("linux", "aarch64") => "piper_linux_aarch64.tar.gz",
        ("macos", "x86_64") => "piper_macos_x64.tar.gz",
        ("macos", "aarch64") => "piper_macos_aarch64.tar.gz",
        _ => return None,
    };
    Some(format!("{BASE}/{file}"))
}

/// `uv` release archive for this host, if one is published.
pub fn uv_download_url(version: &str) -> Option<String> {
    const BASE: &str = "https://github.com/astral-sh/uv/releases/download";
    let file = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "uv-x86_64-pc-windows-msvc.zip",
        ("linux", "x86_64") => "uv-x86_64-unknown-linux-gnu.tar.gz",
        ("linux", "aarch64") => "uv-aarch64-unknown-linux-gnu.tar.gz",
        ("macos", "x86_64") => "uv-x86_64-apple-darwin.tar.gz",
        ("macos", "aarch64") => "uv-aarch64-apple-darwin.tar.gz",
        _ => return None,
    };
    Some(format!("{BASE}/{version}/{file}"))
}

/// Process names the game client runs under. Proton keeps the Windows
/// binary's name; on macOS there is no client, so nothing to look for.
pub fn game_process_names() -> &'static [&'static str] {
    if cfg!(target_os = "macos") {
        &[]
    } else {
        &["EliteDangerous64.exe", "EliteDangerous64", "EliteDangerous32.exe"]
    }
}

/// Steam's app id for Elite Dangerous: the Proton prefix is named after it.
const STEAM_APP_ID: &str = "359320";

/// Journal folders worth trying under `home` on this host, likeliest
/// first. Windows asks the shell for Saved Games instead (ed-journal).
pub fn journal_dir_candidates_under(home: &Path) -> Vec<PathBuf> {
    const FRONTIER: &str = "Saved Games/Frontier Developments/Elite Dangerous";
    if cfg!(target_os = "linux") {
        let pfx = format!("steamapps/compatdata/{STEAM_APP_ID}/pfx/drive_c/users/steamuser/{FRONTIER}");
        vec![
            home.join(".steam/steam").join(&pfx),
            home.join(".local/share/Steam").join(&pfx),
            home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam").join(&pfx),
        ]
    } else {
        Vec::new()
    }
}

/// Every journal folder worth trying on this host: ed-journal's (the
/// Windows known folder, the dev-sandbox mount) plus the host's own.
pub fn journal_dir_candidates() -> Vec<PathBuf> {
    let mut out = ed_journal::journal::candidate_journal_dirs();
    if let Some(h) = home() {
        out.extend(journal_dir_candidates_under(&h));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_data_dir_follows_the_host_convention() {
        let home = Path::new("/home/cmdr");
        let got = data_dir_under(home, Some(Path::new("C:/Users/cmdr/AppData/Local")), Some(Path::new("/home/cmdr/xdg")));
        let want: PathBuf = if cfg!(windows) {
            PathBuf::from("C:/Users/cmdr/AppData/Local").join("edda")
        } else if cfg!(target_os = "macos") {
            home.join("Library/Application Support/edda")
        } else {
            PathBuf::from("/home/cmdr/xdg/edda")
        };
        assert_eq!(got, want);
        // Linux without XDG_DATA_HOME falls back to ~/.local/share.
        if cfg!(target_os = "linux") {
            assert_eq!(data_dir_under(home, None, None), home.join(".local/share/edda"));
        }
    }

    #[test]
    fn sidecar_and_library_names_match_the_host() {
        if cfg!(windows) {
            assert_eq!(exe("piper"), "piper.exe");
            assert_eq!(exe("uv"), "uv.exe");
            assert_eq!(dylib("onnxruntime"), "onnxruntime.dll");
            assert_eq!(dylib("vosk"), "libvosk.dll");
        } else if cfg!(target_os = "macos") {
            assert_eq!(exe("piper"), "piper");
            assert_eq!(dylib("onnxruntime"), "libonnxruntime.dylib");
            assert_eq!(dylib("vosk"), "libvosk.dylib");
        } else {
            assert_eq!(exe("uv"), "uv");
            assert_eq!(dylib("onnxruntime"), "libonnxruntime.so");
            assert_eq!(dylib("vosk"), "libvosk.so");
        }
    }

    #[test]
    fn piper_is_installed_when_the_host_named_files_exist() {
        let dir = tempfile::tempdir().unwrap();
        let piper = dir.path().join("piper");
        std::fs::create_dir_all(piper.join("espeak-ng-data")).unwrap();
        assert!(!piper_installed(&piper));
        std::fs::write(piper.join(exe("piper")), b"").unwrap();
        std::fs::write(piper.join(dylib("onnxruntime")), b"").unwrap();
        assert!(piper_installed(&piper));
    }

    #[test]
    fn game_process_names_cover_proton_on_linux_and_nothing_on_macos() {
        let names = game_process_names();
        if cfg!(target_os = "macos") {
            assert!(names.is_empty(), "the game does not run on macOS: {names:?}");
        } else {
            // Under Proton the Windows binary keeps its .exe name.
            assert!(names.contains(&"EliteDangerous64.exe"), "{names:?}");
        }
    }

    #[test]
    fn journal_dir_candidates_include_the_proton_prefix_on_linux() {
        let home = Path::new("/home/cmdr");
        let c = journal_dir_candidates_under(home);
        if cfg!(target_os = "linux") {
            assert!(c.iter().any(|p| p.ends_with("compatdata/359320/pfx/drive_c/users/steamuser/Saved Games/Frontier Developments/Elite Dangerous")), "{c:?}");
        } else {
            assert!(c.is_empty(), "{c:?}");
        }
    }

    #[test]
    fn download_urls_exist_for_every_desktop() {
        assert!(piper_download_url().is_some());
        assert!(uv_download_url("0.11.33").is_some_and(|u| u.contains("0.11.33")));
    }
}
