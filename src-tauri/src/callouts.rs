//! Journal-driven callouts: what the ship computer says, and when.
//!
//! Every callout comes straight from a journal event or `Status.json`, so
//! none of this needs a model -- it is pattern matching over facts the game
//! wrote a moment ago. The rules are pure functions of `(event, state)` so
//! they can be tested against real journal lines without a running game.
//!
//! Priorities: 0 quiet (overlay only), 1 notable, 2 warning, 3 critical.
//! `speak` is decided here, not by the voice thread, so the "why did it say
//! that?" question always has a single answer.
//!
//! What is deliberately *not* here: anything that requires guessing. A
//! valuable-scan callout names the body class and, for exploration value,
//! says "high value" rather than inventing a credit figure the journal does
//! not contain.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct Callout {
    pub kind: &'static str,
    pub text: String,
    pub priority: u8,
    pub speak: bool,
    pub ts: String,
}

impl Callout {
    pub fn new(kind: &'static str, ts: &str, priority: u8, speak: bool, text: String) -> Self {
        Callout {
            kind,
            text,
            priority,
            speak,
            ts: ts.to_string(),
        }
    }
}

/// While following a route, a jump produces both an arrival callout and a
/// "what's next" instruction; two utterances seconds apart is noise. Merge
/// the instruction into the arrival when one is in the same pass, shedding
/// an "Arrived at {system}." prefix the arrival already spoke. Returns the
/// text back when there is no arrival to join (the kind may be switched
/// off), so the instruction is never lost.
pub fn merge_follow_into_arrival(
    arrival: Option<&mut Callout>,
    follow_text: String,
    system: &str,
) -> Option<String> {
    let Some(arrival) = arrival else {
        return Some(follow_text);
    };
    let instruction = follow_text
        .strip_prefix(&format!("Arrived at {system}."))
        .map(str::trim_start)
        .unwrap_or(&follow_text);
    if !instruction.is_empty() {
        if !arrival.text.ends_with(' ') {
            arrival.text.push(' ');
        }
        arrival.text.push_str(instruction);
    }
    None
}

/// What has to be remembered between events to avoid repeating ourselves
/// and to phrase things in context.
#[derive(Debug, Default, Clone)]
pub struct CalloutState {
    /// Item 52 A: where each carrier was last heard from (by CarrierID;
    /// 0 when the event names none), so the startup CarrierLocation
    /// heartbeat is silent and only a MOVE speaks. Per carrier: a
    /// squadron carrier and the commander's own alternate their
    /// heartbeats at every login, and one shared slot read that as the
    /// carrier moving every time ("Your carrier is at Outordy…" about a
    /// squadron carrier, 2026-09-09).
    pub carrier_systems: std::collections::HashMap<i64, String>,
    /// Watched signals already announced (per system + signal).
    pub seen_signals: std::collections::HashSet<String>,
    /// Watched signals noticed this pass but not yet spoken: label ->
    /// (count, max threat, latest event ts). A war system spawns dozens
    /// of instances of one signal kind in a single honk; they are
    /// announced as one counted line (maintainer, 2026-09-05: "just sum the
    /// quantity") by [`flush_signals`] at the end of the pass.
    pub pending_signals: std::collections::HashMap<&'static str, (u32, i64, String)>,
    pub commander: Option<String>,
    pub ship: Option<String>,
    /// The journal ShipID last seen, to notice a change of ship.
    pub ship_id: Option<i64>,
    pub fuel_capacity: Option<f64>,
    pub fuel_main: Option<f64>,
    pub low_fuel: bool,
    pub overheating: bool,
    pub in_danger: bool,
    pub tank_full_said: bool,
    pub shields_down: bool,
    pub next_star_class: Option<String>,
    pub target_fuel_warned: bool,
    /// SystemAddress the fuel-trap guard already warned about, so
    /// retargeting the same system stays quiet until the next jump.
    pub trap_warned_target: Option<i64>,
    /// SystemAddress the beyond-range replan already fired for, so a
    /// re-target of the same impossible system replans once, not per
    /// FSDTarget event.
    pub replanned_target: Option<i64>,
    /// SystemAddress the off-route notice already fired for, so nudging
    /// the same off-plan target stays a single line.
    pub off_route_target: Option<i64>,
    /// Systems whose "All bodies found" already announced: the game
    /// re-emits FSSAllBodiesFound on FSS re-entry and journal flushes
    /// batch duplicates (three in one millisecond, 2026-09-04 flight
    /// transcript) — one announcement per system.
    pub all_bodies_said: std::collections::HashSet<u64>,
    /// The overcharge strand guard warned during this SCO burn; cleared
    /// when the SCO flag drops or the ship jumps.
    pub sco_strand_warned: bool,
    /// Live overcharge burn-rate estimate for the strand guard.
    pub sco_burn: crate::trap::ScoBurn,
    /// The power the commander is pledged to, from the `Powerplay` event.
    pub pledged: Option<String>,
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}
fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}
fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}
fn b(v: &Value, k: &str) -> Option<bool> {
    v.get(k).and_then(Value::as_bool)
}
/// `Foo_Localised` if present, else `Foo`.
fn loc<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    s(v, &format!("{k}_Localised")).or_else(|| s(v, k))
}

/// Credits as a ship computer would say them.
pub fn spoken_credits(n: i64) -> String {
    let abs = n.abs() as f64;
    let sign = if n < 0 { "minus " } else { "" };
    if abs >= 1_000_000_000.0 {
        format!("{sign}{:.2} billion credits", abs / 1e9)
    } else if abs >= 1_000_000.0 {
        format!("{sign}{:.1} million credits", abs / 1e6)
    } else if abs >= 10_000.0 {
        format!("{sign}{:.0} thousand credits", abs / 1e3)
    } else {
        format!("{sign}{n} credits")
    }
}

/// Grade 4 and 5 materials, keyed on the journal's lowercase symbol.
///
/// Curated from the in-game material grades. These are the drops worth
/// interrupting a fight for; everything else is noise in a RES.
const RARE_MATERIALS: &[(&str, u8)] = &[
    // Raw, grade 4
    ("antimony", 4),
    ("polonium", 4),
    ("ruthenium", 4),
    ("technetium", 4),
    ("tellurium", 4),
    ("yttrium", 4),
    // Manufactured, grade 5
    ("biotechconductors", 5),
    ("fedcorecomposites", 5),
    ("exquisitefocuscrystals", 5),
    ("imperialshielding", 5),
    ("improvisedcomponents", 5),
    ("militarygradealloys", 5),
    ("militarysupercapacitors", 5),
    ("pharmaceuticalisolators", 5),
    ("protoheatradiators", 5),
    ("protolightalloys", 5),
    ("protoradiolicalloys", 5),
    // Manufactured, grade 4
    ("chemicalmanipulators", 4),
    ("compoundshielding", 4),
    ("conductivepolymers", 4),
    ("configurablecomponents", 4),
    ("heatvanes", 4),
    ("polymercapacitors", 4),
    ("fedproprietarycomposites", 4),
    ("refinedfocuscrystals", 4),
    ("thermicalloys", 4),
    // Encoded, grade 5
    ("adaptiveencryptors", 5),
    ("dataminedwake", 5),
    ("classifiedscandata", 5),
    ("embeddedfirmware", 5),
    ("shieldfrequencydata", 5),
    ("compactemissionsdata", 5),
    // Encoded, grade 4
    ("shieldpatternanalysis", 4),
    ("encryptionarchives", 4),
    ("decodedemissiondata", 4),
    ("encodedscandata", 4),
    ("hyperspacetrajectories", 4),
    ("securityfirmware", 4),
    ("shieldsoundings", 4),
];

pub fn material_grade(symbol: &str) -> Option<u8> {
    let key = symbol.to_ascii_lowercase();
    RARE_MATERIALS
        .iter()
        .find(|(s, _)| *s == key)
        .map(|(_, g)| *g)
}

/// Faction states worth mentioning on arrival, with how to say them.
fn notable_state(state: &str) -> Option<&'static str> {
    Some(match state {
        "War" => "war",
        "CivilWar" => "civil war",
        "Boom" => "boom",
        "Bust" => "bust",
        "Outbreak" => "outbreak",
        "Famine" => "famine",
        "Lockdown" => "lockdown",
        "CivilUnrest" => "civil unrest",
        "PirateAttack" => "pirate attack",
        "InfrastructureFailure" => "infrastructure failure",
        "Blight" => "blight",
        "Drought" => "drought",
        "NaturalDisaster" => "natural disaster",
        "PublicHoliday" => "public holiday",
        "Terrorism" => "terrorist attack",
        "Election" => "election",
        _ => return None,
    })
}

/// Callouts for one journal event.
/// Signal sources the commander can ask to be told about. `(id, label,
/// what the journal calls it)`; matching is a case-insensitive prefix on
/// the USS type or the signal name.
pub const SIGNALS: &[(&str, &str, &str)] = &[
    ("hge", "High grade emissions", "High grade emissions"),
    ("encoded", "Encoded emissions", "Encoded emissions"),
    ("degraded", "Degraded emissions", "Degraded emissions"),
    ("weapons_fire", "Weapons fire", "Weapons fire"),
    (
        "convoy",
        "Convoy dispersal pattern",
        "Convoy dispersal pattern",
    ),
    ("distress", "Distress call", "Distress call"),
    (
        "power_convoy",
        "Power convoy distress signal",
        "Power Convoy Distress Signal",
    ),
    (
        "power_wreckage",
        "Power wreckage signature",
        "Power Wreckage Signature",
    ),
    (
        "power_weapons",
        "Power weapons fire signature",
        "Power Weapons Fire Signature",
    ),
    ("power_cz", "Power conflict zone", "Power Conflict Zone"),
    (
        "nonhuman",
        "Non-human signal source",
        "Nonhuman signal source",
    ),
    ("pirates", "Pirate activity", "Pirate Activity Detected"),
    (
        "compromised_beacon",
        "Compromised nav beacon",
        "Compromised Navigation Beacon",
    ),
];

static WATCH: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub fn set_signal_watch(ids: Vec<String>) {
    let mut w = WATCH.lock().unwrap_or_else(|e| e.into_inner());
    *w = ids
        .into_iter()
        .filter(|i| SIGNALS.iter().any(|(id, _, _)| id == i))
        .collect();
    w.sort();
    w.dedup();
}

pub fn signal_watch() -> Vec<String> {
    WATCH.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The watched signal a `FSSSignalDiscovered` event is, if any.
fn watched_signal(v: &Value) -> Option<&'static (&'static str, &'static str, &'static str)> {
    let name = s(v, "USSType_Localised")
        .or_else(|| s(v, "SignalName_Localised"))
        .or_else(|| s(v, "SignalName"))?;
    let lower = name.to_ascii_lowercase();
    let watch = signal_watch();
    SIGNALS.iter().find(|(id, _, needle)| {
        watch.iter().any(|w| w == id) && lower.starts_with(&needle.to_ascii_lowercase())
    })
}

/// "hge", "high grade", "power convoy"... -> the signal entry.
pub fn signal_by_words(text: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    let t = text.to_ascii_lowercase();
    let t = t.trim();
    if t.is_empty() {
        return None;
    }
    let aliases: &[(&str, &str)] = &[
        ("hge", "hge"),
        ("high grade", "hge"),
        ("high-grade", "hge"),
        ("encoded", "encoded"),
        ("degraded", "degraded"),
        ("weapons fire", "weapons_fire"),
        ("convoy dispersal", "convoy"),
        ("dispersal", "convoy"),
        ("distress call", "distress"),
        ("power convoy", "power_convoy"),
        ("convoy distress", "power_convoy"),
        ("wreckage", "power_wreckage"),
        ("power weapons", "power_weapons"),
        ("power conflict", "power_cz"),
        ("power cz", "power_cz"),
        ("non-human", "nonhuman"),
        ("nonhuman", "nonhuman"),
        ("non human", "nonhuman"),
        ("thargoid", "nonhuman"),
        ("pirate", "pirates"),
        ("compromised", "compromised_beacon"),
    ];
    let id = aliases
        .iter()
        .find(|(a, _)| t.contains(a))
        .map(|(_, id)| *id)
        .or_else(|| {
            SIGNALS
                .iter()
                .find(|(_, label, _)| t.contains(&label.to_ascii_lowercase()))
                .map(|(id, _, _)| *id)
        })?;
    SIGNALS.iter().find(|(i, _, _)| *i == id)
}

/// Speak what the pass's [`FSSSignalDiscovered`] events accumulated: one
/// line per signal kind, counted — "5 power conflict zones on sensors."
/// A single instance keeps the classic phrasing (with its threat); a
/// count carries the highest threat seen. Call once per watcher pass.
pub fn flush_signals(st: &mut CalloutState) -> Vec<Callout> {
    let mut pending: Vec<_> = st.pending_signals.drain().collect();
    // Deterministic order for speech and tests alike.
    pending.sort_by_key(|(label, _)| *label);
    pending
        .into_iter()
        .map(|(label, (count, threat, ts))| {
            let threat = if threat > 0 {
                {
                    if count == 1 {
                        format!(", threat {threat}")
                    } else {
                        format!(", threat up to {threat}")
                    }
                }
            } else {
                Default::default()
            };
            let text = if count == 1 {
                format!("{label} on sensors{threat}.")
            } else {
                // "Power conflict zone" -> "power conflict zones"; a label
                // already plural ("High grade emissions") stays as it is.
                let mut plural = label.to_lowercase();
                if !plural.ends_with('s') {
                    plural.push('s');
                }
                format!("{count} {plural} on sensors{threat}.")
            };
            Callout::new("signal", &ts, 2, true, text)
        })
        .collect()
}

pub fn from_event(v: &Value, st: &mut CalloutState) -> Vec<Callout> {
    let ts = s(v, "timestamp").unwrap_or("");
    let event = s(v, "event").unwrap_or("");
    let mut out = Vec::new();

    match event {
        "FSSSignalDiscovered" => {
            // The same signal is rediscovered every time supercruise is
            // entered; each INSTANCE counts once per system (war systems
            // spawn dozens with unique ":#index=N" names — measured in
            // Ega 2026-09-05: 81 distinct SignalNames in one honk).
            // Nothing is spoken here: instances accumulate and
            // [`flush_signals`] speaks one counted line per kind.
            if let Some((_, label, _)) = watched_signal(v) {
                let key = format!(
                    "{}:{}",
                    v.get("SystemAddress").and_then(Value::as_i64).unwrap_or(0),
                    s(v, "SignalName").unwrap_or("")
                );
                if st.seen_signals.insert(key) {
                    let threat = v.get("ThreatLevel").and_then(Value::as_i64).unwrap_or(0);
                    let entry = st
                        .pending_signals
                        .entry(label)
                        .or_insert((0, 0, String::new()));
                    entry.0 += 1;
                    entry.1 = entry.1.max(threat);
                    entry.2 = ts.to_string();
                }
            }
        }
        "Commander" => {
            st.commander = s(v, "Name").map(str::to_string);
        }
        "LoadGame" => {
            st.commander = s(v, "Commander")
                .map(str::to_string)
                .or(st.commander.take());
            st.ship = loc(v, "Ship").map(ed_journal::ships::display_name);
            st.ship_id = i(v, "ShipID").or(st.ship_id);
            st.fuel_capacity = f(v, "FuelCapacity");
            st.fuel_main = f(v, "FuelLevel");
            let ship_said = s(v, "ShipName")
                .filter(|n| !n.trim().is_empty())
                .map(str::to_string)
                .or_else(|| st.ship.clone());
            out.push(Callout::new(
                "greeting",
                ts,
                1,
                true,
                ed_voice::greeting(st.commander.as_deref(), ship_said.as_deref(), None),
            ));
        }
        "Loadout" => {
            st.ship = loc(v, "Ship").map(ed_journal::ships::display_name);
            if let Some(cap) = v.get("FuelCapacity").and_then(|c| f(c, "Main")) {
                st.fuel_capacity = Some(cap);
            }
            // A different ship than before: say which one we are in now.
            let id = i(v, "ShipID");
            if let (Some(new), Some(old)) = (id, st.ship_id) {
                if new != old {
                    let name = s(v, "ShipName")
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .map(str::to_string);
                    let hull = st.ship.clone().unwrap_or_else(|| "a new ship".into());
                    out.push(Callout::new(
                        "ship",
                        ts,
                        1,
                        true,
                        match name {
                            Some(n) => format!("Now flying {n}, the {hull}."),
                            None => format!("Now flying the {hull}."),
                        },
                    ));
                }
            }
            if id.is_some() {
                st.ship_id = id;
            }
        }
        "USSDrop" => {
            // The moment the game is sure to log a signal source: arrival.
            if let Some((_, label, _)) = watched_signal(v) {
                let threat = i(v, "USSThreat")
                    .filter(|&t| t > 0)
                    .map(|t| format!(", threat {t}"))
                    .unwrap_or_default();
                out.push(Callout::new(
                    "signal",
                    ts,
                    2,
                    true,
                    format!("Dropped into a {}{threat}.", label.to_ascii_lowercase()),
                ));
            }
        }
        "FSDTarget" => {
            st.next_star_class = s(v, "StarClass").map(str::to_string);
            st.target_fuel_warned = false;
            if let (Some(class), Some(jumps)) = (s(v, "StarClass"), i(v, "RemainingJumpsInRoute")) {
                let scoopable = ed_galaxy::StarClass::from_journal(class).scoopable();
                out.push(Callout::new(
                    "route",
                    ts,
                    0,
                    false,
                    format!(
                        "Next: {} ({class}{}), {jumps} jump{} remaining",
                        s(v, "Name").unwrap_or("?"),
                        if scoopable { "" } else { ", not scoopable" },
                        if jumps == 1 { "" } else { "s" }
                    ),
                ));
                if !scoopable {
                    if let Some(frac) = match (st.fuel_main, st.fuel_capacity) {
                        (Some(main), Some(cap)) if cap > 0.0 => Some(main / cap),
                        _ => None,
                    }
                    .filter(|x| *x < 0.35)
                    {
                        st.target_fuel_warned = true;
                        out.push(Callout::new("fuel", ts, 2, true, format!("Caution: {} is not scoopable and fuel is at {:.0} percent. Check the return jump or choose a fuel stop first.", s(v, "Name").unwrap_or("the targeted system"), frac * 100.0)));
                    }
                }
            }
        }
        "StartJump" if s(v, "JumpType") == Some("Hyperspace") => {
            st.tank_full_said = false;
            if let Some(class) = s(v, "StarClass") {
                let star = ed_galaxy::StarClass::from_journal(class);
                // Hazardous arrivals: the jet cones of a neutron star or
                // white dwarf are right where you drop in.
                if let Some(h) = star.hazard_label() {
                    out.push(Callout::new(
                        "hazard",
                        ts,
                        2,
                        true,
                        format!("Caution: jumping to a {h}. Throttle down on arrival."),
                    ));
                }
                let frac = match (st.fuel_main, st.fuel_capacity) {
                    (Some(m), Some(c)) if c > 0.0 => Some(m / c),
                    _ => None,
                };
                if !st.target_fuel_warned && !star.scoopable() {
                    if let Some(frac) = frac.filter(|x| *x < 0.35) {
                        out.push(Callout::new(
                            "fuel",
                            ts,
                            2,
                            true,
                            format!(
                                "Warning: {class} class star ahead, not scoopable. Fuel at {:.0} percent.",
                                frac * 100.0
                            ),
                        ));
                    }
                }
            }
        }
        "Powerplay" => {
            st.pledged = s(v, "Power").map(str::to_string);
        }
        // Fuel moving from main tank to reservoir: the strand guard's
        // burn-rate signal. No callout of its own.
        "ReservoirReplenished" => {
            if let (Some(epoch), Some(fuel)) = (crate::trap::epoch_secs(ts), f(v, "FuelMain")) {
                st.sco_burn.observe(epoch, fuel);
            }
        }
        "FSDJump" => {
            st.target_fuel_warned = false;
            st.trap_warned_target = None;
            st.sco_strand_warned = false;
            let system = s(v, "StarSystem").unwrap_or("unknown system");
            let mut parts = vec![format!("Arrived in {system}.")];
            if let Some(power) = s(v, "ControllingPower") {
                let state = s(v, "PowerplayState").unwrap_or("");
                parts.push(format!(
                    "{power}{}.",
                    if state.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", state.to_lowercase())
                    }
                ));
                // Pledged to someone else: their security treats you as hostile.
                if st
                    .pledged
                    .as_deref()
                    .is_some_and(|p| !p.eq_ignore_ascii_case(power))
                {
                    parts.push(
                        "Opposing power territory: expect their security to be hostile.".into(),
                    );
                }
            }
            let mut states: Vec<&'static str> = Vec::new();
            if let Some(factions) = v.get("Factions").and_then(Value::as_array) {
                for fac in factions {
                    if let Some(active) = fac.get("ActiveStates").and_then(Value::as_array) {
                        for a in active {
                            if let Some(n) = s(a, "State").and_then(notable_state) {
                                if !states.contains(&n) {
                                    states.push(n);
                                }
                            }
                        }
                    }
                }
            }
            if !states.is_empty() {
                parts.push(format!("States: {}.", states.join(", ")));
            }
            if let Some(conflicts) = v.get("Conflicts").and_then(Value::as_array) {
                if !conflicts.is_empty() {
                    parts.push(format!(
                        "{} active conflict{}.",
                        conflicts.len(),
                        if conflicts.len() == 1 { "" } else { "s" }
                    ));
                }
            }
            if i(v, "Population") == Some(0) {
                parts.push("Unpopulated.".into());
            }
            let notable = !states.is_empty() || v.get("ControllingPower").is_some();
            out.push(Callout::new(
                "arrival",
                ts,
                if notable { 1 } else { 0 },
                true,
                parts.join(" "),
            ));
            if let Some(fuel) = f(v, "FuelLevel") {
                st.fuel_main = Some(fuel);
            }
        }
        "FSSDiscoveryScan" => {
            if let Some(n) = i(v, "BodyCount") {
                out.push(Callout::new(
                    "scan",
                    ts,
                    0,
                    false,
                    format!("{n} bod{} in system", if n == 1 { "y" } else { "ies" }),
                ));
            }
        }
        "FSSAllBodiesFound" => {
            let address = v.get("SystemAddress").and_then(Value::as_u64).unwrap_or(0);
            if st.all_bodies_said.insert(address) {
                out.push(Callout::new(
                    "scan",
                    ts,
                    0,
                    false,
                    "All bodies found".into(),
                ));
            }
        }
        "Scan" => {
            let body = s(v, "BodyName").unwrap_or("body");
            let class = s(v, "PlanetClass").unwrap_or("");
            let terraformable = s(v, "TerraformState") == Some("Terraformable");
            let undiscovered = b(v, "WasDiscovered") == Some(false);
            let valuable = matches!(class, "Earthlike body" | "Water world" | "Ammonia world")
                || terraformable;
            if valuable {
                let what = match class {
                    "Earthlike body" => "Earth-like world".to_string(),
                    "Water world" => "water world".to_string(),
                    "Ammonia world" => "ammonia world".to_string(),
                    other => other.to_lowercase(),
                };
                let vowel = what
                    .chars()
                    .next()
                    .is_some_and(|c| "aeiou".contains(c.to_ascii_lowercase()));
                let mut text = format!(
                    "Valuable scan: {body} is a{} {what}",
                    if vowel { "n" } else { "" }
                );
                if terraformable {
                    text.push_str(", terraformable");
                }
                if undiscovered {
                    text.push_str(", undiscovered");
                }
                text.push('.');
                out.push(Callout::new("scan", ts, 1, true, text));
            } else if undiscovered && !class.is_empty() {
                out.push(Callout::new(
                    "scan",
                    ts,
                    0,
                    false,
                    format!("{body}: undiscovered {}", class.to_lowercase()),
                ));
            }
        }
        "CodexEntry" if b(v, "IsNewEntry") == Some(true) => {
            out.push(Callout::new(
                "codex",
                ts,
                1,
                true,
                format!("New codex entry: {}.", loc(v, "Name").unwrap_or("unknown")),
            ));
        }
        "ShipTargeted" if i(v, "ScanStage") == Some(3) => {
            if let Some(bounty) = i(v, "Bounty").filter(|b| *b > 0) {
                let ship = loc(v, "Ship")
                    .map(ed_journal::ships::display_name)
                    .unwrap_or_else(|| "target".to_string());
                out.push(Callout::new(
                    "target",
                    ts,
                    if bounty >= 100_000 { 1 } else { 0 },
                    bounty >= 100_000,
                    format!("Wanted {ship}, bounty {}.", spoken_credits(bounty)),
                ));
            }
        }
        "Bounty" => {
            let reward = i(v, "TotalReward").or_else(|| i(v, "Reward")).unwrap_or(0);
            let target = loc(v, "Target").unwrap_or("target");
            out.push(Callout::new(
                "kill",
                ts,
                if reward >= 250_000 { 1 } else { 0 },
                true,
                format!("{target} destroyed. Bounty {}.", spoken_credits(reward)),
            ));
        }
        "FactionKillBond" | "CapShipBond" => {
            let reward = i(v, "Reward").unwrap_or(0);
            out.push(Callout::new(
                "kill",
                ts,
                0,
                reward >= 100_000,
                format!("Kill bond {}.", spoken_credits(reward)),
            ));
        }
        "PVPKill" => {
            out.push(Callout::new(
                "kill",
                ts,
                1,
                true,
                format!(
                    "Commander {} destroyed.",
                    s(v, "Victim").unwrap_or("unknown")
                ),
            ));
        }
        "MaterialCollected" => {
            let symbol = s(v, "Name").unwrap_or("");
            if let Some(grade) = material_grade(symbol) {
                let name = loc(v, "Name").unwrap_or(symbol);
                let count = i(v, "Count").unwrap_or(1);
                out.push(Callout::new(
                    "material",
                    ts,
                    1,
                    true,
                    format!(
                        "Grade {grade} material: {name}{}.",
                        if count > 1 {
                            format!(", {count}")
                        } else {
                            String::new()
                        }
                    ),
                ));
            }
        }
        "Interdicted" => {
            let who = loc(v, "Interdictor").unwrap_or("unknown");
            let player = b(v, "IsPlayer").unwrap_or(false);
            out.push(Callout::new(
                "danger",
                ts,
                3,
                true,
                format!(
                    "Interdicted by {who}{}.",
                    if player { ", a player" } else { "" }
                ),
            ));
        }
        // Someone scanned us. The journal never says who, only what kind of
        // scan; a cargo scan in the black is a pirate sizing you up.
        "Scanned" => {
            let kind = s(v, "ScanType").unwrap_or("").to_ascii_lowercase();
            let (text, prio) = match kind.as_str() {
                "cargo" => (
                    "Cargo scan detected. Someone is sizing up your hold.".to_string(),
                    2,
                ),
                "crime" => ("Being scanned for wanted status.".to_string(), 1),
                "cabin" => ("Cabin scan detected.".to_string(), 1),
                "data" => ("Data scan detected.".to_string(), 1),
                other => (
                    format!(
                        "Scan detected{}.",
                        if other.is_empty() {
                            String::new()
                        } else {
                            format!(": {other}")
                        }
                    ),
                    1,
                ),
            };
            out.push(Callout::new("danger", ts, prio, true, text));
        }
        "UnderAttack" => {
            if s(v, "Target").is_none_or(|t| t.eq_ignore_ascii_case("You")) {
                out.push(Callout::new("danger", ts, 3, true, "Under attack.".into()));
            }
        }
        "EscapeInterdiction" => {
            out.push(Callout::new(
                "danger",
                ts,
                1,
                true,
                "Interdiction evaded.".into(),
            ));
        }
        "HullDamage"
            if b(v, "PlayerPilot").unwrap_or(true) && !b(v, "Fighter").unwrap_or(false) =>
        {
            if let Some(h) = f(v, "Health") {
                let pct = (h * 100.0).round() as i64;
                if pct <= 50 {
                    out.push(Callout::new(
                        "danger",
                        ts,
                        if pct <= 25 { 3 } else { 2 },
                        true,
                        format!("Hull at {pct} percent."),
                    ));
                }
            }
        }
        "ShieldState" => {
            let up = b(v, "ShieldsUp").unwrap_or(true);
            if up != !st.shields_down {
                st.shields_down = !up;
                out.push(Callout::new(
                    "danger",
                    ts,
                    if up { 1 } else { 2 },
                    true,
                    if up {
                        "Shields restored.".into()
                    } else {
                        "Shields down.".into()
                    },
                ));
            }
        }
        "HeatWarning" => {
            out.push(Callout::new("danger", ts, 2, true, "Heat warning.".into()));
        }
        "Died" => {
            out.push(Callout::new(
                "danger",
                ts,
                3,
                true,
                "Ship destroyed.".into(),
            ));
        }
        "DockingGranted" => {
            out.push(Callout::new(
                "docking",
                ts,
                1,
                true,
                format!("Docking granted, pad {}.", i(v, "LandingPad").unwrap_or(0)),
            ));
        }
        "DockingDenied" => {
            out.push(Callout::new(
                "docking",
                ts,
                2,
                true,
                format!(
                    "Docking denied: {}.",
                    s(v, "Reason").unwrap_or("unknown reason")
                ),
            ));
        }
        // Item 52 A: the carrier. A jump is plannable (DepartureTime runs
        // ~30 min ahead, census 2026-09-06), so say the countdown; the
        // location heartbeat speaks only when the carrier actually moved.
        "CarrierJumpRequest" => {
            let system = s(v, "SystemName")
                .unwrap_or("an unknown system")
                .to_string();
            let minutes = s(v, "DepartureTime")
                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .zip(chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|(dep, now)| (dep - now).num_minutes().max(0));
            let when = match minutes {
                Some(m) => format!("departure in {m} minutes"),
                None => "departure time unknown".to_string(),
            };
            out.push(Callout::new(
                "carrier",
                ts,
                1,
                true,
                format!("Carrier jump to {system} scheduled, {when}."),
            ));
        }
        "CarrierJumpCancelled" => {
            out.push(Callout::new(
                "carrier",
                ts,
                1,
                true,
                "Carrier jump cancelled.".into(),
            ));
        }
        "CarrierJump" => {
            let system = s(v, "StarSystem")
                .unwrap_or("an unknown system")
                .to_string();
            let id = v
                .get("CarrierID")
                .or_else(|| v.get("MarketID"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            st.carrier_systems.insert(id, system.clone());
            out.push(Callout::new(
                "carrier",
                ts,
                1,
                true,
                format!("Carrier arrived at {system}."),
            ));
        }
        "CarrierLocation" => {
            let system = s(v, "StarSystem").map(str::to_string);
            let id = v
                .get("CarrierID")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0);
            let squadron = s(v, "CarrierType") == Some("SquadronCarrier");
            let moved = matches!((st.carrier_systems.get(&id), &system), (Some(prev), Some(now)) if prev != now);
            if moved {
                let now = system.clone().unwrap_or_default();
                let line = if squadron {
                    format!("Squadron carrier is at {now}.")
                } else {
                    format!("Your carrier is at {now}.")
                };
                out.push(Callout::new("carrier", ts, 1, true, line));
            }
            if let Some(system) = system {
                st.carrier_systems.insert(id, system);
            }
        }
        "Docked" => {
            out.push(Callout::new(
                "docking",
                ts,
                0,
                false,
                format!("Docked at {}", s(v, "StationName").unwrap_or("station")),
            ));
        }
        "FuelScoop" => {
            if let Some(total) = f(v, "Total") {
                st.fuel_main = Some(total);
                if let Some(cap) = st.fuel_capacity {
                    // A tank holding more than "capacity" is proof the
                    // capacity is a stale ship's (field case 2026-09-05:
                    // the watcher still carried the Anaconda's 32 t while
                    // the Panther scooped past 116 t, announcing "full"
                    // on every sip). Adopt the evidence, stay quiet.
                    if total > cap * 1.02 {
                        st.fuel_capacity = Some(total);
                    } else if total >= cap * 0.98 && !st.tank_full_said {
                        st.tank_full_said = true;
                        out.push(Callout::new("fuel", ts, 1, true, "Fuel tank full.".into()));
                    }
                }
            }
        }
        "PowerplayMerits" => {
            let gained = i(v, "MeritsGained").unwrap_or(0);
            let total = i(v, "TotalMerits").unwrap_or(0);
            out.push(Callout::new(
                "merits",
                ts,
                0,
                gained >= 100,
                format!("Plus {gained} merits, {total} total."),
            ));
        }
        "MarketSell" => {
            let count = i(v, "Count").unwrap_or(0);
            let total = i(v, "TotalSale").unwrap_or(0);
            let paid = i(v, "AvgPricePaid").unwrap_or(0);
            let profit = total - paid * count;
            out.push(Callout::new(
                "trade",
                ts,
                0,
                true,
                format!(
                    "Sold {count} tons of {}, {} profit.",
                    loc(v, "Type").unwrap_or("cargo"),
                    spoken_credits(profit)
                ),
            ));
        }
        "MissionAccepted" => {
            let title = s(v, "LocalisedName")
                .or_else(|| s(v, "Name"))
                .unwrap_or("mission");
            let mut text = format!("Mission accepted: {title}.");
            if let Some(dest) = s(v, "DestinationSystem") {
                text.push_str(&format!(" Destination {dest}."));
            }
            out.push(Callout::new("mission", ts, 1, true, text));
        }
        "MissionRedirected" => {
            let title = s(v, "LocalisedName")
                .or_else(|| s(v, "Name"))
                .unwrap_or("mission");
            let station = s(v, "NewDestinationStation").unwrap_or("the hand-in");
            let system = s(v, "NewDestinationSystem").unwrap_or("");
            out.push(Callout::new(
                "mission",
                ts,
                1,
                true,
                format!(
                    "Objective complete: {title}. Return to {station}{}.",
                    if system.is_empty() {
                        String::new()
                    } else {
                        format!(" in {system}")
                    }
                ),
            ));
        }
        "MissionFailed" => {
            let title = s(v, "LocalisedName")
                .or_else(|| s(v, "Name"))
                .unwrap_or("mission");
            out.push(Callout::new(
                "mission",
                ts,
                2,
                true,
                format!("Mission failed: {title}."),
            ));
        }
        "MissionCompleted" => {
            let reward = i(v, "Reward").unwrap_or(0);
            out.push(Callout::new(
                "mission",
                ts,
                1,
                true,
                if reward > 0 {
                    format!("Mission complete, {}.", spoken_credits(reward))
                } else {
                    "Mission complete.".into()
                },
            ));
        }
        "Promotion" => {
            out.push(Callout::new("rank", ts, 1, true, "Rank promotion.".into()));
        }
        _ => {}
    }
    out
}

/// Callouts for a fresh `Status.json`. Edge-triggered on the flag bits so a
/// warning is said once when it starts, not on every 1 Hz rewrite.
pub fn from_status(status: &Value, st: &mut CalloutState) -> Vec<Callout> {
    use crate::status_flags::{IN_DANGER, LOW_FUEL, OVERHEATING};

    let ts = s(status, "timestamp").unwrap_or("");
    let flags = i(status, "Flags").unwrap_or(0);
    if let Some(fuel) = status.get("Fuel").and_then(|x| f(x, "FuelMain")) {
        st.fuel_main = Some(fuel);
    }
    let mut out = Vec::new();

    let low = flags & LOW_FUEL != 0;
    if low && !st.low_fuel {
        out.push(Callout::new("fuel", ts, 2, true, "Fuel low.".into()));
    }
    st.low_fuel = low;

    let hot = flags & OVERHEATING != 0;
    if hot && !st.overheating {
        out.push(Callout::new("danger", ts, 2, true, "Overheating.".into()));
    }
    st.overheating = hot;

    let danger = flags & IN_DANGER != 0;
    if danger && !st.in_danger {
        out.push(Callout::new(
            "danger",
            ts,
            1,
            false,
            "Danger flag: hostile contacts nearby".into(),
        ));
    }
    st.in_danger = danger;

    out
}

#[cfg(test)]
mod tests {
    /// Item 52 A: the carrier speaks the countdown, the cancel, the
    /// arrival — and the location heartbeat only when it actually moved
    /// (the game writes CarrierLocation at every startup).
    #[test]
    fn carrier_callouts_count_down_and_speak_moves_once() {
        let mut st = CalloutState::default();
        let req = serde_json::json!({"timestamp":"2026-01-15T22:27:00Z","event":"CarrierJumpRequest","CarrierType":"FleetCarrier","CarrierID":1,"SystemName":"Beta","Body":"Beta 1","SystemAddress":22,"BodyID":1,"DepartureTime":"2026-01-15T22:57:00Z"});
        let out = super::from_event(&req, &mut st);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].kind, out[0].speak), ("carrier", true));
        assert!(
            out[0].text.contains("Beta") && out[0].text.contains("30 minutes"),
            "{}",
            out[0].text
        );

        let cancel = serde_json::json!({"timestamp":"2026-01-15T22:30:00Z","event":"CarrierJumpCancelled","CarrierType":"FleetCarrier","CarrierID":1});
        assert_eq!(
            super::from_event(&cancel, &mut st)[0].text,
            "Carrier jump cancelled."
        );

        // Startup heartbeat: first sighting is silent.
        let loc = |sys: &str| serde_json::json!({"timestamp":"2026-01-16T01:00:00Z","event":"CarrierLocation","CarrierType":"FleetCarrier","CarrierID":1,"StarSystem":sys,"SystemAddress":33,"BodyID":0});
        assert!(super::from_event(&loc("Alpha"), &mut st).is_empty());
        assert!(
            super::from_event(&loc("Alpha"), &mut st).is_empty(),
            "same place, nothing to say"
        );
        let moved = super::from_event(&loc("Gamma"), &mut st);
        assert_eq!(moved.len(), 1);
        assert!(moved[0].text.contains("Gamma"));

        let jump = serde_json::json!({"timestamp":"2026-01-15T22:58:00Z","event":"CarrierJump","Docked":true,"StationName":"K3X-9ZQ","StationType":"FleetCarrier","MarketID":1,"StarSystem":"Beta","SystemAddress":22,"Body":"Beta 1","BodyID":1});
        let arrived = super::from_event(&jump, &mut st);
        assert!(arrived
            .iter()
            .any(|c| c.kind == "carrier" && c.text.contains("arrived at Beta")));
        assert_eq!(st.carrier_systems.get(&1).map(String::as_str), Some("Beta"));
        // A squadron carrier's heartbeat is its own: it neither moves
        // "your" carrier nor speaks as it (2026-09-09: "Your carrier is
        // at Outordy" was the squadron's).
        let squad = |sys: &str| serde_json::json!({"timestamp":"2026-01-16T01:05:00Z","event":"CarrierLocation","CarrierType":"SquadronCarrier","CarrierID":2,"StarSystem":sys,"SystemAddress":44,"BodyID":0});
        assert!(
            super::from_event(&squad("Outordy"), &mut st).is_empty(),
            "first sighting of the squadron carrier is silent"
        );
        assert!(
            super::from_event(&loc("Beta"), &mut st).is_empty(),
            "your carrier has not moved"
        );
        let squad_moved = super::from_event(&squad("Elsewhere"), &mut st);
        assert_eq!(squad_moved.len(), 1);
        assert!(
            squad_moved[0].text.starts_with("Squadron carrier is at"),
            "{}",
            squad_moved[0].text
        );
    }

    /// The 2026-09-04 flight transcript: "All bodies found" fired 3-5
    /// times per system — three in the SAME millisecond —
    /// because the game re-emits FSSAllBodiesFound on FSS re-entry and
    /// journal flushes batch duplicates. One announcement per system.
    /// One war system spawns dozens of "$Warzone_Powerplay_Med:#index=N;"
    /// signal instances (measured in Ega, 2026-09-05: 81 distinct names,
    /// a spoken burst per honk). Maintainer ruling: sum the quantity — one
    /// counted line per kind per pass; a re-honk of the same instances
    /// says nothing; a single new instance keeps the classic phrasing.
    #[test]
    fn warzone_instances_are_one_counted_callout() {
        set_signal_watch(vec!["power_cz".into()]);
        let mut st = CalloutState::default();
        let event = |addr: i64, index: u32| {
            serde_json::json!({
                "timestamp": "2026-09-05T13:46:00Z",
                "event": "FSSSignalDiscovered",
                "SystemAddress": addr,
                "SignalName": format!("$Warzone_Powerplay_Med:#index={index};"),
                "SignalName_Localised": "Power Conflict Zone [Medium]",
            })
        };
        for index in 0..30 {
            assert!(
                from_event(&event(500, index), &mut st).is_empty(),
                "instances accumulate silently"
            );
        }
        let flushed = flush_signals(&mut st);
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].text, "30 power conflict zones on sensors.");
        // Re-entering supercruise rediscovers the same instances: silence.
        for index in 0..30 {
            let _ = from_event(&event(500, index), &mut st);
        }
        assert!(
            flush_signals(&mut st).is_empty(),
            "a re-honk repeats nothing"
        );
        // One new instance in another system: the classic single phrasing.
        let _ = from_event(&event(501, 0), &mut st);
        let flushed = flush_signals(&mut st);
        assert_eq!(flushed.len(), 1);
        assert_eq!(flushed[0].text, "Power conflict zone on sensors.");
    }

    /// A scoop past the believed capacity proves the capacity belongs to
    /// another ship (field case 2026-09-05: the Anaconda's 32 t lingered
    /// while the Panther scooped at 116 t, so every sip announced "Fuel
    /// tank full"). The evidence is adopted silently; a genuine top-up
    /// against the corrected capacity still announces, once.
    #[test]
    fn scooping_past_a_stale_capacity_corrects_it_instead_of_announcing_full() {
        let mut st = CalloutState {
            fuel_capacity: Some(32.0),
            ..CalloutState::default()
        };
        let scoop = |total: f64| {
            serde_json::json!({
                "timestamp": "2026-09-05T14:26:54Z",
                "event": "FuelScoop", "Scooped": 1.5, "Total": total,
            })
        };
        assert!(
            from_event(&scoop(116.1), &mut st).is_empty(),
            "a tank fuller than 'capacity' is a stale capacity, not a full tank"
        );
        assert_eq!(st.fuel_capacity, Some(116.1), "the evidence is adopted");
        // The real tank is 128 t; learn it the same way, then a genuine
        // top-up announces exactly once.
        let _ = from_event(&scoop(128.0), &mut st);
        assert_eq!(st.fuel_capacity, Some(128.0));
        st.tank_full_said = false;
        let full = from_event(&scoop(127.0), &mut st);
        assert_eq!(full.len(), 1);
        assert_eq!(full[0].text, "Fuel tank full.");
        assert!(
            from_event(&scoop(127.5), &mut st).is_empty(),
            "said once until the next jump"
        );
    }

    #[test]
    fn all_bodies_found_announces_once_per_system() {
        let mut st = CalloutState::default();
        let event = |address: u64| {
            serde_json::json!({
                "timestamp": "2026-09-04T13:39:10Z",
                "event": "FSSAllBodiesFound",
                "SystemName": "Wongi",
                "SystemAddress": address,
                "Count": 7,
            })
        };
        let first = from_event(&event(631135993203u64), &mut st);
        assert_eq!(
            first
                .iter()
                .filter(|c| c.text == "All bodies found")
                .count(),
            1
        );
        // The transcript's same-millisecond triple: two more of the same.
        for _ in 0..2 {
            let dup = from_event(&event(631135993203u64), &mut st);
            assert!(
                dup.iter().all(|c| c.text != "All bodies found"),
                "duplicate announced"
            );
        }
        // A different system announces again.
        let other = from_event(&event(999u64), &mut st);
        assert_eq!(
            other
                .iter()
                .filter(|c| c.text == "All bodies found")
                .count(),
            1
        );
    }

    use super::*;
    use serde_json::json;

    fn ev(v: Value) -> Vec<Callout> {
        let mut st = CalloutState::default();
        from_event(&v, &mut st)
    }

    fn arrival(text: &str) -> Callout {
        Callout {
            kind: "arrival",
            text: text.into(),
            priority: 1,
            speak: true,
            ts: "t".into(),
        }
    }

    /// One utterance per jump while following: the "what's next" line joins
    /// the arrival callout rather than arriving as its own message.
    #[test]
    fn follow_instruction_joins_the_arrival_callout() {
        let mut a = arrival("Wongi. Fuel stop, scoop here.");
        let leftover =
            merge_follow_into_arrival(Some(&mut a), "1 jump left. Next: Diso.".into(), "Wongi");
        assert_eq!(leftover, None, "merged, so no standalone follow callout");
        assert_eq!(
            a.text,
            "Wongi. Fuel stop, scoop here. 1 jump left. Next: Diso."
        );
    }

    /// On the final hop the arrival already named the system; the follow
    /// text sheds its "Arrived at X." prefix instead of repeating it.
    #[test]
    fn route_completion_does_not_repeat_the_system_name() {
        let mut a = arrival("Wongi. Fuel stop, scoop here.");
        let leftover = merge_follow_into_arrival(
            Some(&mut a),
            "Arrived at Wongi. Route complete.".into(),
            "Wongi",
        );
        assert_eq!(leftover, None);
        assert_eq!(a.text, "Wongi. Fuel stop, scoop here. Route complete.");
    }

    /// With no arrival callout in the pass (the kind may be switched off),
    /// the instruction is never lost: it stays a standalone callout.
    #[test]
    fn follow_instruction_stands_alone_without_an_arrival() {
        let leftover = merge_follow_into_arrival(None, "2 jumps left. Next: Sol.".into(), "Argyre");
        assert_eq!(leftover.as_deref(), Some("2 jumps left. Next: Sol."));
    }

    #[test]
    fn arrival_reports_power_and_notable_states() {
        let c = ev(json!({
            "timestamp": "t", "event": "FSDJump", "StarSystem": "Deciat",
            "ControllingPower": "Aisling Duval", "PowerplayState": "Fortified",
            "Factions": [
                {"Name":"A","ActiveStates":[{"State":"Boom"}]},
                {"Name":"B","ActiveStates":[{"State":"CivilWar"},{"State":"Boom"}]}
            ],
            "Conflicts": [{}]
        }));
        assert_eq!(c.len(), 1);
        assert_eq!(
            c[0].text,
            "Arrived in Deciat. Aisling Duval, fortified. States: boom, civil war. 1 active conflict."
        );
        assert!(c[0].speak);
    }

    #[test]
    fn a_plain_arrival_is_still_announced_but_quietly() {
        let c =
            ev(json!({"timestamp":"t","event":"FSDJump","StarSystem":"Nowhere","Population":0}));
        assert_eq!(c[0].text, "Arrived in Nowhere. Unpopulated.");
        assert_eq!(c[0].priority, 0);
    }

    #[test]
    fn valuable_scans_are_named_without_inventing_a_price() {
        let c = ev(json!({
            "timestamp":"t","event":"Scan","BodyName":"Wongi 2","PlanetClass":"Water world",
            "TerraformState":"Terraformable","WasDiscovered":false
        }));
        assert_eq!(
            c[0].text,
            "Valuable scan: Wongi 2 is a water world, terraformable, undiscovered."
        );
        assert!(c[0].speak);
        assert!(!c[0].text.contains("credits"));
        let elw = ev(
            json!({"timestamp":"t","event":"Scan","BodyName":"X","PlanetClass":"Earthlike body"}),
        );
        assert_eq!(elw[0].text, "Valuable scan: X is an Earth-like world.");
        // A rocky body nobody has seen is noted, not spoken.
        let rock = ev(
            json!({"timestamp":"t","event":"Scan","BodyName":"Y","PlanetClass":"Rocky body","WasDiscovered":false}),
        );
        assert!(!rock[0].speak);
        // A rocky body everyone has seen says nothing.
        assert!(ev(json!({"timestamp":"t","event":"Scan","BodyName":"Z","PlanetClass":"Rocky body","WasDiscovered":true})).is_empty());
    }

    #[test]
    fn kills_speak_the_bounty_and_rare_drops_are_flagged_by_grade() {
        let c = ev(
            json!({"timestamp":"t","event":"Bounty","Target":"anaconda","Target_Localised":"Anaconda","TotalReward":1_621_122}),
        );
        assert_eq!(c[0].text, "Anaconda destroyed. Bounty 1.6 million credits.");
        assert_eq!(c[0].priority, 1);

        let m = ev(
            json!({"timestamp":"t","event":"MaterialCollected","Category":"Manufactured","Name":"protolightalloys","Name_Localised":"Proto Light Alloys","Count":3}),
        );
        assert_eq!(m[0].text, "Grade 5 material: Proto Light Alloys, 3.");
        assert!(m[0].speak);
        // Common drops stay silent.
        assert!(ev(json!({"timestamp":"t","event":"MaterialCollected","Category":"Raw","Name":"iron","Count":3})).is_empty());
    }

    #[test]
    fn unscoopable_star_warns_only_when_fuel_is_actually_low() {
        let mut st = CalloutState {
            fuel_capacity: Some(32.0),
            fuel_main: Some(8.0),
            ..Default::default()
        };
        let c = from_event(
            &json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":"L"}),
            &mut st,
        );
        assert_eq!(
            c[0].text,
            "Warning: L class star ahead, not scoopable. Fuel at 25 percent."
        );
        st.fuel_main = Some(30.0);
        assert!(from_event(
            &json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":"L"}),
            &mut st
        )
        .is_empty());
        // Scoopable star, low fuel: no warning either -- you can refuel there.
        st.fuel_main = Some(8.0);
        assert!(from_event(
            &json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":"K"}),
            &mut st
        )
        .is_empty());
    }

    #[test]
    fn risky_target_warns_before_jump_and_does_not_repeat() {
        let mut st = CalloutState {
            fuel_capacity: Some(32.0),
            fuel_main: Some(8.0),
            ..Default::default()
        };
        let c = from_event(
            &json!({"timestamp":"t","event":"FSDTarget","Name":"Dry Rock","StarClass":"L","RemainingJumpsInRoute":1}),
            &mut st,
        );
        assert!(c
            .iter()
            .any(|x| x.speak && x.text.contains("Dry Rock is not scoopable")));
        let start = from_event(
            &json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":"L"}),
            &mut st,
        );
        assert!(
            !start.iter().any(|x| x.kind == "fuel"),
            "target warning must not repeat at charge"
        );
    }

    /// "AeBe" and "MS" start with scoopable letters but are a Herbig
    /// protostar and an S-type star: the callouts must agree with the
    /// router that they will not refuel you.
    #[test]
    fn aebe_and_ms_are_not_scoopable_anywhere() {
        for class in ["AeBe", "MS", "TTS"] {
            let mut st = CalloutState {
                fuel_capacity: Some(32.0),
                fuel_main: Some(8.0),
                ..Default::default()
            };
            let c = from_event(
                &json!({"timestamp":"t","event":"FSDTarget","Name":"Dry Rock","StarClass":class,"RemainingJumpsInRoute":1}),
                &mut st,
            );
            assert!(
                c.iter().any(|x| x.text.contains("not scoopable")),
                "{class}: {c:?}"
            );
            let mut st = CalloutState {
                fuel_capacity: Some(32.0),
                fuel_main: Some(8.0),
                ..Default::default()
            };
            let c = from_event(
                &json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":class}),
                &mut st,
            );
            assert!(c.iter().any(|x| x.kind == "fuel"), "{class}: {c:?}");
        }
    }

    #[test]
    fn supermassive_black_hole_is_a_hazard_at_jump() {
        let c = ev(
            json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":"SupermassiveBlackHole"}),
        );
        assert!(
            c.iter()
                .any(|x| x.kind == "hazard" && x.text.contains("black hole")),
            "{c:?}"
        );
    }

    #[test]
    fn hazardous_arrivals_are_called_regardless_of_fuel() {
        let c = ev(
            json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":"N"}),
        );
        assert_eq!(
            c[0].text,
            "Caution: jumping to a neutron star. Throttle down on arrival."
        );
        let d = ev(
            json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":"DA"}),
        );
        assert!(d[0].text.contains("white dwarf"));
        assert!(ev(
            json!({"timestamp":"t","event":"StartJump","JumpType":"Hyperspace","StarClass":"G"})
        )
        .is_empty());
        // Supercruise "jumps" are not hyperspace jumps.
        assert!(
            ev(json!({"timestamp":"t","event":"StartJump","JumpType":"Supercruise"})).is_empty()
        );
    }

    #[test]
    fn scans_and_attacks_on_us_are_spoken() {
        let c = ev(json!({"timestamp":"t","event":"Scanned","ScanType":"Cargo"}));
        assert_eq!(
            c[0].text,
            "Cargo scan detected. Someone is sizing up your hold."
        );
        assert!(c[0].speak && c[0].priority == 2);
        let a = ev(json!({"timestamp":"t","event":"UnderAttack","Target":"You"}));
        assert_eq!((a[0].text.as_str(), a[0].priority), ("Under attack.", 3));
        // Our fighter or SRV being attacked is not us.
        assert!(ev(json!({"timestamp":"t","event":"UnderAttack","Target":"Fighter"})).is_empty());
    }

    #[test]
    fn status_flags_are_edge_triggered() {
        let mut st = CalloutState::default();
        let low = json!({"timestamp":"t","Flags": 1<<19, "Fuel": {"FuelMain": 5.0}});
        assert_eq!(from_status(&low, &mut st).len(), 1);
        assert!(
            from_status(&low, &mut st).is_empty(),
            "same state again must not repeat"
        );
        let ok = json!({"timestamp":"t","Flags": 0});
        assert!(from_status(&ok, &mut st).is_empty());
        assert_eq!(
            from_status(&low, &mut st).len(),
            1,
            "fires again after recovering"
        );
        assert_eq!(st.fuel_main, Some(5.0));
    }

    #[test]
    fn greeting_uses_ship_name_over_hull_and_never_says_commander_twice() {
        let c = ev(
            json!({"timestamp":"t","event":"LoadGame","Commander":"Jameson","Ship":"Krait_MkII","Ship_Localised":"Krait Mk II","ShipName":"Kestrel","FuelCapacity":32.0,"FuelLevel":20.0}),
        );
        assert_eq!(
            c[0].text,
            "Welcome back, Commander Jameson. Kestrel systems online."
        );
        let anon =
            ev(json!({"timestamp":"t","event":"LoadGame","Ship":"sidewinder","ShipName":""}));
        assert_eq!(
            anon[0].text,
            "Welcome back, Commander. Sidewinder systems online."
        );
    }

    #[test]
    fn credits_are_spoken_at_a_sensible_precision() {
        assert_eq!(spoken_credits(600), "600 credits");
        assert_eq!(spoken_credits(45_000), "45 thousand credits");
        assert_eq!(spoken_credits(1_250_000), "1.2 million credits");
        assert_eq!(spoken_credits(2_100_000_000), "2.10 billion credits");
    }
}
