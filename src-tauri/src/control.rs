//! Hands: press the commander's own bindings on request ("four pips to
//! systems", "gear down", "deploy hardpoints").
//!
//! Only actions on the list below can be pressed, always through the chord
//! in the commander's `Custom.binds`, and only while the game window has
//! focus. Friendly names map to binding actions so the ship computer can
//! ask for "landing gear" without knowing Frontier's identifiers.

use serde::Serialize;

/// (friendly name, binding action, what it does) -- the whole vocabulary.
pub const ACTIONS: &[(&str, &str, &str)] = &[
    ("pips_systems", "IncreaseSystemsPower", "one pip to SYS"),
    ("pips_engines", "IncreaseEnginesPower", "one pip to ENG"),
    ("pips_weapons", "IncreaseWeaponsPower", "one pip to WEP"),
    ("pips_reset", "ResetPowerDistribution", "pips back to 2/2/2"),
    ("landing_gear", "LandingGearToggle", "toggle landing gear"),
    ("cargo_scoop", "ToggleCargoScoop", "toggle cargo scoop"),
    ("lights", "ShipSpotLightToggle", "toggle ship lights"),
    ("night_vision", "NightVisionToggle", "toggle night vision"),
    (
        "hardpoints",
        "DeployHardpointToggle",
        "deploy/retract hardpoints",
    ),
    (
        "flight_assist",
        "ToggleFlightAssist",
        "toggle flight assist",
    ),
    ("heat_sink", "DeployHeatSink", "fire a heat sink"),
    ("chaff", "FireChaffLauncher", "fire chaff"),
    ("shield_cell", "UseShieldCell", "use a shield cell"),
    ("ecm", "ChargeECM", "charge the ECM"),
    ("boost", "UseBoostJuice", "engine boost"),
    ("supercruise", "Supercruise", "engage supercruise"),
    ("hyperspace", "Hyperspace", "engage the hyperspace jump"),
    (
        "jump_or_supercruise",
        "HyperSuperCombination",
        "frame shift drive (jump if targeted, else supercruise)",
    ),
    (
        "target_next_route",
        "TargetNextRouteSystem",
        "target the next system on the game's plotted route",
    ),
    ("target_ahead", "SelectTarget", "target the ship ahead"),
    ("next_target", "CycleNextTarget", "cycle to the next target"),
    (
        "previous_target",
        "CyclePreviousTarget",
        "cycle to the previous target",
    ),
    (
        "next_hostile",
        "CycleNextHostileTarget",
        "next hostile target",
    ),
    (
        "highest_threat",
        "SelectHighestThreat",
        "target the highest threat",
    ),
    (
        "next_subsystem",
        "CycleNextSubsystem",
        "next subsystem on the target",
    ),
    ("galaxy_map", "GalaxyMapOpen", "open/close the galaxy map"),
    ("system_map", "SystemMapOpen", "open/close the system map"),
    ("fss", "ExplorationFSSEnter", "enter the FSS scanner"),
    (
        "discovery_scan",
        "ExplorationFSSDiscoveryScan",
        "honk the discovery scanner",
    ),
    (
        "hud_mode",
        "PlayerHUDModeToggle",
        "switch analysis/combat HUD mode",
    ),
    (
        "silent_running",
        "ToggleButtonUpInput",
        "toggle silent running",
    ),
    ("cargo_eject_all", "EjectAllCargo", "eject all cargo"),
    ("orbit_lines", "OrbitLinesToggle", "toggle orbit lines"),
    ("headlook_reset", "HeadLookReset", "reset headlook"),
    (
        "fighter_recall",
        "RecallDismissShip",
        "recall/dismiss the ship (SRV)",
    ),
    ("throttle_zero", "SetSpeedZero", "throttle to zero"),
    ("throttle_50", "SetSpeed50", "throttle to 50%"),
    ("throttle_75", "SetSpeed75", "throttle to 75%"),
    ("throttle_100", "SetSpeed100", "throttle to 100%"),
];

#[derive(Debug, Serialize)]
pub struct ControlInfo {
    pub name: String,
    pub action: String,
    pub what: String,
    pub bound: bool,
    pub chord: Option<String>,
}

/// The vocabulary with its current binding state, for the UI and the tool description.
pub fn inventory() -> Vec<ControlInfo> {
    let binds = ed_input::binds::Binds::default_dir()
        .and_then(|d| ed_input::binds::Binds::find_latest(&d))
        .and_then(|p| ed_input::binds::Binds::load(&p).ok());
    ACTIONS
        .iter()
        .map(|(name, action, what)| {
            let chord = binds.as_ref().and_then(|b| b.chord(action));
            ControlInfo {
                name: name.to_string(),
                action: action.to_string(),
                what: what.to_string(),
                bound: chord.is_some_and(|c| c.scan_codes().is_some()),
                chord: chord.map(|c| c.human()),
            }
        })
        .collect()
}

/// Press a named control `times` times. Refuses unknown names, unbound
/// actions, and an unfocused game.
/// The whitelist entry for a friendly control name (spaces/dashes tolerated).
pub fn lookup(name: &str) -> Result<&'static (&'static str, &'static str, &'static str), String> {
    let key = name.trim().to_lowercase().replace([' ', '-'], "_");
    ACTIONS.iter().find(|(n, _, _)| *n == key).ok_or_else(|| {
        format!(
            "unknown control {name:?}; known: {}",
            ACTIONS.iter().map(|a| a.0).collect::<Vec<_>>().join(", ")
        )
    })
}

/// The command boundary: refuses an unfocused game, loads the commander's
/// binds, and presses through this host's [`ed_input::send::NativeSink`].
pub fn press(name: &str, times: u32) -> Result<String, String> {
    lookup(name)?;
    if !ed_input::send::game_is_focused() {
        return Err(format!(
            "the game window is not focused (foreground: {:?})",
            ed_input::send::foreground_title()
        ));
    }
    let binds = ed_input::binds::Binds::default_dir()
        .and_then(|d| ed_input::binds::Binds::find_latest(&d))
        .and_then(|p| ed_input::binds::Binds::load(&p).ok())
        .ok_or("no Custom.binds found")?;
    let mut sink = ed_input::send::NativeSink::default();
    press_with(&mut sink, &binds, name, times)
}

/// Press a named control `times` times through `sink`. Policy-free apart
/// from the whitelist: focus and binds discovery are the caller's.
pub fn press_with(
    sink: &mut dyn ed_input::send::KeySink,
    binds: &ed_input::binds::Binds,
    name: &str,
    times: u32,
) -> Result<String, String> {
    use ed_input::send::{press_chord, Timing};
    let (_, action, what) = lookup(name)?;
    let chord = binds.chord(action).ok_or_else(|| format!("{action} ({what}) has no keyboard binding in your Custom.binds -- bind one in the game's Controls"))?;
    let (mods, sc) = chord.scan_codes().ok_or_else(|| {
        format!(
            "{action} is bound to keys this app cannot press ({})",
            chord.human()
        )
    })?;
    let t = Timing::default();
    let n = times.clamp(1, 8);
    for _ in 0..n {
        press_chord(sink, &mods, sc, t);
        sink.sleep(std::time::Duration::from_millis(120));
    }
    tracing::info!(control = name, action, chord = %chord.human(), times = n, "pressed");
    Ok(format!("{what} x{n} ({})", chord.human()))
}

/// The game's pip rule, in half-pips: a press adds one pip (2 halves) to a
/// gauge, taken from the other two -- one half each when both have some,
/// both halves from the one that has any otherwise. A gauge holds 4 pips
/// (8 halves); 12 halves in play. Pressing a full gauge does nothing.
fn pip_step(s: [u8; 3], i: usize) -> [u8; 3] {
    if s[i] >= 8 {
        return s;
    }
    let mut n = s;
    let (a, b) = ((i + 1) % 3, (i + 2) % 3);
    let mut need = 2u8;
    // Take one half from each of the others first, then the remainder from whoever has it.
    for j in [a, b] {
        if need > 0 && n[j] > 0 {
            n[j] -= 1;
            need -= 1;
        }
    }
    for j in [a, b] {
        while need > 0 && n[j] > 0 {
            n[j] -= 1;
            need -= 1;
        }
    }
    n[i] += 2 - need;
    n
}

/// Shortest press sequence from a reset (2/2/2) to `target` pips
/// (systems, engines, weapons; halves allowed, e.g. 3.5), or the nearest
/// reachable distribution. Returns the presses and what they reach.
pub fn pip_presses(target: [f32; 3]) -> Result<(Vec<(&'static str, u32)>, [f32; 3]), String> {
    let want: [u8; 3] = [0, 1, 2].map(|i| (target[i].clamp(0.0, 4.0) * 2.0).round() as u8);
    if want.iter().map(|&h| h as u32).sum::<u32>() != 12 {
        return Err(format!(
            "pips must add up to 6 (got {}/{}/{} = {})",
            target[0],
            target[1],
            target[2],
            target.iter().sum::<f32>()
        ));
    }
    let names = ["pips_systems", "pips_engines", "pips_weapons"];
    let start = [4u8, 4, 4];
    // BFS over at most 8 presses; the state space is tiny.
    let mut seen = std::collections::HashMap::<[u8; 3], Vec<usize>>::new();
    let mut queue = std::collections::VecDeque::new();
    seen.insert(start, Vec::new());
    queue.push_back(start);
    let mut best: ([u8; 3], Vec<usize>) = (start, Vec::new());
    let dist = |s: [u8; 3]| -> u32 {
        (0..3)
            .map(|i| (s[i] as i32 - want[i] as i32).unsigned_abs())
            .sum()
    };
    while let Some(s) = queue.pop_front() {
        let path = seen[&s].clone();
        if dist(s) < dist(best.0) || (dist(s) == dist(best.0) && path.len() < best.1.len()) {
            best = (s, path.clone());
        }
        if s == want || path.len() >= 8 {
            if s == want {
                break;
            }
            continue;
        }
        for i in 0..3 {
            let n = pip_step(s, i);
            if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(n) {
                let mut p = path.clone();
                p.push(i);
                e.insert(p);
                queue.push_back(n);
            }
        }
    }
    let (reached, path) = best;
    // Collapse consecutive presses of the same gauge into (name, times).
    let mut presses: Vec<(&'static str, u32)> = vec![("pips_reset", 1)];
    for i in path {
        match presses.last_mut() {
            Some((n, t)) if *n == names[i] => *t += 1,
            _ => presses.push((names[i], 1)),
        }
    }
    Ok((
        presses,
        [
            reached[0] as f32 / 2.0,
            reached[1] as f32 / 2.0,
            reached[2] as f32 / 2.0,
        ],
    ))
}

#[cfg(test)]
mod pip_tests {
    use super::*;

    #[test]
    fn recipes() {
        let (p, r) = pip_presses([4.0, 1.0, 1.0]).unwrap();
        assert_eq!(p, vec![("pips_reset", 1), ("pips_systems", 2)]);
        assert_eq!(r, [4.0, 1.0, 1.0]);
        let (_, r) = pip_presses([4.0, 2.0, 0.0]).unwrap();
        assert_eq!(r, [4.0, 2.0, 0.0]);
        let (_, r) = pip_presses([3.0, 3.0, 0.0]).unwrap();
        assert_eq!(r, [3.0, 3.0, 0.0]);
        let (_, r) = pip_presses([2.0, 4.0, 0.0]).unwrap();
        assert_eq!(r, [2.0, 4.0, 0.0]);
        assert!(pip_presses([4.0, 4.0, 4.0]).is_err());
    }

    #[test]
    fn press_records_the_bound_chord_n_times() {
        // The sink is injected, so the press is observable without a game.
        let binds = ed_input::binds::Binds::parse(
            r#"<Root PresetName="Custom"><KeyboardLayout>en-US</KeyboardLayout>
               <IncreaseSystemsPower><Primary Device="Keyboard" Key="Key_5"><Modifier Device="Keyboard" Key="Key_LeftControl" /></Primary></IncreaseSystemsPower>
               </Root>"#,
        )
        .unwrap();
        let mut rec = ed_input::send::Recorder::default();
        let msg = press_with(&mut rec, &binds, "pips systems", 2).unwrap();
        assert_eq!(msg, "one pip to SYS x2 (LControl+5)");
        assert_eq!(
            rec.events,
            [
                "down 0x1d",
                "down 0x06",
                "up 0x06",
                "up 0x1d",
                "down 0x1d",
                "down 0x06",
                "up 0x06",
                "up 0x1d"
            ]
        );
        assert!(press_with(&mut rec, &binds, "landing_gear", 1)
            .unwrap_err()
            .contains("no keyboard binding"));
        assert!(press_with(&mut rec, &binds, "warp drive", 1)
            .unwrap_err()
            .starts_with("unknown control"));
    }
}

#[tauri::command]
pub async fn game_controls() -> Vec<ControlInfo> {
    inventory()
}

#[tauri::command]
pub async fn game_control(name: String, times: Option<u32>) -> Result<String, String> {
    press(&name, times.unwrap_or(1))
}
