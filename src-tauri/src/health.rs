//! Process health, measured: the kernel handle count and the memory the
//! process holds, once a minute.
//!
//! 2026-10-16: the dev app ran two days with a 50 Hz joystick poll on a
//! WinMM id that had moved, leaking a registry key per call until it held
//! 16.7 million handles (the per-process ceiling). From then on `read_dir`
//! on the journal folder, audio output and every HTTPS request failed, and
//! the only trace was "journal sync failed" every five seconds with no
//! cause. A handle count in the log turns that into a line that moves
//! within minutes of the leak starting; the warning above `WARN_AT` reaches
//! telemetry (warn callsites are in the allowlist) with the count only.

use std::time::Duration;

/// An app at rest holds a few thousand handles; past this something leaks.
pub const WARN_AT: u32 = 20_000;

/// Handles this process holds right now, where the platform can say.
pub fn handle_count() -> Option<u32> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
        let mut n: u32 = 0;
        // SAFETY: the pseudo-handle for the current process and an out-pointer we own.
        if unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut n) } != 0 {
            return Some(n);
        }
        None
    }
    #[cfg(not(windows))]
    {
        std::fs::read_dir("/dev/fd").ok().map(|d| d.count() as u32)
    }
}

/// Memory this process holds right now, in MB: (private bytes, working
/// set). 2026-10-16, the boss with no voice installed: "still using 837.9mb
/// of ram dude" -- the number has to be in the log next to what loaded.
pub fn memory_mb() -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        // SAFETY: the pseudo-handle for the current process, an out-struct we
        // own with its size filled in, as the API requires; the EX struct is
        // a prefix-compatible superset of PROCESS_MEMORY_COUNTERS.
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb) };
        if ok != 0 {
            return Some(((c.PrivateUsage / 1_048_576) as u64, (c.WorkingSetSize / 1_048_576) as u64));
        }
        None
    }
    #[cfg(not(windows))]
    {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let kb = |key: &str| status.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok());
        Some((kb("VmData:")? / 1024, kb("VmRSS:")? / 1024))
    }
}

/// A memory figure on its own line for a phase boundary: what a load or a
/// release cost, as `private_mb` after it.
pub fn private_mb() -> Option<u64> {
    memory_mb().map(|(private, _)| private)
}

/// Log the handle count and memory every minute: info while they are
/// ordinary, warn above `WARN_AT` handles, and info again (with the previous
/// figures) when either moves by more than a quarter, so a leak reads as a
/// rising series.
pub fn spawn_handle_watch() {
    std::thread::Builder::new()
        .name("health".into())
        .spawn(|| {
            let mut last: Option<u32> = None;
            let mut last_mb: Option<u64> = None;
            loop {
                let (private_mb, ws_mb) = memory_mb().map(|(p, w)| (Some(p), Some(w))).unwrap_or((None, None));
                if let Some(n) = handle_count() {
                    let moved = last.is_some_and(|p| n.abs_diff(p) * 4 > p)
                        || matches!((last_mb, private_mb), (Some(p), Some(m)) if m.abs_diff(p) * 4 > p.max(1));
                    if n >= WARN_AT {
                        tracing::warn!(handles = n, previous = last, private_mb, ws_mb, "process handle count is high; a leak is likely (joystick, audio, files)");
                    } else if last.is_none() || moved {
                        tracing::info!(handles = n, previous = last, private_mb, ws_mb, previous_mb = last_mb, "process handle count and memory");
                    }
                    last = Some(n);
                    last_mb = private_mb;
                }
                std::thread::sleep(Duration::from_secs(60));
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_count_is_readable_and_modest_under_test() {
        let n = super::handle_count().expect("the platform reports handles");
        assert!(n > 0 && n < super::WARN_AT, "{n} handles in a test process");
    }

    #[test]
    fn memory_is_readable_and_grows_when_we_allocate() {
        let (private, ws) = super::memory_mb().expect("the platform reports memory");
        assert!(private > 0 && ws > 0, "{private} MB private, {ws} MB working set");
        let block = vec![1u8; 64 * 1_048_576];
        let (after, ws_after) = super::memory_mb().unwrap();
        // Touched pages show in the working set everywhere; the private
        // figure follows on Windows and on Linux's VmData for an mmap'd block.
        assert!(after >= private + 60 || ws_after >= ws + 60, "64 MB allocated: {private}/{ws} -> {after}/{ws_after} MB (private/working set)");
        assert_eq!(block[block.len() - 1], 1);
    }
}
