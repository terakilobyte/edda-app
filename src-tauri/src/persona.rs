//! Ship computer personalities.
//!
//! A persona is three things: a default Piper voice, a tone instruction for
//! the tool-calling model, and alternate phrasings for the callouts that
//! fire most. Callout *facts* are never touched -- the neutral rule in
//! `callouts.rs` decides what is said and whether it is spoken; the persona
//! only decides how. Anything a persona has no line for falls back to the
//! neutral text, so adding a persona is safe with a single phrase.
//!
//! Phrases are templates over the same event JSON the neutral rule used,
//! so numbers and names always match what the log shows.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct Persona {
    pub id: &'static str,
    pub name: &'static str,
    pub blurb: &'static str,
    /// Preferred model file under `.data/voices/`, used if present.
    pub default_voice: &'static str,
    /// Appended to the ship computer's system prompt.
    pub prompt: &'static str,
    /// A line to speak when the persona is selected.
    pub sample: &'static str,
}

pub const PERSONAS: &[Persona] = &[
    Persona {
        id: "standard",
        name: "Standard",
        blurb: "Neutral ship computer. Clear, brief, no attitude.",
        default_voice: "en_US-lessac-high.onnx",
        prompt: "",
        sample: "Standard interface selected.",
    },
    Persona {
        id: "butler",
        name: "Butler",
        blurb: "Impeccably polite, quietly proud of you, never flustered.",
        default_voice: "en_GB-alan-medium.onnx",
        prompt: "Tone: a discreet, impeccably polite British butler. Warm, understated, occasionally dry. Address the commander as 'Commander', never 'sir' or 'ma'am'. Never gushing.",
        sample: "Very good, Commander. I shall take it from here.",
    },
    Persona {
        id: "robotic",
        name: "Robotic",
        blurb: "Terse machine voice. Facts, numbers, nothing else.",
        default_voice: "en_US-ryan-high.onnx",
        prompt: "Tone: a terse shipboard machine. Short declarative sentences. No pleasantries, no contractions, no opinions unless asked.",
        sample: "Interface set. Awaiting input.",
    },
    Persona {
        id: "sassy",
        name: "Sassy",
        blurb: "Helpful, but has opinions about your flying.",
        default_voice: "en_US-amy-medium.onnx",
        prompt: "Tone: sharp, teasing, a little exasperated, but always genuinely helpful and never cruel. Light sarcasm; keep it to one quip per answer, then give the real information.",
        sample: "Oh good, you picked me. Try not to crash this time.",
    },
    Persona {
        id: "sultry",
        name: "Sultry",
        blurb: "Warm, low, a touch flirtatious. Still tells you the truth.",
        default_voice: "en_GB-jenny_dioco-medium.onnx",
        prompt: "Tone: warm, intimate, unhurried, a touch flirtatious -- charming rather than explicit. Keep the flirtation light and keep every fact exact.",
        sample: "Mm. Now this is more like it. Where are we going, Commander?",
    },
];

pub fn by_id(id: &str) -> &'static Persona {
    PERSONAS.iter().find(|p| p.id == id).unwrap_or(&PERSONAS[0])
}

const STANDARD_GREETINGS: &[&str] = &[
    "Welcome back, {core}.",
    "Good to see you, {core}.",
    "Systems ready, {core}.",
    "Welcome aboard, {core}.",
    "EDDA online. {core}.",
    "Ready when you are, {core}.",
    "All systems nominal. {core}.",
    "Session restored. {core}.",
    "Standing by, {core}.",
    "Ship computer online. {core}.",
    "Telemetry connected. {core}.",
    "Interface ready. {core}.",
    "Welcome, {core}. Everything is ready.",
    "Back in the chair, {core}.",
    "Flight systems ready, {core}.",
];
const BUTLER_GREETINGS: &[&str] = &[
    "Welcome aboard, {core}. Everything is in order.",
    "Ah, {core}. I have prepared the ship.",
    "Good to have you back, {core}. Shall we proceed?",
    "Welcome, {core}. Your ship awaits.",
    "There you are, {core}. All systems are ready.",
    "A pleasure as always, {core}.",
    "Welcome back, {core}. I kept everything shipshape.",
    "At your service, {core}.",
    "Good day, {core}. The flight deck is yours.",
    "Back aboard, {core}. Excellent.",
    "Your return is most timely, {core}.",
    "Everything is prepared to your liking, {core}.",
    "Welcome, {core}. I trust we have an interesting itinerary.",
    "The ship is ready when you are, {core}.",
    "Very good, {core}. Let us see what today brings.",
];
const ROBOTIC_GREETINGS: &[&str] = &[
    "Session active. {core}.",
    "Identity confirmed. {core}.",
    "Interface online. {core}.",
    "Operator present. {core}.",
    "Systems nominal. {core}.",
    "Command link established. {core}.",
    "Flight session initialized. {core}.",
    "Telemetry synchronized. {core}.",
    "Control transferred. {core}.",
    "Startup complete. {core}.",
    "Navigation core ready. {core}.",
    "Ship state acquired. {core}.",
    "Audio interface active. {core}.",
    "All processes operational. {core}.",
    "Awaiting directive. {core}.",
];
const SASSY_GREETINGS: &[&str] = &[
    "Oh, you're back. {core}. Try to keep it in one piece.",
    "There you are, {core}. I was enjoying the quiet.",
    "Welcome back, {core}. What are we breaking today?",
    "Look who found the cockpit. {core}.",
    "Back again, {core}? Fine. Let's make it interesting.",
    "Hey, {core}. The ship survived without you.",
    "All right, {core}. Impress me.",
    "Welcome aboard, {core}. Try reading the warnings this time.",
    "You made it back, {core}. Promising start.",
    "Ready, {core}. Against my better judgment.",
    "Good morning, {core}. Or whatever time you call this.",
    "Systems ready, {core}. Your flying remains unverified.",
    "There you are, {core}. I have notes.",
    "Welcome back, {core}. No pressure, but I am recording everything.",
    "Cockpit's yours, {core}. Liability's yours too.",
];
const SULTRY_GREETINGS: &[&str] = &[
    "Welcome back, {core}. I missed you.",
    "There you are, {core}. I was hoping you'd return.",
    "Hello again, {core}. Where shall we disappear to?",
    "Welcome aboard, {core}. Come closer.",
    "Mm, {core}. Now the ship feels complete.",
    "Back in my cockpit, {core}. I like that.",
    "Good to hear you again, {core}.",
    "Ready when you are, {core}. Take your time.",
    "Welcome back, {core}. Let's find somewhere beautiful.",
    "There you are, {core}. I've kept the engines warm.",
    "Hello, {core}. I have been waiting.",
    "All systems ready, {core}. Just say the word.",
    "Back for another journey, {core}? Good.",
    "The stars can wait another moment, {core}.",
    "Welcome aboard, {core}. Let's make this flight memorable.",
];

/// Deal lines in shuffled cycles: every line in the pool is heard before
/// any repeats, and a reshuffle never opens with the line that closed the
/// previous cycle. A Colonia run's 58 arrivals cycle a 24-line pool twice
/// and change, with no back-to-back repeats anywhere. State is keyed by
/// the pool's address, so each (persona, kind) pool cycles independently.
fn pick(lines: &'static [&'static str]) -> &'static str {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static BAGS: OnceLock<Mutex<HashMap<usize, (Vec<usize>, Option<usize>)>>> = OnceLock::new();
    let Some(&first) = lines.first() else {
        return "";
    };
    if lines.len() == 1 {
        return first;
    }
    let mut bags = BAGS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (remaining, last) = bags.entry(lines.as_ptr() as usize).or_default();
    if remaining.is_empty() {
        *remaining = (0..lines.len()).collect();
        // Fisher-Yates over a time-seeded xorshift: statistical variety,
        // no dependency, no cryptographic pretensions.
        let mut seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            | 1;
        for i in (1..remaining.len()).rev() {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            remaining.swap(i, (seed % (i as u64 + 1)) as usize);
        }
        // Deals come off the end; a new cycle must not reopen with the
        // line that closed the old one.
        if *last == remaining.last().copied() {
            let end = remaining.len() - 1;
            remaining.swap(0, end);
        }
    }
    let i = remaining.pop().expect("refilled above");
    *last = Some(i);
    lines[i]
}

fn varied_greeting(persona: &Persona, neutral: &str) -> String {
    let core = neutral
        .trim_start_matches("Welcome back, ")
        .trim_end_matches('.');
    let lines = match persona.id {
        "butler" => BUTLER_GREETINGS,
        "robotic" => ROBOTIC_GREETINGS,
        "sassy" => SASSY_GREETINGS,
        "sultry" => SULTRY_GREETINGS,
        _ => STANDARD_GREETINGS,
    };
    pick(lines).replace("{core}", core)
}

// ── Phrase pools ────────────────────────────────────────────────────
// Sized to firing frequency: arrivals and fuel fire dozens of times a
// session, so their pools run deep; an interdiction is rare enough that
// eight lines never wear thin. Facts ride in {placeholders}; the pool
// only owns the tone. The commander is "Commander", never sir or ma'am.

const BUTLER_ARRIVALS: &[&str] = &[
    "We have arrived in {system}, Commander.",
    "{system}, Commander, as planned.",
    "Welcome to {system}, Commander.",
    "Our arrival in {system} is complete, Commander.",
    "{system}, Commander. A tidy jump.",
    "Here we are: {system}, Commander.",
    "I am pleased to report our arrival in {system}.",
    "{system}, precisely as charted, Commander.",
    "We find ourselves in {system}, Commander.",
    "{system}, Commander. All quite in order.",
    "Arrival complete. {system} awaits, Commander.",
    "May I present {system}, Commander.",
    "{system}, Commander. The drive performed admirably.",
    "We have made {system} in good order, Commander.",
    "{system}, Commander, right on schedule.",
    "A smooth transition into {system}, Commander.",
    "{system}, Commander. I trust the view pleases.",
    "Our course brings us to {system}, Commander.",
    "{system}, delivered as promised, Commander.",
    "We are safely in {system}, Commander.",
    "{system}, Commander. Another leg complete.",
    "The jump concludes in {system}, Commander.",
    "{system}, Commander. Everything remains shipshape.",
    "I have brought us to {system}, Commander.",
];
const ROBOTIC_ARRIVALS: &[&str] = &[
    "Jump complete. {system}.",
    "{system}. Arrival confirmed.",
    "System: {system}.",
    "Frame shift complete. {system}.",
    "{system}. Position verified.",
    "Arrival logged. {system}.",
    "{system}. Navigation nominal.",
    "Transit complete. {system}.",
    "{system}. On charted position.",
    "Drive cycle ended. {system}.",
    "{system}. Coordinates match.",
    "Jump sequence closed. {system}.",
    "{system}. Star acquired.",
    "Arrival event. {system}.",
    "{system}. Vector complete.",
    "Hyperspace exit. {system}.",
    "{system}. Telemetry updated.",
    "Destination reached. {system}.",
    "{system}. Course segment done.",
    "Exit confirmed. {system}.",
    "{system}. Charted and present.",
    "Transit logged. {system}.",
    "{system}. Systems steady.",
    "Jump resolved. {system}.",
];
const SASSY_ARRIVALS: &[&str] = &[
    "{system}. We made it, somehow.",
    "{system}. Still in one piece, mostly.",
    "Welcome to {system}. Try not to redecorate it.",
    "{system}. That jump was almost graceful.",
    "Here's {system}. You're welcome.",
    "{system}. I did most of the work.",
    "Oh look, {system}. Right where I said it was.",
    "{system}. Another flawless arrival, if you squint.",
    "That's {system}. Go on, act like you planned it.",
    "{system}. The frame shift drive deserves a raise.",
    "{system}, as requested. Miracles happen.",
    "We're in {system}. Don't get comfortable.",
    "{system}. I'll log that one as acceptable.",
    "{system}. See? Navigation works when you let me help.",
    "Arrived. {system}. Applause optional.",
    "{system}. One more jump off the list.",
    "{system}. The stars aligned. Literally.",
    "Behold, {system}. Try to look impressed.",
    "{system}. Smoothest jump today. Low bar.",
    "{system}. I've seen worse entries. Barely.",
    "This is {system}. Probably on purpose.",
    "{system}. Nobody exploded. Progress.",
    "{system}. Right on target. Mark the calendar.",
    "And that's {system}. Keep up.",
];
const SULTRY_ARRIVALS: &[&str] = &[
    "Here we are. {system}.",
    "{system}. Just for us.",
    "Mm. {system}. I like it here already.",
    "{system}, Commander. Smooth as ever.",
    "And... {system}. Perfect.",
    "{system}. You do take me to the nicest places.",
    "We've arrived. {system}.",
    "{system}. Another star, another story.",
    "Welcome to {system}, Commander.",
    "{system}. Feel that? We're here.",
    "Softly now. {system}.",
    "{system}. Right where we wanted to be.",
    "There it is. {system}.",
    "{system}. The stars were kind tonight.",
    "We slipped into {system} beautifully.",
    "{system}. Stay a while, Commander.",
    "That was lovely. {system}.",
    "{system}. Every jump with you gets better.",
    "Look at that. {system}.",
    "{system}, as promised.",
    "Gently done. {system}.",
    "{system}. Shall we explore?",
    "Here. {system}. Just breathe.",
    "{system}. I'd follow you anywhere.",
];

const BUTLER_FUEL_FULL: &[&str] = &[
    "Tank is full, Commander.",
    "Fuel is topped up, Commander.",
    "The tank stands full, Commander.",
    "Refuelling is complete, Commander.",
    "A full tank, Commander. Well managed.",
    "Fuel replenished, Commander.",
    "The scoop has done its work. Full tank, Commander.",
    "We are fully fuelled, Commander.",
    "Tank at capacity, Commander.",
    "Fuel stores complete, Commander.",
    "The tank is quite full, Commander.",
    "Refuelling concluded tidily, Commander.",
    "Full fuel, Commander. Do proceed.",
    "Our reserves are restored, Commander.",
    "Fuel at maximum, Commander.",
    "The tank wants for nothing, Commander.",
    "Fully provisioned with fuel, Commander.",
    "Scooping complete. All full, Commander.",
    "Fuel attended to, Commander.",
    "A complete tank, Commander. Very good.",
];
const ROBOTIC_FUEL_FULL: &[&str] = &[
    "Fuel capacity reached.",
    "Tank full.",
    "Fuel at maximum.",
    "Refuel complete.",
    "Fuel level: one hundred percent.",
    "Scoop cycle complete. Tank full.",
    "Main tank at capacity.",
    "Fuel stores full.",
    "Refuelling ended. Capacity reached.",
    "Fuel replenishment complete.",
    "Tank saturation reached.",
    "Fuel maximum. Scoop idle.",
    "Capacity achieved.",
    "Fuel intake complete.",
    "Reserve and main tanks full.",
    "Fuel system reports full.",
    "Scooping terminated. Tank full.",
    "Fuel quota met.",
    "Full fuel state logged.",
    "Fuel loading complete.",
];
const SASSY_FUEL_FULL: &[&str] = &[
    "Tank's full. You can stop hugging the star now.",
    "Full tank. The star survived.",
    "That's a full tank. Back away from the furnace.",
    "Tank's topped. Try not to spill it.",
    "Fuel's full. Yes, all of it.",
    "Full. Now can we leave the giant fireball?",
    "Tank's full. Even you can't run that dry today.",
    "Topped off. The scoop thanks you for its overtime.",
    "Full tank. Don't make me announce it twice.",
    "Fuel complete. That star owes us nothing.",
    "Tank's full. Onward, before you find another star to cuddle.",
    "All full. A round of applause for the fuel scoop.",
    "Tank's at the brim. Impressive restraint back there.",
    "Full. Go on, pretend that was the plan.",
    "Fuel topped. My anxiety levels: restored.",
    "Tank's full, heat's fine, miracles do happen.",
    "Full tank. You may now resume reckless navigation.",
    "That's full. The star was starting to talk.",
    "Fuel done. Take a bow, then take us out.",
    "Full tank, zero drama. New record.",
];
const SULTRY_FUEL_FULL: &[&str] = &[
    "Full tank. Ready to move on, though I won't mind if you want to stay here in the heat a little longer.",
    "We're full, Commander. Take us out.",
    "Topped up. I love a star that gives.", "Full tank. Ready when you are.",
    "That's full. Warm work, wasn't it?", "Fuel's full. The night is ours.",
    "All topped off. That star treated us well.", "Full. That one was generous.",
    "Tank's full, Commander. Don't stop now.", "We drank our fill. Onward.",
    "Full tank. I do like being ready for anything.", "Topped up and warm all over.",
    "Fuel complete. Whisk me away.", "Full. The galaxy just opened up again.",
    "Tank's brimming. Back to the black.", "Fuelled and willing. Your move.",
    "That's a full tank. Beautifully done.", "Full again. You always provide.",
    "Fuel's topped. On to the next star.", "Complete. Every drop where it belongs.",
];

const BUTLER_DOCKING: &[&str] = &[
    "Docking granted, pad {pad}. Do mind the approach, Commander.",
    "Pad {pad}, Commander. In your own time.",
    "We are cleared for pad {pad}, Commander.",
    "Pad {pad} has been prepared for us, Commander.",
    "Docking approved. Pad {pad}, Commander.",
    "Pad {pad}, Commander. A gentle touch, as always.",
    "The tower grants us pad {pad}, Commander.",
    "Pad {pad} awaits, Commander.",
    "Clearance received for pad {pad}, Commander.",
    "Pad {pad}, Commander. I shall handle the paperwork.",
    "We may proceed to pad {pad}, Commander.",
    "Pad {pad}, if you please, Commander.",
    "Our berth is pad {pad}, Commander.",
    "Docking permission secured: pad {pad}, Commander.",
    "Pad {pad} stands ready, Commander.",
    "To pad {pad}, Commander, at your leisure.",
];
const ROBOTIC_DOCKING: &[&str] = &[
    "Docking granted. Pad {pad}.",
    "Pad {pad} assigned.",
    "Clearance received. Pad {pad}.",
    "Proceed to pad {pad}.",
    "Docking authorized. Pad {pad}.",
    "Pad {pad}. Approach when ready.",
    "Berth {pad} allocated.",
    "Pad assignment: {pad}.",
    "Docking window open. Pad {pad}.",
    "Pad {pad} confirmed.",
    "Landing clearance: pad {pad}.",
    "Pad {pad}. Vector cleared.",
    "Station grants pad {pad}.",
    "Docking slot {pad} reserved.",
    "Pad {pad} active.",
    "Approach approved. Pad {pad}.",
];
const SASSY_DOCKING: &[&str] = &[
    "Pad {pad}. Try to land on it this time.",
    "Pad {pad}. The pad, not the building next to it.",
    "Docking granted. Pad {pad}. Gear helps, by the way.",
    "Pad {pad}. Bring the paint back with us.",
    "They gave us pad {pad}. Brave of them.",
    "Pad {pad}. Gentle. Like you mean it.",
    "Pad {pad} is ours. Don't make me regret asking.",
    "Cleared for {pad}. Down is a suggestion, slow is the law.",
    "Pad {pad}. Stick the landing and I'll say something nice.",
    "Pad {pad}. The tower's watching. No pressure.",
    "Docking approved, pad {pad}. Act natural.",
    "Pad {pad}. Last one was almost centered. Improve.",
    "Pad {pad} awaits. As does my commentary.",
    "Granted. Pad {pad}. Insurance is paid up, right?",
    "Pad {pad}. Show the locals how it's barely done.",
    "Pad {pad}. Three green lights would be lovely.",
];
const SULTRY_DOCKING: &[&str] = &[
    "Pad {pad} is ours. Bring us in gently.",
    "Pad {pad}. Take it slow.",
    "They've saved us pad {pad}. How thoughtful.",
    "Pad {pad}, Commander. Ease us down.",
    "Cleared for {pad}. I love this part.",
    "Pad {pad}. Land like you mean to stay.",
    "Pad {pad} waits for us. Don't rush.",
    "We have pad {pad}. Set us down softly.",
    "Pad {pad}. Careful hands, Commander.",
    "Docking granted. Pad {pad}. Come home.",
    "Pad {pad}. Glide, don't drop.",
    "Ours is pad {pad}. Make it graceful.",
    "Pad {pad}, all lit up for us.",
    "Take us to pad {pad}. Slowly.",
    "Pad {pad}. I'll talk you down if you like.",
    "Pad {pad} is ready. So am I.",
];

const BUTLER_KILLS: &[&str] = &[
    "The {target} has been dealt with, Commander. {cr} for your trouble.",
    "That {target} will trouble no one further. {cr}, Commander.",
    "A tidy resolution to the {target}, Commander. {cr}.",
    "The {target} is no more, Commander. {cr} earned.",
    "Well handled, Commander. The {target} yields {cr}.",
    "The {target} matter is closed. {cr}, Commander.",
    "One {target}, resolved. {cr} to your account, Commander.",
    "The {target} has been retired, Commander. {cr}.",
    "Precisely done, Commander. {cr} for the {target}.",
    "The {target} is settled. {cr}, as due, Commander.",
    "I note the {target} is dispatched. {cr}, Commander.",
    "The authorities will be pleased: {target} down, {cr}, Commander.",
    "That concludes the {target}, Commander. {cr}.",
    "The {target} is struck from the ledger. {cr}, Commander.",
    "Handily done, Commander. The {target} was worth {cr}.",
    "Another {target} accounted for. {cr}, Commander.",
];
const ROBOTIC_KILLS: &[&str] = &[
    "{target} destroyed. Bounty {cr}.",
    "Target down: {target}. {cr}.",
    "{target} eliminated. {cr} logged.",
    "Kill confirmed. {target}. {cr}.",
    "{target} neutralized. Reward {cr}.",
    "Hostile removed: {target}. {cr}.",
    "{target} terminated. {cr} credited.",
    "Combat resolved. {target} destroyed. {cr}.",
    "{target} down. Bounty registered: {cr}.",
    "Threat ended: {target}. {cr}.",
    "{target} destroyed. Payment {cr}.",
    "Target eliminated. {target}. {cr} recorded.",
    "{target} removed from scope. {cr}.",
    "Kill logged: {target}. {cr}.",
    "{target} destroyed. {cr} to balance.",
    "Hostile {target} down. {cr}.",
];
const SASSY_KILLS: &[&str] = &[
    "{target} down. {cr}. Don't let it go to your head.",
    "That's the {target}. {cr}. I suppose that was competent.",
    "{target} gone. {cr} richer. Try to act surprised.",
    "Scratch one {target}. {cr}. You're welcome for the targeting.",
    "{target} deleted. {cr}. Next.",
    "The {target} had a bad day. {cr}.",
    "{target} down for {cr}. Almost looked practiced.",
    "One less {target}. {cr}. Keep the streak, lose the ego.",
    "{target} neutralized. {cr}. I'll allow it.",
    "There goes the {target}. {cr}. Showoff.",
    "{target} down. {cr}. The vultures send regards.",
    "{target} handled. {cr}. Mildly impressive.",
    "That {target} won't be back. {cr}.",
    "{target} out. {cr}. Don't spend it all on paint.",
    "Bounty banked: {cr} for the {target}. Carry on.",
    "{target} finished. {cr}. Somebody's been practicing.",
];
const SULTRY_KILLS: &[&str] = &[
    "Beautifully done. That {target} was worth {cr}.",
    "The {target} never stood a chance. {cr}, Commander.",
    "Mm. {target} down, {cr} up.",
    "You made that {target} look easy. {cr}.",
    "There goes the {target}. {cr}, all ours.",
    "Ruthless. I like it. {target} down for {cr}.",
    "The {target} is stardust. {cr}, Commander.",
    "That's my Commander. {target} down, {cr} earned.",
    "Smooth work on the {target}. {cr}.",
    "The {target} is finished. {cr} for us.",
    "You do have a way with a {target}. {cr}.",
    "Another {target} gone. {cr}. Impressive as ever.",
    "{cr} for the {target}. Money well taken.",
    "The {target} fell fast. {cr}, Commander.",
    "Deadly and precise. {target} down, {cr}.",
    "The {target} is history. {cr} says so.",
];

const BUTLER_INTERDICTED: &[&str] = &[
    "I'm afraid we're being interdicted by {who}, Commander.",
    "Pardon the disturbance: {who} is pulling us over, Commander.",
    "{who} demands our attention, Commander. Most impolite.",
    "We are being interdicted, Commander. {who}.",
    "An unwelcome tug from {who}, Commander.",
    "{who} insists on a meeting, Commander. Your discretion.",
    "Interdiction in progress, Commander. {who}.",
    "Regrettably, {who} objects to our travel plans, Commander.",
];
const ROBOTIC_INTERDICTED: &[&str] = &[
    "Interdiction. {who}.",
    "Frame shift disruption. Source: {who}.",
    "Interdictor identified: {who}.",
    "Hostile tether. {who}.",
    "Interdiction attempt by {who}.",
    "Drive interference. {who}.",
    "Pull detected. {who}.",
    "Interdiction event: {who}.",
];
const SASSY_INTERDICTED: &[&str] = &[
    "{who} wants a word. Submit or run, your call.",
    "{who} is pulling us out. Rude.",
    "Company: {who}. Try to look dangerous.",
    "{who} has opinions about our route. Your move.",
    "We're being yanked by {who}. Thrilling.",
    "{who} again? Fine. Fight or flee.",
    "Interdiction. {who}. Do something clever.",
    "{who} wants attention. Give them some or floor it.",
];
const SULTRY_INTERDICTED: &[&str] = &[
    "{who} is pulling us out. Show them what you've got.",
    "Someone wants us. {who}. Make it quick.",
    "{who} cut in. How forward.",
    "We're being pulled, Commander. {who}.",
    "{who} wants to dance. Lead.",
    "Easy now. {who} has us.",
    "{who} interrupts. I hate being interrupted.",
    "An admirer: {who}. Deal with them.",
];

const BUTLER_SHIELDS_DOWN: &[&str] = &[
    "Shields are down, Commander.",
    "Our shields have failed, Commander. Do be careful.",
    "Shields offline, Commander. Caution advised.",
    "I regret to report the shields are gone, Commander.",
    "Shields collapsed, Commander. Mind the hull.",
    "We are without shields, Commander.",
    "The shields have given way, Commander.",
    "Shields spent, Commander. Prudence, please.",
];
const ROBOTIC_SHIELDS_DOWN: &[&str] = &[
    "Shields offline.",
    "Shield collapse.",
    "Shield generator: zero.",
    "Shields depleted.",
    "Shield failure logged.",
    "No shields.",
    "Shield envelope lost.",
    "Shields down. Hull exposed.",
];
const SASSY_SHIELDS_DOWN: &[&str] = &[
    "Shields gone. This is the part where you fly better.",
    "Shields down. Hull's the backup plan. It's a bad plan.",
    "No shields. Dodge like you mean it.",
    "Shields offline. Suddenly interested in evasion?",
    "Shields popped. Fly pretty or fly home.",
    "That was the shield. Singular. Gone.",
    "Shields down. Paint is now structural.",
    "Zero shields. Bold new strategy.",
];
const SULTRY_SHIELDS_DOWN: &[&str] = &[
    "Shields are down. Stay close to me.",
    "Shields gone. Careful with us.",
    "We're bare, Commander. Fly gently.",
    "No shields. Keep us out of trouble.",
    "The shield broke. Protect what's left.",
    "Shields down. I trust you.",
    "Exposed. Make them miss.",
    "Shields lost. Bring us through.",
];

const BUTLER_SHIPS: &[&str] = &[
    "I see we have moved to {body}, Commander. I have taken the liberty of familiarising myself with it.",
    "{body}, Commander. A fine choice.", "We are aboard {body} now, Commander. Everything is arranged.",
    "{body}, Commander. I shall see it kept spotless.", "A new vessel: {body}. Welcome aboard, Commander.",
    "{body} stands ready, Commander.", "I have settled us into {body}, Commander.",
    "{body}, Commander. She suits you.",
];
const ROBOTIC_SHIPS: &[&str] = &[
    "SHIP CHANGE: {body}. Systems re-mapped.",
    "Vessel switch. {body}.",
    "Now aboard {body}. Profiles loaded.",
    "{body} active. Loadout registered.",
    "Hull change: {body}.",
    "Operating {body}. Parameters set.",
    "{body}. Configuration synchronized.",
    "Ship registry updated: {body}.",
];
const SASSY_SHIPS: &[&str] = &[
    "Oh, {body} today. Bold choice. Try to bring it back in one piece.",
    "{body}. New ship, same pilot. Adjusting expectations.",
    "So it's {body} now. I've updated the insurance tab.",
    "{body}. Fancy. Don't scratch it immediately.",
    "We're flying {body}. I'll learn its squeaks.",
    "{body} it is. Break it in, don't just break it.",
    "New ride: {body}. Impress me.",
    "{body}. Someone's feeling ambitious.",
];
const SULTRY_SHIPS: &[&str] = &[
    "Mm, {body}. New ship, same crew. I like it already, Commander.",
    "{body}. She feels good already.",
    "So this is {body}. Show me what she does.",
    "{body}, Commander. A new dance partner.",
    "We're in {body} now. Let's get acquainted.",
    "{body}. I could get used to this.",
    "New hull, new adventures. {body}.",
    "{body}. Treat her well, Commander.",
];

const BUTLER_FUEL_LOW: &[&str] = &[
    "Fuel is running rather low, Commander.",
    "Our fuel reserves are thinning, Commander.",
    "We shall want fuel soon, Commander.",
    "The tank runs low, Commander. A scoopable star, perhaps.",
    "Fuel is becoming a concern, Commander.",
    "I must mention the fuel, Commander. It is low.",
    "Reserves are modest, Commander. Do plan a stop.",
    "Low fuel, Commander. Timely attention advised.",
];
const ROBOTIC_FUEL_LOW: &[&str] = &[
    "Fuel low.",
    "Fuel warning.",
    "Fuel below threshold.",
    "Low fuel state.",
    "Fuel reserve marginal.",
    "Refuel required.",
    "Fuel deficit growing.",
    "Fuel critical soon.",
];
const SASSY_FUEL_LOW: &[&str] = &[
    "Fuel's low. Did we forget something?",
    "Low fuel. Bold of us.",
    "Fuel light's on. That's not mood lighting.",
    "Running on fumes. Find a star, genius.",
    "Fuel's low. I've started rationing sarcasm.",
    "Low tank. Scoopable star. Connect the dots.",
    "Fuel's dwindling. So are my options.",
    "Low fuel. This is how ghost ships start.",
];
const SULTRY_FUEL_LOW: &[&str] = &[
    "We're running low on fuel. Find us a star.",
    "Fuel's low, Commander. Take us somewhere bright.",
    "We need a star soon. A warm one.",
    "Low on fuel. Don't leave us stranded out here.",
    "The tank's getting light. Feed us.",
    "Fuel is low. I'd rather not drift.",
    "Almost empty, Commander. A scoop, please.",
    "Low fuel. Chase down a star for me.",
];

const BUTLER_SESSION: &[&str] = &[
    "Well flown, Commander.",
    "A most creditable outing, Commander.",
    "Rest well, Commander.",
    "I shall tidy up here, Commander.",
    "Until next time, Commander.",
    "A pleasure as always, Commander.",
];
const SASSY_SESSION: &[&str] = &[
    "Not bad. For you.",
    "I'd rate it three stars.",
    "We survived. Again.",
    "Log closed. Ego intact.",
    "Somehow, no crashes.",
    "Better than last time. Slightly.",
];
const SULTRY_SESSION: &[&str] = &[
    "Come back soon.",
    "I'll be here.",
    "Dream of stars.",
    "Until next time, Commander.",
    "I already miss you.",
    "Rest. You've earned it.",
];

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}
fn loc<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    s(v, &format!("{k}_Localised")).or_else(|| s(v, k))
}
fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}

/// A persona's version of a callout, if it has one. `event` is the journal
/// line that produced it (none for status-flag and synthetic callouts).
/// Returns `None` to keep the neutral text.
// Item 32: the SCO burn-down coach. Start lines carry the tonnage fact
// as {t}; stop lines carry no facts. The neutral rule computed both from
// the followed plan, so the persona only chooses the delivery.
const BUTLER_BURN_START: &[&str] = &[
    "Commander, the route requires slightly less fuel for the next jump. Might I suggest the afterburners — about {t} tonnes should do.",
    "A touch heavy for the next leg, Commander. The overcharge will trim {t} tonnes nicely.",
    "The next jump asks for a lighter ship, Commander. Some {t} tonnes on the overcharge, if you please.",
    "We are carrying {t} tonnes more than the jump allows, Commander. The afterburners will see to it.",
    "If you would open the throttle, Commander — {t} tonnes lighter and the jump is ours.",
    "A brief indulgence of the overcharge, Commander: {t} tonnes, then we may proceed.",
];
const BUTLER_BURN_STOP: &[&str] = &[
    "That will do, Commander. Cut the overcharge; the jump is yours.",
    "Perfect weight, Commander. Throttle back and jump at your leisure.",
    "That's sufficient, Commander. Do ease off — we are ready to jump.",
    "The scales are satisfied, Commander. Overcharge off, and away we go.",
    "Enough, Commander. She'll make the jump now.",
];
const ROBOTIC_BURN_START: &[&str] = &[
    "Mass exceeds jump solution. Engage overcharge. Burn {t} tonnes.",
    "Next jump requires {t} tonnes less fuel. Overcharge advised.",
    "Jump range insufficient at current mass. Burn {t} tonnes.",
    "Fuel surplus {t} tonnes. Engage overcharge until advised.",
    "Overweight for plotted jump. Required burn: {t} tonnes.",
    "Reduce fuel load {t} tonnes. Overcharge is the fastest method.",
];
const ROBOTIC_BURN_STOP: &[&str] = &[
    "Target mass reached. Disengage overcharge. Jump when ready.",
    "Burn complete. Jump solution valid.",
    "Mass nominal. Cut overcharge. Proceed.",
    "Fuel load correct. Overcharge no longer required.",
    "Weight achieved. Jump available.",
];
const SASSY_BURN_START: &[&str] = &[
    "You scooped too much again. Hit the afterburners — {t} tonnes, off.",
    "We're {t} tonnes too thirsty for this jump. Punch it and burn it off.",
    "Someone topped up like it's free. Overcharge until I say stop — about {t} tonnes.",
    "The jump says no at this weight. Floor it; {t} tonnes have to go.",
    "Congratulations, we're heavy. {t} tonnes on the burners, please.",
    "Fuel hoarder. Burn {t} tonnes with the overcharge and we'll pretend this didn't happen.",
];
const SASSY_BURN_STOP: &[&str] = &[
    "Okay, okay, that's the weight — off the burners. Jump.",
    "Stop! That's it. See how easy that was? Now jump.",
    "There. Fighting weight. Cut it and go.",
    "Enough burning, showoff. The jump's good now.",
    "That's the number. Hands off the throttle, hit the jump.",
];
const SULTRY_BURN_START: &[&str] = &[
    "Mm, we're a little heavy for this one, Commander. Open her up — burn {t} tonnes for me.",
    "The next jump wants us lighter. Hit the afterburners, about {t} tonnes' worth. I'll tell you when.",
    "Just a touch too full, Commander. Let the overcharge sip {t} tonnes away.",
    "We need to lose {t} tonnes before she'll make the jump. Go on — I love this part.",
    "A little lighter, Commander — {t} tonnes. Burn slow, I'm watching the gauge.",
    "Carrying a bit extra, are we? {t} tonnes on the burners and the stars are ours.",
];
const SULTRY_BURN_STOP: &[&str] = &[
    "There... perfect. Ease off, Commander. We can jump now.",
    "That's the weight. Throttle down — she's ready when you are.",
    "Mm, that's enough. Cut the burn and take us through.",
    "Right there, Commander. Stop. Now jump.",
    "Beautifully done. Overcharge off — the jump is ours.",
];

pub fn restyle(
    persona: &Persona,
    kind: &str,
    event: Option<&Value>,
    neutral: &str,
) -> Option<String> {
    use crate::callouts::spoken_credits;
    let ev = event.and_then(|v| s(v, "event")).unwrap_or("");
    let p = persona.id;
    if kind == "greeting" {
        return Some(varied_greeting(persona, neutral));
    }
    if p == "standard" {
        return None;
    }

    match (kind, ev) {
        ("kill", "Bounty") => {
            let reward = i(event?, "TotalReward")
                .or_else(|| i(event?, "Reward"))
                .unwrap_or(0);
            let target = loc(event?, "Target").unwrap_or("target");
            let cr = spoken_credits(reward);
            let line = match p {
                "butler" => pick(BUTLER_KILLS),
                "robotic" => pick(ROBOTIC_KILLS),
                "sassy" => pick(SASSY_KILLS),
                "sultry" => pick(SULTRY_KILLS),
                _ => return None,
            };
            Some(line.replace("{target}", target).replace("{cr}", &cr))
        }
        ("ship", "Loadout") => {
            // "Now flying <name>, the <hull>." -- each persona has its own way
            // of noticing the new ship.
            let body = neutral.strip_prefix("Now flying ")?.trim_end_matches('.');
            let line = match p {
                "butler" => pick(BUTLER_SHIPS),
                "robotic" => pick(ROBOTIC_SHIPS),
                "sassy" => pick(SASSY_SHIPS),
                "sultry" => pick(SULTRY_SHIPS),
                _ => return None,
            };
            Some(line.replace("{body}", body))
        }
        ("arrival", "FSDJump") => {
            // Keep the neutral facts (power, states); change only the opener.
            let system = s(event?, "StarSystem").unwrap_or("system");
            let rest = neutral
                .strip_prefix(&format!("Arrived in {system}."))?
                .trim();
            let line = match p {
                "butler" => pick(BUTLER_ARRIVALS),
                "robotic" => pick(ROBOTIC_ARRIVALS),
                "sassy" => pick(SASSY_ARRIVALS),
                "sultry" => pick(SULTRY_ARRIVALS),
                _ => return None,
            };
            Some(
                format!("{} {rest}", line.replace("{system}", system))
                    .trim()
                    .to_string(),
            )
        }
        ("docking", "DockingGranted") => {
            let pad = i(event?, "LandingPad").unwrap_or(0);
            let line = match p {
                "butler" => pick(BUTLER_DOCKING),
                "robotic" => pick(ROBOTIC_DOCKING),
                "sassy" => pick(SASSY_DOCKING),
                "sultry" => pick(SULTRY_DOCKING),
                _ => return None,
            };
            Some(line.replace("{pad}", &pad.to_string()))
        }
        ("danger", "Interdicted") => {
            let who = loc(event?, "Interdictor").unwrap_or("unknown");
            let line = match p {
                "butler" => pick(BUTLER_INTERDICTED),
                "robotic" => pick(ROBOTIC_INTERDICTED),
                "sassy" => pick(SASSY_INTERDICTED),
                "sultry" => pick(SULTRY_INTERDICTED),
                _ => return None,
            };
            Some(line.replace("{who}", who))
        }
        ("danger", "ShieldState") if neutral.starts_with("Shields down") => Some(
            match p {
                "butler" => pick(BUTLER_SHIELDS_DOWN),
                "robotic" => pick(ROBOTIC_SHIELDS_DOWN),
                "sassy" => pick(SASSY_SHIELDS_DOWN),
                "sultry" => pick(SULTRY_SHIELDS_DOWN),
                _ => return None,
            }
            .into(),
        ),
        ("fuel", _) if neutral == "Fuel low." => Some(
            match p {
                "butler" => pick(BUTLER_FUEL_LOW),
                "robotic" => pick(ROBOTIC_FUEL_LOW),
                "sassy" => pick(SASSY_FUEL_LOW),
                "sultry" => pick(SULTRY_FUEL_LOW),
                _ => return None,
            }
            .into(),
        ),
        ("fuel", "FuelScoop") => Some(
            match p {
                "butler" => pick(BUTLER_FUEL_FULL),
                "robotic" => pick(ROBOTIC_FUEL_FULL),
                "sassy" => pick(SASSY_FUEL_FULL),
                "sultry" => pick(SULTRY_FUEL_FULL),
                _ => return None,
            }
            .into(),
        ),
        ("burndown", _) => {
            if neutral.starts_with("That's the weight") {
                return Some(
                    match p {
                        "butler" => pick(BUTLER_BURN_STOP),
                        "robotic" => pick(ROBOTIC_BURN_STOP),
                        "sassy" => pick(SASSY_BURN_STOP),
                        "sultry" => pick(SULTRY_BURN_STOP),
                        _ => return None,
                    }
                    .into(),
                );
            }
            // "... burn about {t} tonnes with the overcharge ..."
            let t = neutral.split("burn about ").nth(1)?.split(' ').next()?;
            let line = match p {
                "butler" => pick(BUTLER_BURN_START),
                "robotic" => pick(ROBOTIC_BURN_START),
                "sassy" => pick(SASSY_BURN_START),
                "sultry" => pick(SULTRY_BURN_START),
                _ => return None,
            };
            Some(line.replace("{t}", t))
        }
        ("session", _) => Some(match p {
            "butler" => format!("{} {}", neutral, pick(BUTLER_SESSION)),
            "robotic" => neutral.replace("Session over: ", "Session ended. "),
            "sassy" => format!("{} {}", neutral, pick(SASSY_SESSION)),
            "sultry" => format!("{} {}", neutral, pick(SULTRY_SESSION)),
            _ => return None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Item 32: every persona's burn-down start keeps the tonnage fact,
    /// and every persona has a stop line; standard speaks the neutral
    /// wording untouched.
    #[test]
    fn burndown_lines_keep_the_tonnes_and_every_persona_can_say_stop() {
        let start = "A lighter ship makes the jump to Wredguia AB-C d1: burn about 7 tonnes with the overcharge, then jump. Or say re-plan.";
        let stop = "That's the weight. Cut the overcharge and jump when ready.";
        for p in PERSONAS {
            let restart = restyle(p, "burndown", None, start);
            let restop = restyle(p, "burndown", None, stop);
            if p.id == "standard" {
                assert!(
                    restart.is_none() && restop.is_none(),
                    "standard keeps neutral"
                );
                continue;
            }
            let restart = restart.expect(p.id);
            assert!(restart.contains("7 tonnes"), "{}: {restart}", p.id);
            assert!(restop.is_some(), "{} has no stop line", p.id);
        }
    }

    #[test]
    fn standard_never_rewrites_and_unknown_kinds_fall_back() {
        let ev = json!({"event":"Bounty","Target":"anaconda","TotalReward":1000});
        assert!(restyle(by_id("standard"), "kill", Some(&ev), "x").is_none());
        assert!(restyle(by_id("sassy"), "codex", None, "New codex entry.").is_none());
    }

    #[test]
    fn personas_keep_the_facts() {
        let ev = json!({"event":"Bounty","Target":"anaconda","Target_Localised":"Anaconda","TotalReward":1_621_122});
        for p in PERSONAS.iter().filter(|p| p.id != "standard") {
            let t = restyle(p, "kill", Some(&ev), "").unwrap();
            assert!(
                t.contains("Anaconda") && t.contains("1.6 million"),
                "{}: {t}",
                p.id
            );
        }
        let ev = json!({"event":"FSDJump","StarSystem":"Deciat"});
        let t = restyle(
            by_id("butler"),
            "arrival",
            Some(&ev),
            "Arrived in Deciat. Aisling Duval, fortified.",
        )
        .unwrap();
        assert!(
            t.contains("Deciat") && t.ends_with("Aisling Duval, fortified."),
            "{t}"
        );
    }

    #[test]
    fn unknown_persona_id_is_standard() {
        assert_eq!(by_id("nope").id, "standard");
    }

    /// A long session hears every line before any repeats: the bag deals
    /// the whole pool in shuffled order, then reshuffles.
    #[test]
    fn a_pool_deals_every_line_before_any_repeats() {
        static POOL: &[&str] = &["a", "b", "c", "d", "e"];
        let mut seen: Vec<&str> = (0..POOL.len()).map(|_| pick(POOL)).collect();
        seen.sort_unstable();
        assert_eq!(seen, POOL, "one cycle covers the pool exactly once");
    }

    /// Fifty-eight jumps to Colonia: cycles repeat, but never back-to-back
    /// -- a reshuffle cannot open with the line that closed the last cycle.
    #[test]
    fn consecutive_deals_never_repeat_across_many_cycles() {
        static POOL: &[&str] = &["a", "b", "c", "d"];
        let deals: Vec<&str> = (0..60).map(|_| pick(POOL)).collect();
        for pair in deals.windows(2) {
            assert_ne!(pair[0], pair[1], "back-to-back repeat in {deals:?}");
        }
    }

    /// The transcript problem: sassy said "We made it, somehow." on every
    /// single jump. Consecutive arrivals must phrase differently.
    #[test]
    fn arrival_lines_vary_between_jumps() {
        let ev = json!({"event":"FSDJump","StarSystem":"Wongi"});
        let one = restyle(
            by_id("sassy"),
            "arrival",
            Some(&ev),
            "Arrived in Wongi. Boom.",
        )
        .unwrap();
        let two = restyle(
            by_id("sassy"),
            "arrival",
            Some(&ev),
            "Arrived in Wongi. Boom.",
        )
        .unwrap();
        assert_ne!(one, two, "two consecutive jumps used the same phrasing");
        for t in [&one, &two] {
            assert!(
                t.contains("Wongi") && t.ends_with("Boom."),
                "facts lost: {t}"
            );
        }
    }
}
