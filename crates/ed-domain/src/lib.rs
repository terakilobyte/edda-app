//! Database- and transport-independent data operations shared by EDDA.

pub mod discount;
pub mod star;
pub mod station;
pub mod system;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Commodity {
    pub name: String,
    pub buy_price: i64,
    pub sell_price: i64,
    pub demand: i64,
    pub stock: i64,
}

pub mod freshness;

/// When an observation was made. `epoch_seconds` is the value every store
/// compares and persists; `timestamp` is the RFC-3339 text it was parsed
/// from, kept for provenance and display only.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ObservedAt {
    pub timestamp: String,
    pub epoch_seconds: i64,
}

impl ObservedAt {
    pub fn new(timestamp: impl Into<String>, epoch_seconds: i64) -> Self {
        Self {
            timestamp: timestamp.into(),
            epoch_seconds,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum Operation {
    Market(Snapshot<Commodity>),
    Outfitting(Snapshot<String>),
    Shipyard(Snapshot<String>),
    System(SystemObservation),
    /// A `Docked` event: the one wire source for pads, carrier-ness,
    /// arrival distance, station type and services (station identity
    /// ingest, 2026-09-04).
    StationIdentity(StationIdentity),
    /// A main-star class for a system (a `Scan` of the arrival star, a
    /// `NavRoute` entry): what the routing index learns (2026-09-09,
    /// maintainer: whatever the dump adds that EDDN carried, the feed parses).
    Star(StarTeaching),
    /// A planet worth a prospecting row (a `Scan`): landable, materials,
    /// rings.
    Body(BodyTeaching),
    /// Ring hotspots (`SAASignalsFound` on a ring).
    RingHotspots(RingHotspots),
    /// Bio/geo signal counts on a body (`FSSBodySignals`,
    /// `SAASignalsFound` on a planet).
    BodySignals(BodySignals),
}

/// A system's arrival star as the feed reports it. `star_type` is the
/// journal's spelling (`K`, `DA`, `N`, …); the writer maps it to the
/// class code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StarTeaching {
    pub system_address: i64,
    pub system_name: Option<String>,
    pub position: Option<[f64; 3]>,
    pub star_type: String,
    pub observed_at: ObservedAt,
    /// `eddn:scan` (a navroute hop's StarClass is not the main star and
    /// is never taught; see ed-eddn).
    pub source: String,
}

/// The journal's body id64: the system address with the body id in the
/// top bits — the same key Spansh and EDSM use, so a feed row and a dump
/// row for one body meet on it.
pub fn body_id64(system_address: i64, body_id: i64) -> i64 {
    system_address + (body_id << 55)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RingTeaching {
    pub name: String,
    /// `Metallic`, `Rocky`, `Icy`, `Metal Rich` — Spansh's spelling.
    pub kind: Option<String>,
    pub mass: Option<f64>,
    pub inner_radius: Option<f64>,
    pub outer_radius: Option<f64>,
}

/// A body worth a prospecting row: landable, or with materials, rings or
/// signals. Stars and bare gas giants are not sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodyTeaching {
    pub id64: i64,
    pub system_address: i64,
    pub body_id: Option<i32>,
    pub name: Option<String>,
    /// `Planet` / `Star`.
    pub kind: Option<String>,
    pub sub_type: Option<String>,
    pub is_landable: bool,
    pub distance_to_arrival: Option<f64>,
    /// In g.
    pub gravity: Option<f64>,
    pub atmosphere: Option<String>,
    pub volcanism: Option<String>,
    pub bio_signals: Option<i32>,
    pub geo_signals: Option<i32>,
    pub observed_at: ObservedAt,
    pub provenance: String,
    /// `(material, percent)`.
    pub materials: Vec<(String, f64)>,
    pub rings: Vec<RingTeaching>,
    /// `(ring name, material, count)` — a dump carries these with the body.
    pub hotspots: Vec<(String, String, i32)>,
}

/// `SAASignalsFound` on a ring: the ring's parent body is resolved by
/// name at apply time (the journal keys the ring by its own body id,
/// the dumps by the parent's).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RingHotspots {
    pub system_address: i64,
    pub ring_name: String,
    /// `(material wire symbol, count)`.
    pub signals: Vec<(String, i32)>,
    pub observed_at: ObservedAt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodySignals {
    pub id64: i64,
    pub system_address: i64,
    pub body_id: Option<i32>,
    pub name: Option<String>,
    pub bio_signals: Option<i32>,
    pub geo_signals: Option<i32>,
    pub observed_at: ObservedAt,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Snapshot<T> {
    pub system_name: String,
    pub station_name: Option<String>,
    pub market_id: Option<i64>,
    pub observed_at: ObservedAt,
    pub values: Vec<T>,
    /// Commodity snapshots only: goods this market confiscates
    /// (`prohibited` in commodity/3), lowercased. Empty elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prohibited: Vec<String>,
}

/// Station identity from a `Docked` event. `market_id` is the station
/// key every store already uses; pads are the journal's counts; the
/// carrier flag is authoritative (`StationType == "FleetCarrier"`),
/// unlike the name-pattern heuristic it supersedes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct StationIdentity {
    pub system_name: String,
    pub system_address: Option<i64>,
    pub station_name: String,
    pub market_id: i64,
    pub observed_at: ObservedAt,
    pub station_type: Option<String>,
    pub arrival_ls: Option<f64>,
    pub pad_small: Option<i64>,
    pub pad_medium: Option<i64>,
    pub pad_large: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
}

impl StationIdentity {
    /// Journal ("FleetCarrier") or dump ("Drake-Class Carrier") spelling:
    /// one classifier decides.
    pub fn is_carrier(&self) -> bool {
        station::StationClass::of(self.station_type.as_deref()) == station::StationClass::Carrier
    }
    /// The journal service token for a black-market contact.
    pub fn has_black_market(&self) -> bool {
        self.services.iter().any(|s| s.eq_ignore_ascii_case("blackmarket"))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct SystemObservation {
    pub system_name: String,
    pub system_address: Option<i64>,
    pub position: Option<[f64; 3]>,
    pub observed_at: ObservedAt,
    pub controlling_power: Option<String>,
    pub powerplay_state: Option<String>,
    pub powers: Option<Vec<String>>,
    pub population: Option<i64>,
    pub security: Option<String>,
    pub allegiance: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ApplyStats {
    pub messages: u64,
    pub systems: u64,
    pub stations: u64,
    pub market_rows: u64,
    pub market_rows_removed: u64,
    pub outfitting_rows: u64,
    pub shipyard_rows: u64,
    pub skipped: u64,
    /// Star classes taught, bodies written, hotspot rows written, body
    /// signal rows updated (the feed's new arms, 2026-09-09).
    pub stars: u64,
    pub bodies: u64,
    pub hotspots: u64,
    pub body_signals: u64,
}

impl std::ops::AddAssign for ApplyStats {
    fn add_assign(&mut self, rhs: Self) {
        self.messages += rhs.messages;
        self.systems += rhs.systems;
        self.stations += rhs.stations;
        self.market_rows += rhs.market_rows;
        self.market_rows_removed += rhs.market_rows_removed;
        self.outfitting_rows += rhs.outfitting_rows;
        self.shipyard_rows += rhs.shipyard_rows;
        self.skipped += rhs.skipped;
        self.stars += rhs.stars;
        self.bodies += rhs.bodies;
        self.hotspots += rhs.hotspots;
        self.body_signals += rhs.body_signals;
    }
}
