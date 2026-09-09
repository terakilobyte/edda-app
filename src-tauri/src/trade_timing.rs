//! The commander's own trade-leg timing, from the journal - but only
//! while EDDA was following a trade route (maintainer, 2026-09-09: "we can
//! only measure when actively following a trade route, so we have to
//! use constants until we have a large enough sample size"). The server
//! prices a leg with `ed_route::cost::Timing` - jump 18 s, undock 60 s,
//! docking 60 s, market 30 s, supercruise base 45 s - unless the
//! request carries the pilot's own numbers.
//!
//! Why follow-only: a wide walk over the maintainer's two months of journals
//! (`docs/benches/trade-timing-profile-2026-09-09-waldorf-*.csv`) found
//! the constants 2-3x short, but that is how he flies while building
//! the app, not how he flies a trade loop - and the supercruise "3x" in
//! that walk compared against the 45 s BASE alone; the model's full
//! term (45 + 22·ln(1+ls)) over-prices his approaches (retraction,
//! 2026-09-09). So the trade follower records
//! its active windows (`trade_follow_windows`), samples are taken from
//! the journal inside those windows only, per phase and per ship, and a
//! phase replaces its constant only once it has [`MIN_SAMPLES`]. Until
//! then the server's defaults stand and nothing says `measured`.
//!
//! Phases, mirroring `docs/benches/knobs/trade_timing_profile.py`:
//!   jump         FSDJump -> FSDJump with only transit events between
//!   undock       Undocked -> first FSDJump (no dock between)
//!   docking      DockingGranted -> Docked
//!   supercruise  arrival FSDJump / SupercruiseEntry -> SupercruiseExit
//!                that a Docked follows within 15 min
//!   market       Docked -> Undocked with a MarketBuy/MarketSell between

use ed_route::cost::Timing;

/// Samples a phase needs before its median replaces the constant.
pub const MIN_SAMPLES: usize = 20;

/// (low, high) seconds a sample must fall in to count - anything outside
/// is a pause, a relog or a different activity, not the phase.
const BOUNDS: [(&str, f64, f64); 5] = [
    ("jump", 5.0, 300.0),
    ("undock", 10.0, 600.0),
    ("docking", 5.0, 600.0),
    ("supercruise", 5.0, 3600.0),
    ("market", 20.0, 1800.0),
];

/// Events that can sit between two jumps without meaning the pilot did
/// something other than travel.
const TRANSIT: &[&str] = &[
    "FSDJump", "FuelScoop", "StartJump", "FSSSignalDiscovered", "FSSDiscoveryScan", "Scan", "NavRoute",
    "NavRouteClear", "ReceiveText", "Music", "ShipTargeted", "SupercruiseEntry", "JetConeBoost",
    "FSSAllBodiesFound", "CodexEntry", "ReservoirReplenished", "Friends", "Shutdown", "Fileheader",
    "Commander", "LoadGame", "Loadout", "Materials", "Rank", "Progress", "Statistics", "Location",
    "Powerplay", "Reputation", "EngineerProgress", "SquadronStartup", "Missions", "Cargo", "Status",
];

/// A journal row the walk needs: event name, epoch seconds, the ship
/// named by a Loadout/LoadGame row (lower-case journal ident).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub event: String,
    pub epoch: f64,
    pub ship: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Profile {
    pub jump: Vec<f64>,
    pub undock: Vec<f64>,
    pub docking: Vec<f64>,
    pub supercruise: Vec<f64>,
    pub market: Vec<f64>,
}

fn bounded(name: &str, xs: &[f64]) -> Vec<f64> {
    let (_, lo, hi) = BOUNDS.iter().find(|(n, _, _)| *n == name).copied().unwrap_or(("", 0.0, f64::MAX));
    xs.iter().copied().filter(|x| (lo..=hi).contains(x)).collect()
}

fn median(xs: &mut [f64]) -> f64 {
    xs.sort_by(|a, b| a.total_cmp(b));
    let n = xs.len();
    if n % 2 == 1 { xs[n / 2] } else { (xs[n / 2 - 1] + xs[n / 2]) / 2.0 }
}

impl Profile {
    /// The wire timing: each phase's median where the sample count
    /// clears the floor, the server default elsewhere; `measured` only
    /// when at least one phase is the pilot's.
    pub fn timing(&self) -> Timing {
        let mut t = Timing::default();
        let mut measured = false;
        let mut take = |name: &str, xs: &[f64], slot: &mut f64| {
            let mut v = bounded(name, xs);
            if v.len() >= MIN_SAMPLES {
                *slot = median(&mut v);
                measured = true;
            }
        };
        take("jump", &self.jump, &mut t.jump_seconds);
        take("undock", &self.undock, &mut t.undock_seconds);
        take("docking", &self.docking, &mut t.docking_seconds);
        take("supercruise", &self.supercruise, &mut t.supercruise_base_seconds);
        take("market", &self.market, &mut t.market_seconds);
        t.measured = measured;
        t
    }

    /// Sample counts per phase (jump, undock, docking, supercruise,
    /// market), for the trace.
    pub fn counts(&self) -> [usize; 5] {
        [self.jump.len(), self.undock.len(), self.docking.len(), self.supercruise.len(), self.market.len()]
    }
}

/// Open a follow window: called when the trade follower starts.
pub fn window_open(conn: &rusqlite::Connection) {
    let _ = conn.execute(
        "INSERT INTO trade_follow_windows (started) VALUES (strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
        [],
    );
}

/// Close every open window: called when the trade follower stops or is
/// cleared. A window left open by a crash ends at "now" when read.
pub fn window_close(conn: &rusqlite::Connection) {
    let _ = conn.execute(
        "UPDATE trade_follow_windows SET ended = strftime('%Y-%m-%dT%H:%M:%SZ','now') WHERE ended IS NULL",
        [],
    );
}

/// The profile for `ship` (lower-case journal ident; `None` = any ship)
/// from the journal rows inside every recorded trade-follow window.
pub fn profile(conn: &rusqlite::Connection, ship: Option<&str>) -> Profile {
    let Ok(mut st) = conn.prepare(
        "SELECT e.event, CAST(COALESCE(strftime('%s', e.ts), 0) AS REAL), lower(json_extract(e.raw, '$.Ship')) \
         FROM events e \
         WHERE EXISTS (SELECT 1 FROM trade_follow_windows w \
                       WHERE e.ts >= w.started AND e.ts <= COALESCE(w.ended, strftime('%Y-%m-%dT%H:%M:%SZ','now'))) \
         ORDER BY e.file, e.offset",
    ) else {
        return Profile::default();
    };
    let rows: Vec<Row> = match st.query_map([], |r| {
        Ok(Row { event: r.get(0)?, epoch: r.get(1)?, ship: r.get(2)? })
    }) {
        Ok(rows) => rows.flatten().collect(),
        Err(_) => return Profile::default(),
    };
    profile_from(&rows, ship)
}

/// The walk over rows in journal order. A sample belongs to the ship
/// named by the latest Loadout/LoadGame before it; with `ship` given,
/// other ships' samples are dropped.
pub fn profile_from(rows: &[Row], ship: Option<&str>) -> Profile {
    let mut p = Profile::default();
    let mut current_ship: Option<String> = None;
    let mut last_jump: Option<f64> = None;
    let mut undocked_at: Option<f64> = None;
    let mut granted_at: Option<f64> = None;
    let mut sc_start: Option<f64> = None;
    let mut last_exit: Option<(f64, Option<f64>)> = None;
    let mut docked_at: Option<f64> = None;
    let mut traded = false;
    for row in rows {
        let (ev, t) = (row.event.as_str(), row.epoch);
        if matches!(ev, "Loadout" | "LoadGame") {
            if let Some(s) = &row.ship {
                if current_ship.as_deref() != Some(s.as_str()) {
                    // A ship change breaks every phase in flight.
                    last_jump = None;
                    undocked_at = None;
                    granted_at = None;
                    sc_start = None;
                    last_exit = None;
                    docked_at = None;
                }
                current_ship = Some(s.clone());
            }
        }
        let counts = ship.is_none_or(|want| current_ship.as_deref().is_some_and(|have| have.eq_ignore_ascii_case(want)));
        // jump cadence: reset on anything that is not travel
        if ev == "FSDJump" {
            if let (Some(prev), true) = (last_jump, counts) {
                p.jump.push(t - prev);
            }
            last_jump = Some(t);
        } else if !TRANSIT.contains(&ev) {
            last_jump = None;
        }
        match ev {
            "Undocked" => {
                undocked_at = Some(t);
                if let (Some(d), true, true) = (docked_at, traded, counts) {
                    p.market.push(t - d);
                }
                docked_at = None;
                traded = false;
            }
            "FSDJump" => {
                if let Some(u) = undocked_at.take() {
                    if counts {
                        p.undock.push(t - u);
                    }
                }
                sc_start = Some(t);
            }
            "SupercruiseEntry" => sc_start = Some(t),
            "SupercruiseExit" => last_exit = Some((t, sc_start.take().map(|s| t - s))),
            "DockingGranted" => granted_at = Some(t),
            "Docked" => {
                docked_at = Some(t);
                traded = false;
                if let Some(g) = granted_at.take() {
                    if counts {
                        p.docking.push(t - g);
                    }
                }
                if let Some((exit_at, Some(sc))) = last_exit.take() {
                    if t - exit_at < 900.0 && counts {
                        p.supercruise.push(sc);
                    }
                }
            }
            "MarketBuy" | "MarketSell" => traded = true,
            _ => {}
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(seq: &[(&str, f64)]) -> Vec<Row> {
        seq.iter().map(|(e, t)| Row { event: e.to_string(), epoch: *t, ship: None }).collect()
    }

    /// One trade cycle, walked: every phase lands one sample with the
    /// right length, and nothing leaks between phases.
    #[test]
    fn one_cycle_yields_one_sample_per_phase() {
        let p = profile_from(&rows(&[
            ("Docked", 0.0), ("MarketBuy", 20.0), ("Undocked", 80.0),           // market 80
            ("FSDJump", 170.0),                                                  // undock 90
            ("FSDJump", 220.0),                                                  // jump 50
            ("SupercruiseExit", 350.0),                                          // supercruise 130 (from the arrival jump)
            ("DockingRequested", 355.0), ("DockingGranted", 356.0), ("Docked", 420.0), // docking 64
            ("Undocked", 500.0),                                                 // no trade: no market sample
        ]), None);
        assert_eq!(p.market, vec![80.0]);
        assert_eq!(p.undock, vec![90.0]);
        assert_eq!(p.jump, vec![50.0]);
        assert_eq!(p.supercruise, vec![130.0]);
        assert_eq!(p.docking, vec![64.0]);
    }

    /// A dock between two jumps is not a jump gap; a jump gap outside
    /// the bounds is not a sample.
    #[test]
    fn non_transit_activity_breaks_the_jump_cadence() {
        let p = profile_from(&rows(&[("FSDJump", 0.0), ("Docked", 10.0), ("Undocked", 20.0), ("FSDJump", 60.0), ("FSDJump", 1000.0)]), None);
        assert_eq!(p.jump, vec![940.0], "only the two jumps with nothing but travel between count");
        assert!(bounded("jump", &p.jump).is_empty(), "and 940 s is a pause, not a jump");
    }

    /// Samples belong to the ship flown; asking for one ship drops the
    /// other's, and a ship change mid-phase drops the phase.
    #[test]
    fn samples_are_per_ship() {
        let mut r = rows(&[("Loadout", 0.0), ("FSDJump", 10.0), ("FSDJump", 60.0), ("Loadout", 100.0), ("FSDJump", 110.0), ("FSDJump", 170.0)]);
        r[0].ship = Some("cutter".into());
        r[3].ship = Some("mandalay".into());
        assert_eq!(profile_from(&r, Some("cutter")).jump, vec![50.0]);
        assert_eq!(profile_from(&r, Some("Mandalay")).jump, vec![60.0]);
        assert_eq!(profile_from(&r, None).jump, vec![50.0, 60.0]);
    }

    /// Below the floor a phase keeps the server default and the timing
    /// is not marked measured; at the floor the median goes on the wire.
    #[test]
    fn the_floor_gates_each_phase_independently() {
        let mut p = Profile::default();
        p.jump = vec![50.0; MIN_SAMPLES - 1];
        let t = p.timing();
        assert_eq!(t, Timing::default());
        assert!(!t.measured);
        p.jump.push(52.0);
        p.market = (0..MIN_SAMPLES).map(|i| 60.0 + i as f64).collect();
        let t = p.timing();
        assert!(t.measured);
        assert_eq!(t.jump_seconds, 50.0);
        assert_eq!(t.market_seconds, 69.5);
        assert_eq!(t.undock_seconds, Timing::default().undock_seconds, "unmeasured phases keep the default");
    }

    /// Only journal rows inside a recorded trade-follow window are
    /// samples: the same jumps outside any window count for nothing.
    #[test]
    fn only_rows_inside_follow_windows_count() {
        let dir = tempfile::tempdir().unwrap();
        let store = ed_store::Store::open_in_memory(dir.path()).unwrap();
        let conn = store.conn();
        let insert = |offset: i64, ts: &str, event: &str| {
            conn.execute(
                "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', ?1, ?2, ?3, ?4)",
                rusqlite::params![offset, ts, event, format!(r#"{{"timestamp":"{ts}","event":"{event}"}}"#)],
            )
            .unwrap();
        };
        insert(1, "2026-09-09T10:00:00Z", "FSDJump");
        insert(2, "2026-09-09T10:00:50Z", "FSDJump");
        insert(3, "2026-09-09T12:00:00Z", "FSDJump");
        insert(4, "2026-09-09T12:00:40Z", "FSDJump");
        assert_eq!(profile(conn, None).jump, Vec::<f64>::new(), "no window, no samples");
        conn.execute("INSERT INTO trade_follow_windows (started, ended) VALUES ('2026-09-09T11:30:00Z', '2026-09-09T12:30:00Z')", []).unwrap();
        assert_eq!(profile(conn, None).jump, vec![40.0], "only the followed jumps");
        conn.execute("INSERT INTO trade_follow_windows (started) VALUES ('2026-09-09T09:00:00Z')", []).unwrap();
        window_close(conn);
        let all = profile(conn, None).jump;
        assert_eq!(bounded("jump", &all), vec![50.0, 40.0], "a window closed now covers the morning too (the two-hour gap is not a jump)");
    }
}
