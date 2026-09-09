//! Powerplay merit model, calibrated from the commander's own sales.
//!
//! # What the journal actually says
//!
//! Two published models were tested against 29 merit-earning sales and both
//! are wrong:
//!
//! * `merits = 70·√tons` (this project's earlier Python solver) fits exactly
//!   one sale -- the one its constant was back-calculated from -- and misses
//!   the rest by 4-6x.
//! * `merits = 0.375219·√profit`, the widely-cited community formula, misses
//!   every observed sale by 0.5x to 3.6x.
//!
//! The journal says merits are **linear in credit profit**, not any square
//! root:
//!
//! ```text
//! merits = floor(profit / K)
//! ```
//!
//! At Herzog Prospect that single relation holds exactly across 8 different
//! commodities spanning three orders of magnitude of profit (12,882 to
//! 969,355 credits), with `K` pinned to a 1.8-credit-wide interval. A sqrt
//! law cannot do that.
//!
//! # What is still unknown
//!
//! `K` varies by station, and what sets it is unresolved. Observed:
//!
//! | Station | System | State | K |
//! |---|---|---|---|
//! | Herzog Prospect | Uterni | Unoccupied | ~1330.6 |
//! | Ramon City | Paesia | Stronghold | ~4092-4781, inconsistent |
//! | Hopper Horizons | Col 285 WU-G a40-5 | Exploited | ~8372.7 |
//! | Hickam Orbital | Bodia | Unoccupied | ~9.04 |
//!
//! Two things that must not be glossed over:
//!
//! * **Ramon City does not fit a single K.** Two Platinum sales at the same
//!   station imply 4781 and 4092. Either `K` moves over time (control
//!   progress shifts continuously) or some awards are split across several
//!   `PowerplayMerits` events in a way the 5-second attribution window
//!   mis-assigns. Unresolved -- so this module reports per-station intervals
//!   and refuses to extrapolate.
//! * **Hickam Orbital's K of ~9 is a different mechanic entirely**, roughly
//!   150x more merits per credit than ordinary trade. The commodity there
//!   (`hr7221wheat`) is a Powerplay delivery commodity, not a market good.
//!   Trade-merit reasoning must not be applied to those.
//!
//! Consequently [`MeritModel::estimate`] returns `None` for a station it has
//! not seen. A route planner that guesses `K` produces confident, specific,
//! wrong advice -- the exact failure this crate exists to avoid.

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

/// Merits earned for `profit` credits at a station whose constant is `k`.
pub fn merits_for(profit: i64, k: f64) -> i64 {
    if profit <= 0 || k <= 0.0 {
        return 0;
    }
    (profit as f64 / k).floor() as i64
}

/// Calibration for one station, as a *bounded interval* rather than a point.
///
/// `merits = floor(profit / K)` only pins `K` to a range: observing `m`
/// merits for `p` profit means `K` lies in `(p/(m+1), p/m]`. Intersecting
/// those ranges over several sales narrows it. An empty intersection means
/// no single `K` explains the station, which is information, not an error.
#[derive(Debug, Clone, Serialize)]
pub struct StationCalibration {
    pub market_id: i64,
    pub station: Option<String>,
    pub system: Option<String>,
    pub powerplay_state: Option<String>,
    pub controlling_power: Option<String>,
    pub samples: usize,
    /// Exclusive lower bound.
    pub k_lo: f64,
    /// Inclusive upper bound.
    pub k_hi: f64,
    /// False when the observations contradict each other.
    pub consistent: bool,
}

impl StationCalibration {
    /// Midpoint of the interval -- only meaningful when `consistent`.
    pub fn k(&self) -> Option<f64> {
        self.consistent.then(|| (self.k_lo + self.k_hi) / 2.0)
    }

    /// Width of the interval as a fraction of K. Small means well-pinned.
    pub fn precision(&self) -> Option<f64> {
        self.k().map(|k| (self.k_hi - self.k_lo) / k)
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MeritModel {
    pub stations: Vec<StationCalibration>,
}

impl MeritModel {
    /// Estimated merits for a sale at a known station.
    ///
    /// `None` when the station has never been observed, or when its
    /// observations are mutually inconsistent. Deliberately not a fallback
    /// to some galaxy-wide average: the observed constants span 1330 to 8373
    /// for ordinary trade, so an average would be wrong everywhere.
    pub fn estimate(&self, market_id: i64, profit: i64) -> Option<i64> {
        let cal = self.stations.iter().find(|s| s.market_id == market_id)?;
        cal.k().map(|k| merits_for(profit, k))
    }

    pub fn for_station(&self, market_id: i64) -> Option<&StationCalibration> {
        self.stations.iter().find(|s| s.market_id == market_id)
    }
}

/// Narrow a `K` interval with one observation of `m` merits for `p` profit.
fn narrow(bounds: &mut (f64, f64), profit: i64, merits: i64) {
    if merits <= 0 || profit <= 0 {
        return;
    }
    let (p, m) = (profit as f64, merits as f64);
    bounds.0 = bounds.0.max(p / (m + 1.0));
    bounds.1 = bounds.1.min(p / m);
}

/// Calibrate from `sales` joined to `merit_events`, grouped by station.
///
/// `window_secs` is how long after a sale a `PowerplayMerits` event may
/// arrive and still be attributed to it. Each merit event is assigned to the
/// *nearest preceding* sale so no award is counted twice.
pub fn calibrate(conn: &Connection, window_secs: i64) -> Result<MeritModel> {
    let joined = crate::query::sales_with_merits(conn, window_secs)?;

    let mut by_station: std::collections::HashMap<i64, (f64, f64, usize)> = Default::default();
    for sale in &joined {
        let (Some(market_id), true) = (sale.market_id, sale.merits > 0) else {
            continue;
        };
        // Prefer measured profit; a mined commodity has no purchase price,
        // so total sale IS the profit there.
        let Some(profit) = sale.profit.or(sale.total_sale) else {
            continue;
        };

        let entry = by_station
            .entry(market_id)
            .or_insert((f64::NEG_INFINITY, f64::INFINITY, 0));
        let mut bounds = (entry.0, entry.1);
        narrow(&mut bounds, profit, sale.merits);
        entry.0 = bounds.0;
        entry.1 = bounds.1;
        entry.2 += 1;
    }

    let mut stations: Vec<StationCalibration> = by_station
        .into_iter()
        .map(|(market_id, (lo, hi, n))| {
            let (station, system) = station_for(conn, market_id).unwrap_or((None, None));
            let (state, power) = system
                .as_deref()
                .and_then(|s| powerplay_latest(conn, s).ok().flatten())
                .unwrap_or((None, None));
            StationCalibration {
                market_id,
                station,
                system,
                powerplay_state: state,
                controlling_power: power,
                samples: n,
                k_lo: lo,
                k_hi: hi,
                consistent: lo < hi,
            }
        })
        .collect();
    stations.sort_by(|a, b| b.samples.cmp(&a.samples));

    Ok(MeritModel { stations })
}

fn station_for(conn: &Connection, market_id: i64) -> Result<(Option<String>, Option<String>)> {
    let row = conn
        .query_row(
            "SELECT json_extract(raw,'$.StationName'), json_extract(raw,'$.StarSystem')
             FROM events WHERE event='Docked' AND market_id = ?1 ORDER BY ts DESC LIMIT 1",
            [market_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap_or((None, None));
    Ok(row)
}

fn powerplay_latest(
    conn: &Connection,
    system: &str,
) -> Result<Option<(Option<String>, Option<String>)>> {
    let row = conn
        .query_row(
            "SELECT powerplay_state, controlling_power FROM powerplay_observations
             WHERE system_name = ?1 COLLATE NOCASE ORDER BY ts DESC LIMIT 1",
            [system],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real sales from Herzog Prospect / Uterni: (profit, merits observed).
    /// Eight commodities, profit spanning 12,882 to 969,355 credits.
    const HERZOG: &[(i64, i64)] = &[
        (12_882, 9),
        (20_800, 15),
        (49_464, 37),
        (62_166, 46),
        (96_255, 72),
        (96_993, 72),
        (751_280, 564),
        (969_355, 728),
    ];

    fn solve(obs: &[(i64, i64)]) -> (f64, f64) {
        let mut b = (f64::NEG_INFINITY, f64::INFINITY);
        for (p, m) in obs {
            narrow(&mut b, *p, *m);
        }
        b
    }

    #[test]
    fn a_single_constant_explains_every_herzog_sale() {
        let (lo, hi) = solve(HERZOG);
        assert!(
            lo < hi,
            "no single K explains the observations: ({lo}, {hi}]"
        );
        // Pinned to under two credits across three orders of magnitude.
        assert!(hi - lo < 2.0, "interval too wide: ({lo}, {hi}]");
        assert!((1329.0..1332.0).contains(&lo));
        assert!((1329.0..1332.0).contains(&hi));
    }

    #[test]
    fn the_calibrated_constant_reproduces_every_observation() {
        let (lo, hi) = solve(HERZOG);
        let k = (lo + hi) / 2.0;
        for (profit, expected) in HERZOG {
            assert_eq!(
                merits_for(*profit, k),
                *expected,
                "profit {profit} should yield {expected} merits at K={k}"
            );
        }
    }

    #[test]
    fn the_community_sqrt_formula_does_not_fit() {
        // merits = 0.375219 * sqrt(profit), per the wiki and Frontier forums.
        for (profit, actual) in HERZOG {
            let predicted = (0.375219 * (*profit as f64).sqrt()).round() as i64;
            let ratio = predicted as f64 / *actual as f64;
            assert!(
                !(0.9..1.1).contains(&ratio),
                "sqrt formula unexpectedly matched at profit={profit}: \
                 predicted {predicted}, actual {actual}"
            );
        }
    }

    #[test]
    fn the_old_sqrt_tons_model_does_not_fit_either() {
        // merits = 70 * sqrt(tons), from aisling_trade_loop.py.
        // Herzog tonnages, in the same order as HERZOG.
        let tons = [19i64, 13, 24, 26, 5, 13, 4, 5];
        for ((_, actual), t) in HERZOG.iter().zip(tons) {
            let predicted = (70.0 * (t as f64).sqrt()).round() as i64;
            assert_ne!(predicted, *actual);
        }
    }

    #[test]
    fn contradictory_observations_are_reported_not_averaged() {
        // Two Platinum sales at Ramon City imply different constants.
        let contradictory = &[(20_138_000i64, 4212i64), (31_415_280, 7677)];
        let (lo, hi) = solve(contradictory);
        assert!(lo > hi, "these observations should not admit a single K");

        let cal = StationCalibration {
            market_id: 1,
            station: None,
            system: None,
            powerplay_state: None,
            controlling_power: None,
            samples: 2,
            k_lo: lo,
            k_hi: hi,
            consistent: lo < hi,
        };
        assert!(!cal.consistent);
        assert_eq!(cal.k(), None, "an inconsistent station must not yield a K");
    }

    #[test]
    fn an_unseen_station_yields_no_estimate() {
        let model = MeritModel::default();
        assert_eq!(model.estimate(999, 1_000_000), None);
    }

    #[test]
    fn merits_for_is_floor_division_and_never_negative() {
        assert_eq!(merits_for(1331 * 3, 1331.0), 3);
        assert_eq!(merits_for(1331 * 3 - 1, 1331.0), 2);
        assert_eq!(merits_for(-500, 1331.0), 0);
        assert_eq!(merits_for(500, 0.0), 0);
    }
}
