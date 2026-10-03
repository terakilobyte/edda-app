//! Mission tracking, derived on read from the event log.
//!
//! The journal gives us the whole lifecycle: `MissionAccepted` with the
//! objective, `CargoDepot` with cumulative collection/delivery counts,
//! `MissionRedirected` when the objective is met and the game
//! points you at the hand-in, then `MissionCompleted` / `Failed` /
//! `Abandoned`.
//!
//! What it does NOT give is kill progress, and we no longer guess at it
//! (maintainer, 2026-09-19: "abandon kill count tracking"). Counting
//! `Bounty` / `FactionKillBond` events was tried and measured: a target
//! that dies before the ship's scan completes writes no journal event
//! yet counts for the mission (live, 2026-09-19: three kills in game,
//! one `Bounty` in the journal), same-giver missions credit one after
//! another while different givers credit together, and only the system
//! the mission names counts. On the maintainer's journal 46 of 65
//! redirected massacres were 2-23 kills short at the redirect
//! (`docs/benches/2026-09-19-mission-kill-credit-at-redirect.csv`).
//! An estimate that is wrong on the HUD is worse than no number, so a
//! massacre shows its target count and its status, and the status comes
//! from the game: `MissionRedirected` is the completion signal for
//! do-then-return missions, and nothing else moves one to
//! ReadyToTurnIn. Frontier's API has no mission endpoint either
//! (probed 2026-09-19).
//!
//! The hand-in is the station the mission was accepted at (the last
//! `Docked` before `MissionAccepted`) until the game redirects it;
//! `DestinationSystem`/`DestinationStation` at acceptance are the
//! OBJECTIVE for a kill mission, not where it is turned in. A redirect
//! completes a mission whose objective is done-then-return (kills,
//! assassinations, salvage, scans); for deliveries, couriers and
//! passengers it only moves the destination.
//!
//! The game's own word wins over ours: every login writes a `Missions`
//! event listing what is Active, Complete (ready to turn in) and Failed
//! by id. A mission we still hold open that is in none of the three is
//! over — some end without any event of their own (maintainer,
//! 2026-09-29: a "Permit Acquisition Opportunity", `MISSION_genericPermit1`,
//! wrote `MissionAccepted` and nothing else; the permit was granted on
//! the spot and nine logins since listed `Active: []`).

use anyhow::Result;
use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissionStatus {
    /// Accepted, objective not yet met.
    Active,
    /// Objective met (game redirected to the hand-in), not yet turned in.
    ReadyToTurnIn,
    Completed,
    Failed,
    Abandoned,
    /// Expiry passed without a terminal event.
    Expired,
}

#[derive(Debug, Clone, Serialize)]
pub struct Mission {
    pub id: i64,
    pub accepted: String,
    pub name: String,
    pub title: String,
    pub faction: String,
    pub kind: String,
    pub target_faction: Option<String>,
    pub target: Option<String>,
    pub target_type: Option<String>,
    /// The target count the game stated at acceptance. The status says
    /// when it is met; `kills_seen` is the opt-in estimate.
    pub kill_count: Option<i64>,
    /// SPECULATIVE, opt-in (boss, 2026-10-03: "let's make it a checkbox"
    /// after users asked again): the kills the journal has SEEN credited
    /// to this mission — `Bounty`/`FactionKillBond` on the target faction,
    /// in the mission's system, one mission per giver at a time (the
    /// game's rules as measured 2026-09-16/19), capped at the target. A
    /// floor, not the game's tally: a target that dies before the scan
    /// completes writes no event (46 of 65 redirected massacres were 2–23
    /// short at the redirect, `docs/benches/2026-09-19-*`). It never moves
    /// the status; the redirect does. None when the estimate is off.
    pub kills_seen: Option<i64>,
    pub commodity: Option<String>,
    pub count: Option<i64>,
    /// Cumulative delivery-depot counters reported directly by the game.
    pub items_collected: i64,
    pub items_delivered: i64,
    pub total_items_to_deliver: Option<i64>,
    /// The objective's location as the game stated it at acceptance
    /// (for a kill mission: where the kills must happen). Not the hand-in.
    pub destination_system: Option<String>,
    pub destination_station: Option<String>,
    /// Where the mission was accepted: the last `Docked` before it.
    pub giver_system: Option<String>,
    pub giver_station: Option<String>,
    /// Where to turn it in: the giver until `MissionRedirected` says otherwise.
    pub hand_in_system: Option<String>,
    pub hand_in_station: Option<String>,
    pub expiry: Option<String>,
    /// The credits: as offered at acceptance, then the paid figure from
    /// `MissionCompleted.Reward` once turned in (the game states both).
    pub reward: Option<i64>,
    /// What an altruism mission took, from `MissionCompleted.Donated`.
    pub donated: Option<i64>,
    pub wing: bool,
    pub status: MissionStatus,
    pub ended: Option<String>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}
fn loc(v: &Value, k: &str) -> Option<String> {
    s(v, &format!("{k}_Localised")).or_else(|| s(v, k))
}
fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}

/// Rough kind from the internal name: `Mission_Massacre` → `massacre`.
pub fn kind_of(name: &str) -> String {
    let n = name.trim_start_matches("Mission_").to_ascii_lowercase();
    n.split('_').next().unwrap_or(&n).to_string()
}

/// Kinds whose `MissionRedirected` is a change of destination, not an
/// objective met: the destination IS the objective. Everything else
/// (massacre, assassinate, salvage, scan, disable, sightseeing, ...) is
/// do-then-return, and the game's redirect is its "objective complete".
pub const REROUTE_ONLY_KINDS: [&str; 8] =
    ["delivery", "courier", "collect", "altruism", "altruismcredits", "passengervip", "passengerbulk", "smuggle"];

/// Whether the game's redirect of a mission of this kind means its
/// objective is met (see [`REROUTE_ONLY_KINDS`]).
pub fn redirect_completes(kind: &str) -> bool {
    !REROUTE_ONLY_KINDS.contains(&kind)
}

/// Every mission accepted at or after `since` (ISO timestamp; `""` for all),
/// newest first. `now` is an ISO timestamp used to mark expiry. Without
/// the speculative kill estimate; see [`missions_with`].
pub fn missions(conn: &Connection, since: &str, now: &str) -> Result<Vec<Mission>> {
    missions_with(conn, since, now, false)
}

/// [`missions`], optionally with the speculative kill estimate
/// (`kills_seen`) folded in from the kill events.
pub fn missions_with(conn: &Connection, since: &str, now: &str, speculative_kills: bool) -> Result<Vec<Mission>> {
    let kill_events = if speculative_kills { ",'Bounty','FactionKillBond'" } else { "" };
    let mut stmt = conn.prepare(&format!(
        "SELECT ts, event, raw FROM events
         WHERE event IN ('MissionAccepted','MissionCompleted','MissionFailed',
                         'MissionAbandoned','MissionRedirected','CargoDepot','Missions',
                         'Docked','Location','FSDJump','CarrierJump'{kill_events})
           AND ts >= ?1
         ORDER BY ts, file, offset"
    ))?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let mut out: Vec<Mission> = Vec::new();
    // Where the commander is, and where they are docked, as of the event
    // being read: the dock at acceptance is the giver and first hand-in.
    let mut here: Option<String> = None;
    let mut docked: Option<(String, String)> = None;
    for (ts, event, raw) in rows {
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        match event.as_str() {
            "FSDJump" => {
                here = s(&v, "StarSystem").or(here);
                docked = None;
            }
            "Docked" => {
                here = s(&v, "StarSystem").or(here);
                docked = here.clone().zip(s(&v, "StationName"));
            }
            "Location" | "CarrierJump" => {
                here = s(&v, "StarSystem").or(here);
                docked = match v.get("Docked").and_then(Value::as_bool) {
                    Some(true) => here.clone().zip(s(&v, "StationName")),
                    _ => None,
                };
            }
            "MissionAccepted" => {
                let Some(id) = i(&v, "MissionID") else {
                    continue;
                };
                let name = s(&v, "Name").unwrap_or_default();
                out.push(Mission {
                    id,
                    accepted: ts,
                    kind: kind_of(&name),
                    title: s(&v, "LocalisedName").unwrap_or_else(|| name.clone()),
                    name,
                    faction: s(&v, "Faction").unwrap_or_default(),
                    target_faction: s(&v, "TargetFaction"),
                    target: loc(&v, "Target"),
                    target_type: loc(&v, "TargetType"),
                    kill_count: i(&v, "KillCount"),
                    kills_seen: if speculative_kills { Some(0) } else { None },
                    commodity: loc(&v, "Commodity"),
                    count: i(&v, "Count"),
                    items_collected: 0,
                    items_delivered: 0,
                    total_items_to_deliver: None,
                    destination_system: s(&v, "DestinationSystem"),
                    destination_station: s(&v, "DestinationStation"),
                    giver_system: docked.as_ref().map(|(sy, _)| sy.clone()),
                    giver_station: docked.as_ref().map(|(_, st)| st.clone()),
                    hand_in_system: docked.as_ref().map(|(sy, _)| sy.clone()),
                    hand_in_station: docked.as_ref().map(|(_, st)| st.clone()),
                    expiry: s(&v, "Expiry"),
                    reward: i(&v, "Reward"),
                    donated: None,
                    wing: v.get("Wing").and_then(Value::as_bool).unwrap_or(false),
                    status: MissionStatus::Active,
                    ended: None,
                });
            }
            "CargoDepot" => {
                let Some(id) = i(&v, "MissionID") else {
                    continue;
                };
                if let Some(m) = out.iter_mut().find(|m| m.id == id) {
                    // These are cumulative counters, not deltas. Taking the
                    // event values directly also makes journal replay idempotent.
                    m.items_collected = i(&v, "ItemsCollected").unwrap_or(m.items_collected);
                    m.items_delivered = i(&v, "ItemsDelivered").unwrap_or(m.items_delivered);
                    m.total_items_to_deliver = i(&v, "TotalItemsToDeliver")
                        .or(m.total_items_to_deliver)
                        .or(m.count);
                    if m.commodity.is_none() {
                        m.commodity = loc(&v, "CargoType");
                    }
                    if m.items_delivered >= m.total_items_to_deliver.unwrap_or(i64::MAX) {
                        m.status = MissionStatus::ReadyToTurnIn;
                    }
                }
            }
            "MissionRedirected" | "MissionCompleted" | "MissionFailed" | "MissionAbandoned" => {
                let Some(id) = i(&v, "MissionID") else {
                    continue;
                };
                if let Some(m) = out.iter_mut().find(|m| m.id == id) {
                    match event.as_str() {
                        "MissionRedirected" => {
                            // A courier's redirect is a new drop-off, not a
                            // job done (tester report, 2026-09-19).
                            if m.status == MissionStatus::Active && redirect_completes(&m.kind) {
                                m.status = MissionStatus::ReadyToTurnIn;
                            }
                            // The hand-in moves even if we already inferred completion.
                            if let Some(sys) = s(&v, "NewDestinationSystem") {
                                m.hand_in_system = Some(sys);
                            }
                            if let Some(st) = s(&v, "NewDestinationStation") {
                                m.hand_in_station = Some(st);
                            }
                        }
                        "MissionCompleted" => {
                            m.status = MissionStatus::Completed;
                            m.ended = Some(ts.clone());
                            // The paid figure wins over the offer.
                            if let Some(paid) = i(&v, "Reward") {
                                m.reward = Some(paid);
                            }
                            m.donated = i(&v, "Donated").or_else(|| i(&v, "Donation"));
                        }
                        "MissionFailed" => {
                            m.status = MissionStatus::Failed;
                            m.ended = Some(ts.clone());
                        }
                        _ => {
                            m.status = MissionStatus::Abandoned;
                            m.ended = Some(ts.clone());
                        }
                    }
                }
            }
            "Bounty" | "FactionKillBond" => {
                // Only read when the estimate is on. One kill credits ONE
                // mission per giver (consecutive within a faction), every
                // giver at once (concurrent across them); `out` is in
                // acceptance order, so the earliest live mission of each
                // giver advances. Only in the mission's system; unknown
                // whereabouts (no jump seen yet) still credit. The status
                // is never touched here.
                let victim = s(&v, "VictimFaction");
                let pilot = loc(&v, "PilotName");
                let mut credited_givers: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
                for m in out.iter_mut().filter(|m| m.status == MissionStatus::Active) {
                    if m.kill_count.is_some() && m.target_faction.is_some() && m.target_faction == victim {
                        if let (Some(h), Some(d)) = (&here, &m.destination_system) {
                            if !h.eq_ignore_ascii_case(d) {
                                continue;
                            }
                        }
                        let giver = (m.faction.to_ascii_lowercase(), m.target_faction.as_deref().unwrap_or("").to_ascii_lowercase());
                        if !credited_givers.insert(giver) {
                            continue;
                        }
                        let seen = m.kills_seen.unwrap_or(0) + 1;
                        m.kills_seen = Some(match m.kill_count { Some(k) => seen.min(k), None => seen });
                    } else if let (Some(t), Some(p)) = (&m.target, &pilot) {
                        if m.kind == "assassinate" && p.eq_ignore_ascii_case(t) {
                            m.kills_seen = Some(1);
                        }
                    }
                }
            }
            "Missions" => {
                // The login roll-call. Ids the game lists decide the status
                // of every mission we hold open; one it no longer lists at
                // all is closed as completed (an abandon or a failure writes
                // its own event, a silent end is the game handing over what
                // the mission was for). `Expires` is seconds left, filled in
                // only where the acceptance gave no `Expiry`.
                let ids = |list: &str| -> Vec<(i64, i64)> {
                    v.get(list)
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(|e| Some((i(e, "MissionID")?, i(e, "Expires").unwrap_or(0)))).collect())
                        .unwrap_or_default()
                };
                let (active, complete, failed) = (ids("Active"), ids("Complete"), ids("Failed"));
                let epoch = crate::query::epoch_secs(&ts);
                for m in out.iter_mut().filter(|m| matches!(m.status, MissionStatus::Active | MissionStatus::ReadyToTurnIn)) {
                    let listed = |list: &[(i64, i64)]| list.iter().find(|(id, _)| *id == m.id).map(|(_, left)| *left);
                    if let Some(left) = listed(&active) {
                        if m.expiry.is_none() && left > 0 {
                            m.expiry = epoch.map(|e| crate::session::iso_from_epoch(e + left));
                        }
                    } else if let Some(left) = listed(&complete) {
                        m.status = MissionStatus::ReadyToTurnIn;
                        if m.expiry.is_none() && left > 0 {
                            m.expiry = epoch.map(|e| crate::session::iso_from_epoch(e + left));
                        }
                    } else if listed(&failed).is_some() {
                        m.status = MissionStatus::Failed;
                        m.ended = Some(ts.clone());
                    } else {
                        m.status = MissionStatus::Completed;
                        m.ended = Some(ts.clone());
                    }
                }
            }
            _ => {}
        }
    }

    for m in out.iter_mut() {
        if matches!(
            m.status,
            MissionStatus::Active | MissionStatus::ReadyToTurnIn
        ) {
            if let Some(e) = &m.expiry {
                if e.as_str() < now {
                    m.status = MissionStatus::Expired;
                }
            }
        }
    }
    out.reverse();
    Ok(out)
}

/// Only what is still in play -- finished missions leave the HUD rather
/// than sinking to the bottom of it (maintainer, 2026-09-16) -- in HUD
/// order (see [`in_hud_order`]).
pub fn active(conn: &Connection, now: &str) -> Result<Vec<Mission>> {
    active_with(conn, now, false)
}

/// [`active`], optionally with the speculative kill estimate.
pub fn active_with(conn: &Connection, now: &str, speculative_kills: bool) -> Result<Vec<Mission>> {
    let mut live: Vec<Mission> = missions_with(conn, "", now, speculative_kills)?
        .into_iter()
        .filter(|m| {
            matches!(
                m.status,
                MissionStatus::Active | MissionStatus::ReadyToTurnIn
            )
        })
        .collect();
    in_hud_order(&mut live);
    Ok(live)
}

/// The order missions are shown in, as the maintainer stated it
/// (2026-09-16, flying a twenty-mission massacre stack whose three HUD
/// slots were all taken by finished missions): work that still needs
/// doing comes first, soonest expiry next; what remains tied falls to
/// acceptance order, so two reads give the same list. His third key,
/// fewest kills left, went with kill counting (2026-09-19): the number
/// was an estimate the game kept contradicting.
///
/// Mission type is deliberately not a key (maintainer: "not until we
/// properly take on mission stacking"). A holding position until
/// stacking is designed properly.
pub fn in_hud_order(missions: &mut [Mission]) {
    missions.sort_by_key(|m| {
        (
            status_rank(&m.status),
            // A mission with no expiry sorts after every dated one.
            m.expiry.is_none(),
            m.expiry.clone(),
            m.accepted.clone(),
            m.id,
        )
    });
}

/// Where a status sits on the HUD: active work first, ready to turn in
/// after it. The terminal states are never shown (`active()` drops them);
/// they rank last only so the order is defined for any list. The
/// direction is the bug to watch for here -- "incomplete first" sorted on
/// a boolean the other way round would restore exactly the display that
/// was complained about -- so it is pinned by its own test.
fn status_rank(status: &MissionStatus) -> u8 {
    match status {
        MissionStatus::Active => 0,
        MissionStatus::ReadyToTurnIn => 1,
        MissionStatus::Completed
        | MissionStatus::Failed
        | MissionStatus::Abandoned
        | MissionStatus::Expired => 2,
    }
}

/// One mission giver in stacking mode: a faction the commander already
/// holds a massacre from against the chosen target.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StackGiver {
    pub faction: String,
    /// How many live missions this giver has against the target.
    pub missions: usize,
    /// How many of those are waiting to be handed in.
    pub ready: usize,
    /// Two or more against the SAME target: the game queues those, so
    /// the second one is not earning while the first is unfinished.
    pub duplicate: bool,
}

/// The board view for stacking mode.
///
/// The figures are the game's stated fields summed, never an estimate
/// (the 2026-09-19 ruling): every kill count and reward is what
/// `MissionAccepted` said. Kill credit is consecutive within one giver
/// and concurrent across givers, so the kills that clear the stack are
/// the largest per-giver sum, and every kill counts for every giver at
/// once — which is why `kills_credited` runs to several times
/// `kills_needed`. The idea of putting these numbers next to the givers
/// comes from ODEliteTracker (WarmedxMints), studied 2026-09-20; no code
/// was copied.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Stack {
    pub target_faction: String,
    /// Where the kills happen, when the missions agree (the most common
    /// stated destination system).
    pub target_system: Option<String>,
    pub givers: Vec<StackGiver>,
    /// Live massacres against some OTHER target, so the display can say
    /// they exist rather than silently omit them.
    pub other_targets: usize,
    /// Kills that clear the whole stack as accepted: the largest per-giver
    /// sum of kill counts over every live mission against the target.
    pub kills_needed: i64,
    /// Kills still to make: the largest per-giver sum over the missions
    /// the game has not yet completed (ready ones are done).
    pub kills_remaining: i64,
    /// Every kill count in the stack added up: what the kills are worth in
    /// mission credit, all givers together.
    pub kills_credited: i64,
    /// Rewards of every live mission against the target, as stated.
    pub value: i64,
    /// Rewards of the missions ready to turn in now.
    pub value_ready: i64,
    /// Rewards of the wing missions, which a wing shares.
    pub value_shareable: i64,
}

/// Every giver the commander already holds a massacre from against one
/// target, for reading at a mission board.
///
/// Kill credit is concurrent across DIFFERENT givers sharing a target
/// and consecutive within one giver (maintainer, 2026-09-16), so the
/// stack worth flying is many givers and one target, and a second
/// mission from a giver you already hold is not earning. That is what
/// `duplicate` marks. A giver stays listed until TURN-IN, not until the
/// objective is met: `active()` drops Completed, and both Active and
/// ReadyToTurnIn keep the giver's board spent.
///
/// The list must be COMPLETE. Truncating it is how a commander accepts
/// the duplicate it exists to prevent, which is why the caller renders
/// all of them however long the names are.
///
/// Alphabetical by faction, because this is read by scanning for a name
/// at a board, not by urgency. `None` when no massacre is live.
pub fn stacking_givers(live: &[Mission]) -> Option<Stack> {
    let massacres: Vec<&Mission> = live
        .iter()
        .filter(|m| m.kill_count.is_some() && m.target_faction.is_some())
        .collect();
    if massacres.is_empty() {
        return None;
    }

    // The target the commander is actually stacking: the one with the
    // most live missions. Ties break alphabetically so the choice is
    // stable rather than dependent on row order.
    let mut per_target: BTreeMap<&str, usize> = BTreeMap::new();
    for m in &massacres {
        *per_target.entry(m.target_faction.as_deref().unwrap_or_default()).or_default() += 1;
    }
    let target = per_target.iter().max_by_key(|(name, n)| (**n, std::cmp::Reverse(**name)))?.0.to_string();

    let mut by_giver: BTreeMap<String, (String, usize, usize)> = BTreeMap::new();
    for m in massacres.iter().filter(|m| m.target_faction.as_deref() == Some(target.as_str())) {
        let entry = by_giver
            .entry(m.faction.to_lowercase())
            .or_insert_with(|| (m.faction.clone(), 0, 0));
        entry.1 += 1;
        if m.status == MissionStatus::ReadyToTurnIn {
            entry.2 += 1;
        }
    }
    let givers: Vec<StackGiver> = by_giver
        .into_values()
        .map(|(faction, missions, ready)| StackGiver { faction, missions, ready, duplicate: missions > 1 })
        .collect();
    let other_targets = massacres
        .iter()
        .filter(|m| m.target_faction.as_deref() != Some(target.as_str()))
        .count();

    let mine: Vec<&&Mission> = massacres.iter().filter(|m| m.target_faction.as_deref() == Some(target.as_str())).collect();
    let mut per_giver_all: BTreeMap<String, i64> = BTreeMap::new();
    let mut per_giver_open: BTreeMap<String, i64> = BTreeMap::new();
    let mut systems: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut kills_credited, mut value, mut value_ready, mut value_shareable) = (0, 0, 0, 0);
    for m in &mine {
        let kills = m.kill_count.unwrap_or(0);
        let giver = m.faction.to_lowercase();
        *per_giver_all.entry(giver.clone()).or_default() += kills;
        if m.status == MissionStatus::Active {
            *per_giver_open.entry(giver).or_default() += kills;
        }
        kills_credited += kills;
        let reward = m.reward.unwrap_or(0);
        value += reward;
        if m.status == MissionStatus::ReadyToTurnIn {
            value_ready += reward;
        }
        if m.wing {
            value_shareable += reward;
        }
        if let Some(sys) = m.destination_system.as_deref() {
            *systems.entry(sys).or_default() += 1;
        }
    }
    let target_system = systems.iter().max_by_key(|(name, n)| (**n, std::cmp::Reverse(**name))).map(|(s, _)| s.to_string());
    Some(Stack {
        target_faction: target,
        target_system,
        givers,
        other_targets,
        kills_needed: per_giver_all.values().copied().max().unwrap_or(0),
        kills_remaining: per_giver_open.values().copied().max().unwrap_or(0),
        kills_credited,
        value,
        value_ready,
        value_shareable,
    })
}

/// The missions ready to turn in at THIS dock: `ReadyToTurnIn` with the
/// hand-in station here (and the system, when the mission names one).
/// The idea — say it at the dock, not at the board — is ODEliteTracker's
/// (studied 2026-09-20; no code copied); the facts are the game's.
pub fn ready_here<'a>(live: &'a [Mission], system: &str, station: &str) -> Vec<&'a Mission> {
    live.iter()
        .filter(|m| m.status == MissionStatus::ReadyToTurnIn)
        .filter(|m| m.hand_in_station.as_deref().is_some_and(|st| st.eq_ignore_ascii_case(station)))
        .filter(|m| m.hand_in_system.as_deref().is_none_or(|sy| sy.eq_ignore_ascii_case(system)))
        .collect()
}

/// What is ready to hand in where the commander is docked.
#[derive(Debug, Clone, Serialize)]
pub struct HandIns {
    pub system: String,
    pub station: String,
    pub missions: Vec<Mission>,
    /// The stated rewards of those missions, added up.
    pub credits: i64,
}

/// The hand-ins at the current dock, or None when not docked or nothing
/// is ready here. Reads the derived `location` table for the dock.
pub fn hand_ins_here(conn: &Connection, now: &str) -> Result<Option<HandIns>> {
    let Some(loc) = crate::query::location(conn)? else { return Ok(None) };
    if !loc.docked {
        return Ok(None);
    }
    let (Some(system), Some(station)) = (loc.system_name, loc.station_name) else { return Ok(None) };
    let live = active(conn, now)?;
    let here: Vec<Mission> = ready_here(&live, &system, &station).into_iter().cloned().collect();
    if here.is_empty() {
        return Ok(None);
    }
    let credits = here.iter().map(|m| m.reward.unwrap_or(0)).sum();
    Ok(Some(HandIns { system, station, missions: here, credits }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',1,'2026-08-26T12:33:26Z','MissionAccepted','{"timestamp":"2026-08-26T12:33:26Z","event":"MissionAccepted","Faction":"Wongi General Corp.","Name":"Mission_Massacre","LocalisedName":"Kill Kulkan Lung Blue Ring faction Pirates","TargetFaction":"Kulkan Lung Blue Ring","KillCount":3,"DestinationSystem":"Crucis Sector WU-P b5-1","DestinationStation":"Fremion City","Expiry":"2026-08-28T05:31:15Z","Reward":2433946,"MissionID":1}'),
            ('J',2,'2026-08-26T12:33:47Z','MissionAccepted','{"timestamp":"2026-08-26T12:33:47Z","event":"MissionAccepted","Faction":"Wongi Defence Force","Name":"Mission_Assassinate","LocalisedName":"Assassinate Known Pirate: Saintmaur","TargetFaction":"Kulkan Lung Blue Ring","Target":"Saintmaur","Expiry":"2026-08-27T12:30:25Z","Reward":389952,"MissionID":2}'),
            ('J',3,'2026-08-26T13:00:00Z','Bounty','{"timestamp":"2026-08-26T13:00:00Z","event":"Bounty","Target":"eagle","VictimFaction":"Kulkan Lung Blue Ring","TotalReward":1000}'),
            ('J',4,'2026-08-26T13:01:00Z','Bounty','{"timestamp":"2026-08-26T13:01:00Z","event":"Bounty","Target":"eagle","VictimFaction":"Kulkan Lung Blue Ring","TotalReward":1000}'),
            ('J',5,'2026-08-26T13:02:00Z','Bounty','{"timestamp":"2026-08-26T13:02:00Z","event":"Bounty","Target":"anaconda","VictimFaction":"Kulkan Lung Blue Ring","PilotName":"$npc_name_decorate:#name=Saintmaur;","PilotName_Localised":"Saintmaur","TotalReward":500000}');
        "#).unwrap();
        conn
    }

    /// The opt-in estimate (2026-10-03): with it on, the kills the journal
    /// saw are counted under the game's rules and capped; the status still
    /// never moves. With it off, `kills_seen` is None.
    #[test]
    fn speculative_kills_are_a_floor_and_never_the_status() {
        let conn = db();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',6,'2026-08-26T12:34:00Z','MissionAccepted','{"timestamp":"2026-08-26T12:34:00Z","event":"MissionAccepted","Faction":"Wongi General Corp.","Name":"Mission_Massacre","LocalisedName":"second from the same giver","TargetFaction":"Kulkan Lung Blue Ring","KillCount":3,"DestinationSystem":"Crucis Sector WU-P b5-1","Expiry":"2026-08-28T05:31:15Z","Reward":1,"MissionID":9}'),
            ('J',7,'2026-08-26T12:35:00Z','MissionAccepted','{"timestamp":"2026-08-26T12:35:00Z","event":"MissionAccepted","Faction":"Another Giver","Name":"Mission_Massacre","LocalisedName":"another giver, same target","TargetFaction":"Kulkan Lung Blue Ring","KillCount":2,"DestinationSystem":"Crucis Sector WU-P b5-1","Expiry":"2026-08-28T05:31:15Z","Reward":1,"MissionID":10}'),
            ('J',8,'2026-08-26T13:03:00Z','Bounty','{"timestamp":"2026-08-26T13:03:00Z","event":"Bounty","Target":"eagle","VictimFaction":"Kulkan Lung Blue Ring","TotalReward":1000}'),
            ('J',9,'2026-08-26T13:04:00Z','Bounty','{"timestamp":"2026-08-26T13:04:00Z","event":"Bounty","Target":"eagle","VictimFaction":"Kulkan Lung Blue Ring","TotalReward":1000}');
        "#).unwrap();
        let off = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        assert!(off.iter().all(|m| m.kills_seen.is_none()), "off: no estimate anywhere");
        let on = missions_with(&conn, "", "2026-08-26T14:00:00Z", true).unwrap();
        let by = |id: i64| on.iter().find(|m| m.id == id).unwrap();
        // Five target-faction kills in the fixture (three in db(), two here);
        // the first Wongi massacre takes them all up to its cap of 3 and the
        // second Wongi one, same giver, waits its turn: consecutive within a
        // giver. The other giver counts concurrently, capped at 2.
        assert_eq!(by(1).kills_seen, Some(3), "capped at the target");
        assert_eq!(by(9).kills_seen, Some(0), "same giver: not until the first is done");
        assert_eq!(by(10).kills_seen, Some(2), "another giver counts at the same time, capped");
        assert_eq!(by(2).kills_seen, Some(1), "the named pilot's bounty marks the hit");
        assert!(on.iter().all(|m| m.status == MissionStatus::Active), "an estimate never moves a status");
    }

    /// The decision of 2026-09-19: kills move nothing. Three target-faction
    /// bounties, one of them the named assassination target, and both
    /// missions are still Active -- only the game's redirect completes them.
    #[test]
    fn kills_do_not_advance_a_mission() {
        let conn = db();
        let ms = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        let massacre = ms.iter().find(|m| m.id == 1).unwrap();
        assert_eq!(massacre.status, MissionStatus::Active, "three bounties, still the game's call");
        assert_eq!(massacre.kill_count, Some(3), "the target count is still shown");
        let hit = ms.iter().find(|m| m.id == 2).unwrap();
        assert_eq!(hit.status, MissionStatus::Active, "the named pilot's bounty does not finish the hit");
        assert_eq!(hit.kind, "assassinate");
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',6,'2026-08-26T13:03:00Z','MissionRedirected','{"timestamp":"2026-08-26T13:03:00Z","event":"MissionRedirected","MissionID":2,"NewDestinationStation":"Fremion City","NewDestinationSystem":"Crucis Sector WU-P b5-1"}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        assert_eq!(ms.iter().find(|m| m.id == 2).unwrap().status, MissionStatus::ReadyToTurnIn, "the redirect does");
    }

    /// The maintainer's permit mission of 2026-09-27: accepted, granted on
    /// the spot, never another event of its own; the next login's roll-call
    /// does not list it, so it is over. A mission the roll-call DOES list
    /// stays as the game says, and one it lists as Complete is ready to
    /// turn in.
    #[test]
    fn the_login_roll_call_closes_what_the_game_no_longer_lists() {
        let conn = db();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',6,'2026-08-26T13:05:00Z','MissionAccepted','{"timestamp":"2026-08-26T13:05:00Z","event":"MissionAccepted","Faction":"Azimuth Biotech","Name":"MISSION_genericPermit1","LocalisedName":"Permit Acquisition Opportunity","Wing":false,"Influence":"None","Reputation":"None","MissionID":3}'),
            ('K',1,'2026-08-26T20:00:00Z','Missions','{"timestamp":"2026-08-26T20:00:00Z","event":"Missions","Active":[{"MissionID":1,"Name":"Mission_Massacre_name","PassengerMission":false,"Expires":3600}],"Failed":[],"Complete":[{"MissionID":2,"Name":"Mission_Assassinate_name","PassengerMission":false,"Expires":100}]}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-08-26T20:30:00Z").unwrap();
        let permit = ms.iter().find(|m| m.id == 3).unwrap();
        assert_eq!(permit.status, MissionStatus::Completed, "not in the roll-call: over");
        assert_eq!(permit.ended.as_deref(), Some("2026-08-26T20:00:00Z"));
        assert_eq!(ms.iter().find(|m| m.id == 1).unwrap().status, MissionStatus::Active);
        assert_eq!(ms.iter().find(|m| m.id == 2).unwrap().status, MissionStatus::ReadyToTurnIn, "listed Complete: ready to turn in");
        let live = active(&conn, "2026-08-26T20:30:00Z").unwrap();
        assert!(live.iter().all(|m| m.id != 3), "the permit is off the HUD");
        assert_eq!(live.len(), 2);
        // A roll-call BEFORE a mission was accepted says nothing about it.
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('L',1,'2026-08-26T21:00:00Z','MissionAccepted','{"timestamp":"2026-08-26T21:00:00Z","event":"MissionAccepted","Faction":"X","Name":"Mission_Courier","LocalisedName":"Courier","Expiry":"2026-08-28T00:00:00Z","Reward":1,"MissionID":4}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-08-26T21:30:00Z").unwrap();
        assert_eq!(ms.iter().find(|m| m.id == 4).unwrap().status, MissionStatus::Active);
    }

    #[test]
    fn redirect_and_completion_advance_the_status_and_expiry_is_applied() {
        let conn = db();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',7,'2026-08-26T13:06:00Z','MissionRedirected','{"timestamp":"2026-08-26T13:06:00Z","event":"MissionRedirected","MissionID":1,"NewDestinationStation":"Schmitt Enterprise","NewDestinationSystem":"Wongi"}'),
            ('J',8,'2026-08-26T13:30:00Z','MissionCompleted','{"timestamp":"2026-08-26T13:30:00Z","event":"MissionCompleted","MissionID":2,"Reward":389952}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        let massacre = ms.iter().find(|m| m.id == 1).unwrap();
        assert_eq!(massacre.status, MissionStatus::ReadyToTurnIn);
        assert_eq!(massacre.hand_in_station.as_deref(), Some("Schmitt Enterprise"), "the redirect moves the hand-in");
        assert_eq!(
            massacre.destination_station.as_deref(),
            Some("Fremion City"),
            "the objective's location is what the game said at acceptance"
        );
        assert_eq!(
            ms.iter().find(|m| m.id == 2).unwrap().status,
            MissionStatus::Completed
        );
        // A day later, the massacre (never turned in) has expired.
        let later = missions(&conn, "", "2026-08-29T00:00:00Z").unwrap();
        assert_eq!(
            later.iter().find(|m| m.id == 1).unwrap().status,
            MissionStatus::Expired
        );
        assert!(active(&conn, "2026-08-29T00:00:00Z").unwrap().is_empty());
    }

    #[test]
    fn active_work_ranks_before_hand_ins_and_terminal_states_last() {
        assert!(status_rank(&MissionStatus::Active) < status_rank(&MissionStatus::ReadyToTurnIn));
        for done in [
            MissionStatus::Completed,
            MissionStatus::Failed,
            MissionStatus::Abandoned,
            MissionStatus::Expired,
        ] {
            assert!(status_rank(&MissionStatus::ReadyToTurnIn) < status_rank(&done), "{done:?}");
        }
    }

    /// A massacre mission as the stack holds it: id, accepted, giver,
    /// target, kills counted, expiry, status. Everything else is the same
    /// across the stack and does not enter the order.
    /// Stacking mode reads the maintainer's real stack: every giver he
    /// already holds a massacre from against one target, complete, with
    /// the wasted duplicates marked.
    #[test]
    fn the_stack_lists_every_giver_and_flags_the_duplicates() {
        let missions = the_stack();
        let stack = stacking_givers(&missions).expect("a live stack");
        assert_eq!(stack.target_faction, "Anana Brotherhood");
        assert_eq!(stack.other_targets, 0, "every mission in the stack shares one target");

        // Complete: the count of givers must equal the distinct factions
        // in the fixture. Truncation is the defect this exists to stop.
        let distinct: std::collections::BTreeSet<&str> =
            missions.iter().map(|m| m.faction.as_str()).collect();
        assert_eq!(stack.givers.len(), distinct.len(), "every giver is listed: {:?}", stack.givers);

        // Alphabetical, because it is read by scanning for a name.
        let names: Vec<&str> = stack.givers.iter().map(|g| g.faction.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(names, sorted, "read at a board, so ordered by name");

        // The seven givers he took twice from, each wasting a mission.
        let flagged: Vec<&str> =
            stack.givers.iter().filter(|g| g.duplicate).map(|g| g.faction.as_str()).collect();
        assert_eq!(
            flagged,
            vec![
                "Ahayan Defence Party",
                "Crimson Armada",
                "HIP 90112 Jet Central Corp.",
                "HIP 96854 Empire League",
                "HR 7169 Union Party",
                "Labour Union of Ahayan",
                "United Tagii League",
            ]
        );
        let jet = stack.givers.iter().find(|g| g.faction.starts_with("HIP 90112")).unwrap();
        assert_eq!((jet.missions, jet.ready), (2, 1), "one done, one still running");
        let solo = stack.givers.iter().find(|g| g.faction == "Pilots Trade Network").unwrap();
        assert_eq!((solo.missions, solo.ready, solo.duplicate), (1, 0, false));
    }

    /// The same giver against a DIFFERENT target is a second earning
    /// mission, not a wasted one (maintainer, 2026-09-16: "If it's
    /// against a different target, it's not a duplicate even if from the
    /// same faction"). It is counted as another target, never hidden.
    #[test]
    fn the_same_giver_against_another_target_is_not_a_duplicate() {
        let mut missions = the_stack();
        let mut elsewhere = m(
            9001,
            "2026-09-16T17:00:00Z",
            "Pilots Trade Network",
            Some(20),
            Some("2026-09-23T17:00:00Z"),
            MissionStatus::Active,
        );
        elsewhere.target_faction = Some("Kulkan Lung Blue Ring".into());
        missions.push(elsewhere);

        let stack = stacking_givers(&missions).expect("a live stack");
        assert_eq!(stack.target_faction, "Anana Brotherhood", "the bigger stack wins");
        assert_eq!(stack.other_targets, 1, "said out loud, not hidden");
        let ptn = stack.givers.iter().find(|g| g.faction == "Pilots Trade Network").unwrap();
        assert!(!ptn.duplicate, "a different target is not a duplicate");
        assert_eq!(ptn.missions, 1, "only this target's missions are counted");
    }

    /// Pre-registered before the function was written (2026-09-20), from
    /// the fixture: HIP 90112 Jet Central Corp. holds 72 + 48 = 120, the
    /// largest per-giver sum, so 120 kills clear the stack as accepted;
    /// its ready mission is done, so the largest OPEN sum is Pilots Trade
    /// Network's or Natural HIP 90112 Party's 72; every kill count added
    /// is 824. Rewards are given here (the fixture has none): 1,000,000
    /// each, so value 20M, ready 12M (twelve ready), all wing.
    #[test]
    fn the_stacks_figures_are_the_stated_fields_summed() {
        let mut missions = the_stack();
        for m in missions.iter_mut() {
            m.reward = Some(1_000_000);
            m.destination_system = Some("Anana".into());
        }
        let stack = stacking_givers(&missions).expect("a live stack");
        assert_eq!(stack.kills_needed, 120, "the largest per-giver sum");
        assert_eq!(stack.kills_remaining, 72, "the largest per-giver sum of what is still open");
        assert_eq!(stack.kills_credited, 824, "every kill count added");
        assert_eq!(stack.value, 20_000_000);
        assert_eq!(stack.value_ready, 12_000_000);
        assert_eq!(stack.value_shareable, 20_000_000, "all wing");
        assert_eq!(stack.target_system.as_deref(), Some("Anana"));
    }

    /// At the dock: only the ready missions whose hand-in is this station.
    #[test]
    fn ready_here_is_this_stations_ready_missions_only() {
        let mut a = m(1, "2026-09-16T10:00:00Z", "A", Some(10), None, MissionStatus::ReadyToTurnIn);
        a.hand_in_system = Some("Puneith".into());
        a.hand_in_station = Some("Wheelock Port".into());
        let mut b = m(2, "2026-09-16T10:00:00Z", "B", Some(10), None, MissionStatus::Active);
        b.hand_in_system = Some("Puneith".into());
        b.hand_in_station = Some("Wheelock Port".into());
        let mut c = m(3, "2026-09-16T10:00:00Z", "C", Some(10), None, MissionStatus::ReadyToTurnIn);
        c.hand_in_system = Some("Ahayan".into());
        c.hand_in_station = Some("Goeppert-Mayer Vision".into());
        let live = vec![a, b, c];
        let here: Vec<i64> = ready_here(&live, "Puneith", "wheelock port").iter().map(|m| m.id).collect();
        assert_eq!(here, vec![1], "ready and here; not the active one, not the other station");
        assert!(ready_here(&live, "Puneith", "Nowhere").is_empty());
    }

    /// The paid reward replaces the offer once the game pays.
    #[test]
    fn the_paid_reward_wins_over_the_offer() {
        let conn = db();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',9,'2026-08-26T13:30:00Z','MissionCompleted','{"timestamp":"2026-08-26T13:30:00Z","event":"MissionCompleted","MissionID":2,"Reward":400000}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        let hit = ms.iter().find(|m| m.id == 2).unwrap();
        assert_eq!(hit.reward, Some(400_000), "offered 389,952; paid 400,000");
        assert_eq!(ms.iter().find(|m| m.id == 1).unwrap().reward, Some(2_433_946), "still the offer while live");
    }

    /// A courier-only board, or nothing live at all, has no stack.
    #[test]
    fn a_stack_needs_a_massacre() {
        assert!(stacking_givers(&[]).is_none());
        let mut courier = m(1, "2026-09-16T10:00:00Z", "Someone", None, None, MissionStatus::Active);
        courier.target_faction = None;
        assert!(stacking_givers(&[courier]).is_none(), "no kill target, no stack");
    }

    fn m(id: i64, accepted: &str, faction: &str, target: Option<i64>, expiry: Option<&str>, status: MissionStatus) -> Mission {
        Mission {
            id,
            accepted: accepted.into(),
            name: "Mission_MassacreWing".into(),
            title: "Massacre Anana Brotherhood pirates".into(),
            faction: faction.into(),
            kind: "massacrewing".into(),
            target_faction: Some("Anana Brotherhood".into()),
            target: None,
            target_type: None,
            kill_count: target,
            kills_seen: None,
            commodity: None,
            count: None,
            items_collected: 0,
            items_delivered: 0,
            total_items_to_deliver: None,
            destination_system: None,
            destination_station: None,
            giver_system: None,
            giver_station: None,
            hand_in_system: None,
            hand_in_station: None,
            expiry: expiry.map(str::to_string),
            reward: None,
            donated: None,
            wing: true,
            status,
            ended: None,
        }
    }

    /// The maintainer's stack as it stood on 2026-09-16 17:20Z (twenty
    /// wing massacres against Anana Brotherhood from twelve givers; twelve
    /// ready to turn in, eight still active). In acceptance order the
    /// HUD's three slots went to finished missions. Under the rule the
    /// three slots hold the active mission expiring tomorrow, then the
    /// soonest-expiring of the rest in acceptance order.
    fn the_stack() -> Vec<Mission> {
        use MissionStatus::*;
        vec![
            m(1066077652, "2026-09-15T12:18:01Z", "HIP 90112 Jet Central Corp.", Some(72), Some("2026-09-22T12:16:26Z"), ReadyToTurnIn),
            m(1066132317, "2026-09-16T03:17:41Z", "HIP 96854 Empire League", Some(54), Some("2026-09-23T03:12:54Z"), ReadyToTurnIn),
            m(1066132330, "2026-09-16T03:18:04Z", "Labour Union of Ahayan", Some(36), Some("2026-09-23T03:12:54Z"), ReadyToTurnIn),
            m(1066132341, "2026-09-16T03:18:15Z", "Ahayan Gold Creative Co", Some(30), Some("2026-09-23T03:12:54Z"), ReadyToTurnIn),
            m(1066132348, "2026-09-16T03:18:34Z", "Ahayan Defence Party", Some(25), Some("2026-09-23T03:12:54Z"), ReadyToTurnIn),
            m(1066132366, "2026-09-16T03:18:55Z", "Liberals of Ahayan", Some(15), Some("2026-09-17T22:25:27Z"), ReadyToTurnIn),
            m(1066136753, "2026-09-16T05:18:19Z", "United Tagii League", Some(40), Some("2026-09-23T05:17:39Z"), ReadyToTurnIn),
            m(1066136777, "2026-09-16T05:18:57Z", "Crimson Armada", Some(40), Some("2026-09-23T04:55:04Z"), ReadyToTurnIn),
            m(1066136793, "2026-09-16T05:19:23Z", "HR 7169 Union Party", Some(56), Some("2026-09-23T05:17:39Z"), ReadyToTurnIn),
            m(1066136797, "2026-09-16T05:19:38Z", "Puneith Values Party", Some(40), Some("2026-09-23T04:55:04Z"), ReadyToTurnIn),
            m(1066167981, "2026-09-16T15:56:34Z", "HIP 90112 Jet Central Corp.", Some(48), Some("2026-09-23T15:56:02Z"), Active),
            m(1066167987, "2026-09-16T15:56:42Z", "Pilots Trade Network", Some(72), Some("2026-09-23T15:56:02Z"), Active),
            m(1066168007, "2026-09-16T15:57:08Z", "Natural HIP 90112 Party", Some(72), Some("2026-09-23T15:56:02Z"), Active),
            m(1066168268, "2026-09-16T16:01:36Z", "Labour Union of Ahayan", Some(30), Some("2026-09-17T23:16:53Z"), Active),
            m(1066168637, "2026-09-16T16:07:15Z", "Workers of Dimocorna Union", Some(25), Some("2026-09-23T16:06:52Z"), ReadyToTurnIn),
            m(1066168655, "2026-09-16T16:07:26Z", "Crimson Armada", Some(54), Some("2026-09-23T16:06:52Z"), Active),
            m(1066168667, "2026-09-16T16:07:35Z", "HR 7169 Union Party", Some(35), Some("2026-09-23T16:06:52Z"), Active),
            m(1066168696, "2026-09-16T16:07:52Z", "United Tagii League", Some(40), Some("2026-09-23T16:06:52Z"), Active),
            m(1066169021, "2026-09-16T16:12:29Z", "HIP 96854 Empire League", Some(5), Some("2026-09-18T05:09:38Z"), ReadyToTurnIn),
            m(1066169080, "2026-09-16T16:13:29Z", "Ahayan Defence Party", Some(35), Some("2026-09-23T16:12:06Z"), Active),
        ]
    }

    #[test]
    fn the_maintainers_stack_shows_work_first_then_soonest_expiry_then_acceptance() {
        let mut stack = the_stack();
        in_hud_order(&mut stack);
        let top: Vec<&str> = stack.iter().take(3).map(|m| m.faction.as_str()).collect();
        assert_eq!(
            top,
            [
                "Labour Union of Ahayan",       // expires 09-17
                "HIP 90112 Jet Central Corp.",  // expires 09-23 15:56, accepted 15:56:34
                "Pilots Trade Network",         // same expiry, accepted 15:56:42, before Natural
            ]
        );
        // The eight active missions fill the list before any hand-in.
        assert!(stack[..8].iter().all(|m| m.status == MissionStatus::Active));
        assert!(stack[8..].iter().all(|m| m.status == MissionStatus::ReadyToTurnIn));
        // The tie pinned: same expiry to the second, acceptance order.
        assert_eq!(stack[3].faction, "Natural HIP 90112 Party");
        // The hand-ins go soonest expiry first, so the one to turn in tomorrow leads them.
        assert_eq!(stack[8].faction, "Liberals of Ahayan");
        // The same stack read in any order gives the same list.
        let mut again = the_stack();
        again.reverse();
        in_hud_order(&mut again);
        let ids = |v: &[Mission]| v.iter().map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(&again), ids(&stack));
    }

    #[test]
    fn missions_without_an_expiry_do_not_jump_the_queue() {
        use MissionStatus::*;
        let mut courier = m(3, "2026-09-16T10:00:00Z", "A", None, Some("2026-09-23T15:56:02Z"), Active);
        courier.kind = "courier".into();
        courier.target_faction = None;
        let undated = m(4, "2026-09-16T09:00:00Z", "B", Some(10), None, Active);
        let massacre = m(5, "2026-09-16T11:00:00Z", "C", Some(72), Some("2026-09-23T15:56:02Z"), Active);
        let mut list = vec![courier, undated, massacre];
        in_hud_order(&mut list);
        assert_eq!(
            list.iter().map(|m| m.id).collect::<Vec<_>>(),
            [3, 5, 4],
            "the courier and the massacre tie on expiry and fall to acceptance; the undated one, accepted first, is still last"
        );
    }

    /// `active()` drops finished missions and returns the rest in HUD
    /// order: a massacre accepted first but already met (the game
    /// redirected it) no longer takes the first slot, and a turned-in one
    /// is not listed at all.
    #[test]
    fn active_missions_drop_the_finished_and_lead_with_the_unfinished() {
        let conn = db();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',6,'2026-08-26T13:05:00Z','MissionRedirected','{"timestamp":"2026-08-26T13:05:00Z","event":"MissionRedirected","MissionID":1,"NewDestinationStation":"Schmitt Enterprise","NewDestinationSystem":"Wongi"}'),
            ('J',7,'2026-08-26T13:10:00Z','MissionAccepted','{"timestamp":"2026-08-26T13:10:00Z","event":"MissionAccepted","Faction":"Later Giver","Name":"Mission_Massacre","TargetFaction":"Kulkan Lung Blue Ring","KillCount":9,"Expiry":"2026-08-28T05:31:15Z","MissionID":9}'),
            ('J',8,'2026-08-26T13:30:00Z','MissionCompleted','{"timestamp":"2026-08-26T13:30:00Z","event":"MissionCompleted","MissionID":2,"Reward":389952}');
        "#).unwrap();
        let live = active(&conn, "2026-08-26T14:00:00Z").unwrap();
        let order: Vec<(i64, MissionStatus)> = live.iter().map(|m| (m.id, m.status.clone())).collect();
        assert_eq!(order, [(9, MissionStatus::Active), (1, MissionStatus::ReadyToTurnIn)]);
    }

    #[test]
    fn cargo_depot_uses_cumulative_delivery_progress() {
        let conn = db();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',9,'2026-08-26T13:31:00Z','MissionAccepted','{"timestamp":"2026-08-26T13:31:00Z","event":"MissionAccepted","Faction":"A","Name":"Mission_Delivery_Boom","LocalisedName":"Deliver shelters","Commodity_Localised":"Evacuation Shelter","Count":98,"MissionID":3}'),
            ('J',10,'2026-08-26T13:32:00Z','CargoDepot','{"timestamp":"2026-08-26T13:32:00Z","event":"CargoDepot","MissionID":3,"UpdateType":"Collect","ItemsCollected":98,"ItemsDelivered":52,"TotalItemsToDeliver":98}'),
            ('J',11,'2026-08-26T13:33:00Z','CargoDepot','{"timestamp":"2026-08-26T13:33:00Z","event":"CargoDepot","MissionID":3,"UpdateType":"Deliver","ItemsCollected":98,"ItemsDelivered":78,"TotalItemsToDeliver":98}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        let delivery = ms.iter().find(|m| m.id == 3).unwrap();
        assert_eq!(
            (delivery.items_collected, delivery.items_delivered),
            (98, 78)
        );
        assert_eq!(delivery.total_items_to_deliver, Some(98));
        assert_eq!(delivery.status, MissionStatus::Active);
    }

    /// The tester's report (feedback 5, 2026-09-19): "EDDA keeps saying
    /// Yamazaki Port". The hand-in is where the mission was ACCEPTED,
    /// not the objective's station the game lists at acceptance.
    #[test]
    fn the_hand_in_is_the_station_the_mission_was_accepted_at_until_the_game_redirects() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',1,'2026-09-19T05:00:00Z','Docked','{"timestamp":"2026-09-19T05:00:00Z","event":"Docked","StationName":"Papin Works","StarSystem":"Puneith","MarketID":1}'),
            ('J',2,'2026-09-19T05:01:00Z','MissionAccepted','{"timestamp":"2026-09-19T05:01:00Z","event":"MissionAccepted","MissionID":1,"Faction":"United Tagii League","Name":"Mission_Massacre","TargetFaction":"Anana Brotherhood","KillCount":15,"DestinationSystem":"Anana","DestinationStation":"Yamazaki Port","Expiry":"2026-09-26T00:00:00Z"}'),
            ('J',3,'2026-09-19T05:10:00Z','FSDJump','{"timestamp":"2026-09-19T05:10:00Z","event":"FSDJump","StarSystem":"Anana"}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-09-19T06:00:00Z").unwrap();
        let m = ms.iter().find(|m| m.id == 1).unwrap();
        assert_eq!((m.giver_system.as_deref(), m.giver_station.as_deref()), (Some("Puneith"), Some("Papin Works")));
        assert_eq!((m.hand_in_system.as_deref(), m.hand_in_station.as_deref()), (Some("Puneith"), Some("Papin Works")));
        assert_eq!((m.destination_system.as_deref(), m.destination_station.as_deref()), (Some("Anana"), Some("Yamazaki Port")), "the objective stays what the game said");
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',4,'2026-09-19T05:20:00Z','MissionRedirected','{"timestamp":"2026-09-19T05:20:00Z","event":"MissionRedirected","MissionID":1,"NewDestinationStation":"Papin Works","NewDestinationSystem":"Puneith"}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-09-19T06:00:00Z").unwrap();
        let m = ms.iter().find(|m| m.id == 1).unwrap();
        assert_eq!(m.status, MissionStatus::ReadyToTurnIn);
        assert_eq!(m.hand_in_station.as_deref(), Some("Papin Works"));
    }

    /// A courier's `MissionRedirected` is a new drop-off, not a delivery
    /// made: the hand-in moves, the status does not.
    #[test]
    fn a_couriers_redirect_moves_the_drop_off_without_completing_it() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',1,'2026-09-19T05:01:00Z','MissionAccepted','{"timestamp":"2026-09-19T05:01:00Z","event":"MissionAccepted","MissionID":1,"Faction":"United Tagii League","Name":"Mission_Courier","DestinationSystem":"Urarina","DestinationStation":"Yamazaki Port"}'),
            ('J',2,'2026-09-19T05:20:00Z','MissionRedirected','{"timestamp":"2026-09-19T05:20:00Z","event":"MissionRedirected","MissionID":1,"NewDestinationStation":"Papin Works","NewDestinationSystem":"Puneith"}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-09-19T06:00:00Z").unwrap();
        let m = ms.iter().find(|m| m.id == 1).unwrap();
        assert_eq!(m.status, MissionStatus::Active, "not done: the parcel is still aboard");
        assert_eq!(m.hand_in_station.as_deref(), Some("Papin Works"));
        assert!(redirect_completes("massacre") && redirect_completes("assassinate") && !redirect_completes("delivery"));
    }
}
