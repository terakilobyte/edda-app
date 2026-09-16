
/// Replay of the maintainer's journal through the real callout code, one
/// watcher pass at a time. Not a unit test: run it by hand with
/// `EDDA_REPLAY_DB=<path> cargo test -p edda replay_journal -- --ignored --nocapture`.
/// Timestamps are shifted forward so every mission is live at the real
/// clock the functions read (`now_iso`), which keeps `active()` honest.
#[cfg(test)]
mod replay {
    use super::*;
    use std::collections::{BTreeMap, HashSet};

    const KINDS: &str = "('MissionAccepted','MissionCompleted','MissionFailed','MissionAbandoned','MissionRedirected','CargoDepot','Bounty','FactionKillBond')";
    const PASS_GAP_SECS: i64 = 1;

    fn parse(ts: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(ts).unwrap().with_timezone(&chrono::Utc)
    }
    fn shift_iso(ts: &str, by: chrono::Duration) -> String {
        (parse(ts) + by).format("%Y-%m-%dT%H:%M:%SZ").to_string()
    }
    /// Shift every `"timestamp"` and `"Expiry"` in the raw JSON.
    fn shift_raw(raw: &str, by: chrono::Duration) -> String {
        let mut v: Value = serde_json::from_str(raw).unwrap();
        for key in ["timestamp", "Expiry"] {
            if let Some(s) = v.get(key).and_then(Value::as_str).map(str::to_string) {
                v[key] = Value::String(shift_iso(&s, by));
            }
        }
        v.to_string()
    }

    #[test]
    #[ignore]
    fn replay_journal() {
        let Ok(path) = std::env::var("EDDA_REPLAY_DB") else { return };
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
        assert!(!rows.is_empty(), "no events since {since}");
        // Everything moves forward so the earliest event is a day from now.
        let by = chrono::Utc::now() + chrono::Duration::days(1) - parse(&rows[0].0);

        let conn = rusqlite::Connection::open_in_memory().unwrap();
        ed_store::schema::migrate(&conn).unwrap();
        ed_store::schema::attach_galaxy(&conn, None).unwrap();

        // Passes: consecutive events no more than PASS_GAP_SECS apart.
        let mut passes: Vec<Vec<(String, String, String)>> = Vec::new();
        let mut prev: Option<chrono::DateTime<chrono::Utc>> = None;
        for row in rows {
            let t = parse(&row.0);
            if prev.is_some_and(|p| (t - p).num_seconds() > PASS_GAP_SECS) || passes.is_empty() {
                passes.push(Vec::new());
            }
            passes.last_mut().unwrap().push(row);
            prev = Some(t);
        }

        let now = crate::commands::now_iso();
        let mut offset = 0i64;
        let (mut kills, mut credited, mut post_stack_kills, mut post_stack_lines) = (0, 0, 0, 0);
        let mut by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut complete_from_kills = 0;
        let mut redirects = 0;
        let mut announced_ids: HashSet<i64> = HashSet::new();
        let mut repeat_announcements = 0;
        let mut described_from_store = 0;
        let mut fell_back = 0;
        let mut plural = 0;
        let mut progress_named_nearest = 0;
        let mut progress_lines = 0;
        let mut log: Vec<String> = Vec::new();

        for pass in &passes {
            let parsed: Vec<Value> = pass.iter().map(|(_, _, raw)| serde_json::from_str(&shift_raw(raw, by)).unwrap()).collect();
            for (i, (ts, event, raw)) in pass.iter().enumerate() {
                offset += 1;
                conn.execute(
                    "INSERT INTO events (file, offset, ts, event, raw) VALUES ('replay', ?1, ?2, ?3, ?4)",
                    rusqlite::params![offset, shift_iso(ts, by), event, shift_raw(raw, by)],
                )
                .unwrap();
                let v = &parsed[i];
                match event.as_str() {
                    "Bounty" | "FactionKillBond" => {
                        kills += 1;
                        let victim = v.get("VictimFaction").and_then(Value::as_str);
                        let live = ed_store::missions::active(&conn, &now).unwrap();
                        let matching: Vec<_> = live.iter().filter(|m| m.kill_count.is_some() && m.target_faction.as_deref() == victim).collect();
                        if matching.is_empty() {
                            continue;
                        }
                        credited += 1;
                        let any_active = matching.iter().any(|m| m.status == ed_store::missions::MissionStatus::Active);
                        let nearest = matching
                            .iter()
                            .filter(|m| m.status == ed_store::missions::MissionStatus::Active)
                            .min_by_key(|m| ed_store::missions::kills_remaining(m).unwrap_or(i64::MAX))
                            .map(|m| (m.kills_done, m.kill_count.unwrap()));
                        let out = mission_progress(&conn, v);
                        if !any_active {
                            post_stack_kills += 1;
                            post_stack_lines += out.len();
                        }
                        for c in &out {
                            *by_kind.entry(c.kind).or_default() += 1;
                            if c.text.contains("complete") {
                                complete_from_kills += 1;
                            }
                            if c.text.starts_with("Mission progress") {
                                progress_lines += 1;
                                if let Some((done, total)) = nearest {
                                    if c.text.contains(&format!("{done} of {total} ")) {
                                        progress_named_nearest += 1;
                                    }
                                }
                            }
                        }
                        if credited % 50 == 0 || !any_active && post_stack_kills <= 3 {
                            log.push(format!("{} kill#{credited:<4} active={any_active} -> {}", &ts[5..19], out.iter().map(|c| format!("[{}] {}", c.kind, c.text)).collect::<Vec<_>>().join(" | ")));
                        }
                    }
                    "MissionRedirected" => redirects += 1,
                    _ => {}
                }
            }
            let ids: Vec<i64> = parsed
                .iter()
                .filter(|v| v.get("event").and_then(Value::as_str) == Some("MissionRedirected"))
                .filter_map(|v| v.get("MissionID").and_then(Value::as_i64))
                .collect();
            if let Some(c) = REDIRECT_FN(&conn, &parsed) {
                *by_kind.entry(c.kind).or_default() += 1;
                if ids.len() > 1 {
                    plural += 1;
                }
                for id in &ids {
                    if !announced_ids.insert(*id) {
                        repeat_announcements += 1;
                    }
                }
                let stored: Vec<_> = ed_store::missions::active(&conn, &now).unwrap();
                for id in &ids {
                    match stored.iter().find(|m| m.id == *id) {
                        Some(m) if c.text.contains(&m.faction) => described_from_store += 1,
                        _ => fell_back += 1,
                    }
                }
                log.push(format!("{} pass of {} events -> [{}] {}", &pass[0].0[5..19], pass.len(), c.kind, c.text));
            }
        }
        println!("{}", log.join("\n"));
        println!("\npasses: {} (gap <= {PASS_GAP_SECS} s); kill events {kills}, credited to a live mission {credited}", passes.len());
        println!("kills after the last Active mission for that faction went ready: {post_stack_kills} kills -> {post_stack_lines} utterances");
        println!("completion wording produced by a KILL: {complete_from_kills}");
        println!("progress lines: {progress_lines}, naming the mission nearest to done: {progress_named_nearest}");
        println!("MissionRedirected events: {redirects}; announced from the store (giver named): {described_from_store}; fell back to the game's wording: {fell_back}; repeat announcements of one mission: {repeat_announcements}; plural passes: {plural}");
        println!("by kind: {by_kind:?}");
    }
}
