//! Item 28: per-commander time models, fitted from the pilot's own
//! journals. The public website/server prices a jump at a conservative
//! 60 s and a stop at 120 s for pilots it knows nothing about; EDDA
//! knows its pilot, so the local plotter judges routes by how THIS
//! commander actually flies — the median plain inter-jump gap per ship,
//! and the approach overhead their scooping gaps actually carry. This
//! is `docs/benches/knobs/journal_fit.py` ported to run against the
//! app's own event store, with sample floors so a fresh install stays
//! on the defaults until the journals have something to say.
//!
//! Item 28a (the maintainer, verbatim in the ledger): the fit is
//! TRANSIT-ONLY. "It's hard to tell when a commander jumps into a
//! system to do something, or if they are trying to get somewhere" —
//! so a gap only counts while a plotted route is active (between
//! NavRoute and NavRouteClear in the journal), and any gap with
//! interleaved non-transit activity (dropping from supercruise,
//! docking, landing, surface scanning) is discarded. The every-jump
//! auto-honk stays; it happens in transit too.

use std::collections::HashMap;
use std::sync::Mutex;

/// A per-ship fit. Either field may be absent when its sample count is
/// under the floor — they feed `RouteRequest` independently.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeFit {
    pub t_jump_s: Option<f32>,
    pub stop_overhead_s: Option<f32>,
    /// 28b: the jet-dive surcharge — median boosted-departure gap minus
    /// the plain median, from gaps carrying a JetConeBoost. Measured
    /// and reported for the user's test flights; NOT yet fed to the
    /// engine (that pricing change is matrix-gated on real data).
    pub dive_extra_s: Option<f32>,
    pub plain_gaps: usize,
    pub scoop_gaps: usize,
    pub dive_gaps: usize,
}

/// Below these, the fit stays silent and the defaults hold.
const MIN_PLAIN_GAPS: usize = 20;
const MIN_SCOOP_GAPS: usize = 8;
/// A gap only counts as flying cadence inside this window: shorter is a
/// double event, longer is dinner.
const GAP_MIN_S: f64 = 20.0;
const GAP_MAX_S: f64 = 600.0;

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

/// Fit every ship in one pass over the journal events, in order.
fn fit_all(conn: &rusqlite::Connection) -> HashMap<String, TimeFit> {
    let mut plain: HashMap<String, Vec<f64>> = HashMap::new();
    let mut scoop: HashMap<String, Vec<(f64, f64)>> = HashMap::new();
    let mut dive: HashMap<String, Vec<f64>> = HashMap::new();
    let mut dove = false; // JetConeBoost since the last jump
    let mut ship = String::new();
    let mut last_jump: Option<(String, f64)> = None; // (file, epoch)
    let mut scooped = 0.0f64;
    let mut route_active = false;
    let mut dirty = false; // non-transit activity since the last jump
    let Ok(mut st) = conn.prepare(
        "SELECT file, event, json_extract(raw,'$.Ship'), json_extract(raw,'$.Scooped'), \
         CAST(COALESCE(strftime('%s', json_extract(raw,'$.timestamp')), 0) AS REAL) \
         FROM events WHERE event IN ('Loadout','LoadGame','FSDJump','FuelScoop', \
         'NavRoute','NavRouteClear','SupercruiseExit','Touchdown','Docked','Undocked','SAAScanComplete','JetConeBoost') \
         ORDER BY file, offset",
    ) else {
        return HashMap::new();
    };
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<f64>>(3)?,
            r.get::<_, f64>(4)?,
        ))
    });
    let Ok(rows) = rows else { return HashMap::new() };
    for row in rows.flatten() {
        let (file, event, ev_ship, ev_scooped, epoch) = row;
        match event.as_str() {
            "Loadout" | "LoadGame" => {
                if let Some(s) = ev_ship {
                    ship = s.to_ascii_lowercase();
                }
            }
            "FuelScoop" => scooped += ev_scooped.unwrap_or(0.0),
            "JetConeBoost" => dove = true,
            "NavRoute" => route_active = true,
            "NavRouteClear" => route_active = false,
            "SupercruiseExit" | "Touchdown" | "Docked" | "Undocked" | "SAAScanComplete" => dirty = true,
            "FSDJump"
                if epoch > 0.0 => {
                    if let Some((ref lf, lt)) = last_jump {
                        let gap = epoch - lt;
                        // Gaps never span journal files: a new file is a
                        // new session, and the "gap" would be the night.
                        // 28a: transit-only — a route must be active,
                        // and the gap must contain no station/surface
                        // business. Everything else is life, not cadence.
                        if *lf == file && route_active && !dirty && (GAP_MIN_S..=GAP_MAX_S).contains(&gap) && !ship.is_empty() {
                            if scooped > 0.5 {
                                scoop.entry(ship.clone()).or_default().push((gap, scooped));
                            } else if dove {
                                // 28b: the jet dive is its own cadence.
                                dive.entry(ship.clone()).or_default().push(gap);
                            } else {
                                plain.entry(ship.clone()).or_default().push(gap);
                            }
                        }
                    }
                    last_jump = Some((file, epoch));
                    scooped = 0.0;
                    dirty = false;
                    dove = false;
                }
            _ => {}
        }
    }
    let mut out = HashMap::new();
    for (ship, mut gaps) in plain {
        let n = gaps.len();
        let t_jump = (n >= MIN_PLAIN_GAPS).then(|| median(&mut gaps) as f32);
        let sg = scoop.remove(&ship).unwrap_or_default();
        let sn = sg.len();
        // Overhead: what a scooping gap costs beyond a plain jump,
        // minus the fill itself at the ship's hardware rate (unknown
        // here, so the fill term uses the observed tonnes against the
        // fitted seconds — median of (gap - plain_median), floored;
        // the engine prices tonnage separately via scoop_rate).
        let overhead = (sn >= MIN_SCOOP_GAPS && t_jump.is_some()).then(|| {
            let base = t_jump.unwrap() as f64;
            let mut ov: Vec<f64> = sg.iter().map(|(g, _)| (g - base).max(0.0)).collect();
            median(&mut ov).clamp(5.0, 240.0) as f32
        });
        let dg = dive.remove(&ship).unwrap_or_default();
        let dn = dg.len();
        let dive_extra = (dn >= MIN_SCOOP_GAPS && t_jump.is_some()).then(|| {
            let base = t_jump.unwrap() as f64;
            let mut d: Vec<f64> = dg.iter().map(|g| (g - base).max(0.0)).collect();
            median(&mut d).clamp(0.0, 240.0) as f32
        });
        if t_jump.is_some() || overhead.is_some() || dive_extra.is_some() {
            out.insert(ship, TimeFit { t_jump_s: t_jump, stop_overhead_s: overhead, dive_extra_s: dive_extra, plain_gaps: n, scoop_gaps: sn, dive_gaps: dn });
        }
    }
    out
}

/// The cached fit for one ship (journal `Ship` name, case-insensitive).
/// Recomputed only when new events have landed since the last fit.
pub fn fit_for_ship(conn: &rusqlite::Connection, ship: &str) -> Option<TimeFit> {
    static CACHE: Mutex<Option<(i64, HashMap<String, TimeFit>)>> = Mutex::new(None);
    let latest: i64 = conn
        .query_row("SELECT COALESCE(MAX(rowid),0) FROM events", [], |r| r.get(0))
        .unwrap_or(0);
    let mut guard = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let stale = guard.as_ref().is_none_or(|(seen, _)| *seen != latest);
    if stale {
        *guard = Some((latest, fit_all(conn)));
    }
    guard.as_ref().and_then(|(_, m)| m.get(&ship.to_ascii_lowercase()).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_journal(lines: &[(&str, &str)]) -> ed_store::Store {
        let store = ed_store::Store::open_in_memory(std::path::Path::new(".")).unwrap();
        for (i, (event, raw)) in lines.iter().enumerate() {
            store
                .conn()
                .execute(
                    "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', ?1, ?2, ?3, ?4)",
                    rusqlite::params![i as i64, format!("t{i}"), event, raw],
                )
                .unwrap();
        }
        store
    }

    /// The fitter reads this commander's actual cadence: 25 plain
    /// 90-second jumps in the mandalay fit to t_jump 90; ten scooping
    /// gaps at 150 s fit to ~60 s of overhead beyond the plain median.
    #[test]
    fn fits_jump_cadence_and_scoop_overhead_per_ship() {
        let mut lines: Vec<(String, String)> = vec![
            ("Loadout".into(), r#"{"Ship":"mandalay","timestamp":"2026-09-01T00:00:00Z"}"#.into()),
            ("NavRoute".into(), r#"{"timestamp":"2026-09-01T00:00:01Z"}"#.into()),
        ];
        let mut t = 0i64;
        let stamp = |t: i64| format!("2026-09-01T{:02}:{:02}:{:02}Z", t / 3600, (t % 3600) / 60, t % 60);
        for i in 0..26 {
            t += 90;
            lines.push(("FSDJump".into(), format!(r#"{{"timestamp":"{}"}}"#, stamp(t))));
            let _ = i;
        }
        for _ in 0..11 {
            t += 75;
            lines.push(("FuelScoop".into(), format!(r#"{{"Scooped":4.5,"timestamp":"{}"}}"#, stamp(t))));
            t += 75;
            lines.push(("FSDJump".into(), format!(r#"{{"timestamp":"{}"}}"#, stamp(t))));
        }
        // 28b: nine dive gaps at 130 s — JetConeBoost inside the gap
        // classifies it as a dive, not a plain jump, and the surcharge
        // fits to 130 - 90 = 40 s.
        for _ in 0..9 {
            t += 65;
            lines.push(("JetConeBoost".into(), format!(r#"{{"timestamp":"{}"}}"#, stamp(t))));
            t += 65;
            lines.push(("FSDJump".into(), format!(r#"{{"timestamp":"{}"}}"#, stamp(t))));
        }
        let store = store_with_journal(&lines.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect::<Vec<_>>());
        let fit = fit_for_ship(store.conn(), "Mandalay").expect("fit exists");
        assert_eq!(fit.t_jump_s, Some(90.0), "{fit:?}");
        assert_eq!(fit.stop_overhead_s, Some(60.0), "{fit:?}");
        assert_eq!(fit.dive_extra_s, Some(40.0), "{fit:?}");
        assert!(fit.plain_gaps >= MIN_PLAIN_GAPS && fit.scoop_gaps >= MIN_SCOOP_GAPS && fit.dive_gaps == 9);
    }

    /// Item 28a: gaps outside an active route, and gaps carrying
    /// station or surface business, are life rather than cadence and
    /// never reach the fit — even at volumes over the floor.
    #[test]
    fn off_route_and_dirty_gaps_are_excluded() {
        let stamp = |t: i64| format!("2026-09-01T{:02}:{:02}:{:02}Z", t / 3600, (t % 3600) / 60, t % 60);
        let mut lines: Vec<(String, String)> = vec![
            ("Loadout".into(), r#"{"Ship":"mandalay","timestamp":"2026-09-01T00:00:00Z"}"#.into()),
        ];
        let mut t = 0i64;
        // 30 perfect 90 s gaps with NO route plotted: all ignored.
        for _ in 0..31 {
            t += 90;
            lines.push(("FSDJump".into(), format!(r#"{{"timestamp":"{}"}}"#, stamp(t))));
        }
        // Route goes active; 25 gaps of 90 s, but every third gap has a
        // docking in the middle and must be discarded.
        lines.push(("NavRoute".into(), format!(r#"{{"timestamp":"{}"}}"#, stamp(t))));
        for i in 0..25 {
            if i % 3 == 0 {
                t += 45;
                lines.push(("Docked".into(), format!(r#"{{"timestamp":"{}"}}"#, stamp(t))));
                t += 45;
            } else {
                t += 90;
            }
            lines.push(("FSDJump".into(), format!(r#"{{"timestamp":"{}"}}"#, stamp(t))));
        }
        let store = store_with_journal(&lines.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect::<Vec<_>>());
        // 16 clean on-route gaps < the 20 floor: no fit may exist.
        assert!(fit_for_ship(store.conn(), "mandalay").is_none());
    }

    /// A fresh install stays on the defaults: below the sample floors
    /// the fit declines to exist rather than fitting noise.
    #[test]
    fn too_little_history_yields_no_fit() {
        let lines = vec![
            ("Loadout", r#"{"Ship":"mandalay","timestamp":"2026-09-01T00:00:00Z"}"#),
            ("FSDJump", r#"{"timestamp":"2026-09-01T00:02:00Z"}"#),
            ("FSDJump", r#"{"timestamp":"2026-09-01T00:04:00Z"}"#),
        ];
        let store = store_with_journal(&lines);
        assert!(fit_for_ship(store.conn(), "mandalay").is_none());
    }
}
