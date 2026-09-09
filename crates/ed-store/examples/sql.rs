//! Run one read-only SQL statement against the default database.
//!
//!     cargo run -p ed-store --example sql --release -- "SELECT COUNT(*) FROM events"
//!
//! For poking at real data when a result looks wrong. Read-only: the
//! connection is opened with `SQLITE_OPEN_READ_ONLY`.

use rusqlite::{types::ValueRef, Connection, OpenFlags};

fn main() -> anyhow::Result<()> {
    let sql = std::env::args().nth(1).unwrap_or_else(|| "SELECT 1".into());
    let conn = Connection::open_with_flags(
        ed_store::Store::default_db_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut stmt = conn.prepare(&sql)?;
    let cols: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    println!("{}", cols.join(" | "));
    let n = cols.len();
    let mut rows = stmt.query([])?;
    let started = std::time::Instant::now();
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let cells: Vec<String> = (0..n)
            .map(|i| match row.get_ref(i) {
                Ok(ValueRef::Null) => "NULL".into(),
                Ok(ValueRef::Integer(v)) => v.to_string(),
                Ok(ValueRef::Real(v)) => format!("{v:.3}"),
                Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).into_owned(),
                Ok(ValueRef::Blob(b)) => format!("<{} bytes>", b.len()),
                Err(e) => format!("<err {e}>"),
            })
            .collect();
        println!("{}", cells.join(" | "));
        count += 1;
    }
    eprintln!("({count} rows, {:.3}s)", started.elapsed().as_secs_f64());
    Ok(())
}
