//! The live galactic activity heatmap (ledger 2026-09-04, maintainer-claimed
//! slice): the app hears EDDN firsthand, so it can paint where the
//! galaxy is ECONOMICALLY ALIVE right now — board updates, docks and
//! jumps aggregated into a time-decayed spatial layer over the galaxy
//! view.
//!
//! Privacy is a design constraint, not a styling choice (maintainer: "no
//! system labels... no ctrl-click to identify. I don't want it to
//! become a tool for pirates"): cells are 100 ly cubes holding many
//! systems, the snapshot carries positions and intensities ONLY —
//! never a name, address or station — and the galaxy view disables its
//! identify interactions while the layer is shown.
//!
//! Two decay clocks per cell make route lifecycles visible (maintainer:
//! "old hot routes fading out and new hot routes becoming hot"): a
//! SLOW half-life carries the glow of established activity and fades
//! retired routes out; a FAST half-life tracks the last couple of
//! minutes, and its surplus over the slow value marks a cell as
//! RISING — rendered hotter while a route catches fire.

use ed_eddn::Operation;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

/// Cell edge in light years. Coarse on purpose: many systems per cell.
pub const CELL_LY: f32 = 100.0;
/// Established-activity half-life: how long a dead route stays warm.
pub const SLOW_HALF_LIFE_S: f64 = 900.0;
/// Recent-activity half-life: what "right now" means for RISING cells.
pub const FAST_HALF_LIFE_S: f64 = 120.0;
/// Cells below this total intensity are dropped at snapshot/prune time.
const FLOOR: f64 = 0.01;
/// Soft cap on tracked cells; the weakest are pruned past it.
const MAX_CELLS: usize = 20_000;

/// Event weights: a market board update is the strongest economic
/// signal; availability boards and dock identities are commerce echoes;
/// a system observation (an FSDJump heard galaxy-wide) is traffic.
const W_MARKET: f64 = 1.0;
const W_AVAILABILITY: f64 = 0.4;
const W_DOCK: f64 = 0.5;
const W_TRAFFIC: f64 = 1.0;

#[derive(Debug, Default)]
struct Cell {
    market: f64,
    traffic: f64,
    fast: f64,
    /// Weighted mean y of contributions, for placing the glow in 3D.
    y_sum: f64,
    weight: f64,
    /// Unix millis of the last decay application.
    at: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HeatCell {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub market: f32,
    pub traffic: f32,
    /// 0..1: how much of this cell's heat arrived in the last couple of
    /// minutes — a route becoming hot, not merely staying warm.
    pub rising: f32,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HeatSnapshot {
    pub cells: Vec<HeatCell>,
    /// Aggregate instrument row (doctrine rule 2) — counts only, no
    /// identities: events seen/placed since start, and events the name
    /// resolver could not place (no index, or unknown system).
    pub seen: u64,
    pub placed: u64,
    pub unplaced: u64,
}

type Resolver = Box<dyn Fn(&str) -> Option<[f32; 3]> + Send + Sync>;

#[derive(Default)]
pub struct Heatmap {
    cells: Mutex<HashMap<(i32, i32, i32), Cell>>,
    resolver: RwLock<Option<Resolver>>,
    seen: AtomicU64,
    placed: AtomicU64,
    unplaced: AtomicU64,
}

fn decay(value: f64, elapsed_s: f64, half_life_s: f64) -> f64 {
    value * 0.5f64.powf(elapsed_s / half_life_s)
}

impl Heatmap {
    /// Install the name → position resolver (the galaxy index lookup).
    /// Until one is set, only operations that carry positions place.
    pub fn set_resolver(&self, resolver: impl Fn(&str) -> Option<[f32; 3]> + Send + Sync + 'static) {
        *self.resolver.write().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(resolver));
    }

    /// Fold applied EDDN operations into the map. Called from the feed's
    /// write path after a batch lands; cheap (a hash update per event,
    /// plus one name lookup for events without positions).
    pub fn record_ops(&self, ops: &[Operation], now_ms: u64) {
        for op in ops {
            self.seen.fetch_add(1, Ordering::Relaxed);
            let (name, pos, market_w, traffic_w) = match op {
                Operation::Market(s) => (Some(s.system_name.as_str()), None, W_MARKET, 0.0),
                Operation::Outfitting(s) => (Some(s.system_name.as_str()), None, W_AVAILABILITY, 0.0),
                Operation::Shipyard(s) => (Some(s.system_name.as_str()), None, W_AVAILABILITY, 0.0),
                Operation::StationIdentity(s) => (Some(s.system_name.as_str()), None, 0.0, W_DOCK),
                Operation::System(s) => (
                    Some(s.system_name.as_str()),
                    s.position.map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]),
                    0.0,
                    W_TRAFFIC,
                ),
                // Teachings (star classes, bodies, hotspots, signals) are
                // not presence: a NavRoute is a plan, a Scan already
                // counted through its own system event. The map stays a
                // map of traffic and trade.
                Operation::Star(_) | Operation::Body(_) | Operation::RingHotspots(_) | Operation::BodySignals(_) => continue,
            };
            let pos = pos.or_else(|| {
                let resolver = self.resolver.read().unwrap_or_else(|e| e.into_inner());
                resolver.as_ref().and_then(|r| name.and_then(r))
            });
            let Some(pos) = pos else {
                self.unplaced.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            self.placed.fetch_add(1, Ordering::Relaxed);
            self.record_at(pos, market_w, traffic_w, now_ms);
        }
    }

    fn record_at(&self, pos: [f32; 3], market_w: f64, traffic_w: f64, now_ms: u64) {
        let key = (
            (pos[0] / CELL_LY).floor() as i32,
            (pos[1] / CELL_LY).floor() as i32,
            (pos[2] / CELL_LY).floor() as i32,
        );
        let mut cells = self.cells.lock().unwrap_or_else(|e| e.into_inner());
        if cells.len() >= MAX_CELLS && !cells.contains_key(&key) {
            let floor = FLOOR.max(
                cells.values().map(|c| c.market + c.traffic).fold(f64::MAX, f64::min),
            );
            cells.retain(|_, c| c.market + c.traffic > floor);
        }
        let cell = cells.entry(key).or_default();
        let elapsed = (now_ms.saturating_sub(cell.at)) as f64 / 1_000.0;
        if cell.at != 0 {
            cell.market = decay(cell.market, elapsed, SLOW_HALF_LIFE_S);
            cell.traffic = decay(cell.traffic, elapsed, SLOW_HALF_LIFE_S);
            cell.fast = decay(cell.fast, elapsed, FAST_HALF_LIFE_S);
        }
        cell.at = now_ms;
        cell.market += market_w;
        cell.traffic += traffic_w;
        cell.fast += market_w + traffic_w;
        cell.y_sum += f64::from(pos[1]) * (market_w + traffic_w);
        cell.weight += market_w + traffic_w;
    }

    /// The layer as of `now_ms`: decayed intensities at cell centers,
    /// cold cells dropped. Positions and numbers only — nothing in a
    /// snapshot can name a system.
    pub fn snapshot(&self, now_ms: u64) -> HeatSnapshot {
        let mut cells = self.cells.lock().unwrap_or_else(|e| e.into_inner());
        let mut out = Vec::with_capacity(cells.len());
        cells.retain(|key, cell| {
            let elapsed = (now_ms.saturating_sub(cell.at)) as f64 / 1_000.0;
            let market = decay(cell.market, elapsed, SLOW_HALF_LIFE_S);
            let traffic = decay(cell.traffic, elapsed, SLOW_HALF_LIFE_S);
            let fast = decay(cell.fast, elapsed, FAST_HALF_LIFE_S);
            let total = market + traffic;
            if total < FLOOR {
                return false;
            }
            let y = if cell.weight > 0.0 { (cell.y_sum / cell.weight) as f32 } else { 0.0 };
            out.push(HeatCell {
                x: (key.0 as f32 + 0.5) * CELL_LY,
                y,
                z: (key.2 as f32 + 0.5) * CELL_LY,
                market: market as f32,
                traffic: traffic as f32,
                rising: (fast / total.max(f64::MIN_POSITIVE)).clamp(0.0, 1.0) as f32,
            });
            true
        });
        HeatSnapshot {
            cells: out,
            seen: self.seen.load(Ordering::Relaxed),
            placed: self.placed.load(Ordering::Relaxed),
            unplaced: self.unplaced.load(Ordering::Relaxed),
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed_eddn::{ObservedAt, Snapshot, SystemObservation};

    fn at() -> ObservedAt {
        ObservedAt::new("2026-09-04T00:00:00Z", 1_000)
    }

    fn market(system: &str) -> Operation {
        Operation::Market(Snapshot {
            system_name: system.into(),
            station_name: Some("Port".into()),
            market_id: Some(1),
            observed_at: at(),
            values: Vec::new(),
            prohibited: Vec::new(),
        })
    }

    fn jump(system: &str, pos: [f64; 3]) -> Operation {
        Operation::System(SystemObservation {
            system_name: system.into(),
            system_address: None,
            position: Some(pos),
            observed_at: at(),
            controlling_power: None,
            powerplay_state: None,
            powers: None,
            population: None,
            security: None,
            allegiance: None,
        })
    }

    /// The lifecycle the maintainer asked to SEE: a cell heats (rising near 1),
    /// stabilizes under steady ticks (rising falls), and fades toward
    /// nothing when the route dies — while never emitting a name.
    #[test]
    fn routes_heat_stabilize_and_fade() {
        let map = Heatmap::default();
        map.set_resolver(|name| (name == "Ega").then_some([100.0, 5.0, 100.0]));
        let t0 = 1_000_000u64;
        map.record_ops(&[market("Ega")], t0);
        let fresh = map.snapshot(t0);
        assert_eq!(fresh.cells.len(), 1);
        let cell = &fresh.cells[0];
        assert!(cell.rising > 0.9, "a brand-new hot cell is rising: {}", cell.rising);
        assert!(cell.market > 0.9 && cell.traffic == 0.0);
        // Steady ticks for 20 minutes: still hot, no longer "rising"
        // (the fast clock decays while the slow one accumulates).
        let mut t = t0;
        for _ in 0..40 {
            t += 30_000;
            map.record_ops(&[market("Ega")], t);
        }
        let steady = map.snapshot(t);
        assert!(steady.cells[0].market > 2.0, "steady route is hot: {}", steady.cells[0].market);
        assert!(steady.cells[0].rising < 0.75, "steady is not rising: {}", steady.cells[0].rising);
        // Forty-five minutes of silence: three slow half-lives, faded to
        // a remnant; four hours (sixteen half-lives): collected entirely
        // — a 20-minute steady route accumulates ~44 intensity, so it
        // rightly takes hours, not minutes, to leave the map.
        let faded = map.snapshot(t + 2_700_000);
        assert!(faded.cells.is_empty() || faded.cells[0].market < steady.cells[0].market / 7.0);
        assert!(map.snapshot(t + 14_400_000).cells.is_empty(), "dead routes leave the map");
    }

    /// Privacy: the snapshot type carries positions and intensities only.
    /// (The compiler enforces the shape; this pins the serialized form so
    /// a name can never ride along unnoticed.)
    #[test]
    fn snapshots_never_carry_names() {
        let map = Heatmap::default();
        map.record_ops(&[jump("Secret Hotspot", [1.0, 2.0, 3.0])], 5_000);
        let json = serde_json::to_string(&map.snapshot(5_000)).unwrap();
        assert!(!json.contains("Secret"), "no identity may leave the accumulator: {json}");
        assert!(json.contains("rising"));
    }

    /// Events without positions place through the resolver; unknown
    /// systems count as unplaced rather than guessing.
    #[test]
    fn unresolvable_events_are_counted_not_guessed() {
        let map = Heatmap::default();
        map.record_ops(&[market("Nowhere")], 1_000);
        let snap = map.snapshot(1_000);
        assert!(snap.cells.is_empty());
        assert_eq!((snap.seen, snap.placed, snap.unplaced), (1, 0, 1));
    }
}
