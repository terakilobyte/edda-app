//! Fleet-carrier route planning (Item 52 C, maintainer's item 6: "move my
//! carrier anywhere in the galaxy").
//!
//! A carrier jumps a fixed 500 ly, never boosts, never scoops, and burns
//! tritium by mass:
//!
//! ```text
//! fuel_t = round(5 + d_ly × (capacity_used_t + tank_t + 25 000) / 200 000)
//! ```
//!
//! (the community formula, math.edomh.nl/pages/fleet-carrier-jump, checked
//! 2026-09-07: BASE 5 t, divisor 200 000, fleet-carrier mass 25 000 t,
//! squadron 15 000 t; `capacity_used` = Crew + Cargo + CargoSpaceReserved +
//! ShipPacks + ModulePacks, i.e. TotalCapacity − FreeSpace, with tritium in
//! the HOLD counted as cargo and tritium in the TANK as `fuelInReservoir`;
//! min 5 t, max 133 t; their worked example 500 ly / 5 000 t used / 1 000 t
//! tank = 83 t is a test below; a measured first jump — 16 t over
//! 82.28 ly at 680 t crew mass and a 500 t tank, tank read 500 → 484 — is
//! another, and it fits to the tonne). Fuel is deducted only
//! when the jump executes.
//!
//! Timing (Elite wiki + 2025 field reports): the preparation countdown is
//! AT LEAST 15 minutes — under server load it runs 16:30 and has been seen
//! past 40 — pre-flight lockdown at 3:20 before departure, and a 5-minute
//! cooldown after arrival. So `DEFAULT_MINUTES_PER_JUMP` is a FLOOR (15 +
//! 5), and the ETA says so; the follow layer measures the commander's own
//! request → jump gaps and prefers them. Beside [`crate::router::plan`]
//! rather than inside it: the
//! ship planner's boosts, scoops and fuel-state search are exactly what a
//! carrier lacks, and a carrier's constraints — permit-locked systems,
//! hold tritium topping the tank up — are exactly what a ship lacks.
//!
//! The search is A* on jumps over the same star index, with the ship
//! router's fan-out trick: of every star within reach, only the `fanout`
//! that bring the carrier closest to the goal are expanded, which keeps a
//! bubble expansion (tens of thousands of stars inside 500 ly) tractable.
//! Fuel is settled on the reconstructed path — it depends only on the hop
//! distances and the falling mass — and a route the tank cannot finish is
//! still returned, with an honest `ShortBy` verdict at the hop it fails.
//!
//! Pinned by `docs/benches/2026-09-07-carrier-router-pins.csv`.

use crate::format::{dist, Galaxy};
use rustc_hash::FxHashMap as HashMap;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// A carrier's jump reach, fixed by the game.
pub const MAX_JUMP_LY: f32 = 500.0;
/// The tritium tank.
pub const TANK_T: f32 = 1_000.0;
/// The hull's own mass in the fuel formula.
pub const CARRIER_MASS_T: f32 = 25_000.0;
/// The game's FLOOR: 15-minute preparation plus the 5-minute cooldown.
/// Real jumps run longer under server load (16:30 is common, 40+ seen);
/// the follow layer measures the commander's own request → jump gaps.
pub const DEFAULT_MINUTES_PER_JUMP: f32 = 20.0;
/// Of every star in reach, how many (nearest to the goal) are expanded.
const FANOUT: usize = 64;
/// A second, wider try when the first fan-out finds no route (a sparse
/// stretch where the nearest-to-goal stars all sit past a gap).
const FANOUT_WIDE: usize = 512;

/// Tritium for one jump.
pub fn jump_fuel_t(distance_ly: f32, capacity_used_t: f32, tank_t: f32) -> u32 {
    (5.0 + distance_ly * (capacity_used_t + tank_t + CARRIER_MASS_T) / 200_000.0).round().max(5.0) as u32
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarrierRequest {
    pub from: u32,
    pub to: u32,
    /// Cargo + modules + crew aboard (TotalCapacity − FreeSpace), tonnes.
    pub capacity_used_t: f32,
    /// Tritium in the tank at departure.
    pub tank_t: f32,
    /// Tritium in the HOLD, moved into the tank whenever the next jump
    /// needs it (counted in `capacity_used_t` already while it sits there).
    pub hold_tritium_t: f32,
    /// Systems a carrier may not jump into (permit-locked). Sorted.
    pub blocked: Vec<u32>,
    pub minutes_per_jump: f32,
    /// Give up after this many expansions (0 = no limit).
    pub max_expansions: u64,
    pub time_budget_ms: u64,
}

impl Default for CarrierRequest {
    fn default() -> Self {
        CarrierRequest {
            from: 0,
            to: 0,
            capacity_used_t: 0.0,
            tank_t: TANK_T,
            hold_tritium_t: 0.0,
            blocked: Vec::new(),
            minutes_per_jump: DEFAULT_MINUTES_PER_JUMP,
            max_expansions: 2_000_000,
            time_budget_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CarrierHop {
    pub idx: u32,
    pub name: String,
    pub pos: [f32; 3],
    pub distance_ly: f32,
    pub fuel_t: u32,
    /// Tritium moved from the hold into the tank BEFORE this jump.
    pub topped_up_t: u32,
    pub tank_after_t: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    /// The tank (plus hold) runs dry: this many tonnes short, at this hop
    /// (1-based). The route is still returned so the commander can plan
    /// a refuel.
    ShortBy { tons: u32, at_hop: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CarrierRoute {
    pub from: String,
    pub to: String,
    pub hops: Vec<CarrierHop>,
    pub jumps: u32,
    pub total_ly: f32,
    pub straight_ly: f32,
    pub fuel_t: u32,
    pub tank_end_t: i32,
    pub hold_tritium_end_t: u32,
    /// `jumps × minutes_per_jump` — a floor unless `minutes_per_jump` was
    /// measured from the commander's own jumps.
    pub eta_minutes: u32,
    pub minutes_per_jump: f32,
    pub verdict: Verdict,
    pub expansions: u64,
    pub wall_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarrierError {
    /// No chain of ≤500 ly hops connects the two (a gap, or every bridge
    /// is permit-locked).
    NoRoute,
    Budget,
    Cancelled,
    /// The destination itself is permit-locked.
    DestinationBlocked,
}

impl std::fmt::Display for CarrierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CarrierError::NoRoute => write!(f, "no carrier route: a gap wider than 500 ly, or only permit-locked bridges"),
            CarrierError::Budget => write!(f, "no carrier route found within the search limit"),
            CarrierError::Cancelled => write!(f, "carrier plot cancelled"),
            CarrierError::DestinationBlocked => write!(f, "a carrier cannot jump into a permit-locked system"),
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
struct Open {
    f: f32,
    g: u32,
    ly: f32,
    idx: u32,
}
impl Eq for Open {}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal).then_with(|| o.ly.partial_cmp(&self.ly).unwrap_or(Ordering::Equal))
    }
}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Plan a carrier route. `cancelled` is polled every 200 expansions.
pub fn plan(g: &Galaxy, req: &CarrierRequest, cancelled: &dyn Fn() -> bool) -> Result<CarrierRoute, CarrierError> {
    if req.blocked.binary_search(&req.to).is_ok() {
        return Err(CarrierError::DestinationBlocked);
    }
    let started = std::time::Instant::now();
    let mut expansions = 0u64;
    for fanout in [FANOUT, FANOUT_WIDE] {
        match search(g, req, fanout, cancelled, &mut expansions, &started) {
            Err(CarrierError::NoRoute) => continue,
            other => return other,
        }
    }
    Err(CarrierError::NoRoute)
}

fn search(
    g: &Galaxy,
    req: &CarrierRequest,
    fanout: usize,
    cancelled: &dyn Fn() -> bool,
    expansions: &mut u64,
    started: &std::time::Instant,
) -> Result<CarrierRoute, CarrierError> {
    let goal = g.record(req.to).pos();
    let start = g.record(req.from).pos();
    let straight = dist(start, goal);
    let h = |p: [f32; 3]| dist(p, goal) / MAX_JUMP_LY;
    // Jumps first, then light-years, well below one jump: deterministic
    // and fuel-minimising among equal-jump routes.
    let cost = |jumps: u32, ly: f32| jumps as f32 + 0.0001 * ly / MAX_JUMP_LY;

    let mut best: HashMap<u32, (u32, f32)> = HashMap::default();
    let mut parent: HashMap<u32, u32> = HashMap::default();
    let mut open = BinaryHeap::new();
    best.insert(req.from, (0, 0.0));
    open.push(Open { f: h(start), g: 0, ly: 0.0, idx: req.from });
    let mut cands: Vec<(f32, u32, f32)> = Vec::with_capacity(fanout * 4);

    while let Some(cur) = open.pop() {
        if best.get(&cur.idx).is_some_and(|&(bg, bl)| (bg, bl) < (cur.g, cur.ly)) {
            continue;
        }
        if cur.idx == req.to {
            return Ok(reconstruct(g, req, &parent, straight, *expansions, started));
        }
        *expansions += 1;
        if expansions.is_multiple_of(200) {
            if cancelled() {
                return Err(CarrierError::Cancelled);
            }
            if req.max_expansions > 0 && *expansions > req.max_expansions {
                return Err(CarrierError::Budget);
            }
            if req.time_budget_ms > 0 && started.elapsed().as_millis() as u64 > req.time_budget_ms {
                return Err(CarrierError::Budget);
            }
        }
        let here = g.pos_of(cur.idx);
        cands.clear();
        for (idx, d) in g.within(here, MAX_JUMP_LY) {
            if idx == cur.idx || req.blocked.binary_search(&idx).is_ok() {
                continue;
            }
            cands.push((dist(g.pos_of(idx), goal), idx, d));
        }
        // Nearest to the goal first; the goal itself, if in reach, is
        // first by construction (distance 0).
        cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));
        for &(_, idx, d) in cands.iter().take(fanout) {
            let ng = cur.g + 1;
            let nly = cur.ly + d;
            if best.get(&idx).is_some_and(|&(bg, bl)| cost(bg, bl) <= cost(ng, nly)) {
                continue;
            }
            best.insert(idx, (ng, nly));
            parent.insert(idx, cur.idx);
            open.push(Open { f: cost(ng, nly) + h(g.pos_of(idx)), g: ng, ly: nly, idx });
        }
    }
    Err(CarrierError::NoRoute)
}

fn reconstruct(g: &Galaxy, req: &CarrierRequest, parent: &HashMap<u32, u32>, straight: f32, expansions: u64, started: &std::time::Instant) -> CarrierRoute {
    let mut path = vec![req.to];
    let mut at = req.to;
    while let Some(&p) = parent.get(&at) {
        path.push(p);
        at = p;
    }
    path.reverse();
    // Fuel settles along the path: burned tritium leaves the mass, hold
    // tritium moves into the tank only when the next jump needs it.
    let mut tank = req.tank_t;
    let mut hold = req.hold_tritium_t.max(0.0);
    let mut used = req.capacity_used_t;
    let mut hops = Vec::with_capacity(path.len());
    let mut total_ly = 0.0f32;
    let mut fuel_total = 0u32;
    let mut verdict = Verdict::Ok;
    for (n, pair) in path.windows(2).enumerate() {
        let (a, b) = (pair[0], pair[1]);
        let d = dist(g.pos_of(a), g.pos_of(b));
        let need = jump_fuel_t(d, used, tank) as f32;
        let mut topped = 0.0f32;
        if tank < need && hold > 0.0 {
            topped = (need - tank).min(hold).min(TANK_T - tank).max(0.0);
            // Tritium that was hold mass becomes tank mass: net zero for the
            // formula, but the hold shrinks and the tank grows.
            hold -= topped;
            used -= topped;
            tank += topped;
        }
        let fuel = jump_fuel_t(d, used, tank);
        tank -= fuel as f32;
        if tank < 0.0 && verdict == Verdict::Ok {
            verdict = Verdict::ShortBy { tons: (-tank).ceil() as u32, at_hop: (n + 1) as u32 };
        }
        total_ly += d;
        fuel_total += fuel;
        let rec = g.record(b);
        hops.push(CarrierHop {
            idx: b,
            name: g.name(&rec).to_string(),
            pos: rec.pos(),
            distance_ly: d,
            fuel_t: fuel,
            topped_up_t: topped.round() as u32,
            tank_after_t: tank.round() as i32,
        });
    }
    let jumps = hops.len() as u32;
    CarrierRoute {
        from: g.name(&g.record(req.from)).to_string(),
        to: g.name(&g.record(req.to)).to_string(),
        jumps,
        total_ly,
        straight_ly: straight,
        fuel_t: fuel_total,
        tank_end_t: tank.round() as i32,
        hold_tritium_end_t: hold.round() as u32,
        eta_minutes: (jumps as f32 * req.minutes_per_jump).round() as u32,
        minutes_per_jump: req.minutes_per_jump,
        verdict,
        expansions,
        wall_ms: started.elapsed().as_millis() as u64,
        hops,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import_reader;

    /// Stars along x every 400 ly to 4,000 ly, plus a gap: "Far" at 4,600
    /// (600 ly past the last), and "Bridge" at 4,400 — a stepping stone
    /// only 400 from both sides, off the straight line.
    fn galaxy() -> (tempfile::TempDir, Galaxy) {
        let mut lines = vec!["[".to_string()];
        let mut id = 1;
        for i in 0..=10 {
            lines.push(format!(
                r#"{{"id64":{id},"name":"S{i}","coords":{{"x":{},"y":0,"z":0}},"bodies":[{{"type":"Star","subType":"K (Yellow-Orange) Star","mainStar":true}}]}},"#,
                i * 400
            ));
            id += 1;
        }
        lines.push(format!(r#"{{"id64":{id},"name":"Bridge","coords":{{"x":4300,"y":250,"z":0}},"bodies":[{{"type":"Star","subType":"M (Red dwarf) Star","mainStar":true}}]}},"#));
        id += 1;
        lines.push(format!(r#"{{"id64":{id},"name":"Far","coords":{{"x":4600,"y":0,"z":0}},"bodies":[{{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}}]}},"#));
        id += 1;
        lines.push(format!(r#"{{"id64":{id},"name":"Lost","coords":{{"x":9000,"y":0,"z":0}},"bodies":[{{"type":"Star","subType":"G (White-Yellow) Star","mainStar":true}}]}}"#));
        lines.push("]".into());
        let dir = tempfile::tempdir().unwrap();
        import_reader(Box::new(std::io::Cursor::new(lines.join("\n").into_bytes())), dir.path(), &mut |_| {}).unwrap();
        let g = Galaxy::open(dir.path()).unwrap();
        (dir, g)
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn fuel_formula_matches_the_community_pins() {
        // ed-math: empty carrier, full tank, 500 ly → 70 t; full carrier → 133 t;
        // a measured first jump (census 2026-09-07): 82.28 ly with 680 t aboard
        // and a 500 t tank → 16 t, and the tank read 500 → 484. Exact.
        // ed-math's own worked example: 500 ly, 5,000 t used, 1,000 t tank → 83 t.
        assert_eq!(jump_fuel_t(500.0, 5_000.0, 1000.0), 83);
        assert_eq!(jump_fuel_t(500.0, 0.0, 1000.0), 70);
        assert_eq!(jump_fuel_t(500.0, 25_000.0, 1000.0), 133);
        assert_eq!(jump_fuel_t(82.28, 680.0, 500.0), 16);
        assert_eq!(jump_fuel_t(1.0, 0.0, 0.0), 5, "never below the 5 t floor");
    }

    #[test]
    fn minimises_jumps_at_a_fixed_500_ly_reach() {
        let (_d, g) = galaxy();
        let req = CarrierRequest { from: g.find("S0").unwrap(), to: g.find("S10").unwrap(), ..Default::default() };
        let r = plan(&g, &req, &never).unwrap();
        // 4,000 ly at 400 ly spacing: a 500 ly reach still needs every star
        // (800 is out of reach), so 10 jumps, all 400 ly.
        assert_eq!(r.jumps, 10);
        assert!(r.hops.iter().all(|h| (h.distance_ly - 400.0).abs() < 0.5));
        assert_eq!(r.eta_minutes, 200);
        assert_eq!(r.verdict, Verdict::Ok);
        // Fuel falls as the tank empties: each hop costs a little less.
        assert!(r.hops[0].fuel_t >= r.hops[9].fuel_t);
        assert_eq!(r.tank_end_t, 1000 - r.fuel_t as i32);
    }

    #[test]
    fn a_gap_is_bridged_by_the_off_line_star_and_a_true_gap_is_no_route() {
        let (_d, g) = galaxy();
        let req = CarrierRequest { from: g.find("S10").unwrap(), to: g.find("Far").unwrap(), ..Default::default() };
        let r = plan(&g, &req, &never).unwrap();
        assert_eq!(r.hops.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), vec!["Bridge", "Far"]);
        let req = CarrierRequest { from: g.find("Far").unwrap(), to: g.find("Lost").unwrap(), ..Default::default() };
        assert_eq!(plan(&g, &req, &never).err(), Some(CarrierError::NoRoute));
    }

    #[test]
    fn permit_locked_systems_are_never_entered() {
        let (_d, g) = galaxy();
        let mut blocked = vec![g.find("Bridge").unwrap()];
        blocked.sort();
        let req = CarrierRequest { from: g.find("S10").unwrap(), to: g.find("Far").unwrap(), blocked, ..Default::default() };
        assert_eq!(plan(&g, &req, &never).err(), Some(CarrierError::NoRoute), "the only bridge is locked");
        let mut blocked = vec![g.find("Far").unwrap()];
        blocked.sort();
        let req = CarrierRequest { from: g.find("S10").unwrap(), to: g.find("Far").unwrap(), blocked, ..Default::default() };
        assert_eq!(plan(&g, &req, &never).err(), Some(CarrierError::DestinationBlocked));
    }

    #[test]
    fn a_dry_tank_is_an_honest_shortfall_not_a_truncated_route() {
        let (_d, g) = galaxy();
        let req = CarrierRequest { from: g.find("S0").unwrap(), to: g.find("S10").unwrap(), tank_t: 100.0, capacity_used_t: 20_000.0, ..Default::default() };
        let r = plan(&g, &req, &never).unwrap();
        assert_eq!(r.jumps, 10, "the path is still the path");
        match r.verdict {
            Verdict::ShortBy { tons, at_hop } => {
                assert!(tons > 0 && (1..=10).contains(&at_hop), "{tons} t short at hop {at_hop}");
            }
            Verdict::Ok => panic!("100 t cannot move a laden carrier 4,000 ly"),
        }
        assert!(r.tank_end_t < 0);
    }

    #[test]
    fn hold_tritium_tops_the_tank_up_only_when_a_jump_needs_it() {
        let (_d, g) = galaxy();
        let req = CarrierRequest { from: g.find("S0").unwrap(), to: g.find("S10").unwrap(), tank_t: 100.0, hold_tritium_t: 900.0, capacity_used_t: 900.0, ..Default::default() };
        let r = plan(&g, &req, &never).unwrap();
        assert_eq!(r.verdict, Verdict::Ok);
        let topped: u32 = r.hops.iter().map(|h| h.topped_up_t).sum();
        assert!(topped > 0 && topped <= 900, "{topped}");
        assert!(r.hops[0].topped_up_t == 0, "the first hop is affordable from the tank");
        assert_eq!(r.hold_tritium_end_t, 900 - topped);
    }

    /// The pre-registered pins over the FULL galaxy index — run where the
    /// index lives: `ED_GALAXY_DIR=<dir> cargo test -p ed-galaxy --release
    /// -- --ignored carrier_pins --nocapture`, then paste the CSV rows into
    /// docs/benches/2026-09-07-carrier-router-pins.csv.
    #[test]
    #[ignore]
    fn carrier_pins() {
        let Some(dir) = std::env::var_os("ED_GALAXY_DIR") else {
            eprintln!("ED_GALAXY_DIR not set; skipping");
            return;
        };
        let g = Galaxy::open(std::path::Path::new(&dir)).unwrap();
        println!("from,to,straight_ly,jumps,total_ly,fuel_t,eta_min,expansions,wall_ms,verdict");
        for (from, to) in [("Sol", "Colonia"), ("Sol", "Beagle Point"), ("Sol", "Deciat"), ("Deciat", "Maia"), ("Sol", "Sagittarius A*")] {
            let (Some(a), Some(b)) = (g.find(from), g.find(to)) else {
                println!("{from},{to},,,,,,,,unknown system in this index");
                continue;
            };
            let req = CarrierRequest { from: a, to: b, capacity_used_t: 6_270.0, tank_t: 1000.0, time_budget_ms: 120_000, ..Default::default() };
            match plan(&g, &req, &never) {
                Ok(r) => println!("{from},{to},{:.0},{},{:.0},{},{},{},{},{:?}", r.straight_ly, r.jumps, r.total_ly, r.fuel_t, r.eta_minutes, r.expansions, r.wall_ms, r.verdict),
                Err(e) => println!("{from},{to},,,,,,,,{e}"),
            }
        }
    }
}
