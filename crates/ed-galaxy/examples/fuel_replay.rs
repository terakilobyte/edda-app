//! Item 31 probe (a): replay an archived route through OUR fuel model.
//!
//!     cargo run -p ed-galaxy --example fuel_replay --release -- <route.json>
//!
//! Accepts either a Spansh exact-plotter result (`{"jumps":[{distance,
//! fuel_used, fuel_in_tank, has_neutron, must_refuel, name, ...}]}` or
//! the object under its `result` key) or a bench `--route-out` line.
//! Two passes over the hops, both against the bench Caspian model:
//!
//! 1. HOP LEGALITY: from the source ledger's own claimed tank state at
//!    each departure, does our `FuelModel::jump` permit the hop (reach,
//!    per-jump burn cap, reserve floor), and how far does our predicted
//!    burn drift from theirs? Answers "is their stride legal under our
//!    physics" without trusting either side's simulation end to end.
//! 2. END-TO-END: start full, scoop to full only at their marked refuel
//!    stops, burn by our model the whole way. Answers "could we FLY
//!    their plan" — a hop can be legal from their claimed state but
//!    unreachable from ours if burns drift.
//!
//! No galaxy index needed; this runs anywhere the repo does.

use ed_galaxy::fuel::{BoostProfile, FuelModel};

#[derive(Debug)]
struct Hop {
    name: String,
    dist: f32,       // ly, jump INTO this row (0 for the source row)
    fuel_used: f32,  // source ledger's burn for that jump
    fuel_at: f32,    // source ledger's tank on arrival
    departs_boosted: bool,
    refuel: bool,
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("usage: fuel_replay <route.json>");
    let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    let rows = raw
        .get("result")
        .unwrap_or(&raw)
        .get("jumps")
        .or_else(|| raw.get("hops"))
        .and_then(|j| j.as_array())
        .expect("no jumps/hops array")
        .clone();
    let f32of = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
    let hops: Vec<Hop> = rows
        .iter()
        .map(|r| Hop {
            name: r.get("name").and_then(|n| n.as_str()).unwrap_or("?").to_string(),
            dist: {
                let d = f32of(r, "distance");
                if d > 0.0 { d } else { f32of(r, "distance_ly") }
            },
            fuel_used: f32of(r, "fuel_used"),
            fuel_at: {
                let t = f32of(r, "fuel_in_tank");
                if t > 0.0 { t } else { f32of(r, "fuel_after") }
            },
            departs_boosted: r.get("has_neutron").and_then(|b| b.as_bool()).unwrap_or(false),
            refuel: r.get("must_refuel").and_then(|b| b.as_bool()).unwrap_or(false),
        })
        .collect();

    // The bench Caspian: the same construction bench.rs uses.
    let m = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
    let boost = BoostProfile::MK2_SCO;

    // Pass 1: hop legality from the ledger's own departure states.
    let (mut illegal, mut max_drift, mut our_burn_total, mut their_burn_total) = (0usize, 0.0f32, 0.0f32, 0.0f32);
    for i in 1..hops.len() {
        let dep = &hops[i - 1];
        let arr = &hops[i];
        let b = if dep.departs_boosted { boost.neutron } else { 1.0 };
        let fuel = if dep.fuel_at > 0.0 { dep.fuel_at } else { m.capacity };
        let ours = m.fuel_for(arr.dist, fuel, b);
        let drift = (ours - arr.fuel_used).abs();
        max_drift = max_drift.max(drift);
        our_burn_total += ours;
        their_burn_total += arr.fuel_used;
        if m.jump(arr.dist, fuel, b).is_none() {
            illegal += 1;
            println!(
                "  ILLEGAL hop {i}: {} -> {} {:.1} ly boost x{b} | tank {:.1} t, our reach {:.1}, our burn {:.2} (cap {:.1}), theirs {:.2}",
                dep.name, arr.name, arr.dist, fuel, m.reach(fuel, b), ours, m.max_fuel_per_jump, arr.fuel_used
            );
        }
    }
    println!(
        "pass 1 (from their claimed states): {} of {} hops legal under our model; burn drift max {:.2} t/hop; totals ours {:.0} t vs theirs {:.0} t",
        hops.len() - 1 - illegal,
        hops.len() - 1,
        max_drift,
        our_burn_total,
        their_burn_total
    );

    // Pass 2: end-to-end under our burns, scooping only at their stops.
    let mut fuel = m.capacity;
    let (mut stranded, mut scooped) = (None, 0.0f32);
    for i in 1..hops.len() {
        let dep = &hops[i - 1];
        let arr = &hops[i];
        let b = if dep.departs_boosted { boost.neutron } else { 1.0 };
        match m.jump(arr.dist, fuel, b) {
            Some(left) => fuel = left,
            None => {
                stranded = Some((i, arr.name.clone(), fuel));
                break;
            }
        }
        if arr.refuel {
            scooped += m.capacity - fuel;
            fuel = m.capacity;
        }
    }
    match stranded {
        Some((i, name, tank)) => println!(
            "pass 2 (our burns end-to-end): STRANDED at hop {i} ({name}), tank {tank:.1} t — their plan is not flyable verbatim under our model"
        ),
        None => println!(
            "pass 2 (our burns end-to-end): FLYABLE — arrives with {:.1} t, scooping {:.0} t at their {} stops",
            fuel,
            scooped,
            hops.iter().filter(|h| h.refuel).count()
        ),
    }
    Ok(())
}
