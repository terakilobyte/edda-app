//! The seam between dump decoding and storage.
//!
//! [`spansh`](super::spansh) streams decoded records into a [`GalaxySink`];
//! the sink owns everything about *where* they go. Two adapters exist:
//! [`SqliteSink`](super::SqliteSink) for the desktop store and the
//! in-memory [`RecordingSink`] for tests (the API service adds a PostgreSQL
//! one). The stream is deterministic and single-writer: records arrive in
//! dump order, and `checkpoint` marks the batch boundaries at which an
//! adapter should commit so an interrupted import is resumable.

use anyhow::Result;

use super::spansh::{Body, Faction, Station, StationTimes, System};
use super::ImportStats;

/// What the sink wants to see of a system after its header was offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemVisit {
    /// Stations, bodies and factions.
    Full,
    /// The system is unchanged since the sink last saw it: its bodies and
    /// factions are as stored. Stations carry their own update times and
    /// are still delivered so the sink can check them individually.
    StationsOnly,
    /// Nothing more for this system.
    Skip,
}

pub trait GalaxySink {
    /// The stream is starting. Adapters open their first transaction here.
    fn begin(&mut self) -> Result<()> {
        Ok(())
    }

    /// One decoded system with its `date` as an epoch (`None` when the dump
    /// carries none). The sink decides how much of it to visit.
    fn system(
        &mut self,
        system: &System,
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<SystemVisit>;

    /// One station of `system`, with the body it orbits when the dump nests
    /// it under a body, and every timestamp it carries already parsed.
    fn station(
        &mut self,
        system: &System,
        station: &Station,
        body_name: Option<&str>,
        times: &StationTimes,
        stats: &mut ImportStats,
    ) -> Result<()>;

    /// One body of the system `system_id64`.
    fn body(
        &mut self,
        system_id64: i64,
        body: &Body,
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<()>;

    /// The complete faction list of `system_id64` as of `updated`.
    fn factions(
        &mut self,
        system_id64: i64,
        factions: &[Faction],
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<()>;

    /// A batch boundary: everything delivered so far may be committed.
    fn checkpoint(&mut self, _stats: &ImportStats) -> Result<()> {
        Ok(())
    }

    /// End of stream. Adapters commit their last batch here.
    fn finish(&mut self, _stats: &ImportStats) -> Result<()> {
        Ok(())
    }
}

/// What a [`RecordingSink`] saw, in order.
#[derive(Debug, Clone, PartialEq)]
pub enum SinkEvent {
    Begin,
    System {
        id64: i64,
        name: Option<String>,
        updated: Option<i64>,
        population: Option<i64>,
        controlling_power: Option<String>,
    },
    Station {
        system_id64: i64,
        id: i64,
        name: Option<String>,
        body_name: Option<String>,
        times: StationTimes,
        market_rows: usize,
        outfitting_rows: usize,
        shipyard_rows: usize,
    },
    Body {
        system_id64: i64,
        id64: i64,
        updated: Option<i64>,
    },
    Factions {
        system_id64: i64,
        count: usize,
        updated: Option<i64>,
    },
    Checkpoint,
    Finish,
}

/// An adapter that keeps every record in memory. It visits every system in
/// full, so it also describes exactly what a database adapter would be
/// offered on a cold import.
#[derive(Debug, Default)]
pub struct RecordingSink {
    pub events: Vec<SinkEvent>,
}

impl GalaxySink for RecordingSink {
    fn begin(&mut self) -> Result<()> {
        self.events.push(SinkEvent::Begin);
        Ok(())
    }

    fn system(
        &mut self,
        system: &System,
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<SystemVisit> {
        stats.systems += 1;
        self.events.push(SinkEvent::System {
            id64: system.id64.unwrap_or_default(),
            name: system.name.clone(),
            updated,
            population: system.population,
            controlling_power: system.controlling_power.clone(),
        });
        Ok(SystemVisit::Full)
    }

    fn station(
        &mut self,
        system: &System,
        station: &Station,
        body_name: Option<&str>,
        times: &StationTimes,
        stats: &mut ImportStats,
    ) -> Result<()> {
        stats.stations += 1;
        let market_rows = station.market.as_ref().map_or(0, |m| m.commodities.len());
        let outfitting_rows = station.outfitting.as_ref().map_or(0, |o| o.modules.len());
        let shipyard_rows = station.shipyard.as_ref().map_or(0, |s| s.ships.len());
        stats.market_rows += market_rows as u64;
        stats.outfitting_rows += outfitting_rows as u64;
        stats.shipyard_rows += shipyard_rows as u64;
        self.events.push(SinkEvent::Station {
            system_id64: system.id64.unwrap_or_default(),
            id: station.id.unwrap_or_default(),
            name: station.name.clone(),
            body_name: body_name.map(str::to_owned),
            times: times.clone(),
            market_rows,
            outfitting_rows,
            shipyard_rows,
        });
        Ok(())
    }

    fn body(
        &mut self,
        system_id64: i64,
        body: &Body,
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<()> {
        stats.bodies += 1;
        self.events.push(SinkEvent::Body {
            system_id64,
            id64: body.id64.unwrap_or_default(),
            updated,
        });
        Ok(())
    }

    fn factions(
        &mut self,
        system_id64: i64,
        factions: &[Faction],
        updated: Option<i64>,
        stats: &mut ImportStats,
    ) -> Result<()> {
        stats.factions += factions.len() as u64;
        self.events.push(SinkEvent::Factions {
            system_id64,
            count: factions.len(),
            updated,
        });
        Ok(())
    }

    fn checkpoint(&mut self, _stats: &ImportStats) -> Result<()> {
        self.events.push(SinkEvent::Checkpoint);
        Ok(())
    }

    fn finish(&mut self, _stats: &ImportStats) -> Result<()> {
        self.events.push(SinkEvent::Finish);
        Ok(())
    }
}
