//! What a Tauri command costs when it opens a fresh connection on a WAL
//! store and runs one derivation, repeated: the HUD does this for every
//! "journal changed" signal, seven commands at a time.
//!
//!   cargo run --release -p ed-store --example conn_churn -- <store.sqlite3> <iterations>
fn main() -> anyhow::Result<()> {
    let db = std::env::args().nth(1).expect("store path");
    let n: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let now = ed_store::session::iso_from_epoch(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64);
    for i in 1..=n {
        let t = std::time::Instant::now();
        let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
        conn.execute_batch("PRAGMA temp_store = MEMORY; PRAGMA cache_size = -16000;")?;
        let t_open = t.elapsed();
        let t = std::time::Instant::now();
        let live = ed_store::missions::active(&conn, &now)?;
        let t_q = t.elapsed();
        drop(conn);
        println!("iter {i:>2}: open {:>7.2} ms | missions::active {:>7.2} ms ({} live)", t_open.as_secs_f64() * 1e3, t_q.as_secs_f64() * 1e3, live.len());
    }
    // Hold so the caller can read this process's IO counters.
    std::thread::sleep(std::time::Duration::from_secs(3));
    Ok(())
}
