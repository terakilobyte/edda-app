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
    /// The target count the game stated at acceptance. Progress towards
    /// it is not tracked (see module docs); the status says when it is met.
    pub kill_count: Option<i64>,
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
    pub reward: Option<i64>,
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
/// newest first. `now` is an ISO timestamp used to mark expiry.
pub fn missions(conn: &Connection, since: &str, now: &str) -> Result<Vec<Mission>> {
    let mut stmt = conn.prepare(
        "SELECT ts, event, raw FROM events
         WHERE event IN ('MissionAccepted','MissionCompleted','MissionFailed',
                         'MissionAbandoned','MissionRedirected','CargoDepot',
                         'Docked','Location','FSDJump','CarrierJump')
           AND ts >= ?1
         ORDER BY ts, file, offset",
    )?;
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
    let mut live: Vec<Mission> = missions(conn, "", now)?
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
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Stack {
    pub target_faction: String,
    pub givers: Vec<StackGiver>,
    /// Live massacres against some OTHER target, so the display can say
    /// they exist rather than silently omit them.
    pub other_targets: usize,
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
    Some(Stack { target_faction: target, givers, other_targets })
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
