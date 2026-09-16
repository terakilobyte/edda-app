//! Mission tracking, derived on read from the event log.
//!
//! The journal gives us the whole lifecycle: `MissionAccepted` with the
//! objective, `CargoDepot` with cumulative collection/delivery counts,
//! `MissionRedirected` when the objective is met and the game
//! points you at the hand-in, then `MissionCompleted` / `Failed` /
//! `Abandoned`. What it does **not** give is a per-kill progress counter,
//! so massacre progress is inferred: kills (`Bounty`, `FactionKillBond`)
//! after acceptance whose `VictimFaction` is the mission's target faction.
//! That is the same rule the game applies, with one honest caveat -- the
//! game also requires the kill to happen in the destination system, and the
//! kill events do not carry a system, so a kill of the right faction
//! elsewhere would be over-counted here. `kills_done` is therefore capped at
//! the target and labelled as inferred.
//!
//! Assassinations are done when the named target dies (the `Bounty`'s
//! pilot name matches) or when the game redirects the mission, whichever
//! comes first.

use anyhow::Result;
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
    pub kill_count: Option<i64>,
    /// Inferred from kill events; see module docs.
    pub kills_done: i64,
    pub commodity: Option<String>,
    pub count: Option<i64>,
    /// Cumulative delivery-depot counters reported directly by the game.
    pub items_collected: i64,
    pub items_delivered: i64,
    pub total_items_to_deliver: Option<i64>,
    pub destination_system: Option<String>,
    pub destination_station: Option<String>,
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
fn kind_of(name: &str) -> String {
    let n = name.trim_start_matches("Mission_").to_ascii_lowercase();
    n.split('_').next().unwrap_or(&n).to_string()
}

/// Every mission accepted at or after `since` (ISO timestamp; `""` for all),
/// newest first. `now` is an ISO timestamp used to mark expiry.
pub fn missions(conn: &Connection, since: &str, now: &str) -> Result<Vec<Mission>> {
    let mut stmt = conn.prepare(
        "SELECT ts, event, raw FROM events
         WHERE event IN ('MissionAccepted','MissionCompleted','MissionFailed',
                         'MissionAbandoned','MissionRedirected','CargoDepot',
                         'Bounty','FactionKillBond')
           AND ts >= ?1
         ORDER BY ts, file, offset",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;

    let mut out: Vec<Mission> = Vec::new();
    for (ts, event, raw) in rows {
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        match event.as_str() {
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
                    kills_done: 0,
                    commodity: loc(&v, "Commodity"),
                    count: i(&v, "Count"),
                    items_collected: 0,
                    items_delivered: 0,
                    total_items_to_deliver: None,
                    destination_system: s(&v, "DestinationSystem"),
                    destination_station: s(&v, "DestinationStation"),
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
                            if m.status == MissionStatus::Active {
                                m.status = MissionStatus::ReadyToTurnIn;
                                if let Some(k) = m.kill_count {
                                    m.kills_done = k;
                                }
                            }
                            // The hand-in moves even if we already inferred completion.
                            if let Some(sys) = s(&v, "NewDestinationSystem") {
                                m.destination_system = Some(sys);
                            }
                            if let Some(st) = s(&v, "NewDestinationStation") {
                                m.destination_station = Some(st);
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
            "Bounty" | "FactionKillBond" => {
                let victim = s(&v, "VictimFaction");
                let pilot = loc(&v, "PilotName");
                for m in out.iter_mut().filter(|m| m.status == MissionStatus::Active) {
                    if m.kill_count.is_some()
                        && m.target_faction.is_some()
                        && m.target_faction == victim
                    {
                        m.kills_done += 1;
                        if let Some(k) = m.kill_count {
                            if m.kills_done >= k {
                                m.kills_done = k;
                                m.status = MissionStatus::ReadyToTurnIn;
                            }
                        }
                    } else if let (Some(t), Some(p)) = (&m.target, &pilot) {
                        if m.kind == "assassinate" && p.eq_ignore_ascii_case(t) {
                            m.kills_done = 1;
                            m.status = MissionStatus::ReadyToTurnIn;
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
/// slots were all taken by finished missions):
/// `sort(incomplete, time_remaining, remaining_kills)`. Work that still
/// needs doing comes first, soonest expiry next, then the fewest kills
/// left; what remains tied falls to acceptance order, so two reads give
/// the same list. The keys are the fields already tracked -- the status
/// enum says whether a mission is complete, and kills remaining is the
/// target less the kills counted -- not a parallel flag.
///
/// Mission type is deliberately not a key (maintainer: "not until we
/// properly take on mission stacking"), so a mission with no kill target
/// must not smuggle one in by sorting as if it had no kills left: it
/// takes the last place on that key instead. A holding position until
/// stacking is designed properly.
pub fn in_hud_order(missions: &mut [Mission]) {
    missions.sort_by_key(|m| {
        (
            status_rank(&m.status),
            // A mission with no expiry sorts after every dated one.
            m.expiry.is_none(),
            m.expiry.clone(),
            kills_remaining(m).unwrap_or(i64::MAX),
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

/// Kills still owed on a massacre, or None when the mission has no kill
/// target at all.
pub fn kills_remaining(m: &Mission) -> Option<i64> {
    m.kill_count.map(|k| (k - m.kills_done).max(0))
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
            ('J',4,'2026-08-26T13:01:00Z','Bounty','{"timestamp":"2026-08-26T13:01:00Z","event":"Bounty","Target":"eagle","VictimFaction":"Someone Else","TotalReward":1000}'),
            ('J',5,'2026-08-26T13:02:00Z','Bounty','{"timestamp":"2026-08-26T13:02:00Z","event":"Bounty","Target":"anaconda","VictimFaction":"Kulkan Lung Blue Ring","PilotName":"$npc_name_decorate:#name=Saintmaur;","PilotName_Localised":"Saintmaur","TotalReward":500000}');
        "#).unwrap();
        conn
    }

    #[test]
    fn massacre_progress_counts_only_the_target_faction() {
        let conn = db();
        let ms = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        let massacre = ms.iter().find(|m| m.id == 1).unwrap();
        assert_eq!(massacre.kills_done, 2, "one kill was another faction");
        assert_eq!(massacre.status, MissionStatus::Active);
    }

    #[test]
    fn assassination_completes_when_the_named_pilot_dies() {
        let conn = db();
        let ms = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        let hit = ms.iter().find(|m| m.id == 2).unwrap();
        assert_eq!(hit.status, MissionStatus::ReadyToTurnIn);
        assert_eq!(hit.kind, "assassinate");
    }

    #[test]
    fn redirect_and_completion_advance_the_status_and_expiry_is_applied() {
        let conn = db();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',6,'2026-08-26T13:05:00Z','Bounty','{"timestamp":"2026-08-26T13:05:00Z","event":"Bounty","VictimFaction":"Kulkan Lung Blue Ring","TotalReward":1}'),
            ('J',7,'2026-08-26T13:06:00Z','MissionRedirected','{"timestamp":"2026-08-26T13:06:00Z","event":"MissionRedirected","MissionID":1,"NewDestinationStation":"Schmitt Enterprise","NewDestinationSystem":"Wongi"}'),
            ('J',8,'2026-08-26T13:30:00Z','MissionCompleted','{"timestamp":"2026-08-26T13:30:00Z","event":"MissionCompleted","MissionID":2,"Reward":389952}');
        "#).unwrap();
        let ms = missions(&conn, "", "2026-08-26T14:00:00Z").unwrap();
        let massacre = ms.iter().find(|m| m.id == 1).unwrap();
        assert_eq!(massacre.status, MissionStatus::ReadyToTurnIn);
        assert_eq!(massacre.kills_done, 3, "capped at the target");
        assert_eq!(
            massacre.destination_station.as_deref(),
            Some("Schmitt Enterprise")
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
    fn m(id: i64, accepted: &str, faction: &str, target: Option<i64>, done: i64, expiry: Option<&str>, status: MissionStatus) -> Mission {
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
            kills_done: done,
            commodity: None,
            count: None,
            items_collected: 0,
            items_delivered: 0,
            total_items_to_deliver: None,
            destination_system: None,
            destination_station: None,
            expiry: expiry.map(str::to_string),
            reward: None,
            wing: true,
            status,
            ended: None,
        }
    }

    /// The maintainer's stack as it stood on 2026-09-16 17:20Z (twenty
    /// wing massacres against Anana Brotherhood from twelve givers; twelve
    /// ready to turn in, eight still active; 28 kills counted against every
    /// active one). In acceptance order the HUD's three slots went to
    /// finished missions. Under the stated rule the three slots hold the
    /// mission two kills from paying out and expiring tomorrow, then the
    /// two soonest-expiring of the rest by fewest kills left; the tie
    /// between two 72-kill missions sharing an expiry to the second falls
    /// to which was accepted first.
    fn the_stack() -> Vec<Mission> {
        use MissionStatus::*;
        vec![
            m(1066077652, "2026-09-15T12:18:01Z", "HIP 90112 Jet Central Corp.", Some(72), 72, Some("2026-09-22T12:16:26Z"), ReadyToTurnIn),
            m(1066132317, "2026-09-16T03:17:41Z", "HIP 96854 Empire League", Some(54), 54, Some("2026-09-23T03:12:54Z"), ReadyToTurnIn),
            m(1066132330, "2026-09-16T03:18:04Z", "Labour Union of Ahayan", Some(36), 36, Some("2026-09-23T03:12:54Z"), ReadyToTurnIn),
            m(1066132341, "2026-09-16T03:18:15Z", "Ahayan Gold Creative Co", Some(30), 30, Some("2026-09-23T03:12:54Z"), ReadyToTurnIn),
            m(1066132348, "2026-09-16T03:18:34Z", "Ahayan Defence Party", Some(25), 25, Some("2026-09-23T03:12:54Z"), ReadyToTurnIn),
            m(1066132366, "2026-09-16T03:18:55Z", "Liberals of Ahayan", Some(15), 15, Some("2026-09-17T22:25:27Z"), ReadyToTurnIn),
            m(1066136753, "2026-09-16T05:18:19Z", "United Tagii League", Some(40), 40, Some("2026-09-23T05:17:39Z"), ReadyToTurnIn),
            m(1066136777, "2026-09-16T05:18:57Z", "Crimson Armada", Some(40), 40, Some("2026-09-23T04:55:04Z"), ReadyToTurnIn),
            m(1066136793, "2026-09-16T05:19:23Z", "HR 7169 Union Party", Some(56), 56, Some("2026-09-23T05:17:39Z"), ReadyToTurnIn),
            m(1066136797, "2026-09-16T05:19:38Z", "Puneith Values Party", Some(40), 40, Some("2026-09-23T04:55:04Z"), ReadyToTurnIn),
            m(1066167981, "2026-09-16T15:56:34Z", "HIP 90112 Jet Central Corp.", Some(48), 28, Some("2026-09-23T15:56:02Z"), Active),
            m(1066167987, "2026-09-16T15:56:42Z", "Pilots Trade Network", Some(72), 28, Some("2026-09-23T15:56:02Z"), Active),
            m(1066168007, "2026-09-16T15:57:08Z", "Natural HIP 90112 Party", Some(72), 28, Some("2026-09-23T15:56:02Z"), Active),
            m(1066168268, "2026-09-16T16:01:36Z", "Labour Union of Ahayan", Some(30), 28, Some("2026-09-17T23:16:53Z"), Active),
            m(1066168637, "2026-09-16T16:07:15Z", "Workers of Dimocorna Union", Some(25), 25, Some("2026-09-23T16:06:52Z"), ReadyToTurnIn),
            m(1066168655, "2026-09-16T16:07:26Z", "Crimson Armada", Some(54), 28, Some("2026-09-23T16:06:52Z"), Active),
            m(1066168667, "2026-09-16T16:07:35Z", "HR 7169 Union Party", Some(35), 28, Some("2026-09-23T16:06:52Z"), Active),
            m(1066168696, "2026-09-16T16:07:52Z", "United Tagii League", Some(40), 28, Some("2026-09-23T16:06:52Z"), Active),
            m(1066169021, "2026-09-16T16:12:29Z", "HIP 96854 Empire League", Some(5), 5, Some("2026-09-18T05:09:38Z"), ReadyToTurnIn),
            m(1066169080, "2026-09-16T16:13:29Z", "Ahayan Defence Party", Some(35), 28, Some("2026-09-23T16:12:06Z"), Active),
        ]
    }

    #[test]
    fn the_maintainers_stack_shows_work_first_then_soonest_expiry_then_fewest_kills() {
        let mut stack = the_stack();
        in_hud_order(&mut stack);
        let top: Vec<(&str, Option<i64>)> = stack.iter().take(3).map(|m| (m.faction.as_str(), kills_remaining(m))).collect();
        assert_eq!(
            top,
            [
                ("Labour Union of Ahayan", Some(2)),       // 30 kills, expires 09-17
                ("HIP 90112 Jet Central Corp.", Some(20)), // 48 kills, expires 09-23 15:56
                ("Pilots Trade Network", Some(44)),        // 72 kills, same expiry, accepted before Natural
            ]
        );
        // The eight active missions fill the list before any hand-in.
        assert!(stack[..8].iter().all(|m| m.status == MissionStatus::Active));
        assert!(stack[8..].iter().all(|m| m.status == MissionStatus::ReadyToTurnIn));
        // The tie pinned: same expiry to the second, same kills left, acceptance order.
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
    fn missions_without_a_kill_target_or_an_expiry_do_not_jump_the_queue() {
        use MissionStatus::*;
        let mut courier = m(3, "2026-09-16T10:00:00Z", "A", None, 0, Some("2026-09-23T15:56:02Z"), Active);
        courier.kind = "courier".into();
        courier.target_faction = None;
        let undated = m(4, "2026-09-16T09:00:00Z", "B", Some(10), 1, None, Active);
        let massacre = m(5, "2026-09-16T11:00:00Z", "C", Some(72), 28, Some("2026-09-23T15:56:02Z"), Active);
        let mut list = vec![courier, undated, massacre];
        in_hud_order(&mut list);
        assert_eq!(
            list.iter().map(|m| m.id).collect::<Vec<_>>(),
            [5, 3, 4],
            "the massacre with 44 to go leads the courier it ties with on expiry; the undated one is last"
        );
        assert_eq!(kills_remaining(&list[1]), None);
    }

    /// `active()` drops finished missions and returns the rest in HUD
    /// order: a massacre accepted first but already met no longer takes the
    /// first slot, and a turned-in one is not listed at all.
    #[test]
    fn active_missions_drop_the_finished_and_lead_with_the_unfinished() {
        let conn = db();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',6,'2026-08-26T13:05:00Z','Bounty','{"timestamp":"2026-08-26T13:05:00Z","event":"Bounty","VictimFaction":"Kulkan Lung Blue Ring","TotalReward":1}'),
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
}
