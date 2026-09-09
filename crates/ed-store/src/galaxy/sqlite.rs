//! The SQLite adapter of [`GalaxySink`]: writes decoded Spansh records into
//! the desktop galaxy tables.
//!
//! Every write is an upsert keyed on the game's own ids, so re-running over
//! the same dump, or over the populated and the stations dumps in turn, is
//! safe: `INSERT ... ON CONFLICT DO UPDATE` merges them. Unchanged records
//! (a station whose `updateTime` matches what is stored, a system whose
//! `date` matches) are skipped, which is what makes a refresh cheap -- most
//! of the bubble is untouched between dumps. Each checkpoint is one
//! transaction, so a crash loses seconds, not an hour.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

use super::sink::{GalaxySink, SystemVisit};
use super::spansh::{Body, Faction, Station, StationTimes, System};
use super::ImportStats;

#[derive(Default)]
struct CatalogIds {
    commodities: HashMap<String, i64>,
    modules: HashMap<String, i64>,
    ships: HashMap<String, i64>,
}

pub struct SqliteSink<'c> {
    conn: &'c Connection,
    ids: CatalogIds,
}

impl<'c> SqliteSink<'c> {
    pub fn new(conn: &'c Connection) -> Self {
        Self {
            conn,
            ids: CatalogIds::default(),
        }
    }
}

impl GalaxySink for SqliteSink<'_> {
    fn begin(&mut self) -> Result<()> {
        self.conn.execute_batch("BEGIN")?;
        Ok(())
    }

    fn system(
        &mut self,
        sys: &System,
        system_time: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<SystemVisit> {
        let conn = self.conn;
        let Some(id64) = sys.id64 else {
            return Ok(SystemVisit::Skip);
        };
        // Unchanged system: its bodies and factions are as stored. Stations
        // carry their own update times and are checked individually.
        let system_unchanged = match system_time {
            Some(d) => {
                conn.prepare_cached("SELECT updated FROM sys_systems WHERE id64 = ?1")?
                    .query_row([id64], |r| r.get::<_, Option<i64>>(0))
                    .optional()?
                    .flatten()
                    == Some(d)
            }
            None => false,
        };
        if system_unchanged {
            stats.skipped_systems += 1;
            // Powerplay only arrives in the populated dump; keep it fresh
            // even when the rest of the system is unchanged.
            if sys.controlling_power.is_some() || sys.power_state.is_some() {
                conn.prepare_cached(
                    "UPDATE sys_systems SET controlling_power = ?2, power_state = ?3,
                            powers = COALESCE(?4, powers), population = MAX(COALESCE(?5,0), COALESCE(population,0))
                     WHERE id64 = ?1",
                )?
                .execute(params![id64, sys.controlling_power, sys.power_state, sys.powers.as_ref().map(|p| p.join(", ")), sys.population])?;
            }
            return Ok(SystemVisit::StationsOnly);
        }

        let coords = sys.coords.as_ref();
        conn.execute(
            "INSERT INTO sys_systems
                 (id64, name, x, y, z, allegiance, government, primary_economy,
                  secondary_economy, security, population, controlling_power,
                  power_state, powers, updated)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
             ON CONFLICT(id64) DO UPDATE SET
                 name=excluded.name, x=excluded.x, y=excluded.y, z=excluded.z,
                 allegiance=excluded.allegiance, government=excluded.government,
                 primary_economy=excluded.primary_economy,
                 secondary_economy=excluded.secondary_economy,
                 security=excluded.security,
                 population=MAX(COALESCE(excluded.population,0), COALESCE(sys_systems.population,0)),
                 -- Powerplay only ever arrives from the populated dump; a null
                 -- from the stations dump must not erase it.
                 controlling_power=COALESCE(excluded.controlling_power, sys_systems.controlling_power),
                 power_state=COALESCE(excluded.power_state, sys_systems.power_state),
                 powers=COALESCE(excluded.powers, sys_systems.powers),
                 updated=excluded.updated",
            params![
                id64,
                sys.name,
                coords.and_then(|c| c.x),
                coords.and_then(|c| c.y),
                coords.and_then(|c| c.z),
                sys.allegiance,
                sys.government,
                sys.primary_economy,
                sys.secondary_economy,
                sys.security,
                sys.population,
                sys.controlling_power,
                sys.power_state,
                sys.powers.as_ref().map(|p| p.join(", ")),
                system_time,
            ],
        )?;
        stats.systems += 1;
        Ok(SystemVisit::Full)
    }

    fn station(
        &mut self,
        system: &System,
        st: &Station,
        body_name: Option<&str>,
        times: &StationTimes,
        stats: &mut ImportStats,
    ) -> Result<()> {
        let Some(system_id64) = system.id64 else {
            return Ok(());
        };
        insert_station(
            self.conn,
            system_id64,
            st,
            body_name,
            times,
            stats,
            &mut self.ids,
        )
    }

    fn body(
        &mut self,
        system_id64: i64,
        body: &Body,
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<()> {
        insert_body(self.conn, system_id64, body, updated, stats)
    }

    fn factions(
        &mut self,
        id64: i64,
        factions: &[Faction],
        system_time: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<()> {
        let conn = self.conn;
        conn.execute("DELETE FROM sys_factions WHERE system_id64 = ?1", [id64])?;
        let mut ins = conn.prepare_cached(
            "INSERT OR REPLACE INTO sys_factions
                 (system_id64, name, allegiance, government, influence, state, active_states, updated)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )?;
        for f in factions {
            let Some(name) = &f.name else { continue };
            let active: Vec<&str> = f
                .active_states
                .iter()
                .filter_map(|s| s.state.as_deref())
                .collect();
            ins.execute(params![
                id64,
                name,
                f.allegiance,
                f.government,
                f.influence,
                f.state,
                serde_json::to_string(&active).ok(),
                system_time,
            ])?;
            stats.factions += 1;
        }
        Ok(())
    }

    fn checkpoint(&mut self, _stats: &ImportStats) -> Result<()> {
        self.conn.execute_batch("COMMIT; BEGIN")?;
        Ok(())
    }

    fn finish(&mut self, _stats: &ImportStats) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }
}

fn insert_station(
    conn: &Connection,
    system_id64: i64,
    st: &Station,
    body_name: Option<&str>,
    times: &StationTimes,
    stats: &mut ImportStats,
    ids: &mut CatalogIds,
) -> Result<()> {
    let Some(id) = st.id else { return Ok(()) };
    // Unchanged since last import: nothing to write. This is what makes a
    // refresh cheap -- most of the bubble is untouched between dumps.
    let station_time = times.station;
    if let Some(ut) = station_time {
        let stored: Option<i64> = conn
            .prepare_cached("SELECT updated FROM sys_stations WHERE id = ?1")?
            .query_row([id], |r| r.get(0))
            .optional()?
            .flatten();
        if stored == Some(ut) {
            stats.skipped_stations += 1;
            return Ok(());
        }
    }
    let pads = st.landing_pads.as_ref();
    let economies = st
        .economies
        .as_ref()
        .and_then(|e| serde_json::to_string(e).ok());
    let state = st
        .controlling_faction_state
        .clone()
        .or_else(|| st.state.clone());

    conn.execute(
        "INSERT INTO sys_stations
             (id, system_id64, name, type, distance_to_arrival, primary_economy,
              government, controlling_faction, pad_large, pad_medium, pad_small,
              has_market, has_outfitting, has_shipyard, is_carrier, updated, has_material_trader,
              economies, state, body_name)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)
         ON CONFLICT(id) DO UPDATE SET
             system_id64=excluded.system_id64, name=excluded.name, type=excluded.type,
             distance_to_arrival=excluded.distance_to_arrival,
             primary_economy=excluded.primary_economy, government=excluded.government,
             controlling_faction=excluded.controlling_faction,
             pad_large=excluded.pad_large, pad_medium=excluded.pad_medium,
             pad_small=excluded.pad_small, has_market=excluded.has_market,
             has_outfitting=excluded.has_outfitting, has_shipyard=excluded.has_shipyard,
             is_carrier=excluded.is_carrier, updated=excluded.updated,
             has_material_trader=excluded.has_material_trader,
             economies=excluded.economies, state=excluded.state,
             body_name=COALESCE(excluded.body_name, sys_stations.body_name)",
        params![
            id,
            system_id64,
            st.name,
            st.kind,
            st.distance_to_arrival,
            st.primary_economy,
            st.government,
            st.controlling_faction,
            pads.and_then(|p| p.large),
            pads.and_then(|p| p.medium),
            pads.and_then(|p| p.small),
            st.has_service("Market") as i64,
            st.has_service("Outfitting") as i64,
            st.has_service("Shipyard") as i64,
            st.is_carrier() as i64,
            station_time,
            st.has_service("Material Trader") as i64,
            economies,
            state,
            body_name,
        ],
    )?;
    stats.stations += 1;

    // The full service list, replaced wholesale so a dropped service goes away.
    conn.execute(
        "DELETE FROM sys_station_services WHERE station_id = ?1",
        [id],
    )?;
    {
        let mut ins = conn.prepare_cached(
            "INSERT OR IGNORE INTO sys_station_services (station_id, service) VALUES (?1, ?2)",
        )?;
        for svc in &st.services {
            ins.execute(params![id, svc])?;
        }
    }

    // A dump market is a complete board observed at `updateTime`, so it goes
    // through the same snapshot writer as EDDN and EBEX: newer than the
    // station's watermark replaces it whole, otherwise live data stands.
    if let Some(market) = &st.market {
        let mut rows = Vec::with_capacity(market.commodities.len());
        for c in &market.commodities {
            let Some(symbol) = c.key() else { continue };
            let commodity_id = match ids.commodities.get(&symbol) {
                Some(id) => *id,
                None => {
                    let item_id = crate::market::intern_commodity(
                        conn,
                        &symbol,
                        c.name.as_deref(),
                        c.category.as_deref(),
                    )?;
                    ids.commodities.insert(symbol.clone(), item_id);
                    item_id
                }
            };
            rows.push(crate::market::MarketRow {
                commodity_id,
                buy_price: c.buy_price.unwrap_or(0),
                sell_price: c.sell_price.unwrap_or(0),
                demand: c.demand.unwrap_or(0),
                supply: c.supply.unwrap_or(0),
            });
        }
        if let crate::market::SnapshotOutcome::Applied { written, .. } =
            crate::market::write_snapshot(conn, id, times.market, &rows)?
        {
            stats.market_rows += written;
        }

        conn.execute(
            "DELETE FROM sys_market_prohibited WHERE station_id = ?1",
            [id],
        )?;
        let mut ins = conn.prepare_cached(
            "INSERT OR IGNORE INTO sys_market_prohibited (station_id, symbol) VALUES (?1, ?2)",
        )?;
        for p in &market.prohibited {
            ins.execute(params![id, p.to_lowercase()])?;
        }
    }

    if let Some(o) = &st.outfitting {
        let mut item = conn.prepare_cached(
            "INSERT INTO sys_modules (symbol,name,class,rating,category,ship) VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(symbol) DO UPDATE SET name=COALESCE(excluded.name,sys_modules.name),
               class=COALESCE(excluded.class,sys_modules.class), rating=COALESCE(excluded.rating,sys_modules.rating),
               category=COALESCE(excluded.category,sys_modules.category), ship=COALESCE(excluded.ship,sys_modules.ship)
             RETURNING id",
        )?;
        let mut ins = conn.prepare_cached(
            "INSERT OR REPLACE INTO sys_outfitting (station_id, module_id) VALUES (?1,?2)",
        )?;
        for m in &o.modules {
            let Some(symbol) = m.key() else { continue };
            let item_id = match ids.modules.get(&symbol) {
                Some(id) => *id,
                None => {
                    let item_id = item.query_row(
                        params![symbol, m.name, m.class, m.rating, m.category, m.ship],
                        |r| r.get(0),
                    )?;
                    ids.modules.insert(symbol.clone(), item_id);
                    item_id
                }
            };
            ins.execute(params![id, item_id])?;
            stats.outfitting_rows += 1;
        }
    }

    if let Some(s) = &st.shipyard {
        let mut item = conn.prepare_cached(
            "INSERT INTO sys_ships (symbol,name) VALUES (?1,?2)
             ON CONFLICT(symbol) DO UPDATE SET name=COALESCE(excluded.name,sys_ships.name) RETURNING id",
        )?;
        let mut ins = conn.prepare_cached(
            "INSERT OR REPLACE INTO sys_shipyard (station_id, ship_id) VALUES (?1,?2)",
        )?;
        for shp in &s.ships {
            let Some(symbol) = shp.key() else { continue };
            let item_id = match ids.ships.get(&symbol) {
                Some(id) => *id,
                None => {
                    let item_id = item.query_row(params![symbol, shp.name], |r| r.get(0))?;
                    ids.ships.insert(symbol.clone(), item_id);
                    item_id
                }
            };
            ins.execute(params![id, item_id])?;
            stats.shipyard_rows += 1;
        }
    }

    Ok(())
}

/// Bodies worth a row: landable planets (surface materials, signals) and
/// anything with rings (hotspots). Stars and bare gas giants are skipped.
fn insert_body(
    conn: &Connection,
    system_id64: i64,
    b: &Body,
    updated: Option<i64>,
    stats: &mut ImportStats,
) -> Result<()> {
    let Some(id64) = b.id64 else { return Ok(()) };
    let has_materials = b.materials.as_ref().is_some_and(|m| !m.is_empty());
    let has_rings = b.rings.iter().any(|r| r.name.is_some());
    let sig =
        |key: &str| -> Option<i64> { b.signals.as_ref().and_then(|s| s.signals.get(key).copied()) };
    let bio = sig("$SAA_SignalType_Biological;");
    let geo = sig("$SAA_SignalType_Geological;");
    if !(b.is_landable || has_materials || has_rings || bio.is_some() || geo.is_some()) {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO sys_bodies
             (id64, system_id64, body_id, name, type, sub_type, is_landable, distance_to_arrival,
              gravity, atmosphere, volcanism, bio_signals, geo_signals, updated)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
         ON CONFLICT(id64) DO UPDATE SET
             system_id64=excluded.system_id64, body_id=excluded.body_id, name=excluded.name,
             type=excluded.type, sub_type=excluded.sub_type, is_landable=excluded.is_landable,
             distance_to_arrival=excluded.distance_to_arrival, gravity=excluded.gravity,
             atmosphere=excluded.atmosphere, volcanism=excluded.volcanism,
             bio_signals=COALESCE(excluded.bio_signals, sys_bodies.bio_signals),
             geo_signals=COALESCE(excluded.geo_signals, sys_bodies.geo_signals),
             updated=excluded.updated",
        params![
            id64,
            system_id64,
            b.body_id,
            b.name,
            b.kind,
            b.sub_type,
            b.is_landable as i64,
            b.distance_to_arrival,
            b.gravity,
            b.atmosphere,
            b.volcanism,
            bio,
            geo,
            updated,
        ],
    )?;
    stats.bodies += 1;

    if let Some(mats) = &b.materials {
        let mut ins = conn.prepare_cached(
            "INSERT OR REPLACE INTO sys_body_materials (body_id64, material, percent) VALUES (?1,?2,?3)",
        )?;
        for (m, pct) in mats {
            ins.execute(params![id64, m, pct])?;
            stats.body_material_rows += 1;
        }
    }

    for r in &b.rings {
        let Some(name) = &r.name else { continue };
        conn.execute(
            "INSERT OR REPLACE INTO sys_rings (body_id64, name, type, mass, inner_radius, outer_radius)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![id64, name, r.kind, r.mass, r.inner_radius, r.outer_radius],
        )?;
        if let Some(s) = &r.signals {
            let mut ins = conn.prepare_cached(
                "INSERT OR REPLACE INTO sys_ring_hotspots (body_id64, ring_name, material, count, updated)
                 VALUES (?1,?2,?3,?4,?5)",
            )?;
            let ring_updated = s
                .update_time
                .as_deref()
                .and_then(ed_domain::freshness::parse_timestamp);
            for (m, n) in &s.signals {
                ins.execute(params![id64, name, m, n, ring_updated])?;
                stats.hotspots += 1;
            }
        }
    }
    Ok(())
}
