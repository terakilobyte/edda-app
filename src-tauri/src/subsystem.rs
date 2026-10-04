//! Target a named subsystem on the locked ship ("target their power plant").
//!
//! The game has no such binding, only Cycle Next / Previous Subsystem, and
//! the order of the cycle is the target's own module list, so a fixed count
//! of presses lands somewhere different on every hull. What the game does
//! give is a journal line: every change of the targeted subsystem writes a
//! `ShipTargeted` event (ScanStage 3) carrying the subsystem's symbol. The
//! boss's 2026-08-22 log has nine of them in eight seconds, one per press;
//! on 2026-10-04 a single press at 14:31:23.339 produced the Cargo Hatch
//! line inside the same second. So the loop closes through the journal:
//! press, read, compare, press again. It stops on the match, on a lost
//! target, when the game stops answering (an unscanned target cycles
//! nothing), or at the press budget.
//!
//! The journal is tailed here at 150 ms (boss, 2026-10-04: "drop the loop
//! to 150ms until we confirm we're on the correct subsystem"); the
//! watcher's 500 ms poll is for callouts, not for a lap round a Cutter.
//!
//! Measured on every request: presses, milliseconds, the subsystems seen
//! and the verdict, as a `subsystem targeting` trace line.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often the journal is read while a request is in flight.
pub const TAIL_INTERVAL: Duration = Duration::from_millis(150);
/// How long one press may go unanswered before the loop gives up.
pub const ANSWER_TIMEOUT: Duration = Duration::from_millis(1200);
/// Presses before "not on this target". A Cutter targets about thirty
/// modules; duplicates (two multi-cannons) make a lap undetectable by
/// symbol, so the budget is the lap.
pub const PRESS_BUDGET: u32 = 36;
/// Wall-clock budget for one request.
pub const TIME_BUDGET: Duration = Duration::from_secs(12);

/// One thing a commander can ask for. The symbol family is the key (boss
/// rule, 2026-09-27); the phrases are what gets said. Two target keys are
/// not outfitting symbols and are pinned from the journal: `$ext_drive_*`
/// is what the game writes for thrusters, `$modularcargobaydoor` for the
/// hatch.
#[derive(Debug)]
pub struct Kind {
    pub id: &'static str,
    pub spoken: &'static str,
    prefixes: &'static [&'static str],
    /// EDCD outfitting category this kind stands for, when it is a whole
    /// category (weapons, utilities) rather than a module family.
    category: Option<&'static str>,
    phrases: &'static [&'static str],
}

/// Specific kinds first: "shield booster" must win over "shield".
pub const KINDS: &[Kind] = &[
    Kind { id: "power_plant", spoken: "power plant", prefixes: &["int_powerplant"], category: None, phrases: &["power plant", "powerplant", "reactor"] },
    Kind { id: "drives", spoken: "drives", prefixes: &["ext_drive", "int_engine"], category: None, phrases: &["drives", "drive", "thrusters", "thruster", "engines", "engine"] },
    Kind { id: "fsd", spoken: "frame shift drive", prefixes: &["int_hyperdrive"], category: None, phrases: &["frame shift drive", "frameshift drive", "fsd", "hyperdrive"] },
    Kind { id: "life_support", spoken: "life support", prefixes: &["int_lifesupport"], category: None, phrases: &["life support"] },
    Kind { id: "power_distributor", spoken: "power distributor", prefixes: &["int_powerdistributor"], category: None, phrases: &["power distributor", "distributor"] },
    Kind { id: "sensors", spoken: "sensors", prefixes: &["int_sensors"], category: None, phrases: &["sensors", "sensor"] },
    Kind { id: "shield_booster", spoken: "shield booster", prefixes: &["hpt_shieldbooster"], category: None, phrases: &["shield booster", "booster"] },
    Kind { id: "shield_cell_bank", spoken: "shield cell bank", prefixes: &["int_shieldcellbank"], category: None, phrases: &["shield cell", "cell bank"] },
    Kind { id: "shield_generator", spoken: "shield generator", prefixes: &["int_shieldgenerator"], category: None, phrases: &["shield generator", "shields", "shield"] },
    Kind { id: "cargo_hatch", spoken: "cargo hatch", prefixes: &["modularcargobaydoor", "int_cargohatch"], category: None, phrases: &["cargo hatch", "hatch", "cargo bay door"] },
    Kind { id: "interdictor", spoken: "interdictor", prefixes: &["int_fsdinterdictor"], category: None, phrases: &["interdictor"] },
    Kind { id: "weapon", spoken: "weapon", prefixes: &[], category: Some("hardpoint"), phrases: &["weapons", "weapon", "guns", "gun", "hardpoint"] },
    Kind { id: "utility", spoken: "utility mount", prefixes: &[], category: Some("utility"), phrases: &["utility", "utilities"] },
];

pub fn kind_by_id(id: &str) -> Option<&'static Kind> {
    let key = id.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    KINDS.iter().find(|k| k.id == key)
}

/// The journal's `Subsystem` key as an outfitting-style symbol:
/// `$int_powerplant_size6_class3_name;` -> `int_powerplant_size6_class3`.
pub fn symbol_of(target_key: &str) -> String {
    let s = target_key.trim().trim_start_matches('$');
    let s = s.strip_suffix(';').unwrap_or(s);
    let s = s.strip_suffix("_name").unwrap_or(s);
    s.to_ascii_lowercase()
}

/// Which kind a targeted subsystem is, by its symbol family, else by its
/// EDCD category (any `hpt_` that is a `hardpoint` is a weapon).
pub fn kind_of(target_key: &str) -> Option<&'static Kind> {
    let sym = symbol_of(target_key);
    if let Some(k) = KINDS.iter().find(|k| k.prefixes.iter().any(|p| sym.starts_with(p))) {
        return Some(k);
    }
    let cat = ed_journal::modules::category(&sym)?;
    KINDS.iter().find(|k| k.category == Some(cat))
}

/// The kind named in a lowercase utterance, if any.
pub fn parse_request(text: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.phrases.iter().any(|p| contains_word(text, p)))
}

/// A spoken order to target one: a kind plus a targeting verb, and not a
/// pips order ("four pips to shields" is the distributor's business).
pub fn parse_order(text: &str) -> Option<&'static Kind> {
    if text.contains("pip") {
        return None;
    }
    let verb = ["target", "lock", "aim", "select", "subsystem", "sub system", "go for"].iter().any(|v| text.contains(v));
    if !verb {
        return None;
    }
    parse_request(text)
}

fn contains_word(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(i, _)| {
        let before = text[..i].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
        let after = text[i + phrase.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric());
        before && after
    })
}

/// One `ShipTargeted` line, reduced to what the loop needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub locked: bool,
    pub scan_stage: Option<i64>,
    /// The journal's target key (`$int_powerplant_size6_class3_name;`).
    pub subsystem: Option<String>,
}

impl Seen {
    pub fn from_event(v: &Value) -> Option<Seen> {
        if v.get("event").and_then(Value::as_str) != Some("ShipTargeted") {
            return None;
        }
        Some(Seen {
            locked: v.get("TargetLocked").and_then(Value::as_bool).unwrap_or(true),
            scan_stage: v.get("ScanStage").and_then(Value::as_i64),
            subsystem: v.get("Subsystem").and_then(Value::as_str).map(str::to_string),
        })
    }
    fn is(&self, kind: &Kind) -> bool {
        self.subsystem.as_deref().and_then(kind_of).is_some_and(|k| k.id == kind.id)
    }
}

/// The hands and eyes the loop needs; the live one presses the commander's
/// bind and tails the journal, the test one is scripted.
pub trait Cockpit {
    fn press_next(&mut self) -> Result<(), String>;
    /// The next `ShipTargeted` line within `timeout`, or None.
    fn next_target_event(&mut self, timeout: Duration) -> Option<Seen>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Targeted,
    Already,
    /// Pressed the whole budget without seeing it.
    Absent,
    /// The target unlocked mid-lap.
    Lost,
    /// A press went unanswered: nothing to cycle (no scan, no target).
    Silent,
}

#[derive(Debug)]
pub struct Outcome {
    pub kind: &'static Kind,
    pub verdict: Verdict,
    pub presses: u32,
    pub elapsed_ms: u128,
    /// Subsystems the lap passed, as spoken names.
    pub passed: Vec<String>,
    pub scan_stage: Option<i64>,
}

impl Outcome {
    /// What the voice says.
    pub fn text(&self) -> String {
        let k = self.kind.spoken;
        match self.verdict {
            Verdict::Targeted => format!("{} targeted.", cap(k)),
            Verdict::Already => format!("{} already targeted.", cap(k)),
            Verdict::Absent => format!("No {k} found on this target after {} subsystems.", self.presses),
            Verdict::Lost => "Target lost.".into(),
            Verdict::Silent => match self.scan_stage {
                Some(s) if s < 3 => format!("The target isn't fully scanned; its scan is at stage {s} of 3. Subsystems can be targeted once it completes."),
                _ => "The game didn't report a subsystem change. Is a target locked and fully scanned?".into(),
            },
        }
    }
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// The loop. `current` is the last target line the store holds, for the
/// already-on-it case and the scan stage in the silent one.
pub fn run(kind: &'static Kind, cockpit: &mut dyn Cockpit, current: Option<&Seen>) -> Result<Outcome, String> {
    let started = Instant::now();
    let mut out = Outcome { kind, verdict: Verdict::Silent, presses: 0, elapsed_ms: 0, passed: Vec::new(), scan_stage: current.and_then(|c| c.scan_stage) };
    if current.is_some_and(|c| c.locked && c.is(kind)) {
        out.verdict = Verdict::Already;
        return Ok(out);
    }
    while out.presses < PRESS_BUDGET && started.elapsed() < TIME_BUDGET {
        cockpit.press_next()?;
        out.presses += 1;
        let Some(seen) = cockpit.next_target_event(ANSWER_TIMEOUT) else {
            out.verdict = Verdict::Silent;
            break;
        };
        if let Some(s) = seen.scan_stage {
            out.scan_stage = Some(s);
        }
        if !seen.locked {
            out.verdict = Verdict::Lost;
            break;
        }
        if seen.is(kind) {
            out.verdict = Verdict::Targeted;
            break;
        }
        out.passed.push(seen.subsystem.as_deref().map(spoken_name).unwrap_or_else(|| "?".into()));
        out.verdict = Verdict::Absent;
    }
    out.elapsed_ms = started.elapsed().as_millis();
    Ok(out)
}

fn spoken_name(target_key: &str) -> String {
    kind_of(target_key).map(|k| k.spoken.to_string()).unwrap_or_else(|| ed_journal::modules::item_name(&symbol_of(target_key)))
}

/// A journal file read forward from where it was when the request began.
pub struct JournalTail {
    path: PathBuf,
    pos: u64,
    partial: String,
}

impl JournalTail {
    /// The newest live journal file, positioned at its end.
    pub fn open(journal_dir: &Path) -> Option<JournalTail> {
        let path = ed_journal::journal::journal_files(journal_dir).ok()?.pop()?;
        let pos = std::fs::metadata(&path).ok()?.len();
        Some(JournalTail { path, pos, partial: String::new() })
    }

    /// Every complete line written since the last poll, parsed.
    pub fn poll(&mut self) -> Vec<Value> {
        use std::io::{Read, Seek, SeekFrom};
        let Ok(mut f) = std::fs::File::open(&self.path) else {
            return Vec::new();
        };
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            return Vec::new();
        }
        let mut buf = String::new();
        let Ok(n) = f.read_to_string(&mut buf) else {
            return Vec::new();
        };
        self.pos += n as u64;
        self.partial.push_str(&buf);
        let mut out = Vec::new();
        while let Some(nl) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=nl).collect();
            if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                out.push(v);
            }
        }
        out
    }
}

/// The real cockpit: the commander's own Cycle Next Subsystem bind, and the
/// journal at 150 ms.
pub struct LiveCockpit {
    tail: JournalTail,
}

impl LiveCockpit {
    pub fn open(journal_dir: &Path) -> Result<LiveCockpit, String> {
        Ok(LiveCockpit { tail: JournalTail::open(journal_dir).ok_or("no journal file to read the target from")? })
    }
}

impl Cockpit for LiveCockpit {
    fn press_next(&mut self) -> Result<(), String> {
        crate::control::press("next_subsystem", 1).map(|_| ())
    }
    fn next_target_event(&mut self, timeout: Duration) -> Option<Seen> {
        let until = Instant::now() + timeout;
        loop {
            let seen = self.tail.poll().iter().filter_map(Seen::from_event).next_back();
            if seen.is_some() {
                return seen;
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(TAIL_INTERVAL);
        }
    }
}

/// The store's last target line, for the pre-checks.
pub fn last_target(conn: &rusqlite::Connection) -> Option<Seen> {
    let raw: String = conn
        .query_row("SELECT raw FROM events WHERE event = 'ShipTargeted' ORDER BY ts DESC, file DESC, offset DESC LIMIT 1", [], |r| r.get(0))
        .ok()?;
    Seen::from_event(&serde_json::from_str::<Value>(&raw).ok()?)
}

/// The command boundary: refuses when no target is locked, then runs the
/// loop against the game, and traces the measurement.
pub fn target(state: &crate::state::AppState, kind: &'static Kind) -> Result<Outcome, String> {
    let current = state.with_read(|s| last_target(s.conn()));
    if current.as_ref().is_some_and(|c| !c.locked) {
        return Err("no target is locked".into());
    }
    let dir = state.with_store(|s| s.journal_dir().to_path_buf());
    let mut cockpit = LiveCockpit::open(&dir)?;
    let out = run(kind, &mut cockpit, current.as_ref())?;
    tracing::info!(kind = kind.id, verdict = ?out.verdict, presses = out.presses, ms = out.elapsed_ms, passed = ?out.passed, scan_stage = ?out.scan_stage, "subsystem targeting");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Scripted hands: each press pops the next journal line.
    struct Script {
        presses: u32,
        answers: VecDeque<Option<Seen>>,
    }

    fn seen(key: &str) -> Option<Seen> {
        Some(Seen { locked: true, scan_stage: Some(3), subsystem: Some(key.into()) })
    }

    impl Cockpit for Script {
        fn press_next(&mut self) -> Result<(), String> {
            self.presses += 1;
            Ok(())
        }
        fn next_target_event(&mut self, _timeout: Duration) -> Option<Seen> {
            self.answers.pop_front().flatten()
        }
    }

    /// The boss's 2026-08-22 02:10 lap, as the journal wrote it.
    fn lap() -> VecDeque<Option<Seen>> {
        [
            "$hpt_plasmapointdefence_turret_tiny_name;",
            "$hpt_heatsinklauncher_turret_tiny_name;",
            "$hpt_cargoscanner_size0_class5_name;",
            "$hpt_multicannon_fixed_medium_name;",
            "$hpt_dumbfiremissilerack_fixed_medium_name;",
            "$hpt_multicannon_fixed_medium_name;",
            "$hpt_pulselaser_fixed_small_name;",
            "$ext_drive_class4_a_name;",
        ]
        .into_iter()
        .map(seen)
        .collect()
    }

    /// Every target key the boss's journal has written maps to a kind; the
    /// symbol family is the key, the EDCD category breaks the hardpoints.
    #[test]
    fn journal_target_keys_map_to_kinds() {
        let k = |key: &str| kind_of(key).map(|k| k.id);
        assert_eq!(k("$int_powerplant_size6_class3_name;"), Some("power_plant"));
        assert_eq!(k("$ext_drive_class4_a_name;"), Some("drives"), "thrusters are written as ext_drive, not int_engine");
        assert_eq!(k("$int_hyperdrive_size5_class3_name;"), Some("fsd"));
        assert_eq!(k("$int_lifesupport_size3_class3_name;"), Some("life_support"));
        assert_eq!(k("$int_powerdistributor_size6_class3_name;"), Some("power_distributor"));
        assert_eq!(k("$int_shieldgenerator_size4_class5_name;"), Some("shield_generator"));
        assert_eq!(k("$modularcargobaydoor_name;"), Some("cargo_hatch"));
        assert_eq!(k("$int_fsdinterdictor_size1_class3_name;"), Some("interdictor"));
        assert_eq!(k("$hpt_shieldbooster_size0_class3_name;"), Some("shield_booster"), "a booster is not the shield generator");
        assert_eq!(k("$hpt_pulselaser_fixed_small_name;"), Some("weapon"));
        assert_eq!(k("$hpt_railgun_fixed_medium_name;"), Some("weapon"));
        assert_eq!(k("$hpt_plasmapointdefence_turret_tiny_name;"), Some("utility"), "point defence is a utility, not a weapon");
        assert_eq!(k("$hpt_heatsinklauncher_turret_tiny_name;"), Some("utility"));
        assert_eq!(k("$int_dronecontrol_collection_size3_class3_name;"), None, "a limpet controller is nothing anyone asks for");
    }

    #[test]
    fn orders_need_a_kind_and_a_targeting_verb() {
        let id = |t: &str| parse_order(t).map(|k| k.id);
        assert_eq!(id("target their engines."), Some("drives"), "the boss's exact order, 2026-10-04 14:31");
        assert_eq!(id("target the power plant"), Some("power_plant"));
        assert_eq!(id("lock onto their fsd"), Some("fsd"));
        assert_eq!(id("target the shield booster"), Some("shield_booster"), "specific before generic");
        assert_eq!(id("target their shields"), Some("shield_generator"));
        assert_eq!(id("go for the hatch"), Some("cargo_hatch"));
        assert_eq!(id("four pips to shields"), None, "pips are the distributor's");
        assert_eq!(id("next subsystem"), None, "no kind named: the blind step stays a blind step");
        assert_eq!(id("how is my power plant"), None, "no targeting verb");
        assert_eq!(id("target the fsdinterdictor"), None, "whole words only");
    }

    #[test]
    fn the_lap_stops_on_the_match_and_counts_its_presses() {
        let mut c = Script { presses: 0, answers: lap() };
        let o = run(kind_by_id("drives").unwrap(), &mut c, None).unwrap();
        assert_eq!(o.verdict, Verdict::Targeted);
        assert_eq!(o.presses, 8, "the drive came round eighth, as in the log");
        assert_eq!(o.passed, ["utility mount", "utility mount", "utility mount", "weapon", "weapon", "weapon", "weapon"]);
        assert_eq!(o.text(), "Drives targeted.");
        let mut c = Script { presses: 0, answers: lap() };
        let o = run(kind_by_id("weapon").unwrap(), &mut c, None).unwrap();
        assert_eq!((o.verdict, o.presses), (Verdict::Targeted, 4), "the first hardpoint, not the point defence");
    }

    #[test]
    fn already_on_it_presses_nothing() {
        let mut c = Script { presses: 0, answers: lap() };
        let now = seen("$int_powerplant_size4_class3_name;").unwrap();
        let o = run(kind_by_id("power_plant").unwrap(), &mut c, Some(&now)).unwrap();
        assert_eq!((o.verdict, o.presses, c.presses), (Verdict::Already, 0, 0));
        assert_eq!(o.text(), "Power plant already targeted.");
    }

    #[test]
    fn a_lost_target_and_a_silent_game_stop_the_lap() {
        let mut c = Script { presses: 0, answers: VecDeque::from([seen("$modularcargobaydoor_name;"), Some(Seen { locked: false, scan_stage: None, subsystem: None })]) };
        let o = run(kind_by_id("fsd").unwrap(), &mut c, None).unwrap();
        assert_eq!((o.verdict, o.presses), (Verdict::Lost, 2));
        assert_eq!(o.text(), "Target lost.");
        let stage2 = Seen { locked: true, scan_stage: Some(2), subsystem: None };
        let mut c = Script { presses: 0, answers: VecDeque::from([None]) };
        let o = run(kind_by_id("fsd").unwrap(), &mut c, Some(&stage2)).unwrap();
        assert_eq!((o.verdict, o.presses), (Verdict::Silent, 1));
        assert!(o.text().contains("stage 2 of 3"), "{}", o.text());
    }

    #[test]
    fn the_press_budget_is_the_lap() {
        let answers: VecDeque<Option<Seen>> = (0..60).map(|_| seen("$modularcargobaydoor_name;")).collect();
        let mut c = Script { presses: 0, answers };
        let o = run(kind_by_id("shield_generator").unwrap(), &mut c, None).unwrap();
        assert_eq!((o.verdict, o.presses), (Verdict::Absent, PRESS_BUDGET));
        assert_eq!(o.text(), format!("No shield generator found on this target after {PRESS_BUDGET} subsystems."));
    }

    /// The tail reads only what is written after it opens, and holds a
    /// half-written line until its newline arrives.
    #[test]
    fn the_journal_tail_reads_forward_from_where_it_opened() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Journal.2026-10-04T140000.01.log");
        std::fs::write(&path, "{\"event\":\"Fileheader\"}\n").unwrap();
        let mut tail = JournalTail::open(dir.path()).unwrap();
        assert!(tail.poll().is_empty(), "nothing new yet");
        let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{{\"event\":\"ShipTargeted\",\"TargetLocked\":true,\"ScanStage\":3,\"Subsystem\":\"$ext_drive_class4_a_name;\"}}\n{{\"event\":\"ShipTargeted\",\"TargetLocked\":true,\"ScanStage\":3,\"Subsy").unwrap();
        f.flush().unwrap();
        let got: Vec<Seen> = tail.poll().iter().filter_map(Seen::from_event).collect();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].subsystem.as_deref(), Some("$ext_drive_class4_a_name;"));
        writeln!(f, "stem\":\"$int_powerplant_size6_class3_name;\"}}").unwrap();
        f.flush().unwrap();
        let got: Vec<Seen> = tail.poll().iter().filter_map(Seen::from_event).collect();
        assert_eq!(got.len(), 1, "the half line completed");
        assert_eq!(got[0].subsystem.as_deref(), Some("$int_powerplant_size6_class3_name;"));
        assert!(tail.poll().is_empty());
    }
}
