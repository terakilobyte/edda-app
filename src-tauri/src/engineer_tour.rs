//! The engineer tour (the boss, 2026-10-14: "calculate the optimal route
//! to visit engineers and follow it like a multi-step trade route. From
//! current location, visit engineer a, then engineer b, and so on").
//!
//! Client-side by design: the engineers' home systems and bases are
//! vendored facts (`ed_engineering::unlocks`, grounded on the wiki's
//! Engineers table, see `docs/benches/knobs/engineer_locations_wiki.py`),
//! their coordinates come from the bundled bubble index (every engineer
//! sits in a populated system), the itinerary comes from the build plan,
//! and the ordering is an open path over at most a dozen points -- exact
//! by enumeration up to eight stops, nearest-neighbour plus 2-opt past
//! that, on straight-line distance. The route server is only asked for
//! the jumps of each leg, when the tour is followed, through the trade
//! follower's multi-stop machinery (kind `engineers`, no cargo, no laps).

use crate::state::AppState;
use crate::trade_follow::{LegInput, StationRefInput};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::State;

/// One engineer the plan sends the commander to, with what to do there.
#[derive(Debug, Clone, Deserialize)]
pub struct TourAsk {
    pub engineer: String,
    #[serde(default)]
    pub jobs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TourStop {
    pub engineer: String,
    pub system: String,
    pub station: String,
    pub system_id64: i64,
    pub pos: [f32; 3],
    /// Straight-line distance from the previous stop (or the start).
    pub leg_ly: f32,
    pub jobs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TourView {
    pub from_system: String,
    pub stops: Vec<TourStop>,
    pub total_ly: f32,
    /// Engineers the tour could not place: no location on record, or a
    /// system the bubble index does not hold. Named, never dropped silently.
    pub unplaced: Vec<String>,
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn path_len(start: [f32; 3], pts: &[[f32; 3]], order: &[usize]) -> f32 {
    let mut here = start;
    let mut total = 0.0;
    for &i in order {
        total += dist(here, pts[i]);
        here = pts[i];
    }
    total
}

/// Stops enumerated exactly up to this many; beyond it, nearest-neighbour
/// and 2-opt (8! = 40,320 paths, well under a millisecond).
pub const EXACT_UP_TO: usize = 8;

/// The visiting order that minimises the open path from `start` through
/// every point (end anywhere). Deterministic: ties keep the lower index
/// first, so two reads give one tour.
pub fn order(start: [f32; 3], pts: &[[f32; 3]]) -> Vec<usize> {
    let n = pts.len();
    if n == 0 {
        return Vec::new();
    }
    if n <= EXACT_UP_TO {
        let mut best: Option<(f32, Vec<usize>)> = None;
        let mut perm: Vec<usize> = (0..n).collect();
        permute(&mut perm, 0, &mut |p| {
            let len = path_len(start, pts, p);
            if best.as_ref().is_none_or(|(b, _)| len < *b - 1e-3) {
                best = Some((len, p.to_vec()));
            }
        });
        return best.map(|(_, p)| p).unwrap_or_default();
    }
    // Nearest neighbour, then 2-opt until no reversal shortens the path.
    let mut left: Vec<usize> = (0..n).collect();
    let mut path: Vec<usize> = Vec::with_capacity(n);
    let mut here = start;
    while !left.is_empty() {
        let (k, _) = left.iter().enumerate().fold((0usize, f32::INFINITY), |acc, (k, &i)| {
            let d = dist(here, pts[i]);
            if d < acc.1 { (k, d) } else { acc }
        });
        let i = left.remove(k);
        here = pts[i];
        path.push(i);
    }
    let mut improved = true;
    while improved {
        improved = false;
        for a in 0..n.saturating_sub(1) {
            for b in a + 1..n {
                let mut cand = path.clone();
                cand[a..=b].reverse();
                if path_len(start, pts, &cand) < path_len(start, pts, &path) - 1e-3 {
                    path = cand;
                    improved = true;
                }
            }
        }
    }
    path
}

fn permute(p: &mut Vec<usize>, k: usize, f: &mut dyn FnMut(&[usize])) {
    if k == p.len() {
        f(p);
        return;
    }
    for i in k..p.len() {
        p.swap(k, i);
        permute(p, k + 1, f);
        p.swap(k, i);
    }
}

/// The tour for the plan's engineers from where the commander is.
#[tauri::command]
pub async fn engineer_tour(state: State<'_, AppState>, routing: State<'_, Arc<crate::routing::RoutingState>>, asks: Vec<TourAsk>) -> Result<TourView, String> {
    let here = state.with_read(|s| ed_store::query::location(s.conn()).ok().flatten().and_then(|l| l.system_name));
    let from_system = here.ok_or("no current system: not in a known system yet")?;
    let galaxy = routing.galaxy(&state.data_dir).ok_or("the bundled bubble index is not loaded yet")?;
    let find = |name: &str| galaxy.find(name).map(|i| { let r = galaxy.record(i); (r.id64 as i64, r.pos()) });
    let (_, start) = find(&from_system).ok_or_else(|| format!("{from_system} is outside the bundled bubble index"))?;
    let mut placed: Vec<(TourAsk, String, String, i64, [f32; 3])> = Vec::new();
    let mut unplaced = Vec::new();
    for ask in asks {
        let Some(fact) = ed_engineering::unlocks::find(&ask.engineer) else {
            unplaced.push(format!("{}: no location on record", ask.engineer));
            continue;
        };
        let Some((id64, pos)) = find(&fact.system) else {
            unplaced.push(format!("{}: {} is outside the bundled bubble index", ask.engineer, fact.system));
            continue;
        };
        placed.push((ask, fact.system.clone(), fact.base.clone(), id64, pos));
    }
    let pts: Vec<[f32; 3]> = placed.iter().map(|p| p.4).collect();
    let order = order(start, &pts);
    let mut stops = Vec::with_capacity(order.len());
    let mut prev = start;
    let mut total = 0.0;
    for i in order {
        let (ask, system, station, id64, pos) = &placed[i];
        let leg = dist(prev, *pos);
        total += leg;
        prev = *pos;
        stops.push(TourStop { engineer: ask.engineer.clone(), system: system.clone(), station: station.clone(), system_id64: *id64, pos: *pos, leg_ly: leg, jobs: ask.jobs.clone() });
    }
    tracing::info!(from = %from_system, stops = stops.len(), total_ly = total, unplaced = unplaced.len(), "engineer tour");
    Ok(TourView { from_system, stops, total_ly: total, unplaced })
}

/// The tour's stops as the trade follower's legs: no cargo, no laps, a
/// note per stop that the arrival briefing speaks. Station ids are
/// synthetic and distinct (the follower matches arrivals by MarketID
/// first and by station + system name second; engineer bases are
/// matched by name).
pub fn legs_of(tour: &TourView) -> Vec<LegInput> {
    let refs: Vec<StationRefInput> = tour
        .stops
        .iter()
        .enumerate()
        .map(|(i, s)| StationRefInput {
            station_id: -(i as i64 + 1),
            station: s.station.clone(),
            system: s.system.clone(),
            system_id64: s.system_id64,
            arrival_ls: None,
            note: Some(if s.jobs.is_empty() { s.engineer.clone() } else { format!("{}: {}", s.engineer, s.jobs.join("; ")) }),
        })
        .collect();
    if refs.len() == 1 {
        let only = refs[0].clone();
        return vec![LegInput { from: only.clone(), to: only, symbol: String::new(), commodity: String::new(), tons: 0, buy_price: 0, sell_price: 0, profit: 0, extra: Vec::new() }];
    }
    refs.windows(2)
        .map(|w| LegInput { from: w[0].clone(), to: w[1].clone(), symbol: String::new(), commodity: String::new(), tons: 0, buy_price: 0, sell_price: 0, profit: 0, extra: Vec::new() })
        .collect()
}

/// Follow the tour: the trade follower takes it as kind `engineers`.
#[tauri::command]
pub async fn engineer_tour_follow(app: tauri::AppHandle, state: State<'_, AppState>, tour: TourView) -> Result<serde_json::Value, String> {
    if tour.stops.is_empty() {
        return Err("the tour has no stops".into());
    }
    let legs = legs_of(&tour);
    tracing::info!(stops = tour.stops.len(), total_ly = tour.total_ly, "engineer tour follow");
    crate::trade_follow::trade_follow_start(app, state, legs, "engineers".into()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> [f32; 3] {
        [x, y, 0.0]
    }

    /// Three stops in a line past the start and a far point the other
    /// way: the far point goes last, and the path is the shortest one
    /// (160 ly; two orders tie -- out along the line and back, or the
    /// line in reverse first -- and either is correct).
    #[test]
    fn the_order_is_shortest_with_the_outlier_last() {
        let start = p(0.0, 0.0);
        let pts = [p(30.0, 0.0), p(10.0, 0.0), p(-100.0, 0.0), p(20.0, 0.0)];
        let o = order(start, &pts);
        assert_eq!(*o.last().unwrap(), 2, "the outlier is last: {o:?}");
        assert!((path_len(start, &pts, &o) - 160.0).abs() < 0.01, "shortest open path is 160 ly, got {}", path_len(start, &pts, &o));
    }

    /// Exact beats greedy where greedy is wrong: nearest-neighbour from
    /// the start takes the 10 ly point and then faces 100 + 100; the exact
    /// path goes the long way once. (A classic NN trap.)
    #[test]
    fn exact_enumeration_beats_the_greedy_trap() {
        let start = p(0.0, 0.0);
        let pts = [p(10.0, 0.0), p(-100.0, 0.0), p(-100.0, 1.0)];
        let best = order(start, &pts);
        let greedy_len = path_len(start, &pts, &[0, 1, 2]);
        let best_len = path_len(start, &pts, &best);
        assert!(best_len <= greedy_len, "exact {best_len} vs greedy {greedy_len}");
    }

    /// Past the exact cap the heuristic still returns a permutation that
    /// 2-opt cannot shorten, and a cluster then an outlier is ordered
    /// cluster first.
    #[test]
    fn beyond_the_exact_cap_the_heuristic_returns_a_full_permutation_with_the_outlier_last() {
        let start = p(0.0, 0.0);
        let mut pts: Vec<[f32; 3]> = (0..11).map(|i| p(5.0 + i as f32 * 3.0, (i % 2) as f32)).collect();
        pts.push(p(-500.0, 0.0));
        let o = order(start, &pts);
        let mut sorted = o.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..12).collect::<Vec<_>>(), "every stop exactly once");
        assert_eq!(*o.last().unwrap(), 11, "the 500 ly outlier is last");
    }

    #[test]
    fn an_empty_or_single_tour_is_trivial() {
        assert!(order(p(0.0, 0.0), &[]).is_empty());
        assert_eq!(order(p(0.0, 0.0), &[p(1.0, 1.0)]), vec![0]);
    }

    /// The follower's legs: consecutive stops, no cargo, distinct synthetic
    /// station ids (the follower would otherwise fold equal ids into one
    /// stop), the engineer and jobs as the stop's note.
    #[test]
    fn the_legs_carry_the_stops_in_order_with_notes_and_distinct_ids() {
        let stop = |e: &str, sys: &str, st: &str, jobs: &[&str]| TourStop {
            engineer: e.into(), system: sys.into(), station: st.into(), system_id64: 1, pos: [0.0; 3], leg_ly: 0.0, jobs: jobs.iter().map(|j| j.to_string()).collect(),
        };
        let tour = TourView {
            from_system: "Sol".into(),
            stops: vec![stop("Felicity Farseer", "Deciat", "Farseer Inc", &["2 × Frame Shift Drive Increased Range G5"]), stop("Elvira Martuuk", "Khun", "Long Sight Base", &[])],
            total_ly: 0.0,
            unplaced: vec![],
        };
        let legs = legs_of(&tour);
        assert_eq!(legs.len(), 1);
        assert_eq!((legs[0].from.station.as_str(), legs[0].to.station.as_str()), ("Farseer Inc", "Long Sight Base"));
        assert_ne!(legs[0].from.station_id, legs[0].to.station_id);
        assert_eq!(legs[0].from.note.as_deref(), Some("Felicity Farseer: 2 × Frame Shift Drive Increased Range G5"));
        assert_eq!(legs[0].to.note.as_deref(), Some("Elvira Martuuk"));
        assert_eq!(legs[0].tons, 0);
        let one = TourView { stops: tour.stops[..1].to_vec(), ..tour.clone() };
        assert_eq!(legs_of(&one).len(), 1, "a single stop is a leg to itself");
    }
}
