//! Process health, measured: the kernel handle count, once a minute.
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

/// Log the handle count every minute: info while it is ordinary, warn above
/// `WARN_AT`, and info again (with the previous figure) when it moves by
/// more than a quarter either way, so a leak reads as a rising series.
pub fn spawn_handle_watch() {
    std::thread::Builder::new()
        .name("health".into())
        .spawn(|| {
            let mut last: Option<u32> = None;
            loop {
                if let Some(n) = handle_count() {
                    let moved = last.is_some_and(|p| n.abs_diff(p) * 4 > p);
                    if n >= WARN_AT {
                        tracing::warn!(handles = n, previous = last, "process handle count is high; a leak is likely (joystick, audio, files)");
                    } else if last.is_none() || moved {
                        tracing::info!(handles = n, previous = last, "process handle count");
                    }
                    last = Some(n);
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
}
