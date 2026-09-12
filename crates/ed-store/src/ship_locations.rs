//! Where each stored ship sits (Item 53, "Where's my Anaconda?").
//!
//! Two feeds, replayed in journal order on every derive pass:
//! - `StoredShips`, written at every shipyard visit: a full snapshot.
//!   `ShipsHere` sit at the snapshot's own station; `ShipsRemote` carry
//!   `StarSystem` + `ShipMarketID` when parked, or `InTransit: true` and
//!   NOTHING else (census 2026-09-06: no location, no ETA on those).
//! - `ShipyardTransfer`, the only source for a moving ship: `MarketID`
//!   is the DESTINATION market, `System` the origin, `TransferTime` in
//!   seconds; arrival = timestamp + TransferTime.
//!
//! Facts measured on the maintainer's fleet (136 snapshots): the manual's
//! `TransferType` field does not exist (0 of 324 rows); unnamed ships
//! carry `Name: ""`, not an absent key; staleness is a courtesy, not a
//! risk (median gap between snapshots ~1.7 h).
//!
//! The carrier join is numeric: a parked ship's `ShipMarketID` equal to a
//! known `carriers.carrier_id` is "aboard your carrier", and its system
//! is then the CARRIER's current system — the one stored ship that moves.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

pub const EVENTS: &[&str] = &["StoredShips", "ShipyardTransfer"];

#[derive(Debug, Default, Clone)]
struct Row {
    ship_type: Option<String>,
    name: Option<String>,
    system: Option<String>,
    station: Option<String>,
    market_id: Option<i64>,
    in_transit: bool,
    arrival_ts: Option<String>,
    as_of: String,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}
fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}

/// Station name and system for a market id, from the galaxy tables when
/// attached, else nothing (the row still records the id).
fn station_for(conn: &Connection, market_id: i64) -> (Option<String>, Option<String>) {
    conn.query_row(
        "SELECT st.name, sy.name FROM sys_stations st LEFT JOIN sys_systems sy ON sy.id64 = st.system_id64 WHERE st.id = ?1",
        [market_id],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
    )
    .optional()
    .ok()
    .flatten()
    .unwrap_or((None, None))
}

/// Replay both feeds. Idempotent.
pub fn rebuild(conn: &Connection) -> Result<usize> {
    let mut stmt = conn.prepare(
        "SELECT ts, event, raw FROM events WHERE event IN ('StoredShips', 'ShipyardTransfer') ORDER BY file, offset",
    )?;
    let rows: Vec<(String, String, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let galaxy_attached = conn
        .query_row("SELECT 1 FROM sys_stations LIMIT 1", [], |_| Ok(()))
        .optional()
        .is_ok();

    let mut ships: HashMap<i64, Row> = HashMap::new();
    for (ts, event, raw) in rows {
        let Ok(v) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        match event.as_str() {
            "StoredShips" => {
                // A full snapshot of every ship except the one being flown.
                let here_system = s(&v, "StarSystem");
                let here_station = s(&v, "StationName");
                let here_market = i(&v, "MarketID");
                let mut seen = std::collections::HashSet::new();
                for ship in v
                    .get("ShipsHere")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                {
                    let Some(id) = i(ship, "ShipID") else {
                        continue;
                    };
                    seen.insert(id);
                    let row = ships.entry(id).or_default();
                    row.ship_type = s(ship, "ShipType").or(row.ship_type.take());
                    row.name = s(ship, "Name")
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .or(row.name.take());
                    row.system = here_system.clone();
                    row.station = here_station.clone();
                    row.market_id = here_market;
                    row.in_transit = false;
                    row.arrival_ts = None;
                    row.as_of = ts.clone();
                }
                for ship in v
                    .get("ShipsRemote")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                {
                    let Some(id) = i(ship, "ShipID") else {
                        continue;
                    };
                    seen.insert(id);
                    let row = ships.entry(id).or_default();
                    row.ship_type = s(ship, "ShipType").or(row.ship_type.take());
                    row.name = s(ship, "Name")
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty())
                        .or(row.name.take());
                    if ship.get("InTransit").and_then(Value::as_bool) == Some(true) {
                        // No location, no ETA here: keep what ShipyardTransfer said.
                        row.in_transit = true;
                    } else {
                        row.system = s(ship, "StarSystem").or(row.system.take());
                        row.market_id = i(ship, "ShipMarketID").or(row.market_id);
                        row.station = row
                            .market_id
                            .filter(|_| galaxy_attached)
                            .and_then(|m| station_for(conn, m).0)
                            .or(row.station.take());
                        row.in_transit = false;
                        row.arrival_ts = None;
                    }
                    row.as_of = ts.clone();
                }
                // Anything the snapshot no longer lists is sold or being flown.
                ships.retain(|id, _| seen.contains(id));
            }
            "ShipyardTransfer" => {
                let Some(id) = i(&v, "ShipID") else { continue };
                let row = ships.entry(id).or_default();
                row.ship_type = s(&v, "ShipType").or(row.ship_type.take());
                row.in_transit = true;
                row.market_id = i(&v, "MarketID");
                if let Some(dest) = row.market_id {
                    let (station, system) = if galaxy_attached {
                        station_for(conn, dest)
                    } else {
                        (None, None)
                    };
                    row.station = station;
                    row.system = system;
                }
                row.arrival_ts = i(&v, "TransferTime").and_then(|secs| {
                    let start = ed_domain::freshness::parse_timestamp(&ts)?;
                    Some(ed_domain::freshness::format_timestamp(start + secs))
                });
                row.as_of = ts.clone();
            }
            _ => {}
        }
    }

    conn.execute("DELETE FROM ship_locations", [])?;
    let mut ins = conn.prepare(
        "INSERT INTO ship_locations (ship_id, ship_type, name, system_name, station_name, market_id, in_transit, arrival_ts, as_of)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
    )?;
    for (id, r) in &ships {
        ins.execute(params![
            id,
            r.ship_type,
            r.name,
            r.system,
            r.station,
            r.market_id,
            r.in_transit as i64,
            r.arrival_ts,
            r.as_of
        ])?;
    }
    Ok(ships.len())
}

/// One ship's whereabouts, as the panels and the model see it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ShipLocation {
    pub ship_id: i64,
    pub ship_type: Option<String>,
    pub name: Option<String>,
    /// `stored` | `in_transit` | `arrived` (transfer time elapsed, no
    /// snapshot since) | `aboard_carrier`.
    pub status: String,
    pub system: Option<String>,
    pub station: Option<String>,
    pub market_id: Option<i64>,
    /// Set when the ship sits on a known carrier: the callsign.
    pub carrier: Option<String>,
    pub arrival: Option<String>,
    pub minutes_to_arrival: Option<i64>,
    pub as_of: String,
    pub age_hours: f64,
}

/// Every stored ship with its location, from `now` (ISO-8601).
pub fn locations(conn: &Connection, now: &str) -> Result<Vec<ShipLocation>> {
    let now_s = ed_domain::freshness::parse_timestamp(now);
    let mut stmt = conn.prepare(
        "SELECT ship_id, ship_type, name, system_name, station_name, market_id, in_transit, arrival_ts, as_of
         FROM ship_locations ORDER BY name, ship_type, ship_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<i64>>(5)?,
            r.get::<_, i64>(6)? != 0,
            r.get::<_, Option<String>>(7)?,
            r.get::<_, String>(8)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (
            ship_id,
            ship_type,
            name,
            mut system,
            mut station,
            market_id,
            in_transit,
            arrival,
            as_of,
        ) = row?;
        // The carrier join: numeric, never by name. A carrier moves, so
        // its CURRENT system wins over where the ship was parked.
        let carrier: Option<(String, Option<String>)> = market_id.and_then(|m| {
            conn.query_row(
                "SELECT COALESCE(callsign, ''), system_name FROM carriers WHERE carrier_id = ?1",
                [m],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .ok()
            .flatten()
        });
        if let Some((callsign, carrier_system)) = &carrier {
            if carrier_system.is_some() {
                system = carrier_system.clone();
            }
            station = Some(callsign.clone());
        }
        let minutes_to_arrival = match (&arrival, now_s) {
            (Some(a), Some(n)) => ed_domain::freshness::parse_timestamp(a).map(|t| (t - n) / 60),
            _ => None,
        };
        let status = if carrier.is_some() {
            "aboard_carrier"
        } else if in_transit {
            if minutes_to_arrival.is_some_and(|m| m <= 0) {
                "arrived"
            } else {
                "in_transit"
            }
        } else {
            "stored"
        };
        let age = match (now_s, ed_domain::freshness::parse_timestamp(&as_of)) {
            (Some(n), Some(t)) => (((n - t) as f64 / 360.0).round() / 10.0).max(0.0),
            _ => 0.0,
        };
        out.push(ShipLocation {
            ship_id,
            ship_type,
            name,
            status: status.into(),
            system,
            station,
            market_id,
            carrier: carrier.map(|(c, _)| c),
            arrival,
            minutes_to_arrival,
            as_of,
            age_hours: age,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        crate::schema::attach_galaxy(&conn, None).unwrap();
        conn.execute_batch(
            "INSERT INTO sys_systems (id64, name) VALUES (1, 'Shinrarta Dezhra'), (2, 'Deciat'), (3, 'Beta');
             INSERT INTO sys_stations (id, system_id64, name, has_market) VALUES (128666762, 1, 'Jameson Memorial', 1), (3228342528, 2, 'Garay Terminal', 1);",
        )
        .unwrap();
        conn
    }

    fn ev(conn: &Connection, n: i64, raw: &str) {
        let v: Value = serde_json::from_str(raw).unwrap();
        conn.execute(
            "INSERT INTO events (file, offset, ts, event, system_address, market_id, raw) VALUES ('j.log', ?1, ?2, ?3, NULL, ?4, ?5)",
            params![n, v["timestamp"].as_str().unwrap(), v["event"].as_str().unwrap(), v.get("MarketID").and_then(Value::as_i64), raw],
        )
        .unwrap();
    }

    // Shapes from the Journal Manual example plus the census facts:
    // Name "" for unnamed, InTransit entries carry nothing else, no TransferType.
    const SNAPSHOT: &str = r#"{"timestamp":"2026-09-06T10:00:00Z","event":"StoredShips","MarketID":128666762,"StationName":"Jameson Memorial","StarSystem":"Shinrarta Dezhra",
        "ShipsHere":[{"ShipID":64,"ShipType":"sidewinder","Name":"","Value":567962,"Hot":false},{"ShipID":20,"ShipType":"anaconda","Name":"Kestrel","Value":6373956,"Hot":false}],
        "ShipsRemote":[{"ShipID":3,"ShipType":"anaconda","Name":"Sparrow","StarSystem":"Deciat","ShipMarketID":3228342528,"TransferPrice":3777,"TransferTime":1590,"Value":9464239,"Hot":false},
                       {"ShipID":9,"ShipType":"cobramkiii","Name":"","InTransit":true,"Value":1,"Hot":false}]}"#;
    const TRANSFER: &str = r#"{"timestamp":"2026-09-06T09:30:00Z","event":"ShipyardTransfer","ShipType":"cobramkiii","ShipID":9,"System":"Beta","ShipMarketID":999,"MarketID":128666762,"Distance":85.6,"TransferPrice":580,"TransferTime":3600}"#;

    #[test]
    fn a_snapshot_places_ships_here_and_remote() {
        let conn = db();
        ev(&conn, 1, SNAPSHOT);
        rebuild(&conn).unwrap();
        let all = locations(&conn, "2026-09-06T12:00:00Z").unwrap();
        let by_id = |id: i64| all.iter().find(|l| l.ship_id == id).unwrap().clone();
        let kestrel = by_id(20);
        assert_eq!(
            (kestrel.name.as_deref(), kestrel.status.as_str()),
            (Some("Kestrel"), "stored")
        );
        assert_eq!(
            (kestrel.system.as_deref(), kestrel.station.as_deref()),
            (Some("Shinrarta Dezhra"), Some("Jameson Memorial"))
        );
        assert_eq!(kestrel.age_hours, 2.0);
        let unnamed = by_id(64);
        assert_eq!(unnamed.name, None, "Name \"\" is unnamed, not a name");
        let sparrow = by_id(3);
        assert_eq!(
            (
                sparrow.system.as_deref(),
                sparrow.station.as_deref(),
                sparrow.market_id
            ),
            (Some("Deciat"), Some("Garay Terminal"), Some(3228342528))
        );
    }

    #[test]
    fn in_transit_needs_the_transfer_event_for_destination_and_eta() {
        let conn = db();
        ev(&conn, 1, TRANSFER);
        ev(&conn, 2, SNAPSHOT);
        rebuild(&conn).unwrap();
        let all = locations(&conn, "2026-09-06T10:10:00Z").unwrap();
        let cobra = all.iter().find(|l| l.ship_id == 9).unwrap();
        assert_eq!(cobra.status, "in_transit");
        assert_eq!(
            (cobra.system.as_deref(), cobra.station.as_deref()),
            (Some("Shinrarta Dezhra"), Some("Jameson Memorial")),
            "destination from the transfer's MarketID"
        );
        assert_eq!(cobra.arrival.as_deref(), Some("2026-09-06T10:30:00Z"));
        assert_eq!(cobra.minutes_to_arrival, Some(20));
        let later = locations(&conn, "2026-09-06T11:00:00Z").unwrap();
        assert_eq!(
            later.iter().find(|l| l.ship_id == 9).unwrap().status,
            "arrived",
            "transfer time elapsed, no snapshot since"
        );
    }

    #[test]
    fn a_later_snapshot_drops_sold_ships_and_settles_arrivals() {
        let conn = db();
        ev(&conn, 1, TRANSFER);
        ev(&conn, 2, SNAPSHOT);
        ev(&conn, 3, &SNAPSHOT.replace("2026-09-06T10:00:00Z", "2026-09-06T11:00:00Z")
            .replace(r#"{"ShipID":9,"ShipType":"cobramkiii","Name":"","InTransit":true,"Value":1,"Hot":false}"#, r#"{"ShipID":9,"ShipType":"cobramkiii","Name":"","StarSystem":"Shinrarta Dezhra","ShipMarketID":128666762,"Value":1,"Hot":false}"#)
            .replace(r#"{"ShipID":64,"ShipType":"sidewinder","Name":"","Value":567962,"Hot":false},"#, ""));
        rebuild(&conn).unwrap();
        let all = locations(&conn, "2026-09-06T12:00:00Z").unwrap();
        assert!(all.iter().all(|l| l.ship_id != 64), "sold");
        let cobra = all.iter().find(|l| l.ship_id == 9).unwrap();
        assert_eq!(
            (cobra.status.as_str(), cobra.station.as_deref()),
            ("stored", Some("Jameson Memorial"))
        );
    }

    #[test]
    fn a_ship_parked_on_a_carrier_follows_the_carrier() {
        let conn = db();
        conn.execute(
            "INSERT INTO carriers (carrier_id, callsign, owned, system_name) VALUES (3700000001, 'K3X-9ZQ', 1, 'Gamma')",
            [],
        )
        .unwrap();
        ev(
            &conn,
            1,
            &SNAPSHOT.replace(
                r#""StarSystem":"Deciat","ShipMarketID":3228342528"#,
                r#""StarSystem":"Alpha","ShipMarketID":3700000001"#,
            ),
        );
        rebuild(&conn).unwrap();
        let all = locations(&conn, "2026-09-06T12:00:00Z").unwrap();
        let sparrow = all.iter().find(|l| l.ship_id == 3).unwrap();
        assert_eq!(sparrow.status, "aboard_carrier");
        assert_eq!(sparrow.carrier.as_deref(), Some("K3X-9ZQ"));
        assert_eq!(
            sparrow.system.as_deref(),
            Some("Gamma"),
            "the carrier moved since the ship was parked"
        );
    }

    #[test]
    fn rebuild_is_idempotent() {
        let conn = db();
        ev(&conn, 1, SNAPSHOT);
        rebuild(&conn).unwrap();
        rebuild(&conn).unwrap();
        assert_eq!(locations(&conn, "2026-09-06T12:00:00Z").unwrap().len(), 4);
    }
}
