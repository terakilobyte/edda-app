//! Plot fuzzer: random system pairs from the routing index, plotted
//! against a running ed-api, every answer replayed hop by hop with the
//! exact fuel model. A route that the tank cannot fly, that boosts off a
//! star that is not a neutron, that scoops at a star that cannot be
//! scooped, or whose hop distances do not match the stars' positions is
//! a FAKE ROUTE, and the point of this harness is to count them (the
//! boss, 2026-10-09: "I suspect we will have to fuzz test the plotter
//! locally with the dev server to ensure we aren't generating fake
//! routes that just won't work").
//!
//!     cargo run -p ed-api --release --example plot_fuzz -- <index_dir> <api> <n> [seed] [--min-ly L] [--max-ly H] [--no-injection]
//!
//! Pairs come from two pools drawn at random from the index (the boss:
//! "random system pairs that maybe are far away or some other way we can
//! create a pool of 'hard to reach' points, as well as easily reachable
//! ones"): EASY = at least 12 known stars within 100 ly, HARD = at most
//! 3 (an island when none is within the ship's plain range). The cases
//! rotate easy-easy, hard origin, hard destination, hard-hard.
//!     cargo run -p ed-api --release --example plot_fuzz -- --spansh <results.json>
//!
//! The ship is the Caspian Explorer on record (8A SCO Mk II, 128 t,
//! 6.8 t/jump, 77.81 ly light, x6 neutron), full tank at departure,
//! min-fuel on, premium injections x87 unless --no-injection. Each plot
//! is validated twice when the server answers early: the early answer
//! and the finished one (polled until `refining` is false). One CSV row
//! per answer, a summary at the end; exit code 1 when any answer failed.
//!
//! --spansh replays a Spansh exact-plotter result through the same model
//! (boost from the departure star's neutron, injection where Spansh says
//! synthesise, refuel where it says refuel) and reports where our model
//! disagrees with theirs -- the cross-check that our fuel physics is not
//! the thing inventing or refusing routes.

use ed_galaxy::fuel::{BoostProfile, FuelModel};
use ed_galaxy::Galaxy;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn caspian() -> FuelModel {
    FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0)
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A tiny deterministic generator (no rand dependency on the server).
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

/// Replay a served route with the model. Every problem is a sentence.
fn validate(route: &Value, m: &FuelModel, boost: &BoostProfile, injection_mult: f32, from: &str, to: &str, start_fuel: f32) -> Vec<String> {
    let mut bad = Vec::new();
    let hops = match route.get("hops").and_then(|h| h.as_array()) {
        Some(h) if !h.is_empty() => h,
        _ => return vec!["no hops".into()],
    };
    let name = |h: &Value| h.get("name").and_then(|n| n.as_str()).unwrap_or("?").to_string();
    let pos = |h: &Value| -> [f32; 3] {
        let p = h.get("pos").and_then(|p| p.as_array());
        match p {
            Some(p) if p.len() == 3 => [p[0].as_f64().unwrap_or(0.0) as f32, p[1].as_f64().unwrap_or(0.0) as f32, p[2].as_f64().unwrap_or(0.0) as f32],
            _ => [f32::NAN; 3],
        }
    };
    if !name(&hops[0]).eq_ignore_ascii_case(from) {
        bad.push(format!("starts at {} not {from}", name(&hops[0])));
    }
    if !name(&hops[hops.len() - 1]).eq_ignore_ascii_case(to) {
        bad.push(format!("ends at {} not {to}", name(&hops[hops.len() - 1])));
    }
    let jumps = route.get("jumps").and_then(|j| j.as_u64()).unwrap_or(0) as usize;
    if jumps != hops.len() - 1 {
        bad.push(format!("jumps {jumps} but {} hops", hops.len()));
    }
    let mut tank = start_fuel.min(m.capacity);
    let mut stops = 0usize;
    let mut injections = 0usize;
    for i in 1..hops.len() {
        let h = &hops[i];
        let prev = &hops[i - 1];
        let d_reported = h.get("distance_ly").and_then(|d| d.as_f64()).unwrap_or(0.0) as f32;
        let d = dist(pos(prev), pos(h));
        if d.is_finite() && (d - d_reported).abs() > 0.5 {
            bad.push(format!("hop {i} {}: reported {d_reported:.1} ly, the stars are {d:.1} ly apart", name(h)));
        }
        let boosted = h.get("boosted").and_then(|b| b.as_bool()).unwrap_or(false);
        let injection = h.get("injection").and_then(|j| j.as_str());
        let prev_class = prev.get("class").and_then(|c| c.as_str()).unwrap_or("?");
        let b = if boosted {
            match prev_class {
                "neutron" => boost.neutron,
                "white_dwarf" => boost.white_dwarf,
                other => {
                    bad.push(format!("hop {i} {}: boosted off a {other}", name(h)));
                    1.0
                }
            }
        } else if injection.is_some() {
            injection_mult
        } else {
            1.0
        };
        if boosted && injection.is_some() {
            bad.push(format!("hop {i} {}: boosted and injected at once", name(h)));
        }
        if injection.is_some() {
            injections += 1;
        }
        let left = match m.jump(d_reported, tank, b) {
            Some(left) => left,
            None => {
                bad.push(format!("hop {i} {}: {d_reported:.1} ly cannot be flown with {tank:.1} t aboard at x{b} (reach {:.1})", name(h), m.reach(tank, b)));
                // keep going from an empty tank so later problems still show
                0.0
            }
        };
        let refuel = h.get("refuel").and_then(|r| r.as_bool()).unwrap_or(false);
        let scoopable = h.get("scoopable").and_then(|s| s.as_bool()).unwrap_or(false);
        if refuel && !scoopable {
            bad.push(format!("hop {i} {}: scoop at an unscoopable star", name(h)));
        }
        if refuel {
            stops += 1;
        }
        tank = if refuel && scoopable { m.capacity } else { left };
        if let Some(reported) = h.get("fuel_after").and_then(|f| f.as_f64()) {
            if (reported as f32 - tank).abs() > 0.6 {
                bad.push(format!("hop {i} {}: fuel_after {reported:.1} t but the replay has {tank:.1} t", name(h)));
            }
        }
    }
    let reported_stops = route.get("refuel_stops").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
    if reported_stops != stops {
        bad.push(format!("refuel_stops {reported_stops} but {stops} hops marked"));
    }
    let reported_inj = route.get("injections").and_then(|s| s.as_u64()).unwrap_or(0) as usize;
    if reported_inj != injections {
        bad.push(format!("injections {reported_inj} but {injections} hops marked"));
    }
    bad
}

/// Replay a Spansh exact-plotter result through our model.
fn spansh(path: &str) -> anyhow::Result<()> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let jumps = v["result"]["jumps"].as_array().ok_or_else(|| anyhow::anyhow!("no result.jumps"))?;
    let m = caspian();
    let mut tank = m.capacity;
    let mut disagreements = 0;
    let mut unflyable = 0;
    for i in 1..jumps.len() {
        let (prev, j) = (&jumps[i - 1], &jumps[i]);
        let d = j["distance"].as_f64().unwrap_or(0.0) as f32;
        // Spansh flags the arrival: `has_neutron` on a system boosts the
        // jump OUT of it, `must_inject` on a system is synthesised for the
        // jump INTO it.
        let b = if prev["has_neutron"].as_bool().unwrap_or(false) { 6.0 } else if j["must_inject"].as_bool().unwrap_or(false) { 2.0 } else { 1.0 };
        match m.jump(d, tank, b) {
            Some(left) => {
                tank = left;
            }
            None => {
                unflyable += 1;
                if unflyable <= 5 {
                    println!("  spansh hop {i} {}: {d:.1} ly at x{b} with {tank:.1} t: our model says no (reach {:.1})", j["name"].as_str().unwrap_or("?"), m.reach(tank, b));
                }
                tank = 0.0;
            }
        }
        if j["must_refuel"].as_bool().unwrap_or(false) {
            tank = m.capacity;
        }
        if let Some(theirs) = j["fuel_in_tank"].as_f64() {
            if (theirs as f32 - tank).abs() > 3.0 {
                disagreements += 1;
                if disagreements <= 5 {
                    println!("  spansh hop {i} {}: their tank {theirs:.1} t, ours {tank:.1} t", j["name"].as_str().unwrap_or("?"));
                }
            }
        }
    }
    println!("spansh replay: {} hops, {unflyable} our model cannot fly, {disagreements} tank readings differ by > 3 t", jumps.len() - 1);
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--spansh") {
        return spansh(args.get(1).ok_or_else(|| anyhow::anyhow!("--spansh <results.json>"))?);
    }
    if args.len() < 3 {
        eprintln!("usage: plot_fuzz <index_dir> <api> <n> [seed] [--min-ly L] [--max-ly H] [--no-injection]");
        std::process::exit(2);
    }
    let index = &args[0];
    let api = args[1].trim_end_matches('/').to_string();
    let n: usize = args[2].parse()?;
    let seed: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let opt = |flag: &str, def: f32| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(def);
    let (min_ly, max_ly) = (opt("--min-ly", 1_500.0), opt("--max-ly", 90_000.0));
    let inject = !args.iter().any(|a| a == "--no-injection");

    let g = Galaxy::open(std::path::Path::new(index))?;
    let m = caspian();
    let boost = BoostProfile { neutron: 6.0, white_dwarf: 1.0 };
    let mut rng = Lcg(seed);
    let count = g.count as u64;
    // Two pools from random draws: dense neighbourhoods and sparse ones.
    let (mut easy, mut hard): (Vec<u32>, Vec<u32>) = (Vec::new(), Vec::new());
    let mut draws = 0u64;
    while (easy.len() < 200 || hard.len() < 60) && draws < 400_000 {
        draws += 1;
        let idx = (rng.next() % count) as u32;
        let near = g.within(g.pos_of(idx), 100.0).len();
        if near >= 12 && easy.len() < 200 {
            easy.push(idx);
        } else if near <= 3 && hard.len() < 60 {
            hard.push(idx);
        }
    }
    eprintln!("pools: {} easy, {} hard, from {draws} draws", easy.len(), hard.len());
    let pick = |rng: &mut Lcg, pool: &Vec<u32>| pool[(rng.next() % pool.len() as u64) as usize];
    let cases = ["easy-easy", "hard-origin", "hard-destination", "hard-hard"];
    let rt = tokio::runtime::Runtime::new()?;
    let http = reqwest::Client::builder().timeout(Duration::from_secs(220)).build()?;

    println!("n,case,from,to,straight_ly,answer,http,wall_s,jumps,injections,refuel_stops,problems");
    let (mut plots, mut answers, mut failed_answers, mut refusals) = (0usize, 0usize, 0usize, 0usize);
    let mut problem_kinds: std::collections::BTreeMap<String, usize> = Default::default();
    let mut tries = 0u64;
    while plots < n && tries < 100_000 {
        tries += 1;
        let case = cases[plots % cases.len()];
        let (a, b) = match case {
            "easy-easy" => (pick(&mut rng, &easy), pick(&mut rng, &easy)),
            "hard-origin" => (pick(&mut rng, &hard), pick(&mut rng, &easy)),
            "hard-destination" => (pick(&mut rng, &easy), pick(&mut rng, &hard)),
            _ => (pick(&mut rng, &hard), pick(&mut rng, &hard)),
        };
        if a == b {
            continue;
        }
        let straight = dist(g.pos_of(a), g.pos_of(b));
        if !(min_ly..=max_ly).contains(&straight) {
            continue;
        }
        let (from, to) = (g.name(&g.record(a)).to_string(), g.name(&g.record(b)).to_string());
        plots += 1;
        let mut body = json!({
            "from": from, "to": to, "fuel_model": m, "start_fuel": m.capacity,
            "boost": {"neutron": 6.0, "white_dwarf": 3.0}, "supercharge": true, "min_fuel": true, "thorough": false,
        });
        if inject {
            body["injection"] = json!({"grade": "premium", "max": 87});
        }
        let mut answer = "early";
        let started = Instant::now();
        loop {
            let resp = rt.block_on(async { http.post(format!("{api}/v1/route")).json(&body).send().await });
            let wall = started.elapsed().as_secs_f64();
            let (status, v): (u16, Value) = match resp {
                Ok(r) => {
                    let s = r.status().as_u16();
                    (s, rt.block_on(r.json::<Value>()).unwrap_or(Value::Null))
                }
                Err(e) => {
                    println!("{plots},{case},\"{from}\",\"{to}\",{straight:.0},{answer},0,{wall:.1},,,,\"transport: {e}\"");
                    failed_answers += 1;
                    break;
                }
            };
            if status != 200 {
                refusals += 1;
                println!("{plots},{case},\"{from}\",\"{to}\",{straight:.0},{answer},{status},{wall:.1},,,,\"{}{}\"", v.get("error").and_then(|e| e.as_str()).unwrap_or("?"), v.get("why").and_then(|w| w.as_str()).map(|w| format!(" ({w})")).unwrap_or_default());
                break;
            }
            answers += 1;
            let problems = validate(&v, &m, &boost, 2.0, &from, &to, m.capacity);
            let refining = v.get("refining").and_then(|r| r.as_bool()).unwrap_or(false);
            println!(
                "{plots},{case},\"{from}\",\"{to}\",{straight:.0},{},{status},{wall:.1},{},{},{},\"{}\"",
                if refining { "early" } else { answer },
                v["jumps"],
                v["injections"],
                v["refuel_stops"],
                problems.join(" | ").replace('"', "'")
            );
            if !problems.is_empty() {
                failed_answers += 1;
                for p in &problems {
                    let kind = p.split(':').nth(1).map(|s| s.trim()).unwrap_or(p).split(|c: char| c.is_ascii_digit()).next().unwrap_or("").trim().to_string();
                    *problem_kinds.entry(kind).or_default() += 1;
                }
            }
            if !refining || started.elapsed() > Duration::from_secs(150) {
                break;
            }
            answer = "final";
            std::thread::sleep(Duration::from_secs(5));
        }
    }
    println!("# plots {plots}, answers {answers}, refusals {refusals}, answers with problems {failed_answers}");
    for (k, c) in &problem_kinds {
        println!("#   {c} x {k}");
    }
    if failed_answers > 0 {
        std::process::exit(1);
    }
    Ok(())
}
