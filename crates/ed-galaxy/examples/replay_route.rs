//! Item 31 probe (a): replay an archived Spansh route through OUR fuel
//! physics. Answers "is their stride legal under our model?" hop by hop
//! -- if yes, the 505 s gap is a search failure on our side; if no,
//! it's model disagreement (and possibly their optimism overpromising).
//!
//!     replay_route <spansh.json> [--ship explorer]
//!
//! Uses the bench Explorer model verbatim. Refuels where their ledger
//! says must_refuel (tank to capacity, as their own ledger shows).
//! Prints every violation with our reach vs their throw, plus the
//! cumulative burn comparison (our physics vs their fuel_used).

use ed_galaxy::fuel::{BoostProfile, FuelModel};

#[derive(serde::Deserialize)]
struct Hop {
    distance: f32,
    fuel_in_tank: f32,
    fuel_used: f32,
    has_neutron: bool,
    #[serde(default)]
    must_refuel: bool,
    name: String,
    x: f32,
    y: f32,
    z: f32,
}

#[derive(serde::Deserialize)]
struct SpanshRoute {
    jumps: Vec<Hop>,
}

#[derive(serde::Deserialize)]
struct OurHop {
    distance_ly: f32,
    boosted: bool,
    fuel_after: Option<f32>,
    name: String,
}

#[derive(serde::Deserialize)]
struct OurRoute {
    from: String,
    to: String,
    ship: String,
    min_fuel: bool,
    jumps: usize,
    hops: Vec<OurHop>,
}

/// --ours <routes.jsonl>: replay every bench-archived route through the
/// SHIPPED model (default headroom) and count hops the safe margin
/// would forbid -- the load-bearing measure for optimistic plans.
fn replay_ours(path: &str) -> anyhow::Result<()> {
    let explorer = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
    let e86 = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 86.0, 10.5, 0.0);
    let mandalay = FuelModel::from_loadout(319.2, 32.0, 5.0, 5, true, false, 77.86, 10.5, 0.0);
    let boost = BoostProfile::MK2_SCO;
    let std_boost = BoostProfile::default();
    let (mut total_hops, mut total_viol, mut routes_hit, mut worst) = (0u32, 0u32, 0u32, 0.0f32);
    for line in std::io::BufRead::lines(std::io::BufReader::new(std::fs::File::open(path)?)) {
        let r: OurRoute = serde_json::from_str(&line?)?;
        let (model, bp) = match r.ship.as_str() {
            "mandalay" => (&mandalay, &std_boost),
            "explorer86" => (&e86, &boost),
            _ => (&explorer, &boost),
        };
        let mut fuel = model.capacity;
        let mut viol = 0u32;
        for i in 1..r.hops.len() {
            let h = &r.hops[i];
            let b = if h.boosted { bp.neutron } else { 1.0 };
            let reach = model.reach(fuel, b);
            if h.distance_ly > reach {
                viol += 1;
                worst = worst.max(h.distance_ly - reach);
            }
            total_hops += 1;
            if let Some(f) = h.fuel_after {
                fuel = f.min(model.capacity);
            }
        }
        total_viol += viol;
        if viol > 0 {
            routes_hit += 1;
            println!("  {} -> {} [{}{}]: {viol} hops the shipped margin forbids (of {})", r.from, r.to, r.ship, if r.min_fuel { " MF" } else { "" }, r.jumps);
        }
    }
    println!("
load-bearing: {total_viol}/{total_hops} hops forbidden by the shipped 10 t margin across {routes_hit} routes; worst overshoot {worst:.1} ly");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--ours") {
        return replay_ours(&std::env::args().nth(2).expect("--ours <routes.jsonl>"));
    }
    let path = std::env::args().nth(1).expect("usage: replay_route <spansh.json>");
    let route: SpanshRoute = serde_json::from_reader(std::fs::File::open(&path)?)?;
    let model = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
    let boost_profile = BoostProfile::MK2_SCO;
    let hops = &route.jumps;
    println!("{} hops ({} jumps) from {}", hops.len(), hops.len() - 1, hops[0].name);

    let mut fuel = hops[0].fuel_in_tank.min(model.capacity);
    let (mut violations, mut min_margin, mut our_burn, mut their_burn) = (0u32, f32::MAX, 0.0f32, 0.0f32);
    let mut refuels = 0u32;
    for i in 1..hops.len() {
        let (from, to) = (&hops[i - 1], &hops[i]);
        if from.must_refuel {
            fuel = model.capacity;
            refuels += 1;
        }
        let boost = if from.has_neutron { boost_profile.neutron } else { 1.0 };
        let geom = ed_galaxy::format::dist([from.x, from.y, from.z], [to.x, to.y, to.z]);
        if (geom - to.distance).abs() > 0.5 {
            println!("  hop {i}: coordinate/distance mismatch {geom:.1} vs {:.1}", to.distance);
        }
        let reach = model.reach(fuel, boost);
        let margin = reach - to.distance;
        min_margin = min_margin.min(margin);
        match model.jump(to.distance, fuel, boost) {
            Some(after) => {
                our_burn += fuel - after;
                fuel = after;
            }
            None => {
                violations += 1;
                let burn = model.fuel_for(to.distance, fuel, boost).min(model.max_fuel_per_jump);
                println!(
                    "  VIOLATION hop {i} -> {}: throw {:.1} ly, our reach {:.1} ly (fuel {:.1} t, x{boost}), short {:.1} ly",
                    to.name, to.distance, reach, fuel, to.distance - reach
                );
                our_burn += burn;
                fuel = (fuel - burn).max(0.0);
            }
        }
        their_burn += to.fuel_used;
        // Their ledger sanity: our simulated tank vs theirs on arrival.
        let drift = fuel - to.fuel_in_tank;
        if drift.abs() > 3.0 && !from.must_refuel {
            println!("  hop {i}: tank drift {drift:+.1} t (ours {fuel:.1}, theirs {:.1})", to.fuel_in_tank);
        }
    }
    println!(
        "\nverdict: {violations} violations over {} jumps; min margin {min_margin:.1} ly; {refuels} refuels taken",
        hops.len() - 1
    );
    println!(
        "burn: ours {our_burn:.1} t vs their ledger {their_burn:.1} t ({:+.1} t = model disagreement)",
        our_burn - their_burn
    );
    Ok(())
}
