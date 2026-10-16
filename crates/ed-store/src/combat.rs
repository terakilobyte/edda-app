//! Combat sessions: every fight the commander has had, by the ship that
//! fought it, measured from the journal alone.
//!
//! Definitions are pre-registered in
//! `docs/superpowers/specs/2026-10-16-combat-sessions-design.md` and
//! implemented twice: here and in the measurement knob
//! `docs/benches/knobs/combat_sessions.py`, which a fixture test pins this
//! module to. In short: a session is one normal-space instance with a fight
//! in it (a kill, being attacked, hull damage, a lost fighter, or dying);
//! a kill's time-to-kill runs from the first target lock on that pilot;
//! kills with no lock (the fighter's, the turrets') count for throughput
//! and credits but never for time to kill; idle after the last combat
//! event is capped at ten minutes.
//!
//! Derivation is idempotent by construction: it finds the last *closed*
//! session, discards everything after it (the open tail: at most the
//! current instance), restores the state at that point and replays from
//! there. A fresh store replays everything once.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::query::epoch_secs;

/// Idle after the last combat event that still counts as the session.
pub const IDLE_CAP_SECS: i64 = 600;

const KILLS: &[&str] = &["Bounty", "FactionKillBond", "CapShipBond", "PVPKill"];
const QUALIFY: &[&str] = &["Bounty", "FactionKillBond", "CapShipBond", "PVPKill", "UnderAttack", "HullDamage", "FighterDestroyed", "Died"];
const SC_ON: &[&str] = &["SupercruiseEntry", "FSDJump", "CarrierJump"];
const SC_OFF: &[&str] = &["SupercruiseExit", "Docked", "Undocked", "Touchdown", "Liftoff", "LoadGame"];
const ENTER: &[&str] = &["SupercruiseExit", "Undocked", "Liftoff", "LoadGame"];
const LEAVE: &[&str] = &["SupercruiseEntry", "FSDJump", "Docked", "Shutdown", "LoadGame"];
/// Every event the replay reads (the union of the sets above plus the
/// combat events, the fighter's, the ship's and the system's).
const WANT: &[&str] = &[
    "Bounty", "FactionKillBond", "CapShipBond", "PVPKill", "ShipTargeted", "UnderAttack", "HullDamage", "ShieldState", "FighterDestroyed", "Died",
    "SupercruiseEntry", "FSDJump", "CarrierJump", "SupercruiseExit", "Docked", "Undocked", "Touchdown", "Liftoff", "LoadGame", "Shutdown",
    "LaunchFighter", "DockFighter", "Loadout", "Location",
];

struct Timing {
    file: String,
    offset: i64,
    ttk_s: Option<i64>,
    interval_s: i64,
    fighter_out: bool,
}

struct Open {
    ship_id: Option<i64>,
    ship: String,
    ship_name: String,
    entered: i64,
    start: i64,
    end: i64,
    start_file: String,
    start_offset: i64,
    system_name: Option<String>,
    first_lock: HashMap<String, i64>,
    last_lock: Option<(String, String)>,
    prev_kill: i64,
    timings: Vec<Timing>,
    qualifies: bool,
    kills: i64,
    kills_timed: i64,
    kills_fighter: i64,
    credits: i64,
    under_attack: i64,
    hull_hits: i64,
    shields_down: i64,
    died: i64,
}

#[derive(Clone)]
struct Ship {
    id: Option<i64>,
    symbol: String,
    name: String,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}

fn iso(secs: i64) -> String {
    // Inverse of `epoch_secs` for whole seconds (civil-from-days).
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, sec) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{sec:02}Z")
}

/// Rebuild the open tail of `combat_sessions` from the event log. Cheap on
/// a live store (the tail is one instance); a fresh store replays once.
pub fn derive_sessions(conn: &Connection) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    // The anchor: the last closed session. Everything after it is replayed.
    let anchor: Option<(i64, String, String, i64, String)> = tx
        .query_row(
            "SELECT id, left_ts, left_file, left_offset, closed_by FROM combat_sessions
             WHERE left_ts IS NOT NULL ORDER BY id DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()?;
    match &anchor {
        Some((id, ts, file, offset, _)) => {
            tx.execute("DELETE FROM combat_sessions WHERE id > ?1", [id])?;
            tx.execute(
                "UPDATE combat_kills SET session_id = NULL, ttk_s = NULL, interval_s = NULL, fighter_out = NULL
                 WHERE ts > ?1 OR (ts = ?1 AND (file > ?2 OR (file = ?2 AND offset > ?3)))",
                params![ts, file, offset],
            )?;
        }
        None => {
            tx.execute("DELETE FROM combat_sessions", [])?;
            tx.execute("UPDATE combat_kills SET session_id = NULL, ttk_s = NULL, interval_s = NULL, fighter_out = NULL", [])?;
        }
    }

    // State at the anchor.
    let mut ship = Ship { id: None, symbol: "(unknown)".into(), name: String::new() };
    let mut in_sc = false;
    let mut entered: Option<i64> = None;
    let mut system_name: Option<String> = None;
    if let Some((_, ts, file, offset, closed_by)) = &anchor {
        let before = "ts < ?1 OR (ts = ?1 AND (file < ?2 OR (file = ?2 AND offset <= ?3)))";
        if let Some(raw) = tx
            .query_row(
                &format!("SELECT raw FROM events WHERE event = 'Loadout' AND ({before}) ORDER BY ts DESC, file DESC, offset DESC LIMIT 1"),
                params![ts, file, offset],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                ship = ship_of(&v);
            }
        }
        system_name = tx
            .query_row(
                &format!("SELECT COALESCE(json_extract(raw,'$.StarSystem'), json_extract(raw,'$.System')) FROM events WHERE event IN ('Location','FSDJump','CarrierJump') AND ({before}) ORDER BY ts DESC, file DESC, offset DESC LIMIT 1"),
                params![ts, file, offset],
                |r| r.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        in_sc = matches!(closed_by.as_str(), "supercruiseentry" | "fsdjump");
        if closed_by == "loadgame" {
            entered = epoch_secs(ts);
        }
    }

    let placeholders = WANT.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let (sql, bind): (String, Vec<rusqlite::types::Value>) = match &anchor {
        Some((_, ts, file, offset, _)) => (
            format!(
                "SELECT file, offset, ts, event, raw FROM events WHERE event IN ({placeholders})
                   AND (ts > ?{t} OR (ts = ?{t} AND (file > ?{f} OR (file = ?{f} AND offset > ?{o}))))
                 ORDER BY ts, file, offset",
                t = WANT.len() + 1,
                f = WANT.len() + 2,
                o = WANT.len() + 3
            ),
            WANT.iter()
                .map(|w| rusqlite::types::Value::from(w.to_string()))
                .chain([ts.clone().into(), file.clone().into(), (*offset).into()])
                .collect(),
        ),
        None => (
            format!("SELECT file, offset, ts, event, raw FROM events WHERE event IN ({placeholders}) ORDER BY ts, file, offset"),
            WANT.iter().map(|w| rusqlite::types::Value::from(w.to_string())).collect(),
        ),
    };
    let rows: Vec<(String, i64, String, String, String)> = {
        let mut stmt = tx.prepare(&sql)?;
        let mapped = stmt.query_map(rusqlite::params_from_iter(bind.iter()), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut fighter_out = false;
    let mut cur: Option<Open> = None;
    let mut written = 0usize;

    for (file, offset, ts, event, raw) in rows {
        let Some(t) = epoch_secs(&ts) else { continue };
        let v: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let e = event.as_str();
        if e == "Location" {
            system_name = s(&v, "StarSystem");
            continue;
        }
        if e == "Loadout" {
            let new = ship_of(&v);
            if let Some(open) = cur.take() {
                if new.id != open.ship_id {
                    written += close(&tx, open, Some((t, &file, offset, "loadout")))?;
                } else {
                    cur = Some(open);
                }
            }
            ship = new;
            fighter_out = false;
            continue;
        }
        if LEAVE.contains(&e) {
            if let Some(open) = cur.take() {
                written += close(&tx, open, Some((t, &file, offset, &e.to_lowercase())))?;
            }
            // A fighter cannot follow the ship out of the instance.
            fighter_out = false;
        }
        if SC_ON.contains(&e) {
            in_sc = true;
            entered = None;
            if e != "SupercruiseEntry" {
                system_name = s(&v, "StarSystem");
            }
        }
        if SC_OFF.contains(&e) {
            in_sc = false;
        }
        if ENTER.contains(&e) {
            entered = Some(t);
        }
        match e {
            "LaunchFighter" => fighter_out = true,
            "DockFighter" | "FighterDestroyed" => fighter_out = false,
            _ => {}
        }
        let combat = matches!(e, "ShipTargeted" | "UnderAttack" | "HullDamage" | "ShieldState" | "FighterDestroyed" | "Died") || KILLS.contains(&e);
        if !combat || in_sc {
            continue;
        }
        if e == "ShipTargeted" && v.get("TargetLocked").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        if e == "HullDamage" && (v.get("PlayerPilot").and_then(Value::as_bool) != Some(true) || v.get("Fighter").and_then(Value::as_bool) == Some(true)) {
            continue;
        }
        if e == "ShieldState" && v.get("ShieldsUp").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let open = cur.get_or_insert_with(|| Open {
            ship_id: ship.id,
            ship: ship.symbol.clone(),
            ship_name: ship.name.clone(),
            entered: entered.unwrap_or(t),
            start: t,
            end: t,
            start_file: file.clone(),
            start_offset: offset,
            system_name: system_name.clone(),
            first_lock: HashMap::new(),
            last_lock: None,
            prev_kill: t,
            timings: Vec::new(),
            qualifies: false,
            kills: 0,
            kills_timed: 0,
            kills_fighter: 0,
            credits: 0,
            under_attack: 0,
            hull_hits: 0,
            shields_down: 0,
            died: 0,
        });
        open.end = t;
        if QUALIFY.contains(&e) {
            open.qualifies = true;
        }
        match e {
            "ShipTargeted" => {
                if let Some(pilot) = s(&v, "PilotName") {
                    open.first_lock.entry(pilot.clone()).or_insert(t);
                    open.last_lock = Some((pilot, s(&v, "Ship").unwrap_or_default().to_lowercase()));
                }
            }
            "UnderAttack" => open.under_attack += 1,
            "HullDamage" => open.hull_hits += 1,
            "ShieldState" => open.shields_down += 1,
            "Died" => open.died += 1,
            _ if KILLS.contains(&e) => {
                let pilot = if e == "Bounty" { s(&v, "PilotName") } else { open.last_lock.as_ref().map(|(p, _)| p.clone()) };
                let reward = i(&v, "TotalReward").or_else(|| i(&v, "Reward")).unwrap_or(0);
                let lock = pilot.as_deref().and_then(|p| open.first_lock.remove(p));
                open.kills += 1;
                open.credits += reward;
                if lock.is_some() {
                    open.kills_timed += 1;
                }
                if fighter_out {
                    open.kills_fighter += 1;
                }
                open.timings.push(Timing { file: file.clone(), offset, ttk_s: lock.map(|l| t - l), interval_s: t - open.prev_kill, fighter_out });
                open.prev_kill = t;
            }
            _ => {}
        }
        if e == "Died" {
            if let Some(open) = cur.take() {
                written += close(&tx, open, Some((t, &file, offset, "died")))?;
            }
            fighter_out = false;
        }
    }
    if let Some(open) = cur.take() {
        written += close(&tx, open, None)?;
    }
    tx.commit()?;
    Ok(written)
}

fn ship_of(v: &Value) -> Ship {
    Ship {
        id: i(v, "ShipID"),
        symbol: s(v, "Ship").map(|x| x.to_lowercase()).unwrap_or_else(|| "(unknown)".into()),
        name: s(v, "ShipName").map(|n| n.trim().to_string()).unwrap_or_default(),
    }
}

/// Write a session (and its kills' timings) if a fight happened in it.
/// `closer` is `(ts, file, offset, by)`, or `None` while the instance is
/// still open.
fn close(tx: &Connection, open: Open, closer: Option<(i64, &str, i64, &str)>) -> Result<usize> {
    if !open.qualifies {
        return Ok(0);
    }
    let left = closer.map(|(t, ..)| t.min(open.end + IDLE_CAP_SECS)).unwrap_or(open.end);
    tx.execute(
        "INSERT INTO combat_sessions
            (ship_id, ship, ship_name, entered_ts, start_ts, end_ts, left_ts, left_file, left_offset, closed_by,
             secs, fighting_secs, kills, kills_timed, kills_fighter, credits, under_attack, hull_hits, shields_down, died,
             system_name, start_file, start_offset)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)",
        params![
            open.ship_id,
            open.ship,
            open.ship_name,
            iso(open.entered),
            iso(open.start),
            iso(open.end),
            closer.map(|(t, ..)| iso(t)),
            closer.map(|(_, f, ..)| f.to_string()),
            closer.map(|(_, _, o, _)| o),
            closer.map(|(.., by)| by.to_string()),
            left - open.entered,
            open.end - open.start,
            open.kills,
            open.kills_timed,
            open.kills_fighter,
            open.credits,
            open.under_attack,
            open.hull_hits,
            open.shields_down,
            open.died,
            open.system_name,
            open.start_file,
            open.start_offset,
        ],
    )?;
    let id = tx.last_insert_rowid();
    for k in &open.timings {
        tx.execute(
            "UPDATE combat_kills SET session_id = ?1, ttk_s = ?2, interval_s = ?3, fighter_out = ?4 WHERE file = ?5 AND offset = ?6",
            params![id, k.ttk_s, k.interval_s, k.fighter_out as i64, k.file, k.offset],
        )?;
    }
    Ok(1)
}

/// One ship's combat record over a window.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ShipCombat {
    pub ship_id: Option<i64>,
    /// Journal hull symbol (`type9_military`).
    pub ship: String,
    /// The hull's printed name (`Type-10 Defender`).
    pub hull: String,
    /// The commander's name for the ship, if any.
    pub ship_name: String,
    pub sessions: i64,
    pub secs: i64,
    pub fighting_secs: i64,
    pub kills: i64,
    pub kills_timed: i64,
    pub kills_fighter: i64,
    pub credits: i64,
    pub under_attack: i64,
    pub hull_hits: i64,
    pub shields_down: i64,
    pub died: i64,
    pub median_ttk_s: Option<i64>,
    pub p90_ttk_s: Option<i64>,
    pub median_interval_s: Option<i64>,
}

fn median(xs: &mut [i64]) -> Option<i64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_unstable();
    let n = xs.len();
    Some(if n % 2 == 1 { xs[n / 2] } else { (xs[n / 2 - 1] + xs[n / 2]) / 2 })
}

fn p90(xs: &mut [i64]) -> Option<i64> {
    if xs.len() < 10 {
        return None;
    }
    xs.sort_unstable();
    Some(xs[xs.len() * 9 / 10 - 1])
}

/// Ships that fought at or after `since` (all time when `None`), most
/// kills first. A ship with no session in the window is not a row.
pub fn by_ship(conn: &Connection, since: Option<&str>) -> Result<Vec<ShipCombat>> {
    let bound = since.unwrap_or("");
    let mut stmt = conn.prepare(
        "SELECT ship_id, ship,
                COALESCE((SELECT ship_name FROM combat_sessions s2 WHERE s2.ship_id IS s.ship_id AND s2.ship = s.ship AND s2.ship_name <> '' ORDER BY s2.entered_ts DESC LIMIT 1), ''),
                COUNT(*), SUM(secs), SUM(fighting_secs), SUM(kills), SUM(kills_timed), SUM(kills_fighter), SUM(credits),
                SUM(under_attack), SUM(hull_hits), SUM(shields_down), SUM(died)
         FROM combat_sessions s WHERE entered_ts >= ?1
         GROUP BY ship_id, ship ORDER BY SUM(kills) DESC, SUM(secs) DESC",
    )?;
    let mut out: Vec<ShipCombat> = stmt
        .query_map([bound], |r| {
            let ship: String = r.get(1)?;
            Ok(ShipCombat {
                ship_id: r.get(0)?,
                hull: ed_journal::ships::display_name_or(&ship, None),
                ship,
                ship_name: r.get(2)?,
                sessions: r.get(3)?,
                secs: r.get(4)?,
                fighting_secs: r.get(5)?,
                kills: r.get(6)?,
                kills_timed: r.get(7)?,
                kills_fighter: r.get(8)?,
                credits: r.get(9)?,
                under_attack: r.get(10)?,
                hull_hits: r.get(11)?,
                shields_down: r.get(12)?,
                died: r.get(13)?,
                median_ttk_s: None,
                p90_ttk_s: None,
                median_interval_s: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut timing = conn.prepare(
        "SELECT k.ttk_s, k.interval_s FROM combat_kills k JOIN combat_sessions s ON s.id = k.session_id
         WHERE s.ship_id IS ?1 AND s.ship = ?2 AND s.entered_ts >= ?3",
    )?;
    for row in &mut out {
        let pairs: Vec<(Option<i64>, Option<i64>)> = timing
            .query_map(params![row.ship_id, row.ship, bound], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut ttks: Vec<i64> = pairs.iter().filter_map(|(t, _)| *t).collect();
        let mut intervals: Vec<i64> = pairs.iter().filter_map(|(_, i)| *i).collect();
        row.median_ttk_s = median(&mut ttks);
        row.p90_ttk_s = p90(&mut ttks);
        row.median_interval_s = median(&mut intervals);
    }
    Ok(out)
}

/// One session for the list.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionRow {
    pub id: i64,
    pub ship_id: Option<i64>,
    pub ship: String,
    pub hull: String,
    pub ship_name: String,
    pub entered_ts: String,
    pub left_ts: Option<String>,
    pub closed_by: Option<String>,
    pub secs: i64,
    pub fighting_secs: i64,
    pub kills: i64,
    pub kills_timed: i64,
    pub kills_fighter: i64,
    pub credits: i64,
    pub under_attack: i64,
    pub hull_hits: i64,
    pub shields_down: i64,
    pub died: i64,
    pub system_name: Option<String>,
    pub median_ttk_s: Option<i64>,
}

/// Sessions newest first (the open one first), for one ship when
/// `ship_id` is given, else every ship.
pub fn sessions(conn: &Connection, ship_id: Option<i64>, since: Option<&str>, limit: usize) -> Result<Vec<SessionRow>> {
    let bound = since.unwrap_or("");
    let mut stmt = conn.prepare(
        "SELECT id, ship_id, ship, ship_name, entered_ts, left_ts, closed_by, secs, fighting_secs, kills, kills_timed, kills_fighter,
                credits, under_attack, hull_hits, shields_down, died, system_name
         FROM combat_sessions WHERE entered_ts >= ?1 AND (?2 IS NULL OR ship_id = ?2)
         ORDER BY entered_ts DESC LIMIT ?3",
    )?;
    let mut out: Vec<SessionRow> = stmt
        .query_map(params![bound, ship_id, limit as i64], |r| {
            let ship: String = r.get(2)?;
            Ok(SessionRow {
                id: r.get(0)?,
                ship_id: r.get(1)?,
                hull: ed_journal::ships::display_name_or(&ship, None),
                ship,
                ship_name: r.get(3)?,
                entered_ts: r.get(4)?,
                left_ts: r.get(5)?,
                closed_by: r.get(6)?,
                secs: r.get(7)?,
                fighting_secs: r.get(8)?,
                kills: r.get(9)?,
                kills_timed: r.get(10)?,
                kills_fighter: r.get(11)?,
                credits: r.get(12)?,
                under_attack: r.get(13)?,
                hull_hits: r.get(14)?,
                shields_down: r.get(15)?,
                died: r.get(16)?,
                system_name: r.get(17)?,
                median_ttk_s: None,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut timing = conn.prepare("SELECT ttk_s FROM combat_kills WHERE session_id = ?1 AND ttk_s IS NOT NULL")?;
    for row in &mut out {
        let mut ttks: Vec<i64> = timing.query_map([row.id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        row.median_ttk_s = median(&mut ttks);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn
    }

    fn put(conn: &Connection, offset: i64, ts: &str, raw: &str) {
        let v: Value = serde_json::from_str(raw).unwrap();
        let event = v["event"].as_str().unwrap();
        conn.execute(
            "INSERT INTO events (file, offset, ts, event, raw) VALUES ('J.log', ?1, ?2, ?3, ?4)",
            params![offset, ts, event, raw],
        )
        .unwrap();
    }

    fn fight(conn: &Connection) {
        put(conn, 1, "2026-10-05T10:00:00Z", r#"{"event":"Loadout","Ship":"Type9_Military","ShipID":37,"ShipName":" "}"#);
        put(conn, 2, "2026-10-05T10:00:10Z", r#"{"event":"Location","StarSystem":"Anana"}"#);
        put(conn, 3, "2026-10-05T10:01:00Z", r#"{"event":"SupercruiseExit","StarSystem":"Anana"}"#);
        put(conn, 4, "2026-10-05T10:02:00Z", r#"{"event":"ShipTargeted","TargetLocked":true,"Ship":"eagle","PilotName":"$npc_name_decorate:#name=Hawk;","ScanStage":3}"#);
        put(conn, 5, "2026-10-05T10:02:30Z", r#"{"event":"UnderAttack","Target":"You"}"#);
        put(conn, 6, "2026-10-05T10:02:40Z", r#"{"event":"Bounty","Rewards":[{"Faction":"Earls","Reward":1000}],"PilotName":"$npc_name_decorate:#name=Hawk;","Target":"eagle","TotalReward":1000,"VictimFaction":"Pirates"}"#);
        put(conn, 7, "2026-10-05T10:03:00Z", r#"{"event":"LaunchFighter","Loadout":"zero","ID":1,"PlayerControlled":false}"#);
        put(conn, 8, "2026-10-05T10:04:00Z", r#"{"event":"Bounty","Rewards":[{"Faction":"Earls","Reward":500}],"PilotName":"$npc_name_decorate:#name=Olaf;","Target":"sidewinder","TotalReward":500,"VictimFaction":"Pirates"}"#);
        put(conn, 9, "2026-10-05T10:30:00Z", r#"{"event":"SupercruiseEntry","StarSystem":"Anana"}"#);
    }

    #[test]
    fn a_fight_becomes_one_session_with_timed_and_untimed_kills() {
        let conn = store();
        fight(&conn);
        crate::derive::derive_all(&conn).unwrap(); // runs derive_sessions
        assert_eq!(derive_sessions(&conn).unwrap(), 0, "a second pass finds the closed session and nothing after it");
        let ships = by_ship(&conn, None).unwrap();
        assert_eq!(ships.len(), 1);
        let t10 = &ships[0];
        assert_eq!((t10.ship_id, t10.ship.as_str(), t10.hull.as_str(), t10.ship_name.as_str()), (Some(37), "type9_military", "Type-10 Defender", ""));
        assert_eq!((t10.sessions, t10.kills, t10.kills_timed, t10.kills_fighter, t10.credits, t10.under_attack), (1, 2, 1, 1, 1500, 1));
        // Entered at the drop (10:01), left at the climb (10:30) but idle
        // after the last shot (10:04) is capped at ten minutes: 13 min.
        assert_eq!(t10.secs, 13 * 60);
        assert_eq!(t10.fighting_secs, 120, "first lock 10:02 to last kill 10:04");
        assert_eq!(t10.median_ttk_s, Some(40), "Hawk: locked 10:02:00, dead 10:02:40");
        assert_eq!(t10.median_interval_s, Some((40 + 80) / 2));
        let s = sessions(&conn, Some(37), None, 10).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].closed_by.as_deref(), s[0].system_name.as_deref(), s[0].median_ttk_s), (Some("supercruiseentry"), Some("Anana"), Some(40)));
        let timed: Vec<(Option<i64>, i64, i64)> = conn
            .prepare("SELECT ttk_s, interval_s, fighter_out FROM combat_kills ORDER BY ts")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(timed, vec![(Some(40), 40, 0), (None, 80, 1)], "the fighter's kill is untimed and flagged");
    }

    #[test]
    fn locks_alone_and_supercruise_locks_make_no_session() {
        let conn = store();
        put(&conn, 1, "2026-10-05T10:00:00Z", r#"{"event":"Loadout","Ship":"mandalay","ShipID":31,"ShipName":"luxor"}"#);
        put(&conn, 2, "2026-10-05T10:01:00Z", r#"{"event":"SupercruiseEntry","StarSystem":"Sol"}"#);
        put(&conn, 3, "2026-10-05T10:02:00Z", r#"{"event":"ShipTargeted","TargetLocked":true,"Ship":"anaconda","PilotName":"$npc_name_decorate:#name=A;","ScanStage":0}"#);
        put(&conn, 4, "2026-10-05T10:05:00Z", r#"{"event":"SupercruiseExit","StarSystem":"Sol"}"#);
        put(&conn, 5, "2026-10-05T10:06:00Z", r#"{"event":"ShipTargeted","TargetLocked":true,"Ship":"anaconda","PilotName":"$npc_name_decorate:#name=B;","ScanStage":0}"#);
        put(&conn, 6, "2026-10-05T10:07:00Z", r#"{"event":"ShieldState","ShieldsUp":false}"#);
        put(&conn, 7, "2026-10-05T10:08:00Z", r#"{"event":"Docked","StationName":"X","StarSystem":"Sol"}"#);
        assert_eq!(derive_sessions(&conn).unwrap(), 0);
        assert!(by_ship(&conn, None).unwrap().is_empty());
    }

    /// Replaying from the last closed session reaches the state a full
    /// rebuild reaches: the open tail is redone, closed rows are kept.
    #[test]
    fn an_incremental_pass_matches_a_rebuild() {
        let conn = store();
        fight(&conn);
        crate::derive::derive_all(&conn).unwrap();
        derive_sessions(&conn).unwrap();
        // A second instance begins and is still open.
        put(&conn, 10, "2026-10-05T11:00:00Z", r#"{"event":"SupercruiseExit","StarSystem":"Anana"}"#);
        put(&conn, 11, "2026-10-05T11:01:00Z", r#"{"event":"UnderAttack","Target":"You"}"#);
        crate::derive::derive_incremental(&conn).unwrap();
        assert_eq!(derive_sessions(&conn).unwrap(), 1, "only the open tail is written");
        let open = sessions(&conn, None, None, 10).unwrap();
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].closed_by, None);
        assert_eq!(open[0].secs, 60, "open: last combat event minus entered, no idle yet");
        // More arrives; the tail is redone and the result equals a rebuild.
        put(&conn, 12, "2026-10-05T11:02:00Z", r#"{"event":"ShipTargeted","TargetLocked":true,"Ship":"viper","PilotName":"$npc_name_decorate:#name=C;","ScanStage":3}"#);
        put(&conn, 13, "2026-10-05T11:02:50Z", r#"{"event":"FactionKillBond","Reward":2000,"AwardingFaction":"Earls","VictimFaction":"Pirates"}"#);
        put(&conn, 14, "2026-10-05T11:03:00Z", r#"{"event":"Shutdown"}"#);
        crate::derive::derive_incremental(&conn).unwrap();
        derive_sessions(&conn).unwrap();
        let incremental = by_ship(&conn, None).unwrap();
        conn.execute("DELETE FROM combat_sessions", []).unwrap();
        derive_sessions(&conn).unwrap();
        let rebuilt = by_ship(&conn, None).unwrap();
        assert_eq!(incremental, rebuilt);
        assert_eq!((rebuilt[0].sessions, rebuilt[0].kills, rebuilt[0].kills_timed), (2, 3, 2));
        let second = &sessions(&conn, None, None, 10).unwrap()[0];
        assert_eq!((second.closed_by.as_deref(), second.secs, second.median_ttk_s), (Some("shutdown"), 180, Some(50)), "the bond's target is the last lock");
    }

    #[test]
    fn iso_round_trips_through_epoch_secs() {
        for ts in ["1970-01-01T00:00:00Z", "2000-02-29T23:59:59Z", "2026-10-05T02:33:42Z", "2100-03-01T00:00:00Z"] {
            assert_eq!(iso(epoch_secs(ts).unwrap()), ts);
        }
    }
}
