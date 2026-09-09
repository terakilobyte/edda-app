//! Locating the journal folder and the journal files inside it.

use std::path::{Path, PathBuf};

fn journal_dir_under(saved_games: PathBuf) -> PathBuf {
    saved_games.join("Frontier Developments/Elite Dangerous")
}

/// Windows' Saved Games directory can be redirected independently of the
/// profile directory. Ask the shell for the actual known-folder path rather
/// than assuming `%USERPROFILE%\\Saved Games`.
#[cfg(windows)]
fn windows_saved_games() -> Option<PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, ptr, slice};
    use windows_sys::Win32::Foundation::S_OK;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_SavedGames, SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY,
    };

    unsafe {
        let mut raw = ptr::null_mut();
        let status = SHGetKnownFolderPath(
            &FOLDERID_SavedGames,
            KF_FLAG_DONT_VERIFY as u32,
            ptr::null_mut(),
            &mut raw,
        );
        if status != S_OK || raw.is_null() {
            CoTaskMemFree(raw.cast());
            return None;
        }
        let mut len = 0;
        while *raw.add(len) != 0 {
            len += 1;
        }
        let path = PathBuf::from(OsString::from_wide(slice::from_raw_parts(raw, len)));
        CoTaskMemFree(raw.cast());
        Some(path)
    }
}

#[cfg(not(windows))]
fn windows_saved_games() -> Option<PathBuf> {
    None
}

/// Standard journal locations to try, in order, when the caller hasn't
/// specified one explicitly. Covers: running directly on Windows, and
/// running inside this project's dev sandbox where the folder is mounted
/// under `$HOME/mnt/...` (see the repo README for that setup).
pub fn candidate_journal_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(saved_games) = windows_saved_games() {
        out.push(journal_dir_under(saved_games));
    }
    if let Some(home) = dirs_home() {
        out.push(home.join("mnt/Saved Games/Frontier Developments/Elite Dangerous"));
        out.push(home.join("Saved Games/Frontier Developments/Elite Dangerous"));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        out.push(PathBuf::from(profile).join("Saved Games/Frontier Developments/Elite Dangerous"));
    }
    out
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn find_journal_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_dir().then(|| p.to_path_buf());
    }
    candidate_journal_dirs().into_iter().find(|p| p.is_dir())
}

/// All `Journal.*.log` files in the folder, oldest first (the timestamped
/// filenames sort chronologically as plain strings).
pub fn journal_files(journal_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(journal_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("Journal.") && n.ends_with(".log"))
        })
        .collect();
    files.sort();
    Ok(files)
}

/// The most recent `count` journal files, oldest-first (ready to replay in
/// chronological order). Scanning more than one file matters: a fresh
/// session may not have written a `Materials` snapshot yet, so the
/// baseline has to come from wherever the last one actually is.
pub fn recent_journal_files(journal_dir: &Path, count: usize) -> std::io::Result<Vec<PathBuf>> {
    let files = journal_files(journal_dir)?;
    let start = files.len().saturating_sub(count);
    Ok(files[start..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontier_journal_is_relative_to_the_resolved_saved_games_folder() {
        assert_eq!(
            journal_dir_under(PathBuf::from("X:/Redirected Saves")),
            PathBuf::from("X:/Redirected Saves/Frontier Developments/Elite Dangerous")
        );
    }
}
