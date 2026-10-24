//! Replay a journal folder through the registry: one store per commander.
//!
//!     cargo run -p ed-store --release --example multibox_replay -- <journal_dir> <scratch_dir>
//!
//! Prints counts and labels only: no name, FID, system or balance from the
//! folder is printed (FIDs are shown as A, B, ... in first-seen order).
//! Record: docs/benches/2026-10-24-multibox-replay.csv.
use std::path::PathBuf;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let journal = PathBuf::from(args.next().expect("usage: multibox_replay <journal_dir> <scratch_dir>"));
    let scratch = PathBuf::from(args.next().expect("usage: multibox_replay <journal_dir> <scratch_dir>"));
    std::fs::create_dir_all(&scratch)?;
    let base = scratch.join("edda.sqlite3");

    let t0 = Instant::now();
    let reg = ed_store::registry::Registry::open(&base, &journal)?;
    let open_ms = t0.elapsed().as_millis();
    let t1 = Instant::now();
    let mut last = Instant::now();
    let out = reg.sync_all(|name, i, total| {
        if last.elapsed().as_secs() >= 30 || i == total {
            last = Instant::now();
            println!("  [{i:>4}/{total}] {name} at {:.1} s", t1.elapsed().as_secs_f64());
        }
    })?;
    let sync_s = t1.elapsed().as_secs_f64();

    let fids = reg.fids();
    let label = |fid: &str| -> String {
        fids.iter()
            .position(|f| f == fid)
            .map(|i| ((b'A' + i as u8) as char).to_string())
            .unwrap_or_else(|| "?".into())
    };
    println!("\n- registry ---------------------------------");
    println!("  open             {open_ms} ms");
    println!("  sync_all         {sync_s:.1} s");
    println!("  commanders       {}", fids.len());
    println!("  new_fids         {}", out.new_fids.len());
    println!("  menu_only_files  {}", out.menu_only_files);
    println!("  pending_files    {}", out.pending_files);
    println!("  last_writer      {}", reg.last_writer().as_deref().map(label).unwrap_or_default());
    let mut total_events = 0i64;
    let mut purity_ok = true;
    for (fid, store) in reg.stores() {
        let g = store.lock().unwrap();
        let conn = g.conn();
        let events: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        let distinct: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT json_extract(raw,'$.FID')) FROM events WHERE event IN ('Commander','LoadGame') AND json_extract(raw,'$.FID') IS NOT NULL",
            [],
            |r| r.get(0),
        )?;
        let files: i64 = conn.query_row("SELECT COUNT(*) FROM journal_files", [], |r| r.get(0))?;
        let credits = ed_store::session::credits_estimate(conn)?.map(|_| "known").unwrap_or("none");
        let bytes = std::fs::metadata(ed_store::registry::Registry::db_path_for(&base, &fid))
            .map(|m| m.len())
            .unwrap_or(0);
        if distinct != 1 {
            purity_ok = false;
        }
        println!(
            "  store {}: events {events}, files {files}, distinct FIDs {distinct}, credits estimate {credits}, {:.1} MB",
            label(&fid),
            bytes as f64 / 1e6
        );
        total_events += events;
    }
    println!("  total events     {total_events}");
    println!("  purity           {}", if purity_ok { "ok: one FID per store" } else { "FAILED: a store holds two FIDs" });
    println!("\n- companions -------------------------------");
    for (name, att) in &out.companions {
        let to = match att {
            ed_store::registry::Attribution::To(v) => v.iter().map(|f| label(f)).collect::<Vec<_>>().join("+"),
            ed_store::registry::Attribution::Unknown => "unknown".into(),
        };
        println!("  {name:<18} -> {to}");
    }

    println!("\n- quiet sync_all x3 ------------------------");
    for pass in 1..=3 {
        let t = Instant::now();
        let q = reg.sync_all(|_, _, _| {})?;
        let inserted: u64 = q.per_store.values().map(|s| s.ingest.events_inserted).sum();
        println!(
            "  pass {pass}: {} ms (events inserted {inserted}, snapshots {})",
            t.elapsed().as_millis(),
            q.per_store.values().map(|s| s.ingest.snapshots_updated).sum::<usize>()
        );
    }
    Ok(())
}
