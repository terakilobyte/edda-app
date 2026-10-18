//! The feasibility search (planner unification R3, 2026-10-18): a flyable
//! witness route in seconds, or a bounded "no" (the holistic review's
//! spine). Fuel is relaxed to a dry-jump count with full-tank reaches;
//! injections are lexicographic buckets, so none is spent while a
//! plain-and-boost frontier remains; a witness always replays under the
//! real fuel model because every maximum jump burns at most
//! `max_fuel_per_jump`.
use crate::format::Galaxy;
use crate::router::{RouteError, RouteRequest};

/// A flyable star sequence: hop `i` is `stars[i - 1] -> stars[i]`;
/// `injections` lists the hops that need one.
#[derive(Debug, Clone)]
pub struct Witness {
    pub stars: Vec<u32>,
    pub injections: Vec<usize>,
    pub expansions: u64,
}

/// Full-tank reaches the feasibility search flies with: the fuel model's
/// `reach(capacity, boost)` or `range_ly x boost`. Fuel is relaxed to a
/// dry-jump count (see `dry_cap`), so these are the only figures it needs.
pub struct Reaches {
    pub plain: f32,
    pub neutron: f32,
    pub white_dwarf: f32,
    /// A plain jump with the request's injection (a boost does not stack).
    pub injected: f32,
}

pub fn feasible_reaches(req: &RouteRequest) -> Reaches {
    let at = |boost: f32| match req.fuel {
        Some(m) => m.reach(m.capacity, boost),
        None => req.range_ly.max(1.0) * boost,
    };
    let (n, wd) = if req.supercharge { (req.boost.neutron, req.boost.white_dwarf) } else { (1.0, 1.0) };
    Reaches { plain: at(1.0), neutron: at(n), white_dwarf: at(wd), injected: at(req.injection.map(|(m, _, _)| m).unwrap_or(1.0)) }
}

/// Jumps the tank funds between scoops: a maximum jump of any kind burns
/// at most `max_fuel_per_jump`, so `capacity / max_fuel_per_jump` dry
/// jumps always replay under the real model (the Caspian: 18; the
/// Mandalay: 6). 63 without a model.
pub fn dry_cap(m: Option<&crate::fuel::FuelModel>) -> u8 {
    match m {
        Some(m) if m.max_fuel_per_jump > 0.0 => ((m.capacity / m.max_fuel_per_jump).floor() as u8).clamp(1, 63),
        _ => 63,
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// One injection bucket: its own frontier (greedy on distance to the goal)
/// and its own closed set, `best_dry` -- a star is re-expanded only when a
/// new entry arrives with fewer dry jumps than the one that closed it.
struct Bucket {
    open: std::collections::BinaryHeap<std::cmp::Reverse<(u32, u32, u8)>>,
    best_dry: std::collections::HashMap<u32, u8>,
}

impl Bucket {
    fn new() -> Self {
        Bucket { open: std::collections::BinaryHeap::new(), best_dry: std::collections::HashMap::default() }
    }
    fn push(&mut self, key: f32, star: u32, dry: u8) {
        self.open.push(std::cmp::Reverse((key.max(0.0).to_bits(), star, dry)));
    }
}

/// Find a flyable witness route, or say in bounded time that there is none.
/// State = (star, jumps since the last scoop, injections used). Bucket `k`
/// holds the states reached with `k` injections and is searched to
/// exhaustion before bucket `k + 1` opens, so no injection is spent while
/// a plain-and-boost path still has frontier; within a bucket the order is
/// greedy on straight-line distance to the goal. `NoRoute` when every
/// bucket up to the allowance has drained; `Cancelled` when `cancelled()`.
pub fn find_witness(g: &Galaxy, req: &RouteRequest, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Witness, RouteError> {
    let reaches = feasible_reaches(req);
    let cap = dry_cap(req.fuel.as_ref());
    let max_inj = req.injection.map(|(_, _, n)| n as usize).unwrap_or(0);
    let goal = g.pos_of(req.to);
    let reach_from = |star: u32| -> f32 {
        if !req.supercharge {
            return reaches.plain;
        }
        match g.class(&g.record(star)) {
            crate::StarClass::Neutron => reaches.neutron,
            crate::StarClass::WhiteDwarf => reaches.white_dwarf,
            _ => reaches.plain,
        }
    };
    // parent[(star, bucket)] = (parent star, parent bucket, this hop was injected)
    let mut parent: std::collections::HashMap<(u32, u8), (u32, u8, bool)> = std::collections::HashMap::default();
    let mut buckets: Vec<Bucket> = (0..=max_inj).map(|_| Bucket::new()).collect();
    // The tank is full at departure: zero dry jumps so far.
    buckets[0].push(dist(g.pos_of(req.from), goal), req.from, 0);
    let mut expansions: u64 = 0;
    let mut best_seen = f32::INFINITY;
    let trace = std::env::var_os("ED_WITNESS_TRACE").is_some();
    let mut scratch: Vec<(u32, f32)> = Vec::with_capacity(1024);
    for k in 0..=max_inj {
        while let Some(std::cmp::Reverse((_, star, dry))) = buckets[k].open.pop() {
            if buckets[k].best_dry.get(&star).is_some_and(|&d| d <= dry) {
                continue;
            }
            buckets[k].best_dry.insert(star, dry);
            if star == req.to {
                return Ok(reconstruct(req, &parent, k as u8, expansions));
            }
            expansions += 1;
            if expansions.is_multiple_of(256) && cancelled() {
                return Err(RouteError::Cancelled);
            }
            if trace && expansions.is_multiple_of(20_000) {
                eprintln!("    WITNESS exp {expansions} bucket {k} open {} closed {} here-to-goal {:.0} ly best {:.0} ly", buckets[k].open.len(), buckets[k].best_dry.len(), dist(g.pos_of(star), goal), best_seen);
            }
            best_seen = best_seen.min(dist(g.pos_of(star), goal));
            let here = g.pos_of(star);
            let reach = reach_from(star);
            let far = if k < max_inj { reach.max(reaches.injected) } else { reach };
            scratch.clear();
            g.for_each_within(here, far, |t, d| {
                if t != star {
                    scratch.push((t, d));
                }
            });
            for &(t, d) in scratch.iter() {
                let dry_t = if g.scoopable(t) { 0 } else { dry + 1 };
                if dry_t > cap {
                    continue;
                }
                let key = dist(g.pos_of(t), goal);
                if d <= reach {
                    if buckets[k].best_dry.get(&t).is_some_and(|&b| b <= dry_t) {
                        continue;
                    }
                    parent.entry((t, k as u8)).or_insert((star, k as u8, false));
                    buckets[k].push(key, t, dry_t);
                } else if k < max_inj && d <= reaches.injected {
                    let kn = k + 1;
                    if buckets[kn].best_dry.get(&t).is_some_and(|&b| b <= dry_t) {
                        continue;
                    }
                    parent.entry((t, kn as u8)).or_insert((star, k as u8, true));
                    buckets[kn].push(key, t, dry_t);
                }
            }
        }
        // Bucket k has drained: the next bucket's seeds are already in place.
    }
    Err(RouteError::NoRoute)
}

fn reconstruct(req: &RouteRequest, parent: &std::collections::HashMap<(u32, u8), (u32, u8, bool)>, k: u8, expansions: u64) -> Witness {
    let mut stars = vec![req.to];
    let mut injected_hops: Vec<bool> = Vec::new();
    let mut cur = (req.to, k);
    while cur.0 != req.from || cur.1 != 0 {
        let &(p, pk, injected) = parent.get(&cur).expect("every reached star has a parent");
        injected_hops.push(injected);
        stars.push(p);
        cur = (p, pk);
    }
    stars.reverse();
    injected_hops.reverse();
    let injections = injected_hops.iter().enumerate().filter(|(_, &inj)| inj).map(|(i, _)| i + 1).collect();
    Witness { stars, injections, expansions }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuel::{BoostProfile, FuelModel};
    use crate::long_range::tests::{build, star, K, N};

    /// A chain of K stars 60 ly apart, 20 of them: a witness exists with no
    /// injection and at most one dry jump between scoops.
    #[test]
    fn a_plain_chain_has_a_witness_without_injections() {
        let stars: Vec<_> = (0..20).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("K0").unwrap(), to: f.g.find("K19").unwrap(), range_ly: 72.0, ..Default::default() };
        let w = find_witness(&f.g, &req, &|| false).unwrap();
        assert_eq!((w.stars.first(), w.stars.last()), (Some(&req.from), Some(&req.to)));
        assert!(w.injections.is_empty());
        assert_eq!(w.stars.len(), 20, "greedy on distance walks the chain: {}", w.stars.len());
    }

    /// An island K star 100 ly from the chain's end: only a x2 injection
    /// (144 ly at range 72) enters it, and the witness spends exactly one,
    /// on the last hop.
    #[test]
    fn an_island_goal_is_entered_with_one_injection_and_no_more() {
        let mut stars: Vec<_> = (0..5).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        stars.push(star("Island", 340.0, 0.0, 0.0, K));
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("K0").unwrap(), to: f.g.find("Island").unwrap(), range_ly: 72.0, injection: Some((2.0, "premium", 22)), ..Default::default() };
        let w = find_witness(&f.g, &req, &|| false).unwrap();
        assert_eq!(w.injections, vec![w.stars.len() - 1], "one injection, on the hop into the island: {:?}", w.stars);
        let none = RouteRequest { injection: None, ..req.clone() };
        assert!(matches!(find_witness(&f.g, &none, &|| false), Err(RouteError::NoRoute)), "without injections the island is unreachable, and the search says so");
    }

    /// Brown dwarfs (unscoopable) 60 ly apart for 25 hops, a Mandalay with
    /// a 32 t tank and 5.2 t a jump (6 dry jumps): no witness, because the
    /// 7th dry jump cannot be flown; with a K star every 5 hops the witness
    /// exists and never exceeds 6 dry jumps between scoops.
    #[test]
    fn the_witness_never_runs_more_dry_jumps_than_the_tank_allows() {
        let m = FuelModel::from_loadout(319.0, 32.0, 5.2, 5, true, false, 77.9, 10.5, 0.0);
        assert_eq!(dry_cap(Some(&m)), 6);
        let dry: Vec<_> = (0..26).map(|i| star(&format!("D{i}"), i as f32 * 60.0, 0.0, 0.0, "L (Brown dwarf) Star")).collect();
        let f = build(&dry);
        let req = RouteRequest { from: f.g.find("D0").unwrap(), to: f.g.find("D25").unwrap(), range_ly: m.range_at(m.capacity), fuel: Some(m), start_fuel: 32.0, ..Default::default() };
        assert!(matches!(find_witness(&f.g, &req, &|| false), Err(RouteError::NoRoute)));
        let mixed: Vec<_> = (0..26).map(|i| star(&format!("S{i}"), i as f32 * 60.0, 0.0, 0.0, if i % 5 == 0 { K } else { "L (Brown dwarf) Star" })).collect();
        let f = build(&mixed);
        let req = RouteRequest { from: f.g.find("S0").unwrap(), to: f.g.find("S25").unwrap(), ..req };
        let w = find_witness(&f.g, &req, &|| false).unwrap();
        let mut run = 0usize;
        for &s in &w.stars[1..] {
            if f.g.scoopable(s) {
                run = 0;
            } else {
                run += 1;
                assert!(run <= dry_cap(req.fuel.as_ref()) as usize, "{run} dry jumps in a row");
            }
        }
    }

    /// A goal nothing reaches (5,000 ly off, no injections): the frontier
    /// drains and the answer is NoRoute, not a budget.
    #[test]
    fn a_disconnected_island_is_refused_once_the_frontier_drains() {
        let mut stars: Vec<_> = (0..10).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        stars.push(star("Far", 0.0, 5000.0, 0.0, K));
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("K0").unwrap(), to: f.g.find("Far").unwrap(), range_ly: 72.0, ..Default::default() };
        match find_witness(&f.g, &req, &|| false) {
            Err(RouteError::NoRoute) => {}
            other => panic!("{other:?}"),
        }
    }

    /// Neutrons 400 ly apart (boost x4 at range 72 reaches 288 -- too short;
    /// x6 reaches 432): the witness boosts only from neutrons and only when
    /// the profile allows it.
    #[test]
    fn boosted_hops_leave_neutrons_only() {
        let stars: Vec<_> = (0..6).map(|i| star(&format!("N{i}"), i as f32 * 400.0, 0.0, 0.0, N)).collect();
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("N0").unwrap(), to: f.g.find("N5").unwrap(), range_ly: 72.0, boost: BoostProfile { neutron: 6.0, white_dwarf: 1.5 }, ..Default::default() };
        assert_eq!(find_witness(&f.g, &req, &|| false).unwrap().stars.len(), 6);
        let weak = RouteRequest { boost: BoostProfile::default(), ..req.clone() };
        assert!(matches!(find_witness(&f.g, &weak, &|| false), Err(RouteError::NoRoute)));
    }
}
