//! Joystick buttons via WinMM (`joyGetPosEx`): sees every DirectInput
//! device including virtual ones (vJoy, Joystick Gremlin outputs), which
//! is what a HOTAS commander binds push-to-talk to.

#[derive(Debug, Clone, serde::Serialize)]
pub struct JoyDevice {
    pub id: u32,
    pub name: String,
    pub buttons: u32,
}

#[cfg(windows)]
mod imp {
    use super::JoyDevice;
    use windows_sys::Win32::Media::Multimedia::{
        joyGetDevCapsW, joyGetNumDevs, joyGetPosEx, JOYCAPSW, JOYERR_NOERROR, JOYINFOEX,
        JOY_RETURNBUTTONS, JOY_RETURNPOV,
    };

    pub fn devices() -> Vec<JoyDevice> {
        let mut out = Vec::new();
        // SAFETY: plain WinMM queries with properly sized structs we own.
        unsafe {
            let n = joyGetNumDevs();
            for id in 0..n {
                let mut caps: JOYCAPSW = std::mem::zeroed();
                if joyGetDevCapsW(
                    id as usize,
                    &mut caps,
                    std::mem::size_of::<JOYCAPSW>() as u32,
                ) != JOYERR_NOERROR
                {
                    continue;
                }
                let mut info: JOYINFOEX = std::mem::zeroed();
                info.dwSize = std::mem::size_of::<JOYINFOEX>() as u32;
                info.dwFlags = JOY_RETURNBUTTONS as u32;
                if joyGetPosEx(id, &mut info) != JOYERR_NOERROR {
                    continue; // present in caps but not attached
                }
                // JOYCAPSW is packed: copy fields out before taking references.
                let pname = caps.szPname;
                let nbuttons = caps.wNumButtons;
                let name = String::from_utf16_lossy(&pname)
                    .trim_end_matches('\0')
                    .to_string();
                out.push(JoyDevice {
                    id,
                    name: if name.is_empty() {
                        format!("Joystick {id}")
                    } else {
                        name
                    },
                    buttons: nbuttons,
                });
            }
        }
        out
    }

    /// The first hat's angle in hundredths of a degree (0 = up, 9000 =
    /// right), or `None` when centred or absent.
    pub fn pov(id: u32) -> Option<u32> {
        // SAFETY: as above.
        unsafe {
            let mut info: JOYINFOEX = std::mem::zeroed();
            info.dwSize = std::mem::size_of::<JOYINFOEX>() as u32;
            info.dwFlags = JOY_RETURNPOV as u32;
            if joyGetPosEx(id, &mut info) != JOYERR_NOERROR {
                return None;
            }
            (info.dwPOV < 36000).then_some(info.dwPOV)
        }
    }

    /// Bitmask of pressed buttons for a device (bit n = button n+1).
    pub fn buttons(id: u32) -> Option<u32> {
        // SAFETY: as above.
        unsafe {
            let mut info: JOYINFOEX = std::mem::zeroed();
            info.dwSize = std::mem::size_of::<JOYINFOEX>() as u32;
            info.dwFlags = JOY_RETURNBUTTONS as u32;
            (joyGetPosEx(id, &mut info) == JOYERR_NOERROR).then_some(info.dwButtons)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::JoyDevice;
    pub fn devices() -> Vec<JoyDevice> {
        Vec::new()
    }
    pub fn buttons(_id: u32) -> Option<u32> {
        None
    }
    pub fn pov(_id: u32) -> Option<u32> {
        None
    }
}

pub use imp::{buttons, devices, pov};

/// Hat directions are pseudo-buttons above this: HAT_BASE + 1 = up,
/// then clockwise in 45-degree steps to 8 = up-left.
pub const HAT_BASE: u32 = 1000;

/// The hat direction (1..=8) for a POV angle.
pub fn hat_dir(pov: u32) -> u32 {
    ((pov + 2250) % 36000) / 4500 + 1
}

pub fn hat_name(button: u32) -> &'static str {
    match button.saturating_sub(HAT_BASE) {
        1 => "hat up",
        2 => "hat up-right",
        3 => "hat right",
        4 => "hat down-right",
        5 => "hat down",
        6 => "hat down-left",
        7 => "hat left",
        8 => "hat up-left",
        _ => "hat",
    }
}

/// Is button `button` (1-based; hat directions above `HAT_BASE`) down on device `id`?
pub fn is_down(id: u32, button: u32) -> bool {
    if button > HAT_BASE {
        return pov(id).is_some_and(|p| hat_dir(p) == button - HAT_BASE);
    }
    button >= 1 && buttons(id).is_some_and(|m| m & (1u32 << (button - 1)) != 0)
}

/// A button watched for presses: the callback runs on each down edge.
struct Watch {
    id: u32,
    button: u32,
    cb: Box<dyn Fn() + Send>,
    was: bool,
}

static WATCHES: std::sync::OnceLock<std::sync::Mutex<Vec<Watch>>> = std::sync::OnceLock::new();
static POLLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn watches() -> &'static std::sync::Mutex<Vec<Watch>> {
    WATCHES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Run `cb` whenever `button` (1-based) on device `id` goes down. One
/// polling thread (50 Hz) serves every watch for the life of the process.
pub fn watch(id: u32, button: u32, cb: Box<dyn Fn() + Send>) {
    watches()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Watch {
            id,
            button,
            cb,
            was: is_down(id, button),
        });
    if POLLING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut ws = watches().lock().unwrap_or_else(|e| e.into_inner());
        for w in ws.iter_mut() {
            let now = is_down(w.id, w.button);
            if now && !w.was {
                tracing::debug!(joystick = w.id, button = w.button, "joystick watch: press");
                (w.cb)();
            }
            w.was = now;
        }
    });
}

/// Forget every watch (before re-arming).
pub fn unwatch_all() {
    watches().lock().unwrap_or_else(|e| e.into_inner()).clear();
}
