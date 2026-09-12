//! Record a neutron-first plot as JSON for visualisation: every coarse and
//! exact expansion (position + jumps so far), the coarse waypoint chain,
//! each refined leg's hops, and the final route.
//!
//!     cargo run -p ed-galaxy --example trace_plot --release -- <index_dir> <from> <to> [range_ly] [--white-dwarfs] > trace.json
//!
//! Without a range the Explorer Mk II fuel model from `examples/bench.rs`
//! is used (SCO drive, x6 / x3 supercharge).

use ed_galaxy::fuel::{BoostProfile, FuelModel};
use ed_galaxy::long_range::{neutron_dir, plan_long};
use ed_galaxy::router::{Control, RouteRequest};
use ed_galaxy::{Galaxy, StarClassCode as _};
use std::path::Path;
use std::sync::Mutex;

fn main() -> anyhow::Result<()> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    // White dwarfs are opt-in (--white-dwarfs); by default they are plain stars.
    let white_dwarfs = all.iter().any(|a| a == "--white-dwarfs");
    let args: Vec<String> = all.into_iter().filter(|a| !a.starts_with("--")).collect();
    if args.len() < 3 {
        eprintln!("usage: trace_plot <index_dir> <from> <to> [range_ly] [--white-dwarfs]");
        std::process::exit(2);
    }
    let dir = Path::new(&args[0]);
    let g = Galaxy::open(dir)?;
    let ndir = neutron_dir(dir);
    if !Galaxy::exists(&ndir) {
        eprintln!("building highway sub-index at {} ...", ndir.display());
        let st = ed_galaxy::import::subset_cells(
            &g,
            &ndir,
            ed_galaxy::long_range::NEUTRON_CELL_LY,
            |r| ed_galaxy::long_range::highway_star(ed_galaxy::StarClass::from_code(r.class)),
        )?;
        eprintln!("  {} highway stars", st.systems);
    }
    let neutrons = Galaxy::open(&ndir)?;
    let from = g
        .find(&args[1])
        .ok_or_else(|| anyhow::anyhow!("unknown system {}", args[1]))?;
    let to = g
        .find(&args[2])
        .ok_or_else(|| anyhow::anyhow!("unknown system {}", args[2]))?;
    let req = match args.get(3) {
        Some(r) => RouteRequest {
            from,
            to,
            range_ly: r.parse()?,
            ..Default::default()
        },
        None => {
            let model =
                FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
            let boost = if white_dwarfs {
                BoostProfile::MK2_SCO
            } else {
                BoostProfile {
                    white_dwarf: 1.0,
                    ..BoostProfile::MK2_SCO
                }
            };
            RouteRequest {
                from,
                to,
                range_ly: model.range_at(model.capacity),
                supercharge: true,
                boost,
                fuel: Some(model),
                start_fuel: model.capacity,
                ..Default::default()
            }
        }
    };
    // A long plot runs several variants on worker threads; events are tagged
    // by thread so the winning variant's trace can be picked out afterwards.
    let events: Mutex<Vec<(std::thread::ThreadId, &'static str, [f32; 3], f32)>> =
        Mutex::new(Vec::new());
    let trace = |phase: &'static str, pos: [f32; 3], v: f32| {
        events
            .lock()
            .unwrap()
            .push((std::thread::current().id(), phase, pos, v))
    };
    let ctl = Control {
        cancelled: &|| false,
        progress: &|_, _| {},
        stage: &|_, _, _| {},
        found: &|_| {},
        trace: &trace,
    };
    let route = plan_long(&g, &neutrons, &req, &ctl)?;
    eprintln!(
        "{} jumps, {:.0} ly, {} boosted, {} refuels, {} ms",
        route.jumps, route.total_ly, route.boosted_jumps, route.refuel_stops, route.elapsed_ms
    );
    let all = events.into_inner().unwrap();
    // The variant whose refined legs reproduce the final route. Legs are
    // refined on the rayon pool, so match by the coarse thread's chain: the
    // chain whose waypoints all lie on the route. Leg/exact events from
    // any thread are kept when they lie within the route's corridor.
    let on_route = |q: &[f32; 3]| {
        route.hops.iter().any(|h| {
            (h.pos[0] - q[0]).abs() < 0.2
                && (h.pos[1] - q[1]).abs() < 0.2
                && (h.pos[2] - q[2]).abs() < 0.2
        })
    };
    let mut winner: Option<std::thread::ThreadId> = None;
    let mut tids: Vec<std::thread::ThreadId> =
        all.iter().filter(|e| e.1 == "chain").map(|e| e.0).collect();
    tids.dedup();
    for t in tids {
        let chain: Vec<&[f32; 3]> = all
            .iter()
            .filter(|e| e.0 == t && e.1 == "chain")
            .map(|e| &e.2)
            .collect();
        if !chain.is_empty() && chain.iter().all(|q| on_route(q)) {
            winner = Some(t);
            break;
        }
    }
    let winner =
        winner.ok_or_else(|| anyhow::anyhow!("no variant's chain matches the final route"))?;
    let mut seen_leg = std::collections::HashSet::new();
    let mut seen_exact = std::collections::HashSet::new();
    let key = |q: &[f32; 3], v: f32| {
        (
            (v * 10.0) as i64,
            (q[0] * 10.0) as i64,
            (q[1] * 10.0) as i64,
            (q[2] * 10.0) as i64,
        )
    };
    let events: Vec<(&'static str, [f32; 3], f32)> = all
        .iter()
        .filter(|e| match e.1 {
            "coarse" | "chain" => e.0 == winner,
            // Legs are refined by every variant, and a leg whose fuel
            // estimate was off is planned twice; keep only the hops the
            // final route actually flies.
            "leg" => on_route(&e.2) && seen_leg.insert(key(&e.2, e.3)),
            _ => seen_exact.insert(key(&e.2, e.3)),
        })
        .map(|e| (e.1, e.2, e.3))
        .collect();
    let counts = |p: &str| events.iter().filter(|e| e.0 == p).count();
    eprintln!(
        "trace: {} coarse, {} chain, {} exact, {} leg hops",
        counts("coarse"),
        counts("chain"),
        counts("exact"),
        counts("leg")
    );
    // Neutron stars the coarse search could choose between: everything in
    // the sub-index within a corridor around the straight line (one boosted
    // hop plus bridging either side); a sphere around a Colonia plot would
    // hold most of the galaxy's neutrons.
    let a = g.pos_of(req.from);
    let b = g.pos_of(req.to);
    let mid = [
        (a[0] + b[0]) / 2.0,
        (a[1] + b[1]) / 2.0,
        (a[2] + b[2]) / 2.0,
    ];
    let half_width = req.range_ly * 8.0;
    let radius = route.straight_ly / 2.0 + half_width;
    let seg_dist = |q: [f32; 3]| -> f32 {
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let aq = [q[0] - a[0], q[1] - a[1], q[2] - a[2]];
        let len2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
        let t = if len2 > 0.0 {
            ((aq[0] * ab[0] + aq[1] * ab[1] + aq[2] * ab[2]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let c = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
        ((q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2) + (q[2] - c[2]).powi(2)).sqrt()
    };
    let mut near: Vec<String> = Vec::new();
    neutrons.for_each_within(mid, radius, |idx, _| {
        let q = neutrons.pos_of(idx);
        if seg_dist(q) <= half_width {
            near.push(format!("[{:.1},{:.1},{:.1}]", q[0], q[1], q[2]));
        }
    });
    eprintln!(
        "{} neutrons within {half_width:.0} ly of the corridor",
        near.len()
    );
    let ev: Vec<String> = events
        .iter()
        .map(|(p, q, v)| format!("[\"{p}\",{:.1},{:.1},{:.1},{v:.0}]", q[0], q[1], q[2]))
        .collect();
    let hops: Vec<String> = route
        .hops
        .iter()
        .map(|h| {
            format!(
                "{{\"name\":{:?},\"pos\":[{:.2},{:.2},{:.2}],\"class\":\"{}\",\"d\":{:.1},\"boosted\":{},\"scoop\":{},\"refuel\":{},\"fuel\":{}}}",
                h.name, h.pos[0], h.pos[1], h.pos[2], h.class.letter(), h.distance_ly, h.boosted, h.scoopable, h.refuel,
                h.fuel_after.map(|f| format!("{f:.1}")).unwrap_or("null".into())
            )
        })
        .collect();
    println!(
        "{{\"from\":{:?},\"to\":{:?},\"range_ly\":{:.1},\"jumps\":{},\"total_ly\":{:.0},\"straight_ly\":{:.0},\"boosted\":{},\"refuels\":{},\"elapsed_ms\":{},\"hops\":[{}],\"neutrons\":[{}],\"events\":[{}]}}",
        args[1], args[2], req.range_ly, route.jumps, route.total_ly, route.straight_ly, route.boosted_jumps, route.refuel_stops, route.elapsed_ms, hops.join(","), near.join(","), ev.join(",")
    );
    Ok(())
}
