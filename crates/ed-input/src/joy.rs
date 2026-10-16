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
    use windows_sys::Win32::Media::Multimedia::{joyGetDevCapsW, joyGetNumDevs, joyGetPosEx, JOYCAPSW, JOYERR_NOERROR, JOYINFOEX, JOY_RETURNBUTTONS, JOY_RETURNPOV};

    pub fn devices() -> Vec<JoyDevice> {
        let mut out = Vec::new();
        // SAFETY: plain WinMM queries with properly sized structs we own.
        unsafe {
            let n = joyGetNumDevs();
            for id in 0..n {
                let mut caps: JOYCAPSW = std::mem::zeroed();
                if joyGetDevCapsW(id as usize, &mut caps, std::mem::size_of::<JOYCAPSW>() as u32) != JOYERR_NOERROR {
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
                let name = String::from_utf16_lossy(&pname).trim_end_matches('\0').to_string();
                out.push(JoyDevice { id, name: if name.is_empty() { format!("Joystick {id}") } else { name }, buttons: nbuttons });
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
    probe(id, button).unwrap_or(false)
}

/// `is_down`, with `None` when the device did not answer: not there, or
/// WinMM's "was attached, now unplugged" state. Pollers must stop asking
/// a device that answers `None` (see [`Poller`]).
pub fn probe(id: u32, button: u32) -> Option<bool> {
    if button > HAT_BASE {
        return pov(id).map(|p| hat_dir(p) == button - HAT_BASE);
    }
    (button >= 1).then(|| buttons(id).map(|m| m & (1u32 << (button - 1)) != 0)).flatten()
}

/// The device a saved binding means today. The saved id while its name
/// still matches and answers for the button (generic names like
/// "Microsoft PC-joystick driver" cover several sticks); else the first
/// device with that name that answers for it -- a hat lives on one of two
/// same-named sticks, a button 20 needs a stick with 20 buttons -- because
/// WinMM ids move when a stick is re-plugged (2026-10-16: the saved id 2
/// had become 3 for the buttons and 4 for the hat).
pub fn resolve(id: u32, button: u32, name: &str) -> Option<u32> {
    let devs = devices();
    let same = |d: &&JoyDevice| !d.name.is_empty() && name.starts_with(&d.name);
    let answers = |d: &&JoyDevice| probe(d.id, button).is_some();
    devs.iter()
        .find(|d| d.id == id && same(d) && answers(d))
        .or_else(|| devs.iter().find(|d| same(d) && answers(d)))
        .or_else(|| devs.iter().find(same))
        .map(|d| d.id)
}

/// A saved button binding polled at 50 Hz without ever hammering a device
/// that does not answer.
///
/// 2026-10-16: the maintainer's stick moved from WinMM id 2 to ids 3 and
/// 4 after a re-plug while the app ran. Two 50 Hz loops kept calling
/// `joyGetPosEx(2)`, and WinMM leaks a registry key handle per call on a
/// device it once knew and now cannot find: 16.7 million `DirectInput\
/// VID_xxxx&PID_xxxx\Calibration` keys in two days, the per-process
/// ceiling, after which `read_dir` on the journal folder, audio output and
/// HTTPS all failed. A poller that loses its device stops polling it,
/// re-resolves the saved binding by name on a backoff (2 s doubling to
/// 60 s), and resumes on the new id.
#[derive(Debug)]
pub struct Poller {
    saved_id: u32,
    pub button: u32,
    name: String,
    id: Option<u32>,
    retry_at: std::time::Instant,
    backoff: std::time::Duration,
    pub was: bool,
}

impl Poller {
    pub fn new(id: u32, button: u32, name: &str) -> Self {
        Poller { saved_id: id, button, name: name.to_string(), id: Some(id), retry_at: std::time::Instant::now(), backoff: std::time::Duration::from_secs(2), was: false }
    }

    /// The id being polled, when the device answers.
    pub fn id(&self) -> Option<u32> {
        self.id
    }

    /// Is the button down now? `false` while the device is lost.
    pub fn is_down(&mut self) -> bool {
        if let Some(id) = self.id {
            match probe(id, self.button) {
                Some(down) => return down,
                None => {
                    tracing::warn!(joystick = id, button = self.button, name = %self.name, "joystick stopped answering; polling paused, looking for it by name");
                    self.id = None;
                    self.backoff = std::time::Duration::from_secs(2);
                    self.retry_at = std::time::Instant::now() + self.backoff;
                }
            }
            return false;
        }
        if std::time::Instant::now() >= self.retry_at {
            match resolve(self.saved_id, self.button, &self.name) {
                Some(id) if probe(id, self.button).is_some() => {
                    tracing::info!(joystick = id, saved = self.saved_id, button = self.button, name = %self.name, "joystick found again");
                    self.id = Some(id);
                    return probe(id, self.button).unwrap_or(false);
                }
                _ => {
                    self.backoff = (self.backoff * 2).min(std::time::Duration::from_secs(60));
                    self.retry_at = std::time::Instant::now() + self.backoff;
                }
            }
        }
        false
    }
}

/// A button watched for presses: the callback runs on each down edge.
struct Watch {
    poller: Poller,
    cb: Box<dyn Fn() + Send>,
}

static WATCHES: std::sync::OnceLock<std::sync::Mutex<Vec<Watch>>> = std::sync::OnceLock::new();
static POLLING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn watches() -> &'static std::sync::Mutex<Vec<Watch>> {
    WATCHES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Run `cb` whenever `button` (1-based) on the device the saved binding
/// (`id`, `name`) means goes down. One polling thread (50 Hz) serves every
/// watch for the life of the process; a lost device pauses its watch.
pub fn watch(id: u32, button: u32, name: &str, cb: Box<dyn Fn() + Send>) {
    let mut poller = Poller::new(id, button, name);
    poller.was = poller.is_down();
    watches().lock().unwrap_or_else(|e| e.into_inner()).push(Watch { poller, cb });
    if POLLING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut ws = watches().lock().unwrap_or_else(|e| e.into_inner());
        for w in ws.iter_mut() {
            let now = w.poller.is_down();
            if now && !w.poller.was {
                tracing::debug!(joystick = ?w.poller.id(), button = w.poller.button, "joystick watch: press");
                (w.cb)();
            }
            w.poller.was = now;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A poller on a device that does not exist never answers true and
    /// polls it only on its backoff schedule: the first miss pauses it.
    #[test]
    fn a_lost_device_pauses_the_poller() {
        let mut p = Poller::new(u32::MAX - 1, 1, "No such stick");
        assert!(!p.is_down());
        assert_eq!(p.id(), None, "the first miss drops the id");
        let retry = p.retry_at;
        assert!(!p.is_down(), "lost: answers false without touching the device");
        assert_eq!(p.retry_at, retry, "no retry before the backoff elapses");
    }

    #[test]
    fn hat_directions_cover_the_circle() {
        assert_eq!(hat_dir(0), 1);
        assert_eq!(hat_dir(9000), 3);
        assert_eq!(hat_dir(31500), 8);
        assert_eq!(hat_dir(35000), 1);
    }
}

/// Forget every watch (before re-arming).
pub fn unwatch_all() {
    watches().lock().unwrap_or_else(|e| e.into_inner()).clear();
}
