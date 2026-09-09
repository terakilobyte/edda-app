//! Plotter benchmark with the Explorer Mk II fuel model, exact and
//! long-range, so every optimisation is measured on the same runs.
//!
//!     cargo run -p ed-galaxy --example bench --release -- .data/galaxy [from] [to] [--exact]

use ed_galaxy::fuel::{BoostProfile, FuelModel};
use ed_galaxy::long_range::plan_best;
use ed_galaxy::router::{plan, Control, RouteRequest};
use ed_galaxy::{Galaxy, StarClassCode as _};
use std::path::Path;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(args.first().map(String::as_str).unwrap_or(".data/galaxy"));
    let from = args.get(1).map(String::as_str).unwrap_or("Wongi");
    let to = args.get(2).map(String::as_str).unwrap_or("Colonia");
    let exact = args.iter().any(|a| a == "--exact");
    let thorough = args.iter().any(|a| a == "--thorough");
    let injection: Option<(f32, &'static str, u32)> = args.iter().position(|a| a == "--inject").and_then(|i| Some((args.get(i + 1)?.as_str(), args.get(i + 2)?.parse::<u32>().ok()?))).and_then(|(grade, n)| {
        ed_galaxy::router::INJECTION_RECIPES.iter().find(|r| r.1 == grade).map(|r| (r.0, r.1, n))
    });
    let budget_s: u64 = args.iter().position(|a| a == "--budget").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(0);
    // Item 24d probe: --census <path> appends one row per search
    // expansion (route,ship,phase,x,y,z) via the Control trace hook --
    // every variant, every phase the planner traces (coarse incl. bidi,
    // exact, leg, chain). Joined offline against the field oracle's
    // voxel table (field_density_probe --save) by
    // docs/benches/knobs/field_census_report.py.
    let census_path = args.iter().position(|a| a == "--census").and_then(|i| args.get(i + 1)).cloned();
    let start_fuel: Option<f32> = args.iter().position(|a| a == "--fuel").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok());
    // Planner knobs for the search harness: ARG overrides first, plain
    // ED_* env as fallback. Each flag maps onto its env var before any
    // planning starts, so the planner needs no API change and every
    // trial's configuration is visible in its command line.
    for (flag, var) in [
        ("--cone-angles", "ED_CONE_ANGLES"),
        ("--cone-cap", "ED_CONE_CAP"),
        ("--cone-band", "ED_CONE_BAND"),
        ("--greedy-floor", "ED_GREEDY_FLOOR"),
        ("--field-min", "ED_FIELD_MIN"),
        ("--bidi-ratio", "ED_BIDI_RATIO"),
        ("--bidi-offset", "ED_BIDI_OFFSET"),
        ("--meet-settle", "ED_MEET_SETTLE"),
        ("--stop-weight", "ED_STOP_WEIGHT"),
        ("--coarse-w", "ED_COARSE_W"),
        ("--floor-slack", "ED_FLOOR_SLACK"),
        ("--field-floor-slack", "ED_FIELD_FLOOR_SLACK"),
        ("--floor-pad", "ED_FLOOR_PAD"),
        ("--floor-trust-ratio", "ED_FLOOR_TRUST_RATIO"),
        ("--prize-k", "ED_PRIZE_K"),
        ("--t-jump", "ED_TJUMP_S"),
        ("--stop-overhead", "ED_STOP_OVERHEAD_S"),
        ("--refuel-bonus", "ED_REFUEL_BONUS"),
        ("--deadend-factor", "ED_DEADEND_FACTOR"),
        ("--ramp", "ED_RAMP"),
        ("--ramp-angle", "ED_RAMP_ANGLE_DEG"),
        ("--ramp-detour", "ED_RAMP_DETOUR"),
        ("--ramp-radius", "ED_RAMP_RADIUS_LY"),
        ("--ramp-chain", "ED_RAMP_CHAIN"),
        ("--offramp", "ED_OFFRAMP"),
        ("--offramp-reach", "ED_OFFRAMP_REACH"),
        ("--wave-cost", "ED_WAVE_COST"),
        ("--prune-judge", "ED_PRUNE_JUDGE"),
        ("--minfuel-scan", "ED_MINFUEL_SCAN"),
        ("--judge", "ED_JUDGE"),
        ("--crossing-replan", "ED_CROSSING_REPLAN"),
    ] {
        if let Some(v) = args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)) {
            std::env::set_var(var, v);
        }
    }
    for (flag, var) in [
        ("--no-greedy", "ED_NO_GREEDY"),
        ("--no-bidi", "ED_NO_BIDI"),
        ("--no-field", "ED_NO_FIELD"),
        ("--no-alt", "ED_NO_ALT"),
    ] {
        if args.iter().any(|a| a == flag) {
            std::env::set_var(var, "1");
        }
    }

    let g = Galaxy::open(dir)?;
    let ndir = ed_galaxy::long_range::neutron_dir(dir);
    if !Galaxy::exists(&ndir) {
        eprintln!("building neutron sub-index with {} ly cells...", ed_galaxy::long_range::NEUTRON_CELL_LY);
        let t = std::time::Instant::now();
        let st = ed_galaxy::import::subset_cells(&g, &ndir, ed_galaxy::long_range::NEUTRON_CELL_LY, |r| ed_galaxy::long_range::highway_star(ed_galaxy::StarClass::from_code(r.class)))?;
        eprintln!("  {} neutrons in {} s", st.systems, t.elapsed().as_secs());
    }
    let neutrons = Galaxy::open(&ndir)?;
    // --ship explorer (default): size 8A SCO Mk II (6.8 t cap), 128 t tank, size 5 booster, x6 neutron.
    // --ship mandalay: 5A SCO (5.0 t cap), 32 t tank, size 5 booster, standard x4 neutron.
    let ship = args.iter().position(|a| a == "--ship").and_then(|i| args.get(i + 1)).map(String::as_str).unwrap_or("explorer");
    let (model, boost) = match ship {
        "mandalay" => (FuelModel::from_loadout(319.2, 32.0, 5.0, 5, true, false, 77.86, 10.5, 0.0), BoostProfile::default()),
        // The Commander's Caspian at a further-engineered 86 ly max jump
        // (the reach that opens the far rim -- Oevasy needs 85).
        "explorer86" => (FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 86.0, 10.5, 0.0), BoostProfile::MK2_SCO),
        _ => (FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0), BoostProfile::MK2_SCO),
    };
    // Item 22: scoop rates (t/s) per bench ship -- the Caspians carry a
    // 7A scoop (1.245, per the Loadout on record; an earlier comment
    // said 8A with the same correct rate); the Mandalay build is
    // assumed 5A (0.577) until the journal-fit replaces these. The app
    // will read the Loadout.
    let mut model = model;
    model.scoop_rate = if ship == "mandalay" { 0.577 } else { 1.245 };
    let a = g.find(from).ok_or_else(|| anyhow::anyhow!("unknown {from}"))?;
    let b = g.find(to).ok_or_else(|| anyhow::anyhow!("unknown {to}"))?;
    // White dwarfs are opt-in (--white-dwarfs); by default they are plain stars.
    let boost = if args.iter().any(|a| a == "--white-dwarfs") { boost } else { BoostProfile { white_dwarf: 1.0, ..boost } };
    // --grace <ms>: variants left after the first route get this long, then are cancelled (0 = wait for all). The app uses 1000.
    let grace_ms: u64 = args.iter().position(|a| a == "--grace").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(1000);
    // --min-fuel: only stop for fuel when the tank requires it (opt-in, like the app toggle).
    let min_fuel = args.iter().any(|a| a == "--min-fuel");
    // --stop-weight rides the request now (item 20; the ED_STOP_WEIGHT
    // env mapping above stays for harness compatibility but the field
    // is what the judge reads). --try-hard is the app preset.
    let try_hard = args.iter().any(|a| a == "--try-hard");
    let stop_weight: f32 = if try_hard { 0.0 } else {
        args.iter().position(|a| a == "--stop-weight").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(1.0)
    };
    let req = RouteRequest { from: a, to: b, range_ly: model.range_at(model.capacity), supercharge: true, boost, fuel: Some(model), start_fuel: start_fuel.unwrap_or(model.capacity), thorough: thorough || try_hard, time_budget_ms: budget_s * 1000, injection, grace_ms, min_fuel, stop_weight, prize_k: try_hard.then_some(0.0), ..Default::default() };
    let straight = ed_galaxy::format::dist(g.record(a).pos(), g.record(b).pos());
    eprintln!("{} systems; {} neutrons; {from} -> {to}: {straight:.0} ly straight, full-tank range {:.1} ly", g.count, neutrons.count, req.range_ly);
    let t0 = std::time::Instant::now();
    let stage = move |s: &str, i: u32, n: u32| { if s != "refine" || i == 0 || i == n { eprintln!("  stage {s} {i}/{n} at {} ms", t0.elapsed().as_millis()) } };
    let found = move |r: &ed_galaxy::router::Route| eprintln!("  found {} jumps at {} ms", r.jumps, t0.elapsed().as_millis());
    let census: std::sync::Mutex<Vec<(&'static str, [f32; 3])>> = std::sync::Mutex::new(Vec::new());
    let trace_census = |phase: &'static str, pos: [f32; 3], _: f32| census.lock().unwrap().push((phase, pos));
    let trace_off = |_: &'static str, _: [f32; 3], _: f32| {};
    let trace: &(dyn Fn(&'static str, [f32; 3], f32) + Sync) = if census_path.is_some() { &trace_census } else { &trace_off };
    let ctl = Control { cancelled: &|| false, progress: &|_, _| {}, stage: &stage, found: &found, trace };
    let started = std::time::Instant::now();
    let r = if exact { plan(&g, &req, &ctl)? } else { plan_best(&g, Some(&neutrons), &req, &ctl)? };
    if let Some(path) = &census_path {
        use std::io::Write as _;
        let rows = census.lock().unwrap();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        for (phase, p) in rows.iter() {
            writeln!(f, "{from}>{to},{ship},{phase},{:.1},{:.1},{:.1}", p[0], p[1], p[2])?;
        }
        eprintln!("  census: {} trace rows appended to {path}", rows.len());
    }
    println!(
        "{} jumps, {:.0} ly flown ({:.0} straight), {} boosted, {} refuel stops, {} expansions, {} ms (wall {} ms), variants {}/{}",
        r.jumps, r.total_ly, r.straight_ly, r.boosted_jumps, r.refuel_stops, r.expansions, r.elapsed_ms, started.elapsed().as_millis(), r.variants_finished, r.variants_run
    );
    // Item 22: the real refuel bill from the route's own fuel ledger --
    // tonnes scooped (fuel_after rises) over the scoop rate, plus a
    // fixed approach overhead per stop. Journal-fit refines both later.
    let (mut refuel_tonnes, mut prev_fuel) = (0.0f32, None::<f32>);
    for h in &r.hops {
        if let (Some(f), Some(p)) = (h.fuel_after, prev_fuel) {
            if f > p + 0.05 {
                refuel_tonnes += f - p;
            }
        }
        if h.fuel_after.is_some() {
            prev_fuel = h.fuel_after;
        }
    }
    let est_refuel_s = r.refuel_stops as f32 * 36.0 + if model.scoop_rate > 0.0 { refuel_tonnes / model.scoop_rate } else { r.refuel_stops as f32 * 95.0 };
    // --json: one machine-readable line per run, so sweeps parse a
    // stable contract instead of regexing the prose above (the atlas
    // once lost its wall column to a regex against the wrong number).
    if args.iter().any(|a| a == "--json") {
        println!(
            "{}",
            serde_json::json!({
                "from": from, "to": to, "ship": ship,
                "thorough": thorough, "grace_ms": grace_ms, "min_fuel": min_fuel,
                "white_dwarfs": args.iter().any(|a| a == "--white-dwarfs"),
                "straight_ly": straight,
                "jumps": r.jumps, "total_ly": r.total_ly, "boosted": r.boosted_jumps,
                "refuel_stops": r.refuel_stops, "expansions": r.expansions,
                "elapsed_ms": r.elapsed_ms, "wall_ms": started.elapsed().as_millis() as u64,
                "variants_finished": r.variants_finished, "variants_run": r.variants_run,
                "refuel_tonnes": refuel_tonnes, "est_refuel_s": est_refuel_s,
            })
        );
    }
    // Item 31: --route-out <path> archives the full route hop by hop
    // (one JSON object; append-safe, one line per plot) so benches keep
    // the ROUTES, not just the counts — comparisons against a reference
    // route (e.g. docs/benches/spansh_spase_colonia_85j.json) become a
    // diff instead of a re-plot.
    if let Some(path) = args.iter().position(|a| a == "--route-out").and_then(|i| args.get(i + 1)) {
        use std::io::Write as _;
        let hops: Vec<_> = r.hops.iter().map(|h| {
            serde_json::json!({
                "name": h.name, "class": h.class.letter().to_string(),
                "x": h.pos[0], "y": h.pos[1], "z": h.pos[2],
                "distance_ly": h.distance_ly, "boosted": h.boosted,
                "fuel_after": h.fuel_after,
            })
        }).collect();
        let line = serde_json::json!({
            "from": from, "to": to, "ship": ship, "min_fuel": min_fuel,
            "thorough": thorough, "stop_weight": stop_weight,
            "jumps": r.jumps, "boosted": r.boosted_jumps, "refuel_stops": r.refuel_stops,
            "total_ly": r.total_ly, "hops": hops,
        });
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(f, "{line}")?;
        eprintln!("  route archived to {path} ({} hops)", r.hops.len());
    }
    if args.iter().any(|a| a == "--hops") {
        for h in &r.hops {
            println!(
                "{:<30} {:>3} {:>6.1} ly {}{} @[{:.0} {:.0} {:.0}]",
                h.name,
                h.class.letter(),
                h.distance_ly,
                if h.boosted { "BOOST " } else { "" },
                h.fuel_after.map(|f| format!("fuel {f:.1} ")).unwrap_or_default(),
                h.pos[0],
                h.pos[1],
                h.pos[2]
            );
        }
    }
    Ok(())
}
