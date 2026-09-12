//! Materialise derived state from the `events` log.
//!
//! Ingest never writes these tables. Everything here is a pure function of
//! rows already in the database, which is what makes a derivation bug cheap:
//! fix the logic, re-derive, done -- no re-reading the journal folder.
//!
//! Ordering is `(file, offset)`, not `ts`. Journal file names sort
//! chronologically, and offsets within a file increase, so this is a true
//! replay order. Sorting by `ts` would interleave files that overlap in
//! time, which would corrupt the snapshot-then-deltas materials replay.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json::Value;

/// Events that actually feed a derived table. Everything else stays in the
/// log for later phases -- filtering here keeps a re-derive from parsing
/// tens of thousands of `FSSSignalDiscovered` rows it has no use for yet.
pub(crate) const DERIVED_FROM: &[&str] = &[
    // materials replay (delegated to ed_journal::inventory)
    "Materials",
    "MaterialCollected",
    "MaterialDiscarded",
    "MaterialTrade",
    "EngineerCraft",
    "EngineerContribution",
    "Synthesis",
    // everything else
    "EngineerProgress",
    "Location",
    "FSDJump",
    "CarrierJump",
    "Docked",
    "Undocked",
    "Loadout",
    "FSDTarget",
    "MarketSell",
    "PowerplayMerits",
    // combat
    "Bounty",
    "FactionKillBond",
    "CapShipBond",
    "PVPKill",
    "Died",
    "Interdicted",
    "Interdiction",
    "EscapeInterdiction",
    // Item 52 A: the carrier tables are rebuilt by carrier::rebuild at the
    // end of every pass; listing the events here makes an incremental
    // pass notice that one arrived.
    "CarrierBuy",
    "CarrierStats",
    "CarrierNameChange",
    "CarrierJumpRequest",
    "CarrierJumpCancelled",
    "CarrierLocation",
    "CarrierDepositFuel",
    "CarrierCrewServices",
    "CarrierDecommission",
    "CargoTransfer",
    // Item 53: ship_locations::rebuild, same pattern.
    "StoredShips",
    "ShipyardTransfer",
];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DeriveStats {
    pub events_read: u64,
    pub materials: usize,
    pub cargo: usize,
    pub engineers: usize,
    pub powerplay_observations: usize,
    pub sales: usize,
    pub merit_events: usize,
    pub kills: usize,
    pub incidents: usize,
}

#[derive(Default)]
struct LocationState {
    ts: Option<String>,
    system_name: Option<String>,
    system_address: Option<i64>,
    docked: bool,
    station_name: Option<String>,
    station_type: Option<String>,
    security: Option<String>,
    allegiance: Option<String>,
    population: Option<i64>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}
fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k).and_then(Value::as_i64)
}
fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

/// How far derivation has consumed the event log, as `(file, offset)`.
fn watermark(conn: &Connection) -> Option<(String, i64)> {
    let file: String = conn
        .query_row("SELECT value FROM meta WHERE key='derived_file'", [], |r| {
            r.get(0)
        })
        .ok()?;
    let offset: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key='derived_offset'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    Some((file, offset.parse().ok()?))
}

fn set_watermark(conn: &Connection, file: &str, offset: i64) -> Result<()> {
    for (k, v) in [
        ("derived_file", file.to_string()),
        ("derived_offset", offset.to_string()),
    ] {
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![k, v],
        )?;
    }
    Ok(())
}

/// Rebuild every derived table from scratch.
///
/// Use after a derivation-logic change; a normal sync uses
/// [`derive_incremental`], which is what keeps a live session cheap.
pub fn derive_all(conn: &Connection) -> Result<DeriveStats> {
    conn.execute(
        "DELETE FROM meta WHERE key IN ('derived_file','derived_offset')",
        [],
    )?;
    derive_from(conn, None)
}

/// Derive only what has arrived since the last pass.
///
/// The event log is append-only and replayed in `(file, offset)` order, so
/// resuming from a watermark yields the same state as a full rebuild. That
/// matters for a live session: a full re-derive on every journal write grows
/// with playtime, and the journal writes every few seconds.
pub fn derive_incremental(conn: &Connection) -> Result<DeriveStats> {
    let mark = watermark(conn);
    derive_from(conn, mark)
}

fn derive_from(conn: &Connection, after: Option<(String, i64)>) -> Result<DeriveStats> {
    let tx = conn.unchecked_transaction()?;
    let resuming = after.is_some();

    // A full rebuild clears the latest-wins tables. An incremental pass must
    // NOT -- it carries their current values forward and applies new events
    // on top, exactly as a full replay would have reached them.
    if !resuming {
        tx.execute_batch(
            "DELETE FROM materials;
             DELETE FROM cargo;
             DELETE FROM engineers;
             DELETE FROM loadout;
             DELETE FROM location;
             DELETE FROM nav;",
        )?;
    }

    let placeholders = DERIVED_FROM
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let (sql, params): (String, Vec<rusqlite::types::Value>) = match &after {
        Some((file, offset)) => (
            format!(
                "SELECT file, offset, ts, event, raw FROM events
                 WHERE event IN ({placeholders})
                   AND (file > ?{a} OR (file = ?{a} AND offset > ?{b}))
                 ORDER BY file, offset",
                a = DERIVED_FROM.len() + 1,
                b = DERIVED_FROM.len() + 2
            ),
            DERIVED_FROM
                .iter()
                .map(|s| rusqlite::types::Value::from(s.to_string()))
                .chain([
                    rusqlite::types::Value::from(file.clone()),
                    rusqlite::types::Value::from(*offset),
                ])
                .collect(),
        ),
        None => (
            format!(
                "SELECT file, offset, ts, event, raw FROM events
                 WHERE event IN ({placeholders})
                 ORDER BY file, offset"
            ),
            DERIVED_FROM
                .iter()
                .map(|s| rusqlite::types::Value::from(s.to_string()))
                .collect(),
        ),
    };

    let rows: Vec<(String, i64, String, String, String)> = {
        let mut stmt = tx.prepare(&sql)?;
        let mapped = stmt.query_map(rusqlite::params_from_iter(params.iter()), |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut stats = DeriveStats {
        events_read: rows.len() as u64,
        ..Default::default()
    };

    let mut inv = ed_journal::inventory::Inventory::new();
    if resuming {
        // Carry materials forward. A `Materials` snapshot in the new range
        // still clears and resets, exactly as in a full replay.
        let mut stmt = tx.prepare("SELECT symbol, count FROM materials")?;
        let existing = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for (symbol, count) in existing.flatten() {
            inv.insert(symbol, count);
        }
    }

    let mut loc = LocationState::default();
    if resuming {
        // Same for location: without this an incremental pass with no
        // Location event would blank the row.
        if let Ok(current) = tx.query_row(
            "SELECT ts, system_name, system_address, docked, station_name, station_type,
                    system_security, system_allegiance, population
             FROM location WHERE id = 1",
            [],
            |r| {
                Ok(LocationState {
                    ts: r.get(0)?,
                    system_name: r.get(1)?,
                    system_address: r.get(2)?,
                    docked: r.get::<_, i64>(3)? != 0,
                    station_name: r.get(4)?,
                    station_type: r.get(5)?,
                    security: r.get(6)?,
                    allegiance: r.get(7)?,
                    population: r.get(8)?,
                })
            },
        ) {
            loc = current;
        }
    }
    let mut engineers_json: Option<(String, Value)> = None;
    let mut loadout: Option<(String, Value)> = None;
    let mut nav: Option<(String, Value)> = None;

    for (file, offset, ts, event, raw) in &rows {
        let Ok(v) = serde_json::from_str::<Value>(raw) else {
            continue;
        };

        // Materials: delegate to the crate that already gets snapshot+delta
        // replay right, rather than restating the rules here.
        ed_journal::inventory::apply_event(&mut inv, &v);

        match event.as_str() {
            "EngineerProgress" if v.get("Engineers").is_some() => {
                engineers_json = Some((ts.clone(), v.clone()));
            }

            "Location" | "FSDJump" | "CarrierJump" => {
                loc.ts = Some(ts.clone());
                if let Some(n) = s(&v, "StarSystem") {
                    loc.system_name = Some(n);
                }
                loc.system_address = i(&v, "SystemAddress").or(loc.system_address);
                loc.docked = v
                    .get("Docked")
                    .and_then(Value::as_bool)
                    .unwrap_or(loc.docked);
                loc.station_name = s(&v, "StationName").or(loc.station_name.take());
                loc.station_type = s(&v, "StationType").or(loc.station_type.take());
                loc.security = s(&v, "SystemSecurity")
                    .map(|raw| ed_domain::system::security_name(&raw))
                    .or(loc.security.take());
                loc.allegiance = s(&v, "SystemAllegiance").or(loc.allegiance.take());
                loc.population = i(&v, "Population").or(loc.population);

                // Powerplay 2.0 fields ride on these same events. Only record
                // an observation when a power actually controls the system --
                // an unoccupied system has no state worth storing.
                if let Some(system_name) = s(&v, "StarSystem") {
                    if v.get("ControllingPower").is_some() || v.get("PowerplayState").is_some() {
                        tx.execute(
                            "INSERT OR REPLACE INTO powerplay_observations
                                 (file, offset, ts, system_address, system_name,
                                  controlling_power, powerplay_state, control_progress,
                                  reinforcement, undermining)
                             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                            params![
                                file,
                                offset,
                                ts,
                                i(&v, "SystemAddress"),
                                system_name,
                                s(&v, "ControllingPower"),
                                s(&v, "PowerplayState"),
                                f(&v, "PowerplayStateControlProgress"),
                                i(&v, "PowerplayStateReinforcement"),
                                i(&v, "PowerplayStateUndermining"),
                            ],
                        )?;
                        stats.powerplay_observations += 1;
                    }
                }
            }

            "Docked" => {
                loc.ts = Some(ts.clone());
                loc.docked = true;
                loc.station_name = s(&v, "StationName").or(loc.station_name.take());
                loc.station_type = s(&v, "StationType").or(loc.station_type.take());
                if let Some(n) = s(&v, "StarSystem") {
                    loc.system_name = Some(n);
                }
            }

            "Undocked" => {
                loc.ts = Some(ts.clone());
                loc.docked = false;
                loc.station_name = None;
                loc.station_type = None;
            }

            "Loadout" => loadout = Some((ts.clone(), v.clone())),
            "FSDTarget" => nav = Some((ts.clone(), v.clone())),

            "MarketSell" => {
                tx.execute(
                    "INSERT OR REPLACE INTO sales
                         (file, offset, ts, market_id, commodity, count,
                          sell_price, total_sale, avg_price_paid)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        file,
                        offset,
                        ts,
                        i(&v, "MarketID"),
                        s(&v, "Type").unwrap_or_default(),
                        i(&v, "Count").unwrap_or(0),
                        i(&v, "SellPrice"),
                        i(&v, "TotalSale"),
                        i(&v, "AvgPricePaid"),
                    ],
                )?;
                stats.sales += 1;
            }

            "Bounty" | "FactionKillBond" | "CapShipBond" | "PVPKill" => {
                let kind = match event.as_str() {
                    "Bounty" => "bounty",
                    "FactionKillBond" => "faction_kill_bond",
                    "CapShipBond" => "capship_bond",
                    _ => "pvp",
                };
                // Bounty pays an array of factions and carries TotalReward;
                // the bond events pay a single Reward. Taking whichever is
                // present avoids under-counting a multi-faction bounty.
                let reward = i(&v, "TotalReward").or_else(|| i(&v, "Reward"));
                tx.execute(
                    "INSERT OR REPLACE INTO combat_kills
                         (file, offset, ts, kind, target_ship, pilot_name,
                          faction, victim_faction, reward, system_name)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        file,
                        offset,
                        ts,
                        kind,
                        s(&v, "Target").map(|t| ed_journal::ships::display_name_or(
                            &t,
                            s(&v, "Target_Localised").as_deref()
                        )),
                        s(&v, "PilotName_Localised").or_else(|| s(&v, "PilotName")),
                        s(&v, "AwardingFaction").or_else(|| {
                            v.get("Rewards")
                                .and_then(Value::as_array)
                                .and_then(|r| r.first())
                                .and_then(|r| s(r, "Faction"))
                        }),
                        s(&v, "VictimFaction"),
                        reward,
                        loc.system_name.clone(),
                    ],
                )?;
                stats.kills += 1;
            }

            "Died" | "Interdicted" | "Interdiction" | "EscapeInterdiction" => {
                let kind = match event.as_str() {
                    "Died" => "died",
                    "Interdicted" => "interdicted",
                    "Interdiction" => "interdiction",
                    _ => "escaped_interdiction",
                };
                tx.execute(
                    "INSERT OR REPLACE INTO combat_incidents
                         (file, offset, ts, kind, opponent, opponent_ship,
                          is_player, submitted, faction, system_name)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        file,
                        offset,
                        ts,
                        kind,
                        s(&v, "KillerName")
                            .or_else(|| s(&v, "Interdictor"))
                            .or_else(|| s(&v, "Interdicted")),
                        s(&v, "KillerShip"),
                        v.get("IsPlayer").and_then(Value::as_bool).map(|b| b as i64),
                        v.get("Submitted")
                            .and_then(Value::as_bool)
                            .map(|b| b as i64),
                        s(&v, "Faction"),
                        loc.system_name.clone(),
                    ],
                )?;
                stats.incidents += 1;
            }

            "PowerplayMerits" => {
                tx.execute(
                    "INSERT OR REPLACE INTO merit_events
                         (file, offset, ts, power, merits_gained, total_merits)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        file,
                        offset,
                        ts,
                        s(&v, "Power"),
                        i(&v, "MeritsGained"),
                        i(&v, "TotalMerits"),
                    ],
                )?;
                stats.merit_events += 1;
            }

            _ => {}
        }
    }

    // ── Write the latest-wins tables ────────────────────────────────
    {
        // `inv` is the complete carried-forward state, so replacing the table
        // wholesale is both correct and cheap. Upserting only the positive
        // entries would strand a material that has since been spent at its
        // old count -- a wrong number, not a stale one.
        tx.execute("DELETE FROM materials", [])?;
        let mut ins = tx.prepare("INSERT INTO materials (symbol, count) VALUES (?1, ?2)")?;
        for (symbol, count) in inv.iter().filter(|(_, c)| **c > 0) {
            ins.execute(params![symbol, count])?;
            stats.materials += 1;
        }
    }

    if let Some((ts, v)) = engineers_json {
        tx.execute("DELETE FROM engineers", [])?;
        if let Some(list) = v.get("Engineers").and_then(Value::as_array) {
            let mut ins = tx.prepare(
                "INSERT OR REPLACE INTO engineers
                     (name, engineer_id, progress, rank, rank_progress, ts)
                 VALUES (?1,?2,?3,?4,?5,?6)",
            )?;
            for e in list {
                let Some(name) = s(e, "Engineer") else {
                    continue;
                };
                ins.execute(params![
                    name,
                    i(e, "EngineerID"),
                    s(e, "Progress"),
                    i(e, "Rank"),
                    i(e, "RankProgress"),
                    ts,
                ])?;
                stats.engineers += 1;
            }
        }
    }

    if let Some((ts, v)) = loadout {
        tx.execute(
            "INSERT OR REPLACE INTO loadout
                 (id, ts, ship, ship_name, ship_ident, cargo_capacity,
                  unladen_mass, max_jump_range, hull_value, modules)
             VALUES (1,?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                ts,
                s(&v, "Ship"),
                s(&v, "ShipName").filter(|x| !x.is_empty()),
                s(&v, "ShipIdent").filter(|x| !x.is_empty()),
                i(&v, "CargoCapacity"),
                f(&v, "UnladenMass"),
                f(&v, "MaxJumpRange"),
                i(&v, "HullValue"),
                v.get("Modules").map(|m| m.to_string()),
            ],
        )?;
    }

    if let Some((ts, v)) = nav {
        tx.execute(
            "INSERT OR REPLACE INTO nav
                 (id, ts, target_system, system_address, star_class, remaining_jumps)
             VALUES (1,?1,?2,?3,?4,?5)",
            params![
                ts,
                s(&v, "Name"),
                i(&v, "SystemAddress"),
                s(&v, "StarClass"),
                i(&v, "RemainingJumpsInRoute"),
            ],
        )?;
    }

    tx.execute(
        "INSERT OR REPLACE INTO location
             (id, ts, system_name, system_address, docked, station_name,
              station_type, system_security, system_allegiance, population)
         VALUES (1,?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![
            loc.ts,
            loc.system_name,
            loc.system_address,
            loc.docked as i64,
            loc.station_name,
            loc.station_type,
            loc.security,
            loc.allegiance,
            loc.population,
        ],
    )?;

    // Cargo comes from the whole-file snapshot, which the game keeps current
    // on every change -- no snapshot+delta reconstruction needed.
    if let Ok(raw) = tx.query_row(
        "SELECT raw FROM snapshots WHERE name = 'Cargo.json'",
        [],
        |r| r.get::<_, String>(0),
    ) {
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            if let Some(items) = v.get("Inventory").and_then(Value::as_array) {
                // Cargo.json is a complete snapshot; anything not in it has
                // been sold or jettisoned.
                tx.execute("DELETE FROM cargo", [])?;
                let mut ins = tx.prepare("INSERT INTO cargo (symbol, count) VALUES (?1, ?2)")?;
                for it in items {
                    let Some(name) = s(it, "Name") else { continue };
                    let count = i(it, "Count").unwrap_or(0);
                    if count > 0 {
                        ins.execute(params![name.to_lowercase(), count])?;
                        stats.cargo += 1;
                    }
                }
            }
        }
    }

    // Item 52 A: the carrier tables, a full replay of a few hundred rows.
    crate::carrier::rebuild(&tx)?;
    // Item 53: where every stored ship sits (after carriers: the join
    // reads the carrier's current system).
    crate::ship_locations::rebuild(&tx)?;

    if let Some((file, offset, ..)) = rows.last() {
        set_watermark(&tx, file, *offset)?;
    }

    tx.commit()?;
    Ok(stats)
}

/// Ships the commander owns right now, by the game's `ShipID`, replayed
/// from the shipyard events in journal order.
///
/// `StoredShips` is a snapshot (every ship except the one being flown), so
/// it resets the set; the others are deltas. Anything unparseable is an
/// error, never an empty hangar: "you own no ships" is a claim, and a schema
/// change must not make it silently.
pub fn owned_ships(conn: &Connection) -> Result<std::collections::HashSet<i64>> {
    use anyhow::Context;
    let mut owned = std::collections::HashSet::new();
    let mut current = None;
    let mut st = conn.prepare(
        "SELECT event, raw FROM events
         WHERE event IN ('Loadout','StoredShips','ShipyardNew','ShipyardBuy','ShipyardSell','ShipyardSwap','SellShipOnRebuy')
         ORDER BY file, offset",
    )?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (event, raw) = row?;
        let v: Value = serde_json::from_str(&raw).with_context(|| {
            format!(
                "malformed {event} event: {}",
                raw.chars().take(80).collect::<String>()
            )
        })?;
        let id = |key: &str| v.get(key).and_then(Value::as_i64);
        match event.as_str() {
            "Loadout" => {
                if let Some(ship_id) = id("ShipID") {
                    current = Some(ship_id);
                    owned.insert(ship_id);
                }
            }
            "StoredShips" => {
                owned.clear();
                for key in ["ShipsHere", "ShipsRemote"] {
                    if let Some(ships) = v.get(key).and_then(Value::as_array) {
                        owned.extend(
                            ships
                                .iter()
                                .filter_map(|ship| ship.get("ShipID").and_then(Value::as_i64)),
                        );
                    }
                }
                if let Some(ship_id) = current {
                    owned.insert(ship_id);
                }
            }
            "ShipyardNew" => {
                if let Some(ship_id) = id("NewShipID") {
                    owned.insert(ship_id);
                }
            }
            "ShipyardSell" | "SellShipOnRebuy" => {
                if let Some(ship_id) = id("SellShipID").or_else(|| id("ShipID")) {
                    owned.remove(&ship_id);
                }
            }
            "ShipyardBuy" | "ShipyardSwap" => {
                if let Some(ship_id) = id("SellShipID") {
                    owned.remove(&ship_id);
                }
                if let Some(ship_id) = id("ShipID") {
                    owned.insert(ship_id);
                }
                if let Some(ship_id) = id("StoreShipID") {
                    owned.insert(ship_id);
                }
            }
            _ => {}
        }
    }
    Ok(owned)
}

#[cfg(test)]
mod owned_ships_tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&conn).unwrap();
        conn
    }

    fn insert(conn: &Connection, n: i64, event: &str, raw: &str) {
        conn.execute(
            "INSERT INTO events (file, offset, ts, event, raw) VALUES ('Journal.1.log', ?1, ?2, ?3, ?4)",
            params![n, format!("2026-08-0{}T00:00:00Z", n.min(9)), event, raw],
        )
        .unwrap();
    }

    #[test]
    fn owned_ships_replays_purchases_swaps_and_rebuy_sales() {
        let conn = db();
        insert(
            &conn,
            1,
            "Loadout",
            r#"{"event":"Loadout","ShipID":1,"Ship":"cutter"}"#,
        );
        insert(
            &conn,
            2,
            "StoredShips",
            r#"{"event":"StoredShips","ShipsHere":[{"ShipID":2}],"ShipsRemote":[{"ShipID":3}]}"#,
        );
        insert(
            &conn,
            3,
            "ShipyardNew",
            r#"{"event":"ShipyardNew","NewShipID":4}"#,
        );
        // Swap into 3, storing 4; then buy 5 selling 2.
        insert(
            &conn,
            4,
            "ShipyardSwap",
            r#"{"event":"ShipyardSwap","ShipID":3,"StoreShipID":4}"#,
        );
        insert(
            &conn,
            5,
            "ShipyardBuy",
            r#"{"event":"ShipyardBuy","ShipID":5,"SellShipID":2}"#,
        );
        // Lost ship 5 and did not rebuy it.
        insert(
            &conn,
            6,
            "SellShipOnRebuy",
            r#"{"event":"SellShipOnRebuy","SellShipID":5}"#,
        );
        let mut got: Vec<i64> = owned_ships(&conn).unwrap().into_iter().collect();
        got.sort();
        assert_eq!(got, vec![1, 3, 4]);
    }

    /// The journal carries `SystemSecurity` as a localisation symbol; the
    /// status panel showed `$GAlAXY_MAP_INFO_state_anarchy;` verbatim.
    #[test]
    fn location_stores_security_as_a_display_name() {
        let conn = db();
        insert(
            &conn,
            1,
            "Location",
            r#"{"event":"Location","StarSystem":"Eurybia","SystemAddress":1458309141194,
                "SystemSecurity":"$GAlAXY_MAP_INFO_state_anarchy;",
                "SystemSecurity_Localised":"Anarchy","SystemAllegiance":"Independent","Population":145173}"#,
        );
        derive_all(&conn).unwrap();
        let security: String = conn
            .query_row(
                "SELECT system_security FROM location WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(security, "Anarchy");
    }

    #[test]
    fn a_malformed_event_is_an_error_not_an_empty_hangar() {
        let conn = db();
        insert(&conn, 1, "Loadout", r#"{"event":"Loadout","ShipID":1}"#);
        insert(
            &conn,
            2,
            "ShipyardSwap",
            r#"{"event":"ShipyardSwap","ShipID":"#,
        );
        let err = owned_ships(&conn).unwrap_err().to_string();
        assert!(err.contains("ShipyardSwap"), "{err}");
    }
}
