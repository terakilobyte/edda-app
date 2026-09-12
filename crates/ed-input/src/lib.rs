//! Driving the game with its own keybinds.
//!
//! Elite Dangerous has no API; everything the app can *do* in-game is a
//! keystroke. So this crate does two things and nothing else:
//!
//! * [`binds`] reads the commander's `Custom.*.binds` file and resolves an
//!   action name (`IncreaseSystemsPower`, `GalaxyMapOpen`, ...) to the
//!   keyboard chord bound to it -- primary or secondary, whichever is on
//!   the keyboard -- so nothing is ever assumed about their layout.
//! * [`send`] presses those chords through `SendInput` with hardware scan
//!   codes, which is what the game's DirectInput reader actually sees;
//!   virtual-key events alone are ignored by it. Off Windows the same
//!   [`send::KeySink`] is served by uinput/xdotool ([`linux`]) or CGEvent
//!   ([`macos`]), translated through [`keymap`].
//!
//! What is deliberately absent: any notion of *when* an action is safe.
//! That policy lives in the app, next to the journal state that informs it.

pub mod binds;
pub mod joy;
pub mod keymap;
pub mod keys;
#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod record;
pub mod send;

pub use binds::{Binds, Chord};

/// Run the current thread below normal priority: for background planning
/// that should never make the UI wait.
#[cfg(windows)]
pub fn lower_thread_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
    };
    // SAFETY: plain Win32 call on the calling thread's pseudo-handle.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(not(windows))]
pub fn lower_thread_priority() {}
