//! Session facts, event tailing, and timelines.
//!
//! Everything a live companion needs that is *not* a snapshot of current
//! state: who is playing, what just happened since the last look, and how
//! activity accumulates over days and weeks.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// The current ship, from the one place that tracks it: the derived
/// `loadout` row (id = 1), rewritten from every `Loadout` event — which
/// fires on each ship swap. This is THE source; readers must never
/// re-derive the ship from raw `Loadout`/`LoadGame` events, because
/// `LoadGame` is stamped once at launch and goes stale the moment the
/// commander swaps ships without relaunching.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CurrentShip {
    /// Journal hull symbol, e.g. `panthermkii`.
    pub symbol: Option<String>,
    /// The commander's custom name for the ship, if any.
    pub name: Option<String>,
    /// The ship ident (`WI-02P`), if any.
    pub ident: Option<String>,
}

impl CurrentShip {
    /// How the ship should be addressed in prose (greetings, callouts):
    /// the custom name if the commander set one, else the hull name.
    /// `display_hull` maps a symbol to its localised hull (kept in the
    /// caller's crate so `ed-store` needn't own the ship catalog).
    pub fn spoken(&self, display_hull: impl Fn(&str) -> String) -> Option<String> {
        self.name
            .clone()
            .or_else(|| self.symbol.as_deref().map(&display_hull))
    }
}

pub fn current_ship(conn: &Connection) -> Result<Option<CurrentShip>> {
    Ok(conn
        .query_row(
            "SELECT ship, ship_name, ship_ident FROM loadout WHERE id = 1",
            [],
            |r| {
                Ok(CurrentShip {
                    symbol: r.get(0)?,
                    name: r
                        .get::<_, Option<String>>(1)?
                        .filter(|s| !s.trim().is_empty()),
                    ident: r
                        .get::<_, Option<String>>(2)?
                        .filter(|s| !s.trim().is_empty()),
                })
            },
        )
        .optional()?)
}

/// The commander's name from the most recent `Commander` event.
pub fn commander_name(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT json_extract(raw, '$.Name') FROM events WHERE event = 'Commander'
             ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

/// Raw JSON of the most recent event of a given type.
pub fn latest_event_raw(conn: &Connection, event: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT raw FROM events WHERE event = ?1 ORDER BY file DESC, offset DESC LIMIT 1",
            [event],
            |r| r.get(0),
        )
        .optional()?)
}

/// A companion snapshot (`Status.json`, `Cargo.json`, ...) as raw JSON.
pub fn snapshot_raw(conn: &Connection, name: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT raw FROM snapshots WHERE name = ?1", [name], |r| {
            r.get(0)
        })
        .optional()?)
}

/// `(file, offset)` of the newest event -- the watermark a tailer starts at
/// so that history is never replayed as if it were happening now.
pub fn last_event_key(conn: &Connection) -> Result<Option<(String, i64)>> {
    Ok(conn
        .query_row(
            "SELECT file, offset FROM events ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

/// Events strictly after a watermark, oldest first.
pub fn events_after(
    conn: &Connection,
    after: Option<&(String, i64)>,
    limit: usize,
) -> Result<Vec<(String, i64, String)>> {
    let (file, offset) = match after {
        Some((f, o)) => (f.as_str(), *o),
        None => ("", -1),
    };
    let mut stmt = conn.prepare(
        "SELECT file, offset, raw FROM events
         WHERE file > ?1 OR (file = ?1 AND offset > ?2)
         ORDER BY file, offset LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![file, offset, limit as i64], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Journal-style ISO timestamp from Unix seconds (inverse of
/// `query::epoch_secs`). Civil-from-days per Howard Hinnant.
pub fn iso_from_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `strftime` pattern for a bucket name. Validated here so a caller-supplied
/// bucket can never reach the SQL as text.
fn bucket_format(bucket: &str) -> &'static str {
    match bucket {
        "hour" => "%Y-%m-%dT%H:00",
        "week" => "%Y-W%W",
        _ => "%Y-%m-%d",
    }
}

/// One bucket of a combat timeline.
#[derive(Debug, Clone, Serialize)]
pub struct CombatBucket {
    pub bucket: String,
    pub kills: i64,
    pub credits: i64,
    pub bounties: i64,
    pub bonds: i64,
    pub deaths: i64,
    pub interdicted: i64,
}

/// Kills and credits per `day`, `week`, or `hour`, oldest first.
pub fn combat_timeline(
    conn: &Connection,
    since: Option<&str>,
    bucket: &str,
) -> Result<Vec<CombatBucket>> {
    let fmt = bucket_format(bucket);
    let bound = since.unwrap_or("");
    let mut out: Vec<CombatBucket> = Vec::new();
    {
        let sql = format!(
            "SELECT strftime('{fmt}', ts) b, COUNT(*), COALESCE(SUM(reward),0),
                    COALESCE(SUM(CASE WHEN kind='bounty' THEN reward END),0),
                    COALESCE(SUM(CASE WHEN kind<>'bounty' THEN reward END),0)
             FROM combat_kills WHERE ts >= ?1 GROUP BY b ORDER BY b"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([bound], |r| {
            Ok(CombatBucket {
                bucket: r.get(0)?,
                kills: r.get(1)?,
                credits: r.get(2)?,
                bounties: r.get(3)?,
                bonds: r.get(4)?,
                deaths: 0,
                interdicted: 0,
            })
        })?;
        for row in rows {
            out.push(row?);
        }
    }
    {
        let sql = format!(
            "SELECT strftime('{fmt}', ts) b,
                    SUM(CASE WHEN kind='died' THEN 1 ELSE 0 END),
                    SUM(CASE WHEN kind='interdicted' THEN 1 ELSE 0 END)
             FROM combat_incidents WHERE ts >= ?1 GROUP BY b"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([bound], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (b, deaths, interdicted) = row?;
            match out.iter_mut().find(|c| c.bucket == b) {
                Some(c) => {
                    c.deaths = deaths;
                    c.interdicted = interdicted;
                }
                None => out.push(CombatBucket {
                    bucket: b,
                    kills: 0,
                    credits: 0,
                    bounties: 0,
                    bonds: 0,
                    deaths,
                    interdicted,
                }),
            }
        }
    }
    out.sort_by(|a, b| a.bucket.cmp(&b.bucket));
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
pub struct MeritBucket {
    pub bucket: String,
    pub merits: i64,
    pub awards: i64,
    pub total_at_end: Option<i64>,
}

/// Merits earned per day/week/hour, oldest first.
pub fn merit_timeline(
    conn: &Connection,
    since: Option<&str>,
    bucket: &str,
) -> Result<Vec<MeritBucket>> {
    let fmt = bucket_format(bucket);
    let sql = format!(
        "SELECT strftime('{fmt}', ts) b, COALESCE(SUM(merits_gained),0), COUNT(*), MAX(total_merits)
         FROM merit_events WHERE ts >= ?1 GROUP BY b ORDER BY b"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([since.unwrap_or("")], |r| {
        Ok(MeritBucket {
            bucket: r.get(0)?,
            merits: r.get(1)?,
            awards: r.get(2)?,
            total_at_end: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Merits earned at or after `since`, and how many awards that was.
pub fn merits_since(conn: &Connection, since: &str) -> Result<(i64, i64)> {
    Ok(conn.query_row(
        "SELECT COALESCE(SUM(merits_gained),0), COUNT(*) FROM merit_events WHERE ts >= ?1",
        [since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?)
}

/// Timestamp of the current session's `LoadGame`, if one has been seen.
pub fn session_start(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT ts FROM events WHERE event = 'LoadGame' ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

#[derive(Debug, Clone, Serialize)]
pub struct KillRow {
    pub ts: String,
    pub kind: String,
    pub target_ship: Option<String>,
    pub pilot_name: Option<String>,
    pub faction: Option<String>,
    pub reward: Option<i64>,
    pub system_name: Option<String>,
}

/// Most recent kills, newest first.
pub fn recent_kills(conn: &Connection, limit: usize) -> Result<Vec<KillRow>> {
    let mut stmt = conn.prepare(
        "SELECT ts, kind, target_ship, pilot_name, faction, reward, system_name
         FROM combat_kills ORDER BY ts DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(KillRow {
            ts: r.get(0)?,
            kind: r.get(1)?,
            target_ship: r.get(2)?,
            pilot_name: r.get(3)?,
            faction: r.get(4)?,
            reward: r.get(5)?,
            system_name: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ── Ranks ────────────────────────────────────────────────────────────

/// The public rank ladders. Index = the journal's `Rank` value; Elite has
/// sub-tiers I–V above 8 in the newer journal, reported as 9–13.
const COMBAT: [&str; 9] = [
    "Harmless",
    "Mostly Harmless",
    "Novice",
    "Competent",
    "Expert",
    "Master",
    "Dangerous",
    "Deadly",
    "Elite",
];
const TRADE: [&str; 9] = [
    "Penniless",
    "Mostly Penniless",
    "Peddler",
    "Dealer",
    "Merchant",
    "Broker",
    "Entrepreneur",
    "Tycoon",
    "Elite",
];
const EXPLORE: [&str; 9] = [
    "Aimless",
    "Mostly Aimless",
    "Scout",
    "Surveyor",
    "Trailblazer",
    "Pathfinder",
    "Ranger",
    "Pioneer",
    "Elite",
];
const CQC: [&str; 9] = [
    "Helpless",
    "Mostly Helpless",
    "Amateur",
    "Semi Professional",
    "Professional",
    "Champion",
    "Hero",
    "Legend",
    "Elite",
];
const EXOBIOLOGIST: [&str; 9] = [
    "Directionless",
    "Mostly Directionless",
    "Compiler",
    "Collector",
    "Cataloguer",
    "Taxonomist",
    "Ecologist",
    "Geneticist",
    "Elite",
];
const SOLDIER: [&str; 9] = [
    "Defenceless",
    "Mostly Defenceless",
    "Rookie",
    "Soldier",
    "Gunslinger",
    "Warrior",
    "Gladiator",
    "Deadeye",
    "Elite",
];
const FEDERATION: [&str; 15] = [
    "None",
    "Recruit",
    "Cadet",
    "Midshipman",
    "Petty Officer",
    "Chief Petty Officer",
    "Warrant Officer",
    "Ensign",
    "Lieutenant",
    "Lieutenant Commander",
    "Post Commander",
    "Post Captain",
    "Rear Admiral",
    "Vice Admiral",
    "Admiral",
];
const EMPIRE: [&str; 15] = [
    "None", "Outsider", "Serf", "Master", "Squire", "Knight", "Lord", "Baron", "Viscount", "Count",
    "Earl", "Marquis", "Duke", "Prince", "King",
];

fn rank_name(ladder: &[&str], level: i64) -> String {
    if let Some(n) = usize::try_from(level).ok().and_then(|i| ladder.get(i)) {
        return n.to_string();
    }
    // Elite I–V for the careers that have them.
    if ladder.len() == 9 && (9..=13).contains(&level) {
        return format!(
            "Elite {}",
            ["I", "II", "III", "IV", "V"][(level - 9) as usize]
        );
    }
    format!("rank {level}")
}

#[derive(Debug, Clone, Serialize)]
pub struct RankRow {
    pub career: String,
    pub level: i64,
    pub name: String,
    /// Percent toward the next rank, from the `Progress` event.
    pub progress: Option<i64>,
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Ranks {
    pub as_of: Option<String>,
    pub ranks: Vec<RankRow>,
    pub powerplay_power: Option<String>,
    pub powerplay_rank: Option<i64>,
    pub powerplay_merits: Option<i64>,
}

/// Career ranks from the latest `Rank` + `Progress` events (written on every
/// login), and the Powerplay standing from the latest `Powerplay` event.
pub fn ranks(conn: &Connection) -> Result<Ranks> {
    use serde_json::Value;
    let rank = latest_event_raw(conn, "Rank")?.and_then(|r| serde_json::from_str::<Value>(&r).ok());
    let progress =
        latest_event_raw(conn, "Progress")?.and_then(|r| serde_json::from_str::<Value>(&r).ok());
    let pp =
        latest_event_raw(conn, "Powerplay")?.and_then(|r| serde_json::from_str::<Value>(&r).ok());

    let ladders: [(&str, &[&str]); 8] = [
        ("Combat", &COMBAT),
        ("Trade", &TRADE),
        ("Explore", &EXPLORE),
        ("Soldier", &SOLDIER),
        ("Exobiologist", &EXOBIOLOGIST),
        ("CQC", &CQC),
        ("Federation", &FEDERATION),
        ("Empire", &EMPIRE),
    ];
    let mut rows = Vec::new();
    if let Some(r) = &rank {
        for (career, ladder) in ladders {
            let Some(level) = r.get(career).and_then(Value::as_i64) else {
                continue;
            };
            let pct = progress
                .as_ref()
                .and_then(|p| p.get(career))
                .and_then(Value::as_i64);
            let top = if ladder.len() == 9 {
                13
            } else {
                ladder.len() as i64 - 1
            };
            rows.push(RankRow {
                career: career.to_string(),
                level,
                name: rank_name(ladder, level),
                progress: pct,
                next: (level < top).then(|| rank_name(ladder, level + 1)),
            });
        }
    }
    Ok(Ranks {
        as_of: rank
            .as_ref()
            .and_then(|r| r.get("timestamp"))
            .and_then(Value::as_str)
            .map(str::to_string),
        ranks: rows,
        powerplay_power: pp
            .as_ref()
            .and_then(|p| p.get("Power"))
            .and_then(Value::as_str)
            .map(str::to_string),
        powerplay_rank: pp
            .as_ref()
            .and_then(|p| p.get("Rank"))
            .and_then(Value::as_i64),
        powerplay_merits: pp
            .as_ref()
            .and_then(|p| p.get("Merits"))
            .and_then(Value::as_i64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one current-ship source: the swap-fed loadout row. A ship
    /// swap without relaunch (the LoadGame-stale field bug) is a
    /// non-issue here because the row is rewritten on the swap's own
    /// Loadout. `spoken` prefers the custom name, else the hull.
    #[test]
    fn current_ship_is_the_single_swap_fed_source() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        assert!(current_ship(&conn).unwrap().is_none(), "no loadout yet");
        // The swap's Loadout lands: unnamed Panther replaces whatever
        // launch-time ship LoadGame would have named.
        conn.execute(
            "INSERT OR REPLACE INTO loadout (id, ts, ship, ship_name, ship_ident)
             VALUES (1, '2026-09-06T00:00:00Z', 'panthermkii', '', 'WI-02P')",
            [],
        )
        .unwrap();
        let ship = current_ship(&conn).unwrap().unwrap();
        assert_eq!(ship.symbol.as_deref(), Some("panthermkii"));
        assert!(ship.name.is_none(), "blank name is no name");
        assert_eq!(ship.ident.as_deref(), Some("WI-02P"));
        let hull = |s: &str| {
            if s == "panthermkii" {
                "Panther Clipper Mk II".to_string()
            } else {
                s.to_string()
            }
        };
        assert_eq!(ship.spoken(hull).as_deref(), Some("Panther Clipper Mk II"));
        // Name it, and prose uses the name.
        conn.execute(
            "UPDATE loadout SET ship_name = 'Murderface' WHERE id = 1",
            [],
        )
        .unwrap();
        assert_eq!(
            current_ship(&conn)
                .unwrap()
                .unwrap()
                .spoken(hull)
                .as_deref(),
            Some("Murderface")
        );
    }

    #[test]
    fn ranks_are_named_with_progress_to_the_next() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(r#"
            INSERT INTO events (file,offset,ts,event,raw) VALUES
            ('J',1,'2026-08-26T12:00:00Z','Rank','{"timestamp":"2026-08-26T12:00:00Z","event":"Rank","Combat":7,"Trade":8,"Explore":9,"Soldier":0,"Exobiologist":2,"Empire":3,"Federation":0,"CQC":0}'),
            ('J',2,'2026-08-26T12:00:01Z','Progress','{"timestamp":"2026-08-26T12:00:01Z","event":"Progress","Combat":64,"Trade":100,"Explore":12,"Soldier":0,"Exobiologist":50,"Empire":80,"Federation":0,"CQC":0}'),
            ('J',3,'2026-08-26T12:00:02Z','Powerplay','{"timestamp":"2026-08-26T12:00:02Z","event":"Powerplay","Power":"Aisling Duval","Rank":5,"Merits":12345}');
        "#).unwrap();
        let r = ranks(&conn).unwrap();
        let combat = r.ranks.iter().find(|x| x.career == "Combat").unwrap();
        assert_eq!(
            (
                combat.name.as_str(),
                combat.progress,
                combat.next.as_deref()
            ),
            ("Deadly", Some(64), Some("Elite"))
        );
        let explore = r.ranks.iter().find(|x| x.career == "Explore").unwrap();
        assert_eq!(
            (explore.name.as_str(), explore.next.as_deref()),
            ("Elite I", Some("Elite II"))
        );
        let empire = r.ranks.iter().find(|x| x.career == "Empire").unwrap();
        assert_eq!(
            (empire.name.as_str(), empire.next.as_deref()),
            ("Master", Some("Squire"))
        );
        assert_eq!(r.powerplay_merits, Some(12345));
    }

    #[test]
    fn events_after_walks_forward_from_a_watermark_only() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO events (file,offset,ts,event,raw) VALUES
               ('A.log',0,'t','X','{}'),('A.log',50,'t','Y','{}'),('B.log',0,'t','Z','{}');",
        )
        .unwrap();
        assert_eq!(last_event_key(&conn).unwrap(), Some(("B.log".into(), 0)));
        let after = events_after(&conn, Some(&("A.log".to_string(), 0)), 10).unwrap();
        let names: Vec<_> = after.iter().map(|(f, o, _)| format!("{f}:{o}")).collect();
        assert_eq!(names, ["A.log:50", "B.log:0"]);
        assert!(events_after(&conn, Some(&("B.log".to_string(), 0)), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn combat_timeline_buckets_by_day_and_merges_incidents() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO combat_kills (file,offset,ts,kind,reward) VALUES
               ('J',1,'2026-08-24T10:00:00Z','bounty',1000),
               ('J',2,'2026-08-24T11:00:00Z','faction_kill_bond',500),
               ('J',3,'2026-08-25T09:00:00Z','bounty',7000);
             INSERT INTO combat_incidents (file,offset,ts,kind) VALUES
               ('J',4,'2026-08-25T09:30:00Z','died'),
               ('J',5,'2026-08-26T00:00:00Z','interdicted');",
        )
        .unwrap();
        let t = combat_timeline(&conn, None, "day").unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!((t[0].kills, t[0].bounties, t[0].bonds), (2, 1000, 500));
        assert_eq!((t[1].kills, t[1].deaths), (1, 1));
        assert_eq!(
            (t[2].kills, t[2].interdicted),
            (0, 1),
            "an incident-only day still appears"
        );
    }

    #[test]
    fn commander_name_comes_from_the_latest_commander_event() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO events (file,offset,ts,event,raw) VALUES
               ('A.log',0,'t','Commander','{\"Name\":\"Old\"}'),
               ('B.log',0,'t','Commander','{\"Name\":\"New\"}');",
        )
        .unwrap();
        assert_eq!(commander_name(&conn).unwrap().as_deref(), Some("New"));
    }
}
