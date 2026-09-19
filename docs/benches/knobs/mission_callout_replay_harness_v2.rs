
/// Replay of the maintainer's journal through the real mission code, one
/// watcher pass at a time, reporting per redirected massacre what the store
/// had inferred at the instant the game said "objective complete", and the
/// callouts along the way. Not a unit test:
/// `EDDA_REPLAY_DB=<path> EDDA_REPLAY_OUT=<csv> cargo test -p edda replay_journal_v2 -- --ignored --nocapture`.
#[cfg(test)]
mod replay_v2 {
    use super::*;
    use std::collections::{BTreeMap, HashMap};

    const KINDS: &str = "('MissionAccepted','MissionCompleted','MissionFailed','MissionAbandoned','MissionRedirected','CargoDepot','Bounty','FactionKillBond','FSDJump','Location','CarrierJump','Docked')";

    fn parse(ts: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(ts).unwrap().with_timezone(&chrono::Utc)
    }

    #[test]
    #[ignore]
    fn replay_journal_v2() {
        let Ok(path) = std::env::var("EDDA_REPLAY_DB") else { return };
        let out_path = std::env::var("EDDA_REPLAY_OUT").unwrap_or_else(|_| "replay_v2.csv".into());
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

        let mut passes: Vec<Vec<(String, String, String)>> = Vec::new();
        let mut prev: Option<chrono::DateTime<chrono::Utc>> = None;
        for row in rows {
            let t = parse(&row.0);
            if prev.is_some_and(|p| (t - p).num_seconds() > 1) || passes.is_empty() {
                passes.push(Vec::new());
            }
            passes.last_mut().unwrap().push(row);
            prev = Some(t);
        }

        let snapshot = |now: &str| -> HashMap<i64, (i64, Option<i64>, String, Option<String>, Option<String>)> {
            ed_store::missions::missions(&conn, "", now)
                .unwrap()
                .into_iter()
                .map(|m| (m.id, (m.kills_done, m.kill_count, format!("{:?}", m.status), Some(m.faction.clone()), m.target_faction.clone())))
                .collect()
        };

        let mut offset = 0i64;
        let mut here: Option<String> = None;
        let mut cstate = callouts::CalloutState::default();
        let mut credited_systems: HashMap<i64, BTreeMap<String, i64>> = HashMap::new();
        let (mut kill_events, mut kills_credited, mut credits_total, mut completions_at_kills) = (0usize, 0usize, 0i64, 0usize);
        let (mut objective_lines, mut redirected_lines, mut complete_lines, mut progress_lines) = (0usize, 0usize, 0usize, 0usize);
        let mut progress_named: Vec<String> = Vec::new();
        let mut csv = String::from("mission_id,giver,target,kill_count,inferred_at_redirect,status_at_redirect,verdict,credited_kill_systems\n");
        let (mut exact, mut over, mut under, mut redirects) = (0usize, 0usize, 0usize, 0usize);

        for pass in &passes {
            let parsed: Vec<Value> = pass.iter().map(|(_, _, raw)| serde_json::from_str(raw).unwrap()).collect();
            let pass_now = pass.last().unwrap().0.clone();
            for (i, (ts, event, raw)) in pass.iter().enumerate() {
                let v = &parsed[i];
                let now = ts.as_str();
                if matches!(event.as_str(), "FSDJump" | "Location" | "CarrierJump") {
                    if let Some(s) = v.get("StarSystem").and_then(Value::as_str) {
                        here = Some(s.to_owned());
                    }
                }
                // What the store had inferred when the game said "done".
                if event == "MissionRedirected" {
                    if let Some(id) = v.get("MissionID").and_then(Value::as_i64) {
                        let before = snapshot(now);
                        if let Some((done, Some(count), status, giver, target)) = before.get(&id).cloned() {
                            redirects += 1;
                            let verdict = if done == count { exact += 1; "exact" } else if done > count { over += 1; "over" } else { under += 1; "under" };
                            let systems = credited_systems.get(&id).map(|m| m.iter().map(|(s, n)| format!("{s}:{n}")).collect::<Vec<_>>().join(" ")).unwrap_or_default();
                            csv.push_str(&format!("{id},\"{}\",\"{}\",{count},{done},{status},{verdict},\"{systems}\"\n", giver.unwrap_or_default(), target.unwrap_or_default()));
                        }
                    }
                }
                let is_kill = matches!(event.as_str(), "Bounty" | "FactionKillBond");
                let before = if is_kill { Some(snapshot(now)) } else { None };
                offset += 1;
                conn.execute(
                    "INSERT INTO events (file, offset, ts, event, raw) VALUES ('replay', ?1, ?2, ?3, ?4)",
                    rusqlite::params![offset, ts, event, raw],
                )
                .unwrap();
                for c in callouts::from_event(v, &mut cstate) {
                    if c.text.starts_with("Objective complete") { objective_lines += 1; }
                    if c.text.starts_with("Mission redirected:") { redirected_lines += 1; }
                }
                if let Some(before) = before {
                    kill_events += 1;
                    let after = snapshot(now);
                    let mut credited_any = false;
                    for (id, (done_after, _, status_after, ..)) in &after {
                        if let Some((done_before, _, status_before, ..)) = before.get(id) {
                            if done_after > done_before {
                                credited_any = true;
                                credits_total += done_after - done_before;
                                *credited_systems.entry(*id).or_default().entry(here.clone().unwrap_or_else(|| "?".into())).or_default() += done_after - done_before;
                            }
                            if status_before == "Active" && status_after == "ReadyToTurnIn" { completions_at_kills += 1; }
                        }
                    }
                    if credited_any { kills_credited += 1; }
                    for c in mission_progress(&conn, now, v) {
                        if c.text.starts_with("Mission progress") { progress_lines += 1; progress_named.push(format!("{ts} {}", c.text)); }
                    }
                }
            }
            if let Some(c) = mission_redirected(&conn, &pass_now, &parsed) {
                if c.text.contains("complete") { complete_lines += 1; }
                if c.text.starts_with("Mission redirected:") { redirected_lines += 1; }
            }
        }
        csv.push_str(&format!("#totals,redirects={redirects},exact={exact},over={over},under={under}\n"));
        csv.push_str(&format!("#callouts,objective_complete={objective_lines},mission_complete={complete_lines},mission_redirected={redirected_lines},progress={progress_lines}\n"));
        csv.push_str(&format!("#kills,kill_events={kill_events},kills_crediting_a_mission={kills_credited},credits_total={credits_total},completions_inferred_at_kills={completions_at_kills}\n"));
        std::fs::write(&out_path, &csv).unwrap();
        std::fs::write(format!("{out_path}.progress.txt"), progress_named.join("\n")).unwrap();
        println!("{}", csv.lines().filter(|l| l.starts_with('#')).collect::<Vec<_>>().join("\n"));
        println!("wrote {out_path}");
    }
}
