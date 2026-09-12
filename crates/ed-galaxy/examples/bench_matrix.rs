//! The route benchmark matrix: every anchor pair in BOTH directions, on
//! both ships, with and without the fuel model, with and without the
//! cell-graph-first seed. CSV on stdout, one line per run, budget-capped
//! so pathological cells report instead of hanging the grid.
//!
//!     bench_matrix <index_dir> [budget_s] > matrix.csv

use ed_galaxy::fuel::{BoostProfile, FuelModel};
use ed_galaxy::long_range::plan_best;
use ed_galaxy::router::{Control, RouteRequest};
use ed_galaxy::Galaxy;
use std::time::Instant;

const PAIRS: &[(&str, &str)] = &[
    ("Wongi", "Colonia"),
    ("Sol", "Sagittarius A*"),
    ("Wongi", "Spase AA-A a108-0"),
    ("Colonia", "Spase AA-A a108-0"),
    ("Wongi", "Beagle Point"),
    ("Wongi", "Plielou RN-R d5-27"),
    ("Sol", "Byoomao AA-A c1"),
    ("Pheia Auscs AA-A d0", "Spase AA-A a108-0"),
    ("Wongi", "Blaa Hypai AA-A a96-19"),
];

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = std::path::PathBuf::from(
        args.next()
            .expect("usage: bench_matrix <index_dir> [budget_s]"),
    );
    let budget_s: u64 = args.next().and_then(|v| v.parse().ok()).unwrap_or(20);
    let g = Galaxy::open(&dir)?;
    let neutrons = Galaxy::open(&ed_galaxy::long_range::neutron_dir(&dir))?;
    eprintln!(
        "{} systems, {} highway; budget {budget_s}s per run",
        g.count, neutrons.count
    );
    println!(
        "from,to,ship,fuel,cgraph,jumps,total_ly,boosted,refuels,expansions,ms,variants,status"
    );

    let ships: [(&str, FuelModel, BoostProfile); 2] = [
        (
            "explorer",
            FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0),
            BoostProfile::MK2_SCO,
        ),
        (
            "mandalay",
            FuelModel::from_loadout(319.2, 32.0, 5.0, 5, true, false, 77.86, 10.5, 0.0),
            BoostProfile::default(),
        ),
    ];
    for &(a, b) in PAIRS {
        for (from_name, to_name) in [(a, b), (b, a)] {
            let (Some(from), Some(to)) = (g.find(from_name), g.find(to_name)) else {
                eprintln!("unknown pair {from_name} -> {to_name}");
                continue;
            };
            for (ship, model, boost) in &ships {
                for fuel_on in [true, false] {
                    // Fuel-off is a niche view; keep the grid affordable.
                    if !fuel_on
                        && !matches!(
                            (from_name, to_name),
                            ("Wongi", "Colonia")
                                | ("Colonia", "Wongi")
                                | ("Colonia", "Spase AA-A a108-0")
                                | ("Spase AA-A a108-0", "Colonia")
                        )
                    {
                        continue;
                    }
                    for cgraph in [true, false] {
                        if cgraph {
                            std::env::remove_var("ED_NO_CGRAPH");
                        } else {
                            std::env::set_var("ED_NO_CGRAPH", "1");
                        }
                        // White dwarfs stay at the app default (off).
                        let boost = BoostProfile {
                            white_dwarf: 1.0,
                            ..*boost
                        };
                        let req = RouteRequest {
                            from,
                            to,
                            range_ly: model.range_at(model.capacity),
                            supercharge: true,
                            boost,
                            fuel: fuel_on.then_some(*model),
                            start_fuel: model.capacity,
                            time_budget_ms: budget_s * 1000,
                            grace_ms: 1_000,
                            ..Default::default()
                        };
                        let t = Instant::now();
                        let line = match plan_best(&g, Some(&neutrons), &req, &Control::none()) {
                            Ok(r) => format!(
                                "{},{},{},{},{},{},{:.0},{},{},{},{},{}/{},ok",
                                from_name,
                                to_name,
                                ship,
                                fuel_on,
                                cgraph,
                                r.jumps,
                                r.total_ly,
                                r.boosted_jumps,
                                r.refuel_stops,
                                r.expansions,
                                t.elapsed().as_millis(),
                                r.variants_finished,
                                r.variants_run
                            ),
                            Err(e) => format!(
                                "{},{},{},{},{},,,,,,{},,{:?}",
                                from_name,
                                to_name,
                                ship,
                                fuel_on,
                                cgraph,
                                t.elapsed().as_millis(),
                                e
                            ),
                        };
                        println!("{line}");
                        eprintln!("{line}");
                    }
                }
            }
        }
    }
    Ok(())
}
