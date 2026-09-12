//! Read helpers over the derived tables.
//!
//! These are the shapes later phases hand to the AI tool layer, so they are
//! plain serialisable structs rather than raw rows.

use anyhow::Result;
use ed_domain::star::StarClass;
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Engineer {
    pub name: String,
    pub progress: Option<String>,
    pub rank: Option<i64>,
    pub rank_progress: Option<i64>,
}

impl Engineer {
    /// Only `Unlocked` engineers will actually do work for you. "Known" and
    /// "Invited" read as progress but buy you nothing at the workbench, and
    /// conflating them is how a planner recommends something impossible.
    pub fn is_unlocked(&self) -> bool {
        self.progress.as_deref() == Some("Unlocked")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    pub ts: Option<String>,
    pub system_name: Option<String>,
    pub system_address: Option<i64>,
    pub docked: bool,
    pub station_name: Option<String>,
    pub station_type: Option<String>,
    pub system_security: Option<String>,
    pub system_allegiance: Option<String>,
    pub population: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavTarget {
    pub target_system: Option<String>,
    pub star_class: Option<String>,
    pub remaining_jumps: Option<i64>,
    /// KGBFOAM stars can be fuel-scooped; everything else cannot. A route
    /// whose next hop is unscoopable is the thing you want to know *before*
    /// you jump, which is why this is computed here and not in the UI.
    pub scoopable: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PowerplayState {
    pub system_name: String,
    pub ts: String,
    pub controlling_power: Option<String>,
    pub powerplay_state: Option<String>,
    pub control_progress: Option<f64>,
    pub reinforcement: Option<i64>,
    pub undermining: Option<i64>,
}

/// One sale joined to the merits it actually earned.
#[derive(Debug, Clone, Serialize)]
pub struct SaleWithMerits {
    pub ts: String,
    pub market_id: Option<i64>,
    pub commodity: String,
    pub count: i64,
    pub sell_price: Option<i64>,
    pub total_sale: Option<i64>,
    pub avg_price_paid: Option<i64>,
    /// Sum of every merit event attributed to this sale, not just the first.
    pub merits: i64,
    /// How many merit events were attributed. More than one means the game
    /// split the award, which the single-match approach silently dropped.
    pub merit_events: usize,
    pub profit: Option<i64>,
    pub system_name: Option<String>,
    pub powerplay_state: Option<String>,
}

pub fn engineers(conn: &Connection) -> Result<Vec<Engineer>> {
    let mut stmt =
        conn.prepare("SELECT name, progress, rank, rank_progress FROM engineers ORDER BY name")?;
    let rows = stmt.query_map([], |r| {
        Ok(Engineer {
            name: r.get(0)?,
            progress: r.get(1)?,
            rank: r.get(2)?,
            rank_progress: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Status of one engineer by name. `None` means the journal has never
/// mentioned them -- which is different from, and worse than, "Known".
pub fn engineer(conn: &Connection, name: &str) -> Result<Option<Engineer>> {
    let mut stmt = conn.prepare(
        "SELECT name, progress, rank, rank_progress FROM engineers WHERE name = ?1 COLLATE NOCASE",
    )?;
    let mut rows = stmt.query_map([name], |r| {
        Ok(Engineer {
            name: r.get(0)?,
            progress: r.get(1)?,
            rank: r.get(2)?,
            rank_progress: r.get(3)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn location(conn: &Connection) -> Result<Option<Location>> {
    let mut stmt = conn.prepare(
        "SELECT ts, system_name, system_address, docked, station_name, station_type,
                system_security, system_allegiance, population
         FROM location WHERE id = 1",
    )?;
    let mut rows = stmt.query_map([], |r| {
        Ok(Location {
            ts: r.get(0)?,
            system_name: r.get(1)?,
            system_address: r.get(2)?,
            docked: r.get::<_, i64>(3)? != 0,
            station_name: r.get(4)?,
            station_type: r.get(5)?,
            system_security: r.get(6)?,
            system_allegiance: r.get(7)?,
            population: r.get(8)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

/// The next jump target, if there still is one.
///
/// `FSDTarget` is only ever *set*; the game never writes an event that
/// clears it. So a target is live only while it is newer than the latest
/// location fix (a jump, a `Location` on load, docking) and is not the
/// system the commander is already in. Without this the overlay showed
/// "Next: Wongi, 1 jump" to a commander docked in Wongi.
pub fn nav_target(conn: &Connection) -> Result<Option<NavTarget>> {
    let row: Option<(Option<String>, NavTarget)> = {
        let mut stmt = conn.prepare(
            "SELECT ts, target_system, star_class, remaining_jumps FROM nav WHERE id = 1",
        )?;
        let mut rows = stmt.query_map([], |r| {
            let star_class: Option<String> = r.get(2)?;
            Ok((
                r.get::<_, Option<String>>(0)?,
                NavTarget {
                    target_system: r.get(1)?,
                    scoopable: star_class
                        .as_deref()
                        .map(|c| StarClass::from_journal(c).scoopable()),
                    star_class,
                    remaining_jumps: r.get(3)?,
                },
            ))
        })?;
        rows.next().transpose()?
    };
    let Some((nav_ts, nav)) = row else {
        return Ok(None);
    };

    // Clearing the route in the galaxy map writes NavRouteClear and nothing
    // else; without this the HUD kept showing the old next star.
    let cleared: Option<String> = conn
        .query_row(
            "SELECT ts FROM events WHERE event = 'NavRouteClear' ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let (Some(c), Some(n)) = (&cleared, &nav_ts) {
        if c.as_str() >= n.as_str() {
            return Ok(None);
        }
    }

    if let Some(loc) = location(conn)? {
        let reached = match (&nav.target_system, &loc.system_name) {
            (Some(t), Some(s)) => t.eq_ignore_ascii_case(s),
            _ => false,
        };
        let superseded = match (&nav_ts, &loc.ts) {
            (Some(n), Some(l)) => n.as_str() < l.as_str(),
            _ => false,
        };
        if reached || superseded {
            return Ok(None);
        }
    }
    Ok(Some(nav))
}

/// The most recent raw `FSDTarget` name, with NONE of [`nav_target`]'s
/// HUD-oriented suppression (route clears, location supersession).
///
/// Exists for the map-setup targeting test (field case 2026-09-05): the
/// teach flow leaves the test system targeted, Elite does not re-emit
/// FSDTarget when a click re-targets the already-current system, and the
/// test's route-clear made `nav_target` report None forever — so the
/// test failed every time despite the recipe working. The RAW event
/// answers the test's actual question: what does the game say is
/// targeted, regardless of what the HUD should display.
pub fn latest_fsd_target_name(conn: &Connection) -> Result<Option<String>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT raw FROM events WHERE event = 'FSDTarget' ORDER BY file DESC, offset DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw
        .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        .and_then(|v| v.get("Name").and_then(|n| n.as_str()).map(str::to_owned)))
}

/// Most recent Powerplay observation for a system, by name.
pub fn powerplay_for_system(conn: &Connection, system: &str) -> Result<Option<PowerplayState>> {
    let mut stmt = conn.prepare(
        "SELECT system_name, ts, controlling_power, powerplay_state,
                control_progress, reinforcement, undermining
         FROM powerplay_observations
         WHERE system_name = ?1 COLLATE NOCASE
         ORDER BY ts DESC LIMIT 1",
    )?;
    let mut rows = stmt.query_map([system], |r| {
        Ok(PowerplayState {
            system_name: r.get(0)?,
            ts: r.get(1)?,
            controlling_power: r.get(2)?,
            powerplay_state: r.get(3)?,
            control_progress: r.get(4)?,
            reinforcement: r.get(5)?,
            undermining: r.get(6)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

/// Latest observation for every system seen, newest first.
pub fn powerplay_all(conn: &Connection) -> Result<Vec<PowerplayState>> {
    let mut stmt = conn.prepare(
        "SELECT system_name, ts, controlling_power, powerplay_state,
                control_progress, reinforcement, undermining
         FROM powerplay_observations p
         WHERE ts = (SELECT MAX(ts) FROM powerplay_observations q
                      WHERE q.system_name = p.system_name)
         GROUP BY system_name
         ORDER BY ts DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(PowerplayState {
            system_name: r.get(0)?,
            ts: r.get(1)?,
            controlling_power: r.get(2)?,
            powerplay_state: r.get(3)?,
            control_progress: r.get(4)?,
            reinforcement: r.get(5)?,
            undermining: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Join every sale to the merits it earned.
///
/// The naive version of this -- take the first `PowerplayMerits` event after
/// each sale -- undercounts whenever the game splits an award across several
/// events, and double-counts when two sales land within the window of one
/// another. This assigns each merit event to the nearest *preceding* sale
/// inside `window_secs`, so every merit is attributed exactly once.
pub fn sales_with_merits(conn: &Connection, window_secs: i64) -> Result<Vec<SaleWithMerits>> {
    #[derive(Clone)]
    struct Sale {
        ts: String,
        market_id: Option<i64>,
        commodity: String,
        count: i64,
        sell_price: Option<i64>,
        total_sale: Option<i64>,
        avg_price_paid: Option<i64>,
    }

    let sales: Vec<Sale> = {
        let mut stmt = conn.prepare(
            "SELECT ts, market_id, commodity, count, sell_price, total_sale, avg_price_paid
             FROM sales ORDER BY ts, file, offset",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Sale {
                ts: r.get(0)?,
                market_id: r.get(1)?,
                commodity: r.get(2)?,
                count: r.get(3)?,
                sell_price: r.get(4)?,
                total_sale: r.get(5)?,
                avg_price_paid: r.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let merits: Vec<(String, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT ts, COALESCE(merits_gained, 0) FROM merit_events ORDER BY ts, file, offset",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut totals = vec![(0i64, 0usize); sales.len()];
    for (mts, gained) in &merits {
        // Nearest preceding sale within the window.
        let mut best: Option<usize> = None;
        for (idx, sale) in sales.iter().enumerate() {
            if sale.ts.as_str() > mts.as_str() {
                break;
            }
            if seconds_between(&sale.ts, mts).is_some_and(|d| d >= 0 && d <= window_secs) {
                best = Some(idx);
            }
        }
        if let Some(idx) = best {
            totals[idx].0 += gained;
            totals[idx].1 += 1;
        }
    }

    Ok(sales
        .into_iter()
        .zip(totals)
        .map(|(s, (merits, n))| {
            let profit = match (s.total_sale, s.avg_price_paid) {
                (Some(total), Some(paid)) => Some(total - paid * s.count),
                _ => None,
            };
            SaleWithMerits {
                ts: s.ts,
                market_id: s.market_id,
                commodity: s.commodity,
                count: s.count,
                sell_price: s.sell_price,
                total_sale: s.total_sale,
                avg_price_paid: s.avg_price_paid,
                merits,
                merit_events: n,
                profit,
                system_name: None,
                powerplay_state: None,
            }
        })
        .collect())
}

/// Sales that are byte-distinct but otherwise identical. Answers the
/// question the ad-hoc analysis could only guess at: are repeated rows real
/// repeated sales, or the same content ingested from overlapping files?
pub fn duplicate_sales(conn: &Connection) -> Result<Vec<(String, String, i64, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT ts, commodity, count, COUNT(*) n
         FROM sales
         GROUP BY ts, commodity, count, total_sale
         HAVING n > 1
         ORDER BY n DESC, ts",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Whole seconds between two ISO-8601 timestamps, for callers outside this
/// module.
pub fn seconds_between_ts(a: &str, b: &str) -> Option<i64> {
    seconds_between(a, b)
}

/// Whole seconds from `a` to `b` for ISO-8601 `...Z` timestamps.
/// Returns `None` rather than guessing if either fails to parse.
fn seconds_between(a: &str, b: &str) -> Option<i64> {
    Some(epoch_secs(b)? - epoch_secs(a)?)
}

/// Epoch seconds from a journal/EDDN timestamp (or the other shapes the
/// galaxy sources produce -- see `ed_domain::freshness::parse_timestamp`).
pub fn epoch_secs(ts: &str) -> Option<i64> {
    ed_domain::freshness::parse_timestamp(ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_fsd_target_survives_route_clears_and_location_fixes() {
        // The 2026-09-05 field regression: the map-setup test asked
        // nav_target, whose HUD suppression (a newer NavRouteClear)
        // reported None while the game still had the system targeted —
        // and Elite emits no event for re-targeting the current target,
        // so the test could never pass. The RAW read must ignore all of
        // that and simply report the last FSDTarget the journal saw.
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO events (file, offset, ts, event, raw) VALUES
               ('Journal.1.log', 1, '2026-09-05T05:12:10Z', 'FSDTarget', '{\"Name\":\"Crucis Sector WU-P b5-2\"}'),
               ('Journal.1.log', 2, '2026-09-05T05:12:31Z', 'NavRouteClear', '{}');",
        )
        .unwrap();
        assert_eq!(
            latest_fsd_target_name(&conn).unwrap().as_deref(),
            Some("Crucis Sector WU-P b5-2"),
            "route clears must not hide the standing target from the raw read"
        );
        conn.execute_batch(
            "INSERT INTO events (file, offset, ts, event, raw) VALUES
               ('Journal.1.log', 3, '2026-09-05T05:24:28Z', 'FSDTarget', '{\"Name\":\"Dyavata\"}');",
        )
        .unwrap();
        assert_eq!(
            latest_fsd_target_name(&conn).unwrap().as_deref(),
            Some("Dyavata")
        );
    }

    #[test]
    fn aebe_and_ms_nav_targets_are_not_scoopable() {
        for class in ["AeBe", "MS", "TTS"] {
            let conn = Connection::open_in_memory().unwrap();
            crate::schema::migrate(&conn).unwrap();
            crate::schema::attach_galaxy(&conn, None).unwrap();
            conn.execute(
                "INSERT INTO nav (id, ts, target_system, star_class, remaining_jumps)
                   VALUES (1, '2026-08-26T02:41:49Z', 'Dry Rock', ?1, 3)",
                [class],
            )
            .unwrap();
            let t = nav_target(&conn).unwrap().expect("a live target");
            assert_eq!(t.scoopable, Some(false), "{class} is not scoopable");
        }
    }

    #[test]
    fn a_reached_or_stale_nav_target_is_not_reported() {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO nav (id, ts, target_system, star_class, remaining_jumps)
               VALUES (1, '2026-08-26T02:41:49Z', 'Wongi', 'K', 1);
             INSERT INTO location (id, ts, system_name, docked, station_name)
               VALUES (1, '2026-08-26T12:28:03Z', 'Wongi', 1, 'Schmitt Enterprise');",
        )
        .unwrap();
        assert!(
            nav_target(&conn).unwrap().is_none(),
            "docked in the target system"
        );

        // En route: the target is newer than the last jump and elsewhere.
        conn.execute_batch(
            "UPDATE location SET ts = '2026-08-26T12:30:00Z', system_name = 'Deciat', docked = 0;
             UPDATE nav SET ts = '2026-08-26T12:30:05Z', target_system = 'Wongi';",
        )
        .unwrap();
        let t = nav_target(&conn).unwrap().expect("live target");
        assert_eq!(t.target_system.as_deref(), Some("Wongi"));
        assert_eq!(t.scoopable, Some(true));
        // Clearing the route retires it.
        conn.execute_batch(
            "INSERT INTO events (file,offset,ts,event,raw) VALUES ('J',9,'2026-08-26T12:31:00Z','NavRouteClear','{}');",
        )
        .unwrap();
        assert!(nav_target(&conn).unwrap().is_none(), "cleared route");
    }

    #[test]
    fn epoch_conversion_matches_known_timestamps() {
        assert_eq!(epoch_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_secs("2000-01-01T00:00:00Z"), Some(946_684_800));
        assert_eq!(epoch_secs("2026-08-23T15:42:26Z"), Some(1_787_499_746));
    }

    #[test]
    fn seconds_between_handles_a_minute_boundary() {
        assert_eq!(
            seconds_between("2026-08-23T15:42:59Z", "2026-08-23T15:43:01Z"),
            Some(2)
        );
    }
}

/// Combat summary over a time window.
#[derive(Debug, Clone, Serialize, Default)]
pub struct CombatSummary {
    pub kills: i64,
    pub bounty_credits: i64,
    pub bond_credits: i64,
    pub deaths: i64,
    pub interdicted: i64,
    pub interdictions_made: i64,
    pub escaped: i64,
    /// Most-killed ship types, descending.
    pub by_target: Vec<(String, i64)>,
    /// Factions that paid, descending by credits.
    pub by_faction: Vec<(String, i64)>,
}

/// Combat activity since `since` (an ISO timestamp), or all time if `None`.
pub fn combat_summary(conn: &Connection, since: Option<&str>) -> Result<CombatSummary> {
    let bound = since.unwrap_or("");
    let mut out = CombatSummary::default();

    let row = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN kind='bounty' THEN reward END), 0),
                COALESCE(SUM(CASE WHEN kind<>'bounty' THEN reward END), 0)
         FROM combat_kills WHERE ts >= ?1",
        [bound],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    out.kills = row.0;
    out.bounty_credits = row.1;
    out.bond_credits = row.2;

    let mut stmt =
        conn.prepare("SELECT kind, COUNT(*) FROM combat_incidents WHERE ts >= ?1 GROUP BY kind")?;
    for row in stmt.query_map([bound], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })? {
        let (kind, n) = row?;
        match kind.as_str() {
            "died" => out.deaths = n,
            "interdicted" => out.interdicted = n,
            "interdiction" => out.interdictions_made = n,
            "escaped_interdiction" => out.escaped = n,
            _ => {}
        }
    }

    let mut stmt = conn.prepare(
        "SELECT COALESCE(target_ship,'(unknown)'), COUNT(*) FROM combat_kills
         WHERE ts >= ?1 GROUP BY target_ship ORDER BY COUNT(*) DESC LIMIT 15",
    )?;
    out.by_target = stmt
        .query_map([bound], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut stmt = conn.prepare(
        "SELECT COALESCE(faction,'(unknown)'), COALESCE(SUM(reward),0) c FROM combat_kills
         WHERE ts >= ?1 GROUP BY faction ORDER BY c DESC LIMIT 15",
    )?;
    out.by_faction = stmt
        .query_map([bound], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    Ok(out)
}
