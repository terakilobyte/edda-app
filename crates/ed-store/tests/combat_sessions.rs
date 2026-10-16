//! The store's combat sessions pinned to the measurement knob.
//!
//! `fixtures/combat-2026-10-05.jsonl` is one day of the maintainer's
//! journal (every line, in replay order; `Friends`/`Commander` lines and
//! the commander's own name removed). The knob
//! `docs/benches/knobs/combat_sessions.py` run on the same file gives the
//! figures asserted here (2026-10-16); the two implementations of the
//! pre-registered definitions must agree to the second.
use rusqlite::{params, Connection};
use serde_json::Value;

fn store_with_fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ed_store::schema::migrate(&conn).unwrap();
    ed_store::schema::attach_galaxy(&conn, None).unwrap();
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/combat-2026-10-05.jsonl")).unwrap();
    let mut stmt = conn
        .prepare("INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.2026-10-05.01.log', ?1, ?2, ?3, ?4)")
        .unwrap();
    for (n, line) in text.lines().enumerate() {
        let v: Value = serde_json::from_str(line).unwrap();
        stmt.execute(params![n as i64, v["timestamp"].as_str().unwrap(), v["event"].as_str().unwrap(), line]).unwrap();
    }
    drop(stmt);
    ed_store::derive::derive_all(&conn).unwrap();
    conn
}

#[test]
fn a_day_of_journal_reproduces_the_knob() {
    let conn = store_with_fixture();
    let ships = ed_store::combat::by_ship(&conn, None).unwrap();
    // knob: 2 sessions, 332 kills.
    assert_eq!(ships.iter().map(|s| s.sessions).sum::<i64>(), 2);
    assert_eq!(ships.iter().map(|s| s.kills).sum::<i64>(), 332);

    // knob: 37,type9_military,,1,9.50,9.39,304,31,0,32.0,37,166,68,23848716,2510024,280,0,1,0
    let t10 = ships.iter().find(|s| s.ship_id == Some(37)).expect("the Type-10 row");
    assert_eq!(t10.ship, "type9_military");
    assert_eq!(t10.hull, "Type-10 Defender");
    assert_eq!((t10.sessions, t10.kills, t10.kills_timed, t10.kills_fighter), (1, 304, 31, 0));
    assert_eq!((t10.secs, t10.fighting_secs), (34205, 33815));
    assert_eq!((t10.median_ttk_s, t10.p90_ttk_s, t10.median_interval_s), (Some(37), Some(166), Some(68)));
    assert_eq!((t10.credits, t10.under_attack, t10.hull_hits, t10.shields_down, t10.died), (23_848_716, 280, 0, 1, 0));

    // knob: None,(unknown),,1,1.00,0.83,28,12,12,28.0,32,117,57,3388700,3391526,83,0,0,0
    // The day opens mid-fight, before any Loadout: the ship is unknown and
    // the first lock on each pilot is in yesterday's file.
    let unknown = ships.iter().find(|s| s.ship_id.is_none()).expect("the unknown-ship row");
    assert_eq!((unknown.sessions, unknown.kills, unknown.kills_timed, unknown.kills_fighter), (1, 28, 12, 12));
    assert_eq!((unknown.secs, unknown.fighting_secs), (3597, 2997));
    assert_eq!((unknown.median_ttk_s, unknown.p90_ttk_s, unknown.median_interval_s), (Some(32), Some(117), Some(57)));
    assert_eq!((unknown.credits, unknown.under_attack), (3_388_700, 83));

    let sessions = ed_store::combat::sessions(&conn, None, None, 10).unwrap();
    assert_eq!(sessions.len(), 2);
    // knob --sessions: 2026-10-05T02:33:42Z,type9_military,,34205,33815,shutdown,...
    assert_eq!((sessions[0].entered_ts.as_str(), sessions[0].closed_by.as_deref(), sessions[0].secs), ("2026-10-05T02:33:42Z", Some("shutdown"), 34205));
    assert_eq!((sessions[1].entered_ts.as_str(), sessions[1].closed_by.as_deref(), sessions[1].secs), ("2026-10-05T00:01:32Z", Some("loadgame"), 3597));
    assert_eq!(sessions[0].median_ttk_s, Some(37));

    // knob --kills, first rows: diamondbackxl untimed interval 51; eagle ttk 6 interval 36.
    let first: Vec<(String, Option<i64>, i64)> = conn
        .prepare("SELECT target_ship, ttk_s, interval_s FROM combat_kills ORDER BY ts, file, offset LIMIT 2")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(first, vec![("Diamondback Explorer".to_string(), None, 51), ("Eagle".to_string(), Some(6), 36)]);
}

/// A sync after the day's events arrive one file at a time ends where a
/// single rebuild ends.
#[test]
fn incremental_syncs_match_the_rebuild() {
    let conn = Connection::open_in_memory().unwrap();
    ed_store::schema::migrate(&conn).unwrap();
    ed_store::schema::attach_galaxy(&conn, None).unwrap();
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/combat-2026-10-05.jsonl")).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut n = 0i64;
    for chunk in lines.chunks(97) {
        for line in chunk {
            let v: Value = serde_json::from_str(line).unwrap();
            conn.execute(
                "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.2026-10-05.01.log', ?1, ?2, ?3, ?4)",
                params![n, v["timestamp"].as_str().unwrap(), v["event"].as_str().unwrap(), line],
            )
            .unwrap();
            n += 1;
        }
        ed_store::derive::derive_incremental(&conn).unwrap();
    }
    let incremental = ed_store::combat::by_ship(&conn, None).unwrap();
    ed_store::derive::derive_all(&conn).unwrap();
    let rebuilt = ed_store::combat::by_ship(&conn, None).unwrap();
    assert_eq!(incremental, rebuilt);
    assert_eq!(rebuilt.iter().map(|s| s.kills).sum::<i64>(), 332);
}
