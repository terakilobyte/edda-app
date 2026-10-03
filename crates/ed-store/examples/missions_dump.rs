//! Dump the mission derivation for a store, read-only, with the
//! speculative kill estimate on: what the Missions tab would show.
//!
//!   cargo run -p ed-store --example missions_dump -- "C:/Users/you/AppData/Local/edda/edda.sqlite3"
//!
//! A knob for checking the estimate against a real journal without
//! touching the app (2026-10-03, the boss's 81-kill live test).
fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("store path");
    let conn = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let now = chrono_free_now();
    let ms = ed_store::missions::missions_with(&conn, "", &now, true)?;
    println!("{:<12} {:<16} {:<28} {:<24} {:>5} {:>6} {:<20}", "id", "status", "giver", "target", "goal", "seen", "accepted");
    for m in ms.iter().filter(|m| m.kill_count.is_some() && matches!(m.status, ed_store::missions::MissionStatus::Active | ed_store::missions::MissionStatus::ReadyToTurnIn)) {
        println!(
            "{:<12} {:<16} {:<28} {:<24} {:>5} {:>6} {:<20}",
            m.id,
            format!("{:?}", m.status),
            m.faction.chars().take(28).collect::<String>(),
            m.target_faction.clone().unwrap_or_default().chars().take(24).collect::<String>(),
            m.kill_count.unwrap_or(0),
            format!("{}{}", if m.kills_exact { "=" } else { "≥" }, m.kills_seen.unwrap_or(0)),
            &m.accepted[..16],
        );
    }
    Ok(())
}

/// ISO now without pulling a date crate into an example.
fn chrono_free_now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    ed_store::session::iso_from_epoch(secs)
}
