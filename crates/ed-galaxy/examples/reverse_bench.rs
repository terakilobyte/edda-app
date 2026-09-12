//! Bench the reverse-planning idea: hard-direction plots (uphill into
//! thin space) grind while the return trip converges fast, so plan the
//! easy direction and flip the hops — IF the flipped route survives the
//! fuel model, since boosts come from the departure star and scoop stops
//! land differently in reverse.
//!
//!     reverse_bench <index> <hard_from> <hard_to> [--ship mandalay]
//!
//! Reports: direct hard plot, easy plot, and the flipped-easy route's
//! forward fuel simulation (repairable = a scoop was needed at a star
//! that has one; broken = a hop no tank state can fund).

use ed_galaxy::fuel::{BoostProfile, FuelModel};
use ed_galaxy::long_range::plan_best;
use ed_galaxy::router::{Control, Route, RouteRequest};
use ed_galaxy::Galaxy;
use std::path::Path;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().map(String::as_str).unwrap_or(".data/galaxy"));
    let from = args.get(1).map(String::as_str).unwrap_or("SynthStart");
    let to = args.get(2).map(String::as_str).unwrap_or("SynthGoal");
    let ship = args
        .iter()
        .position(|a| a == "--ship")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("explorer");

    let g = Galaxy::open(dir)?;
    let ndir = ed_galaxy::long_range::neutron_dir(dir);
    let neutrons = Galaxy::open(&ndir)?;
    let (model, boost) = match ship {
        "mandalay" => (
            FuelModel::from_loadout(319.2, 32.0, 5.0, 5, true, false, 77.86, 10.5, 0.0),
            BoostProfile::default(),
        ),
        _ => (
            FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0),
            BoostProfile::MK2_SCO,
        ),
    };
    let boost = BoostProfile {
        white_dwarf: 1.0,
        ..boost
    };
    let a = g
        .find(from)
        .ok_or_else(|| anyhow::anyhow!("unknown {from}"))?;
    let b = g.find(to).ok_or_else(|| anyhow::anyhow!("unknown {to}"))?;
    let ctl = Control {
        cancelled: &|| false,
        progress: &|_, _| {},
        stage: &|_, _, _| {},
        found: &|_| {},
        trace: &|_, _, _| {},
    };
    let req = |from: u32, to: u32| RouteRequest {
        from,
        to,
        range_ly: model.range_at(model.capacity),
        supercharge: true,
        boost,
        fuel: Some(model),
        start_fuel: model.capacity,
        thorough: false,
        grace_ms: 1000,
        ..Default::default()
    };

    let plot = |from: u32, to: u32| -> anyhow::Result<(Route, u128)> {
        let t = Instant::now();
        let r = plan_best(&g, Some(&neutrons), &req(from, to), &ctl)?;
        Ok((r, t.elapsed().as_millis()))
    };

    let (hard, hard_ms) = plot(a, b)?;
    let (easy, easy_ms) = plot(b, a)?;
    println!(
        "direct  {from} -> {to}: {} jumps, {} refuels, {hard_ms} ms",
        hard.jumps, hard.refuel_stops
    );
    println!(
        "easy    {to} -> {from}: {} jumps, {} refuels, {easy_ms} ms",
        easy.jumps, easy.refuel_stops
    );

    // Flip the easy route and simulate it forward with the fuel model.
    // Node facts are direction-independent: a star that can refuel you
    // (scoopable main, or the companion the refuel flag proves) refuels
    // you arriving from either side; the boost for a jump comes from the
    // star you DEPART, which in reverse is the other endpoint.
    let t = Instant::now();
    let hops: Vec<_> = easy.hops.iter().rev().collect();
    let can_scoop = |h: &ed_galaxy::router::Hop| h.scoopable || h.refuel;
    let mut fuel = model.capacity;
    let mut scoops = 0u32;
    let mut repairable = 0u32;
    let mut broken = 0u32;
    for w in hops.windows(2) {
        let (from_h, to_h) = (w[0], w[1]);
        let d = ed_galaxy::format::dist(from_h.pos, to_h.pos);
        let jump_boost = if from_h.class == ed_galaxy::StarClass::Neutron
            || from_h.class == ed_galaxy::StarClass::WhiteDwarf
        {
            boost.for_class(from_h.class)
        } else {
            1.0
        };
        // Eager top-up wherever the departure star allows it.
        if can_scoop(from_h) && fuel < model.capacity - 0.5 {
            fuel = model.capacity;
            scoops += 1;
        }
        match model.jump(d, fuel, jump_boost) {
            Some(left) => fuel = left,
            None => {
                // A full tank couldn't fund it either -> truly broken;
                // otherwise a scoop stop insertion would repair it.
                if model.jump(d, model.capacity, jump_boost).is_some() && can_scoop(from_h) {
                    repairable += 1;
                    fuel = model.jump(d, model.capacity, jump_boost).unwrap();
                } else {
                    broken += 1;
                    fuel = model.capacity; // keep walking to count all trouble
                }
            }
        }
    }
    let sim_us = t.elapsed().as_micros();
    println!(
        "flipped {from} -> {to}: {} jumps, {scoops} scoops in sim, {repairable} repairable, {broken} broken, sim {sim_us} us",
        hops.len().saturating_sub(1)
    );
    println!(
        "naive : direct {hard_ms} ms / {} j  vs  flipped {} ms / {} j ({} infeasible)",
        hard.jumps,
        easy_ms + (sim_us / 1000),
        hops.len().saturating_sub(1),
        repairable + broken
    );

    // The real proposal: reverse the CHAIN (nodes are direction-agnostic)
    // and re-refine every leg in the hard direction — boosts and scoops
    // re-chosen by the same leg planner every route uses.
    let t = Instant::now();
    let mfj = model.max_fuel_per_jump;
    // Waypoint only the structural nodes — the chain's refuel stops —
    // so each leg is multi-jump and the leg planner is free to re-choose
    // intermediate stars for the reversed direction (pinning every hop
    // measured +16% jumps: mandated nodes force filler jumps wherever a
    // reversed leg lost its departure boost).
    let mut waypoints: Vec<(u32, f32)> = vec![(a, model.capacity)];
    let stride: usize = args
        .iter()
        .position(|x| x == "--stride")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    for (i, h) in easy.hops.iter().rev().enumerate() {
        if h.idx != a && h.idx != b && (i % stride == 0 || h.refuel) {
            waypoints.push((h.idx, (model.capacity - mfj).max(0.0)));
        }
    }
    waypoints.push((b, 0.0));
    let hard_req = req(a, b);
    let straight = ed_galaxy::format::dist(g.record(a).pos(), g.record(b).pos());
    let refined = ed_galaxy::long_range::refine_waypoints(
        &g,
        &hard_req,
        &ctl,
        waypoints,
        model.capacity,
        Some(model),
        0,
        straight,
        Instant::now(),
    );
    let refine_ms = t.elapsed().as_millis();
    match refined {
        Ok(r) => println!(
            "refine: flipped-chain {} -> {}: {} jumps, {} refuels, easy {easy_ms} + refine {refine_ms} = {} ms  (direct: {} j / {hard_ms} ms)",
            from, to, r.jumps, r.refuel_stops, easy_ms + refine_ms, hard.jumps
        ),
        Err(e) => println!("refine: flipped-chain FAILED: {e:?} after {refine_ms} ms"),
    }
    Ok(())
}
