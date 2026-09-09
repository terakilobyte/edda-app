//! Connect to the live EDDN relay and report what arrives.
//!
//!     cargo run -p ed-eddn --example listen --release -- [seconds]
//!
//! A smoke test for the feed: no database, no writes. Useful for confirming
//! the relay is reachable and for seeing how quickly a given schema actually
//! shows up, which is the honest answer to "how stale is our market data".

use anyhow::Result;
use ed_eddn::{Payload, EDDN_RELAY};
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    println!(
        "connecting to {EDDN_RELAY} for {secs}s as {}\n",
        ed_eddn::USER_AGENT
    );
    let deadline = Instant::now() + Duration::from_secs(secs);
    let started = Instant::now();
    let mut samples: Vec<String> = Vec::new();

    let stats = ed_eddn::live::run(EDDN_RELAY, |env, stats| {
        if samples.len() < 8 {
            let line = match &env.payload {
                Payload::Commodity(m) => format!(
                    "commodity  {:<28} {:<24} {} commodities",
                    m.system_name,
                    m.station_name.as_deref().unwrap_or("-"),
                    m.commodities.len()
                ),
                Payload::Journal(m) => format!(
                    "journal    {:<28} {:<24} {}",
                    m.star_system.as_deref().unwrap_or("-"),
                    m.event.as_deref().unwrap_or("-"),
                    m.controlling_power
                        .as_deref()
                        .map(|p| format!("power: {p}"))
                        .unwrap_or_default()
                ),
                Payload::NavRoute(m) => format!("navroute   {} hops", m.route.len()),
                Payload::Outfitting(m) => {
                    format!(
                        "outfitting {:<28} {} modules",
                        m.system_name,
                        m.modules.len()
                    )
                }
                Payload::Shipyard(m) => {
                    format!("shipyard   {:<28} {} ships", m.system_name, m.ships.len())
                }
                Payload::Other { schema } => format!("other      {schema}"),
            };
            println!("  {line}");
            samples.push(line);
        }
        let _ = stats;
        Instant::now() < deadline
    })
    .await?;

    let secs = started.elapsed().as_secs_f64().max(0.001);
    println!("\n─ {:.0}s of live feed ─────────────────────", secs);
    println!(
        "  received       {:>7}  ({:.1}/s)",
        stats.received,
        stats.received as f64 / secs
    );
    println!("  decoded        {:>7}", stats.decoded);
    println!("  decode errors  {:>7}", stats.decode_errors);
    println!("  commodity      {:>7}", stats.commodity);
    println!("  journal        {:>7}", stats.journal);
    println!("  outfitting     {:>7}", stats.outfitting);
    println!("  shipyard       {:>7}", stats.shipyard);
    println!("  other schemas  {:>7}", stats.other);
    println!("  reconnects     {:>7}", stats.reconnects);
    Ok(())
}
