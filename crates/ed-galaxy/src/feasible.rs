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
/// at most `max_fuel_per_jump`, so `capacity / max_fuel_per_jump` is the
/// most the tank can fly dry, and one of those is kept in hand for the
/// planner's reserve and the burn model's drift at light mass (the
/// Caspian: 17; the Mandalay: 5). 63 without a model.
pub fn dry_cap(m: Option<&crate::fuel::FuelModel>) -> u8 {
    match m {
        Some(m) if m.max_fuel_per_jump > 0.0 => ((m.capacity / m.max_fuel_per_jump).floor() as u8).saturating_sub(1).clamp(1, 63),
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

/// One direction of the search: its own injection buckets, parents and the
/// star closed first in any bucket (for the meet test). Forward sides
/// expand successors (a hop leaving the popped star); backward sides expand
/// predecessors (a hop arriving at it: plain and injected hops are
/// symmetric, a supercharged hop must leave a neutron or white dwarf, which
/// the highway sub-index finds).
struct Side {
    forward: bool,
    origin: u32,
    target_pos: [f32; 3],
    buckets: Vec<Bucket>,
    k: usize,
    parent: std::collections::HashMap<(u32, u8), (u32, u8, bool)>,
    /// star -> (bucket, dry) at which it closed first.
    closed: std::collections::HashMap<u32, (u8, u8)>,
}

impl Side {
    fn new(forward: bool, origin: u32, origin_pos: [f32; 3], target_pos: [f32; 3], max_inj: usize) -> Self {
        let mut buckets: Vec<Bucket> = (0..=max_inj).map(|_| Bucket::new()).collect();
        buckets[0].push(dist(origin_pos, target_pos), origin, 0);
        Side { forward, origin, target_pos, buckets, k: 0, parent: std::collections::HashMap::default(), closed: std::collections::HashMap::default() }
    }

    fn exhausted(&self) -> bool {
        self.k >= self.buckets.len()
    }

    fn open_len(&self) -> usize {
        self.buckets.get(self.k).map_or(0, |b| b.open.len())
    }

    /// Pop and expand one state; `Some((star, bucket, dry))` when a state closed.
    #[allow(clippy::too_many_arguments)]
    fn step(&mut self, g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, reaches: &Reaches, cap: u8, max_inj: usize, scratch: &mut Vec<(u32, f32)>) -> Option<(u32, u8, u8)> {
        while self.k < self.buckets.len() {
            let k = self.k;
            let Some(std::cmp::Reverse((_, star, dry))) = self.buckets[k].open.pop() else {
                self.k += 1;
                continue;
            };
            if self.buckets[k].best_dry.get(&star).is_some_and(|&d| d <= dry) {
                continue;
            }
            self.buckets[k].best_dry.insert(star, dry);
            self.closed.entry(star).or_insert((k as u8, dry));
            self.expand(g, neutrons, req, reaches, cap, max_inj, star, dry, k, scratch);
            return Some((star, k as u8, dry));
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn expand(&mut self, g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, reaches: &Reaches, cap: u8, max_inj: usize, star: u32, dry: u8, k: usize, scratch: &mut Vec<(u32, f32)>) {
        let here = g.pos_of(star);
        // Plain and injected hops are symmetric; the boosted reach is the
        // departing star's: forward it is this star's class, backward it is
        // each neutron or white dwarf that can reach here.
        let plain_far = if k < max_inj { reaches.plain.max(reaches.injected) } else { reaches.plain };
        let boosted_here = if self.forward { reach_of(g, req, reaches, star) } else { reaches.plain };
        scratch.clear();
        g.for_each_within(here, plain_far.max(boosted_here), |t, d| {
            if t != star {
                scratch.push((t, d));
            }
        });
        let mut push = |side: &mut Side, t: u32, d: f32, injected: bool| {
            let dry_t = if side.forward {
                if g.scoopable(t) { 0 } else { dry + 1 }
            } else if g.scoopable(star) {
                // Backward: the run that ends at `star` restarts at a scoop.
                if g.scoopable(t) { 0 } else { 1 }
            } else if g.scoopable(t) {
                0
            } else {
                dry + 1
            };
            if dry_t > cap {
                return;
            }
            let kn = k + usize::from(injected);
            if side.buckets[kn].best_dry.get(&t).is_some_and(|&b| b <= dry_t) {
                return;
            }
            side.parent.entry((t, kn as u8)).or_insert((star, k as u8, injected));
            let key = dist(g.pos_of(t), side.target_pos);
            let _ = d;
            side.buckets[kn].push(key, t, dry_t);
        };
        for i in 0..scratch.len() {
            let (t, d) = scratch[i];
            let reach = if self.forward { boosted_here } else { reach_of(g, req, reaches, t).max(reaches.plain) };
            if d <= reach {
                push(self, t, d, false);
            } else if k < max_inj && d <= reaches.injected {
                push(self, t, d, true);
            }
        }
        if !self.forward {
            // Supercharged predecessors beyond the plain sphere: neutrons and
            // white dwarfs within their boosted reach of `star`.
            let far = reaches.neutron.max(reaches.white_dwarf);
            if far > plain_far.max(boosted_here) {
                let mut found: Vec<(u32, f32)> = Vec::new();
                neutrons.for_each_within(here, far, |n, d| {
                    if d > reaches.plain {
                        if let Some(full) = g.find(neutrons.name(&neutrons.record(n))) {
                            found.push((full, d));
                        }
                    }
                });
                for (t, d) in found {
                    if t != star && d <= reach_of(g, req, reaches, t) {
                        push(self, t, d, false);
                    }
                }
            }
        }
    }
}

/// The reach a hop LEAVING `star` has: its boosted reach when the request
/// supercharges and the star is a neutron or white dwarf, else plain.
fn reach_of(g: &Galaxy, req: &RouteRequest, reaches: &Reaches, star: u32) -> f32 {
    if !req.supercharge {
        return reaches.plain;
    }
    match g.class(&g.record(star)) {
        crate::StarClass::Neutron => reaches.neutron,
        crate::StarClass::WhiteDwarf => reaches.white_dwarf,
        _ => reaches.plain,
    }
}

/// Find a flyable witness route, or say in bounded time that there is none.
/// Two greedy searches, one from each end, each with its own injection
/// buckets (bucket `k` holds the states reached with `k` injections and is
/// searched to exhaustion before `k + 1` opens on that side) and greedy on
/// straight-line distance to the far end inside a bucket; they alternate
/// one expansion at a time and stop at the first star closed by both whose
/// injections sum within the allowance and whose dry runs join under the
/// cap. Neither side drains a pocket the other side can walk around: on the
/// reverse Jongou pair the forward side alone sat at a 5.6 kly wall and
/// expanded 1.8 M stars in 180 s without finishing (2026-10-18), while the
/// backward side walks the arm out of the island. `NoRoute` when both sides
/// have drained every bucket; `Cancelled` when `cancelled()`.
pub fn find_witness(g: &Galaxy, neutrons: &Galaxy, req: &RouteRequest, cancelled: &(dyn Fn() -> bool + Sync)) -> Result<Witness, RouteError> {
    let reaches = feasible_reaches(req);
    let cap = dry_cap(req.fuel.as_ref());
    let max_inj = req.injection.map(|(_, _, n)| n as usize).unwrap_or(0);
    let (from_pos, to_pos) = (g.pos_of(req.from), g.pos_of(req.to));
    if req.from == req.to {
        return Ok(Witness { stars: vec![req.from], injections: vec![], expansions: 0 });
    }
    let mut fwd = Side::new(true, req.from, from_pos, to_pos, max_inj);
    let mut bwd = Side::new(false, req.to, to_pos, from_pos, max_inj);
    let trace = std::env::var_os("ED_WITNESS_TRACE").is_some();
    let mut scratch: Vec<(u32, f32)> = Vec::with_capacity(1024);
    let mut expansions: u64 = 0;
    loop {
        if fwd.exhausted() && bwd.exhausted() {
            return Err(RouteError::NoRoute);
        }
        // The side with the smaller frontier moves; an exhausted side waits.
        let forward_turn = !fwd.exhausted() && (bwd.exhausted() || fwd.open_len() <= bwd.open_len());
        let (mover, other) = if forward_turn { (&mut fwd, &bwd) } else { (&mut bwd, &fwd) };
        let Some((star, k, dry)) = mover.step(g, neutrons, req, &reaches, cap, max_inj, &mut scratch) else {
            continue;
        };
        expansions += 1;
        if expansions.is_multiple_of(256) && cancelled() {
            return Err(RouteError::Cancelled);
        }
        if trace && expansions.is_multiple_of(20_000) {
            eprintln!("    WITNESS exp {expansions} fwd(k {} open {} closed {}) bwd(k {} open {} closed {})", fwd.k, fwd.open_len(), fwd.closed.len(), bwd.k, bwd.open_len(), bwd.closed.len());
        }
        if let Some(&(ko, dry_o)) = other.closed.get(&star) {
            let joined_dry = if g.scoopable(star) { 0 } else { dry as u32 + dry_o as u32 };
            if k as usize + ko as usize <= max_inj && joined_dry <= cap as u32 {
                let (kf, kb) = if forward_turn { (k, ko) } else { (ko, k) };
                return Ok(stitch(&fwd, &bwd, star, kf, kb, expansions));
            }
        }
    }
}

/// The witness through `meet`: the forward chain from the origin to it, then
/// the backward chain from it to the goal.
fn stitch(fwd: &Side, bwd: &Side, meet: u32, kf: u8, kb: u8, expansions: u64) -> Witness {
    let mut head = vec![meet];
    let mut head_inj: Vec<bool> = Vec::new();
    let mut cur = (meet, kf);
    while cur.0 != fwd.origin || cur.1 != 0 {
        let &(p, pk, injected) = fwd.parent.get(&cur).expect("a forward parent");
        head_inj.push(injected);
        head.push(p);
        cur = (p, pk);
    }
    head.reverse();
    head_inj.reverse();
    let mut stars = head;
    let mut injected_hops = head_inj;
    let mut cur = (meet, kb);
    while cur.0 != bwd.origin || cur.1 != 0 {
        let &(n, nk, injected) = bwd.parent.get(&cur).expect("a backward parent");
        injected_hops.push(injected);
        stars.push(n);
        cur = (n, nk);
    }
    let injections = injected_hops.iter().enumerate().filter(|(_, &inj)| inj).map(|(i, _)| i + 1).collect();
    Witness { stars, injections, expansions }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fuel::{BoostProfile, FuelModel};
    use crate::long_range::tests::{build, star, K, N};

    /// A dead-end pocket is not drained when the other side meets (the
    /// reverse Jongou pair: the forward side alone expanded 1.8 M stars in
    /// 180 s at a 5.6 kly wall). The straight line of K stars toward the
    /// goal ends at a 210-star pocket; the only way round is an arc of 79
    /// stars. The backward side walks the arc from the goal and meets the
    /// forward side at the start; the forward side never finishes the pocket.
    #[test]
    fn a_dead_end_pocket_is_not_drained_when_the_other_side_meets() {
        let mut stars = Vec::new();
        for i in 0..=25 {
            stars.push(star(&format!("L{i}"), i as f32 * 60.0, 0.0, 0.0, K));
        }
        for xi in 0..7 {
            for yi in 1..=15 {
                for sign in [-1.0f32, 1.0] {
                    stars.push(star(&format!("P{xi}_{yi}_{}", if sign > 0.0 { "n" } else { "s" }), 1140.0 + xi as f32 * 60.0, sign * yi as f32 * 60.0, 0.0, K));
                }
            }
        }
        for i in 1..=15 {
            stars.push(star(&format!("U{i}"), 0.0, i as f32 * 60.0, 0.0, K));
        }
        for i in 1..=50 {
            stars.push(star(&format!("T{i}"), i as f32 * 60.0, 900.0, 0.0, K));
        }
        for i in 1..=14 {
            stars.push(star(&format!("D{i}"), 3000.0, 900.0 - i as f32 * 60.0, 0.0, K));
        }
        stars.push(star("Goal", 3000.0, 0.0, 0.0, K));
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("L0").unwrap(), to: f.g.find("Goal").unwrap(), range_ly: 72.0, ..Default::default() };
        let w = find_witness(&f.g, &f.neutrons, &req, &|| false).unwrap();
        assert_eq!((w.stars.first(), w.stars.last()), (Some(&req.from), Some(&req.to)));
        assert!(w.injections.is_empty());
        assert!(w.expansions < 240, "the pocket (210 stars) was drained: {} expansions", w.expansions);
        let names: Vec<&str> = w.stars.iter().map(|&s| f.g.name(&f.g.record(s))).collect();
        assert!(names.iter().any(|n| n.starts_with('T')), "the route is the arc: {names:?}");
    }

    /// A chain of K stars 60 ly apart, 20 of them: a witness exists with no
    /// injection and at most one dry jump between scoops.
    #[test]
    fn a_plain_chain_has_a_witness_without_injections() {
        let stars: Vec<_> = (0..20).map(|i| star(&format!("K{i}"), i as f32 * 60.0, 0.0, 0.0, K)).collect();
        let f = build(&stars);
        let req = RouteRequest { from: f.g.find("K0").unwrap(), to: f.g.find("K19").unwrap(), range_ly: 72.0, ..Default::default() };
        let w = find_witness(&f.g, &f.neutrons, &req, &|| false).unwrap();
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
        let w = find_witness(&f.g, &f.neutrons, &req, &|| false).unwrap();
        assert_eq!(w.injections, vec![w.stars.len() - 1], "one injection, on the hop into the island: {:?}", w.stars);
        let none = RouteRequest { injection: None, ..req.clone() };
        assert!(matches!(find_witness(&f.g, &f.neutrons, &none, &|| false), Err(RouteError::NoRoute)), "without injections the island is unreachable, and the search says so");
    }

    /// Brown dwarfs (unscoopable) 60 ly apart for 25 hops, a Mandalay with
    /// a 32 t tank and 5.2 t a jump (5 dry jumps, one jump in hand): no
    /// witness, because the 6th dry jump cannot be flown; with a K star
    /// every 5 hops the witness exists and never exceeds the cap.
    #[test]
    fn the_witness_never_runs_more_dry_jumps_than_the_tank_allows() {
        let m = FuelModel::from_loadout(319.0, 32.0, 5.2, 5, true, false, 77.9, 10.5, 0.0);
        assert_eq!(dry_cap(Some(&m)), 5, "32 t at 5.2 t a jump is 6 maximum jumps; one is kept in hand for the reserve and the burn model's drift");
        let caspian = FuelModel::from_loadout(1323.3, 128.0, 6.8, 8, true, true, 77.81, 10.5, 0.0);
        assert_eq!(dry_cap(Some(&caspian)), 17);
        let dry: Vec<_> = (0..26).map(|i| star(&format!("D{i}"), i as f32 * 60.0, 0.0, 0.0, "L (Brown dwarf) Star")).collect();
        let f = build(&dry);
        let req = RouteRequest { from: f.g.find("D0").unwrap(), to: f.g.find("D25").unwrap(), range_ly: m.range_at(m.capacity), fuel: Some(m), start_fuel: 32.0, ..Default::default() };
        assert!(matches!(find_witness(&f.g, &f.neutrons, &req, &|| false), Err(RouteError::NoRoute)));
        let mixed: Vec<_> = (0..26).map(|i| star(&format!("S{i}"), i as f32 * 60.0, 0.0, 0.0, if i % 5 == 0 { K } else { "L (Brown dwarf) Star" })).collect();
        let f = build(&mixed);
        let req = RouteRequest { from: f.g.find("S0").unwrap(), to: f.g.find("S25").unwrap(), ..req };
        let w = find_witness(&f.g, &f.neutrons, &req, &|| false).unwrap();
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
        match find_witness(&f.g, &f.neutrons, &req, &|| false) {
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
        assert_eq!(find_witness(&f.g, &f.neutrons, &req, &|| false).unwrap().stars.len(), 6);
        let weak = RouteRequest { boost: BoostProfile::default(), ..req.clone() };
        assert!(matches!(find_witness(&f.g, &f.neutrons, &weak, &|| false), Err(RouteError::NoRoute)));
    }
}
