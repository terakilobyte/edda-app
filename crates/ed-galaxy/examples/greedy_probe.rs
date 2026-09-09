//! Bench the economics of the density-adaptive greedy cone (item 13)
//! WITHOUT touching the planner: a pure greedy walker over the highway
//! sub-index that, each hop, considers only stars inside a cone toward
//! the goal in the outer band of boosted reach, escalating the cone
//! half-angle 10 -> 15 -> 20 -> 30 -> 45 -> 60 -> 90 -> 180 degrees
//! when empty. Answers two questions the design hinges on:
//!
//!   1. Quality: how many MORE jumps does pure greed cost vs plan_best
//!      in dense space? (If ~0, the dense-space search work is waste.)
//!   2. Work: how few candidates does greed actually need to look at,
//!      vs the full sphere the coarse search scans today?
//!
//! Fuel is out of scope here (the refine/min-fuel seam owns it); the
//! walker hops neutron-to-neutron at boosted reach, which is what the
//! coarse chain does too.
//!
//!     greedy_probe <index> [from] [to] [--to-xyz x,y,z] [--ship mandalay]
//!
//! `--to-xyz` snaps the goal to the nearest highway star, for testing
//! dense-to-dense pairs that don't have names.

use ed_galaxy::fuel::{BoostProfile, FuelModel};
use ed_galaxy::long_range::plan_best;
use ed_galaxy::router::{Control, RouteRequest};
use ed_galaxy::{format, Galaxy};
use std::path::Path;
use std::time::Instant;

const THETAS_DEG: [f32; 8] = [10.0, 15.0, 20.0, 30.0, 45.0, 60.0, 90.0, 180.0];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().map(String::as_str).unwrap_or(".data/galaxy"));
    let from = args.get(1).map(String::as_str).unwrap_or("SynthStart");
    let to = args.get(2).filter(|a| !a.starts_with("--")).map(String::as_str).unwrap_or("SynthGoal");
    let ship = args.iter().position(|a| a == "--ship").and_then(|i| args.get(i + 1)).map(String::as_str).unwrap_or("explorer");

    let g = Galaxy::open(dir)?;
    let sub = Galaxy::open(&ed_galaxy::long_range::neutron_dir(dir))?;
    let (model, boost) = match ship {
        "mandalay" => (FuelModel::from_loadout(319.2, 32.0, 5.0, 5, true, false, 77.86, 10.5, 0.0), BoostProfile::default()),
        _ => (FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0), BoostProfile::MK2_SCO),
    };
    let range = model.range_at(model.capacity);

    let a = g.find(from).ok_or_else(|| anyhow::anyhow!("unknown {from}"))?;
    let goal_pos: [f32; 3];
    let b_main: u32;
    if let Some(i) = args.iter().position(|x| x == "--to-xyz") {
        let p: Vec<f32> = args[i + 1].split(',').filter_map(|v| v.parse().ok()).collect();
        anyhow::ensure!(p.len() == 3, "--to-xyz wants x,y,z");
        // Snap to the nearest highway star so the goal is reachable in kind.
        let want = [p[0], p[1], p[2]];
        let mut best = (f32::MAX, 0u32);
        for i in 0..sub.count as u32 {
            let d = format::dist(sub.pos_of(i), want);
            if d < best.0 {
                best = (d, i);
            }
        }
        goal_pos = sub.pos_of(best.1);
        let r = sub.record(best.1);
        b_main = g.find(sub.name(&r)).ok_or_else(|| anyhow::anyhow!("snap target not in main index"))?;
        println!("goal snapped to {} at {:?} ({:.0} ly from asked)", sub.name(&r), goal_pos, best.0);
    } else {
        b_main = g.find(to).ok_or_else(|| anyhow::anyhow!("unknown {to}"))?;
        goal_pos = g.record(b_main).pos();
    }

    // --- greedy cone walk over the sub-index ---
    // The walker models the HIGHWAY CHAIN, so it starts at the nearest
    // highway star to the start; the start leg (several unboosted hops
    // through ordinary stars) is refine's job in the real planner and
    // is priced by the --refine arm here, never by the walk itself.
    let t = Instant::now();
    let start_pos = g.record(a).pos();
    let mut nearest = (f32::MAX, 0u32);
    for i in 0..sub.count as u32 {
        let d = format::dist(sub.pos_of(i), start_pos);
        if d < nearest.0 {
            nearest = (d, i);
        }
    }
    let mut pos = sub.pos_of(nearest.1);
    let mut cur_class = {
        let r = sub.record(nearest.1);
        sub.class(&r)
    };
    println!("chain starts {:.0} ly out, at the nearest highway star", nearest.0);
    let mut chain: Vec<u32> = vec![nearest.1]; // sub-index picks, in order
    let mut jumps = 0u32;
    let mut scanned = 0u64; // what the full sphere visits (today's cost)
    let mut in_cone = 0u64; // what the cone would have visited (greedy's cost)
    let mut widens = 0u32;
    let mut theta_hist = [0u32; 8];
    let mut stuck = false;
    loop {
        let reach = range * boost.for_class(cur_class);
        if format::dist(pos, goal_pos) <= reach {
            break;
        }
        // One honest sphere scan (goal-pruned, same as the planner's);
        // cones are then graded against the collected shell.
        let mut cands: Vec<(u32, [f32; 3], f32, f32)> = Vec::new(); // sub idx, pos, d, cos(angle to goal)
        let to_goal = format::dist(pos, goal_pos);
        let gdir = [(goal_pos[0] - pos[0]) / to_goal, (goal_pos[1] - pos[1]) / to_goal, (goal_pos[2] - pos[2]) / to_goal];
        sub.for_each_within_toward(pos, reach, Some((goal_pos, to_goal)), |i, d| {
            scanned += 1;
            if d < 1.0 {
                return; // self
            }
            let p = sub.pos_of(i);
            let dir = [(p[0] - pos[0]) / d, (p[1] - pos[1]) / d, (p[2] - pos[2]) / d];
            let cos = dir[0] * gdir[0] + dir[1] * gdir[1] + dir[2] * gdir[2];
            cands.push((i, p, d, cos));
        });
        // Escalate the cone; inside it prefer the outer band [0.8R, R],
        // falling back to any forward star in the cone; the pick must
        // strictly reduce distance to goal (rules out cycling).
        let mut picked: Option<(u32, [f32; 3])> = None;
        for (ti, th) in THETAS_DEG.iter().enumerate() {
            let min_cos = th.to_radians().cos();
            let mut best = (f32::MAX, None); // dist-to-goal, (sub idx, pos)
            let mut n_cone = 0u64;
            for &(i, p, d, cos) in &cands {
                if cos < min_cos {
                    continue;
                }
                n_cone += 1;
                let band_ok = d >= 0.8 * reach;
                let dg = format::dist(p, goal_pos);
                // Outer-band stars are graded first-class; inner stars
                // only compete when the band is empty (score shifted).
                let score = if band_ok { dg } else { dg + 1e6 };
                if score < best.0 && dg < to_goal {
                    best = (score, Some((i, p)));
                }
            }
            if let Some(ip) = best.1 {
                in_cone += n_cone;
                theta_hist[ti] += 1;
                if ti > 0 {
                    widens += 1;
                }
                picked = Some(ip);
                break;
            }
        }
        match picked {
            Some((i, p)) => {
                chain.push(i);
                pos = p;
                cur_class = {
                    let r = sub.record(i);
                    sub.class(&r)
                };
                jumps += 1;
            }
            None => {
                stuck = true;
                break;
            }
        }
        if jumps > 2000 {
            stuck = true;
            break;
        }
    }
    if !stuck {
        jumps += 1; // the final hop onto the goal
    }
    let greedy_ms = t.elapsed().as_millis();
    let status = if stuck { format!("STUCK at {pos:?} ({:.0} ly short)", format::dist(pos, goal_pos)) } else { "ok".into() };
    println!("greedy : {jumps} jumps, {greedy_ms} ms, {status}");
    println!(
        "work   : scanned {scanned} (full sphere), in-cone {in_cone} ({:.1}x less), {widens} widen events",
        scanned as f64 / in_cone.max(1) as f64
    );
    let named: Vec<String> = THETAS_DEG.iter().zip(theta_hist).filter(|(_, n)| *n > 0).map(|(t, n)| format!("{t}deg x{n}")).collect();
    println!("cones  : {}", named.join(", "));

    // --- the reference plot ---
    let ctl = Control { cancelled: &|| false, progress: &|_, _| {}, stage: &|_, _, _| {}, found: &|_| {}, trace: &|_, _, _| {} };
    let req = RouteRequest {
        from: a,
        to: b_main,
        range_ly: range,
        supercharge: true,
        boost: BoostProfile { white_dwarf: 1.0, ..boost },
        fuel: Some(model),
        start_fuel: model.capacity,
        thorough: false,
        grace_ms: 1000,
        ..Default::default()
    };
    let t = Instant::now();
    let r = plan_best(&g, Some(&sub), &req, &ctl)?;
    println!("planner: {} jumps, {} refuels, {} ms", r.jumps, r.refuel_stops, t.elapsed().as_millis());

    // --- does the greedy chain survive the fuel model? ---
    // Forward fuel simulation first (eager top-up wherever the star can
    // refuel), which both decides feasibility and provides each leg's
    // honest starting-fuel estimate for refine_waypoints (that f32 is
    // the fuel the leg ASSUMES it starts with; a wrong assumption
    // triggers settle replans and poisons the verdict).
    if args.iter().any(|x| x == "--refine") && !stuck {
        // The sim's first hop start -> first highway star is usually a
        // multi-jump unboosted leg; it is simulated as one hop here (an
        // underestimate), and refine prices it exactly.
        let can_refuel = |i: u32| sub.scoopable(i) || sub.companion_ls(i).is_some();
        let mut fuel = model.capacity;
        let mut prev_pos = g.record(a).pos();
        let mut prev_class = {
            let r = g.record(a);
            g.class(&r)
        };
        let mut prev_refuel = g.scoopable(a);
        let mut stops = 0u32;
        let mut broken = 0u32;
        let mut waypoints: Vec<(u32, f32)> = vec![(a, model.capacity)];
        let mut ests: Vec<f32> = Vec::new();
        let mut ends = chain.clone();
        // the final hop onto the goal star
        let goal_sub_pos = goal_pos;
        for (n, &i) in ends.iter().enumerate() {
            let p = sub.pos_of(i);
            let d = format::dist(prev_pos, p);
            if prev_refuel && fuel < model.capacity - 0.5 {
                fuel = model.capacity;
                stops += 1;
            }
            match model.jump(d, fuel, boost.for_class(prev_class)) {
                Some(left) => fuel = left,
                None => {
                    broken += 1;
                    fuel = model.capacity; // keep walking to count all trouble
                }
            }
            // The leg DEPARTING star i assumes post-top-up fuel.
            ests.push(if can_refuel(i) { model.capacity } else { fuel });
            let r = sub.record(i);
            prev_pos = p;
            prev_class = sub.class(&r);
            prev_refuel = can_refuel(i);
            let _ = n;
        }
        let d_goal = format::dist(prev_pos, goal_sub_pos);
        if prev_refuel && fuel < model.capacity - 0.5 {
            fuel = model.capacity;
            stops += 1;
        }
        if model.jump(d_goal, fuel, boost.for_class(prev_class)).is_none() {
            broken += 1;
        }
        println!("fuelsim: {stops} top-ups, {broken} broken hops{}", if broken > 0 { " -> chain NOT fuel-feasible as flown" } else { "" });

        for (&i, &est) in ends.iter().zip(&ests) {
            let r = sub.record(i);
            let main = g.find(sub.name(&r)).ok_or_else(|| anyhow::anyhow!("chain star not in main index"))?;
            if main != b_main {
                waypoints.push((main, est));
            }
        }
        let _ = &mut ends;
        waypoints.push((b_main, 0.0));
        let straight = format::dist(g.record(a).pos(), goal_pos);
        let t = Instant::now();
        match ed_galaxy::long_range::refine_waypoints(&g, &req, &ctl, waypoints, model.capacity, Some(model), 0, straight, Instant::now()) {
            Ok(r) => println!(
                "refine : greedy chain settles at {} jumps, {} refuels, {} ms (chain was {jumps})",
                r.jumps, r.refuel_stops, t.elapsed().as_millis()
            ),
            Err(e) => println!("refine : greedy chain does NOT survive the fuel model: {e:?}"),
        }
    }
    Ok(())
}
