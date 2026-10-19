//! Tap the live EDDN relay for NavRoute hops and write them to a TSV: the
//! instrument for measuring what a route's `StarClass` is worth before the
//! client uploads it anywhere (2026-10-19; the 2026-09-10 repair stopped
//! the server teaching classes from routes after a blind teaching changed
//! 27 % of the classes it touched).
//!
//!     navroute_tap <seconds> <out.tsv>
//!
//! One line per hop: timestamp, id64, name, StarClass (the journal letter,
//! empty when the route carries none), x, y, z. Appends and flushes per
//! message, so `navroute_audit` can read the file while the tap runs.
use anyhow::Result;
use ed_eddn::{Payload, EDDN_RELAY};
use std::io::Write;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(3600);
    let out = std::env::args().nth(2).unwrap_or_else(|| "navroute_hops.tsv".into());
    let mut file = std::io::BufWriter::new(std::fs::OpenOptions::new().create(true).append(true).open(&out)?);
    let deadline = Instant::now() + Duration::from_secs(secs);
    let started = Instant::now();
    let (mut routes, mut hops, mut classed) = (0u64, 0u64, 0u64);
    let stats = ed_eddn::live::run(EDDN_RELAY, |env, _| {
        if let Payload::NavRoute(m) = &env.payload {
            routes += 1;
            let ts = m.timestamp.clone().unwrap_or_default();
            for h in &m.route {
                let (Some(name), Some(addr)) = (h.star_system.as_deref(), h.system_address) else { continue };
                hops += 1;
                let class = h.star_class.as_deref().unwrap_or("").trim();
                if !class.is_empty() {
                    classed += 1;
                }
                let p = h.star_pos.unwrap_or([f64::NAN; 3]);
                let _ = writeln!(file, "{ts}\t{addr}\t{}\t{class}\t{}\t{}\t{}", name.replace('\t', " "), p[0], p[1], p[2]);
            }
            let _ = file.flush();
            if routes % 100 == 0 {
                eprintln!("{:>6} s  {routes} routes  {hops} hops  {classed} with a class", started.elapsed().as_secs());
            }
        }
        Instant::now() < deadline
    })
    .await?;
    let secs = started.elapsed().as_secs_f64().max(0.001);
    println!(
        "{} s: {} messages received, {routes} navroute messages, {hops} hops ({:.0}/h), {classed} with a StarClass, {} reconnects -> {out}",
        secs as u64,
        stats.received,
        hops as f64 / secs * 3600.0,
        stats.reconnects
    );
    Ok(())
}
