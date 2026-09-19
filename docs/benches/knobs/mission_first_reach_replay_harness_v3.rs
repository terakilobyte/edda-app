
/// The instrument the capped count could not be: WHEN the store first
/// inferred a massacre complete (kills_done reached KillCount) versus WHEN
/// the game said so (MissionRedirected). Early = over-credit (the count was
/// fed kills the game gave elsewhere); never = under-credit. Not a unit test:
/// `EDDA_REPLAY_DB=<path> EDDA_REPLAY_OUT=<csv> cargo test --release -p edda replay_journal_v3 -- --ignored --nocapture`.
#[cfg(test)]
mod replay_v3 {
    use super::*;
    use std::collections::HashMap;

    const KINDS: &str = "('MissionAccepted','MissionCompleted','MissionFailed','MissionAbandoned','MissionRedirected','CargoDepot','Bounty','FactionKillBond','FSDJump','Location','CarrierJump','Docked')";

    fn secs(ts: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(ts).unwrap().timestamp()
    }

    #[test]
    #[ignore]
    fn replay_journal_v3() {
        let Ok(path) = std::env::var("EDDA_REPLAY_DB") else { return };
        let out_path = std::env::var("EDDA_REPLAY_OUT").unwrap_or_else(|_| "replay_v3.csv".into());
        let since = std::env::var("EDDA_REPLAY_SINCE").unwrap_or_else(|_| "2026-09-09".into());
        let src = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let mut stmt = src
            .prepare(&format!("SELECT ts, event, raw FROM events WHERE event IN {KINDS} AND ts >= ?1 ORDER BY ts, file, offset"))
            .unwrap();
        let rows: Vec<(String, String, String)> = stmt
            .query_map([&since], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        ed_store::schema::attach_galaxy(&conn, None).unwrap();

        // Per mission: (giver, target, kill_count, accepted), first time kills_done == kill_count, redirect time.
        let mut meta: HashMap<i64, (String, String, i64, String)> = HashMap::new();
        let mut first_reach: HashMap<i64, String> = HashMap::new();
        let mut redirect_at: HashMap<i64, String> = HashMap::new();
        let mut order: Vec<i64> = Vec::new();
        let snapshot = |now: &str| -> HashMap<i64, i64> {
            ed_store::missions::missions(&conn, "", now)
                .unwrap()
                .into_iter()
                .filter(|m| m.kill_count.is_some())
                .map(|m| (m.id, m.kills_done))
                .collect()
        };
        let mut offset = 0i64;
        for (ts, event, raw) in &rows {
            let v: Value = serde_json::from_str(raw).unwrap();
            offset += 1;
            conn.execute(
                "INSERT INTO events (file, offset, ts, event, raw) VALUES ('replay', ?1, ?2, ?3, ?4)",
                rusqlite::params![offset, ts, event, raw],
            )
            .unwrap();
            match event.as_str() {
                "MissionAccepted" => {
                    if let (Some(id), Some(kc)) = (v.get("MissionID").and_then(Value::as_i64), v.get("KillCount").and_then(Value::as_i64)) {
                        meta.insert(id, (v.get("Faction").and_then(Value::as_str).unwrap_or("").to_owned(), v.get("TargetFaction").and_then(Value::as_str).unwrap_or("").to_owned(), kc, ts.clone()));
                        order.push(id);
                    }
                }
                "MissionRedirected" => {
                    if let Some(id) = v.get("MissionID").and_then(Value::as_i64) {
                        redirect_at.entry(id).or_insert_with(|| ts.clone());
                    }
                }
                "Bounty" | "FactionKillBond" => {
                    for (id, done) in snapshot(ts) {
                        if let Some((_, _, kc, _)) = meta.get(&id) {
                            if done >= *kc {
                                first_reach.entry(id).or_insert_with(|| ts.clone());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let mut csv = String::from("mission_id,giver,target,kill_count,accepted,first_reached,redirected,lead_seconds,verdict\n");
        let (mut early, mut on_time, mut never, mut late) = (0, 0, 0, 0);
        for id in &order {
            let Some((giver, target, kc, acc)) = meta.get(id) else { continue };
            let Some(red) = redirect_at.get(id) else { continue }; // not redirected: no verdict
            let (reach, lead, verdict) = match first_reach.get(id) {
                None => { never += 1; (String::new(), None, "never (under)") }
                Some(r) => {
                    let lead = secs(red) - secs(r);
                    if lead > 60 { early += 1; (r.clone(), Some(lead), "early (over-credit)") }
                    else if lead < -60 { late += 1; (r.clone(), Some(lead), "late") }
                    else { on_time += 1; (r.clone(), Some(lead), "on time") }
                }
            };
            csv.push_str(&format!("{id},\"{giver}\",\"{target}\",{kc},{acc},{reach},{red},{},{verdict}\n", lead.map(|l| l.to_string()).unwrap_or_default()));
        }
        csv.push_str(&format!("#totals,redirected={},early={early},on_time={on_time},late={late},never={never}\n", early + on_time + late + never));
        std::fs::write(&out_path, &csv).unwrap();
        println!("{}", csv.lines().last().unwrap());
    }
}
