//! Single-account A/B, side A: the 0.4.8 app path (one Store, Store::sync).
//! Copied into a v0.4.8 checkout as crates/ed-store/examples/ab_single.rs by run.sh.
//!     ab_single <journal_dir> <fresh_dir> <counts.csv>
use anyhow::{Context, Result};
use std::time::Instant;

fn main() -> Result<()> {
    let mut a = std::env::args().skip(1);
    let journal = std::path::PathBuf::from(a.next().context("journal")?);
    let dir = std::path::PathBuf::from(a.next().context("dir")?);
    let counts = std::path::PathBuf::from(a.next().context("counts")?);
    std::fs::create_dir_all(&dir)?;
    let db = dir.join("edda.sqlite3");
    let store = ed_store::Store::open(&db, &journal)?;
    let t = Instant::now();
    store.sync()?;
    println!("cold_s {:.2}", t.elapsed().as_secs_f64());
    let mut quiet = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        store.sync()?;
        quiet.push(t.elapsed().as_millis());
    }
    println!("quiet_ms {:?}", quiet);
    write_counts(store.conn(), &counts)?;
    println!("db_mb {:.1}", std::fs::metadata(&db)?.len() as f64 / 1e6);
    Ok(())
}

fn write_counts(conn: &rusqlite::Connection, out: &std::path::Path) -> Result<()> {
    let mut names: Vec<String> = conn
        .prepare("SELECT name FROM main.sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    names.sort();
    let mut s = String::from("table,rows\n");
    for n in names {
        let c: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM main.\"{n}\""), [], |r| r.get(0))?;
        s.push_str(&format!("{n},{c}\n"));
    }
    std::fs::write(out, s)?;
    Ok(())
}
