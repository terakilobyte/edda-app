//! Live subscriber to EDDN, the Elite Dangerous Data Network.
//!
//! EDDN is the public firehose every third-party tool builds on: when any
//! player running EDMC (or EDDiscovery, or similar) docks, their client
//! broadcasts a market snapshot, and the same for outfitting, shipyard and
//! a subset of journal events. It is a plain ZeroMQ SUB socket with no key
//! and no registration.
//!
//! Two things shape this module:
//!
//! * **Decode is separate from transport.** Every wire format quirk is
//!   handled in [`decode`], a pure function over bytes, so the parsing is
//!   testable without a network connection.
//! * **Data is opportunistic, never authoritative.** A station only updates
//!   when some other commander happens to dock there. Quiet systems go stale
//!   for days, so every message carries its own timestamp and callers are
//!   expected to surface that age rather than imply freshness.
//!
//! The `journal/1` schema matters more than it looks: it carries other
//! players' `FSDJump` and `Location` events, which since Powerplay 2.0
//! include `ControllingPower` and `PowerplayState`. That is the only way to
//! keep Powerplay control current for systems the commander has not
//! personally flown to.

use anyhow::{Context, Result};
use ed_domain::Commodity;
pub use ed_domain::{ObservedAt, Operation, Snapshot, SystemObservation};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io::Read;

pub const EDDN_RELAY: &str = "tcp://eddn.edcd.io:9500";

/// Identify this app honestly on the wire. EDDN is a volunteer-run service;
/// impersonating a browser or another tool would be both rude and useless.
pub const USER_AGENT: &str = concat!("EDDA/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Deserialize)]
pub struct Header {
    #[serde(rename = "uploaderID", default)]
    pub uploader_id: Option<String>,
    #[serde(rename = "softwareName", default)]
    pub software_name: Option<String>,
    #[serde(rename = "softwareVersion", default)]
    pub software_version: Option<String>,
    #[serde(rename = "gatewayTimestamp", default)]
    pub gateway_timestamp: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CommodityEntry {
    /// Internal symbol, lowercase on the wire (e.g. `gold`) -- the same key
    /// space the galaxy tables use.
    pub name: String,
    #[serde(rename = "buyPrice", default)]
    pub buy_price: i64,
    #[serde(rename = "sellPrice", default)]
    pub sell_price: i64,
    #[serde(default)]
    pub demand: i64,
    #[serde(default)]
    pub stock: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommodityMessage {
    #[serde(rename = "systemName")]
    pub system_name: String,
    #[serde(rename = "stationName", default)]
    pub station_name: Option<String>,
    #[serde(rename = "marketId", default)]
    pub market_id: Option<i64>,
    pub timestamp: String,
    #[serde(default)]
    pub commodities: Vec<CommodityEntry>,
    /// Goods this market confiscates. Dropped by the decoder until
    /// 2026-09-04 — the schema's declared rule had no data to enforce.
    #[serde(default)]
    pub prohibited: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModuleEntry {
    #[serde(alias = "Name")]
    pub name: String,
}

/// outfitting/3 declares `modules` as an array of symbol strings, but
/// live uploaders also ship `{ "name": ... }` objects (observed
/// 2026-09-01 at several a minute -- rejecting them silently cost
/// whole boards) and raw Outfitting.json-style entries with capital
/// `Name` plus price/id fields (captured off the relay 2026-09-02,
/// arriving in one uploader's docking bursts -- the very first catch
/// of edda_eddn_decode_errors_total in production). All forms decode
/// to the symbol.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ModuleRef {
    Name(String),
    Entry(ModuleEntry),
}

impl ModuleRef {
    pub fn symbol(&self) -> &str {
        match self {
            ModuleRef::Name(name) => name,
            ModuleRef::Entry(entry) => &entry.name,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutfittingMessage {
    #[serde(rename = "systemName")]
    pub system_name: String,
    #[serde(rename = "stationName", default)]
    pub station_name: Option<String>,
    #[serde(rename = "marketId", default)]
    pub market_id: Option<i64>,
    pub timestamp: String,
    #[serde(default)]
    pub modules: Vec<ModuleRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShipyardMessage {
    #[serde(rename = "systemName")]
    pub system_name: String,
    #[serde(rename = "stationName", default)]
    pub station_name: Option<String>,
    #[serde(rename = "marketId", default)]
    pub market_id: Option<i64>,
    pub timestamp: String,
    #[serde(default)]
    pub ships: Vec<String>,
}

/// A journal event as broadcast by another commander's client.
#[derive(Debug, Clone, Deserialize)]
pub struct JournalMessage {
    #[serde(rename = "StarSystem", default)]
    pub star_system: Option<String>,
    #[serde(rename = "SystemAddress", default)]
    pub system_address: Option<i64>,
    #[serde(rename = "StarPos", default)]
    pub star_pos: Option<[f64; 3]>,
    #[serde(rename = "event", default)]
    pub event: Option<String>,
    pub timestamp: Option<String>,
    #[serde(rename = "ControllingPower", default)]
    pub controlling_power: Option<String>,
    #[serde(rename = "PowerplayState", default)]
    pub powerplay_state: Option<String>,
    #[serde(rename = "PowerplayStateControlProgress", default)]
    pub control_progress: Option<f64>,
    #[serde(rename = "PowerplayStateReinforcement", default)]
    pub reinforcement: Option<i64>,
    #[serde(rename = "PowerplayStateUndermining", default)]
    pub undermining: Option<i64>,
    #[serde(rename = "Powers", default)]
    pub powers: Option<Vec<String>>,
    #[serde(rename = "Population", default)]
    pub population: Option<i64>,
    #[serde(rename = "SystemSecurity", default)]
    pub system_security: Option<String>,
    #[serde(rename = "SystemAllegiance", default)]
    pub system_allegiance: Option<String>,
    // Docked events (station identity ingest, 2026-09-04):
    #[serde(rename = "StationName", default)]
    pub station_name: Option<String>,
    #[serde(rename = "MarketID", default)]
    pub market_id: Option<i64>,
    #[serde(rename = "StationType", default)]
    pub station_type: Option<String>,
    #[serde(rename = "DistFromStarLS", default)]
    pub dist_from_star_ls: Option<f64>,
    #[serde(rename = "LandingPads", default)]
    pub landing_pads: Option<LandingPads>,
    #[serde(rename = "StationServices", default)]
    pub station_services: Option<Vec<String>>,
    // Scan / SAASignalsFound / FSSBodySignals (2026-09-09, maintainer: what the
    // dump adds that the feed carried gets parsed): the arrival star's
    // class for the routing index, prospecting bodies, ring hotspots and
    // body signals for the mining search.
    #[serde(rename = "BodyName", default)]
    pub body_name: Option<String>,
    #[serde(rename = "BodyID", default)]
    pub body_id: Option<i64>,
    #[serde(rename = "StarType", default)]
    pub star_type: Option<String>,
    #[serde(rename = "PlanetClass", default)]
    pub planet_class: Option<String>,
    #[serde(rename = "Landable", default)]
    pub landable: Option<bool>,
    #[serde(rename = "DistanceFromArrivalLS", default)]
    pub distance_from_arrival_ls: Option<f64>,
    /// m/s² in the journal; stored in g.
    #[serde(rename = "SurfaceGravity", default)]
    pub surface_gravity: Option<f64>,
    #[serde(rename = "Atmosphere", default)]
    pub atmosphere: Option<String>,
    #[serde(rename = "Volcanism", default)]
    pub volcanism: Option<String>,
    #[serde(rename = "Materials", default)]
    pub materials: Option<Vec<ScanMaterial>>,
    #[serde(rename = "Rings", default)]
    pub rings: Option<Vec<ScanRing>>,
    #[serde(rename = "Signals", default)]
    pub signals: Option<Vec<ScanSignal>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanMaterial {
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Percent", default)]
    pub percent: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanRing {
    #[serde(rename = "Name", default)]
    pub name: String,
    /// `eRingClass_Metalic` / `eRingClass_MetalRich` / `eRingClass_Rocky` / `eRingClass_Icy`.
    #[serde(rename = "RingClass", default)]
    pub ring_class: Option<String>,
    #[serde(rename = "MassMT", default)]
    pub mass_mt: Option<f64>,
    #[serde(rename = "InnerRad", default)]
    pub inner_rad: Option<f64>,
    #[serde(rename = "OuterRad", default)]
    pub outer_rad: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanSignal {
    /// A material wire symbol on a ring (`Painite`), or
    /// `$SAA_SignalType_Biological;` / `…_Geological;` on a body.
    #[serde(rename = "Type", default)]
    pub kind: String,
    #[serde(rename = "Count", default)]
    pub count: i64,
}

/// The `navroute/1` schema: every system on a plotted route with its
/// star class and position — the feed's richest source of star classes
/// for systems nobody has scanned.
#[derive(Debug, Clone, Deserialize)]
pub struct NavRouteMessage {
    pub timestamp: Option<String>,
    #[serde(rename = "Route", default)]
    pub route: Vec<NavRouteEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NavRouteEntry {
    #[serde(rename = "StarSystem", default)]
    pub star_system: Option<String>,
    #[serde(rename = "SystemAddress", default)]
    pub system_address: Option<i64>,
    #[serde(rename = "StarPos", default)]
    pub star_pos: Option<[f64; 3]>,
    #[serde(rename = "StarClass", default)]
    pub star_class: Option<String>,
}

/// Spansh's ring type spelling from the journal's enum.
pub fn ring_kind(ring_class: &str) -> Option<String> {
    let k = ring_class.trim().trim_start_matches("eRingClass_");
    match k {
        "Metalic" | "Metallic" => Some("Metallic".into()),
        "MetalRich" => Some("Metal Rich".into()),
        "Rocky" => Some("Rocky".into()),
        "Icy" => Some("Icy".into()),
        "" => None,
        other => Some(other.to_string()),
    }
}

/// Spansh's material spelling from the journal's lowercase name.
fn material_name(raw: &str) -> String {
    let mut chars = raw.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The ring's parent body name: `Deciat 6 A Ring` → `Deciat 6`.
pub fn ring_parent_name(ring_name: &str) -> Option<&str> {
    let stripped = ring_name.trim().strip_suffix(" Ring")?;
    let (parent, letter) = stripped.rsplit_once(' ')?;
    (letter.len() == 1 && letter.chars().all(|c| c.is_ascii_uppercase())).then_some(parent)
}

#[derive(Debug, Clone, Deserialize)]
pub struct LandingPads {
    #[serde(rename = "Small", default)]
    pub small: Option<i64>,
    #[serde(rename = "Medium", default)]
    pub medium: Option<i64>,
    #[serde(rename = "Large", default)]
    pub large: Option<i64>,
}

#[derive(Debug, Clone)]
pub enum Payload {
    Commodity(CommodityMessage),
    Outfitting(OutfittingMessage),
    Shipyard(ShipyardMessage),
    Journal(JournalMessage),
    NavRoute(NavRouteMessage),
    /// A schema we do not handle yet. Kept rather than dropped so the
    /// subscriber can report what it is seeing and go unhandled loudly.
    Other {
        schema: String,
    },
}

#[derive(Debug, Clone)]
pub struct Envelope {
    pub schema_ref: String,
    pub header: Header,
    pub payload: Payload,
}

impl Envelope {
    /// Short schema name, e.g. `commodity/3`.
    pub fn schema(&self) -> &str {
        self.schema_ref
            .rsplit("/schemas/")
            .next()
            .unwrap_or(&self.schema_ref)
    }

    /// The first operation of [`Envelope::operations`] — the one-message
    /// callers (the client's own store) keep this shape.
    pub fn operation(&self) -> Option<Operation> {
        self.operations().into_iter().next()
    }

    /// Every database-independent operation a decoded message carries.
    /// A jump is one system; a `Scan` is that system plus a star class or
    /// a body; a `NavRoute` is a system and a star per hop. Destructive
    /// snapshots without a valid timestamp are rejected here so every
    /// storage backend makes the same freshness decision.
    pub fn operations(&self) -> Vec<Operation> {
        match &self.payload {
            Payload::NavRoute(message) => {
                let Some(observed) = message.timestamp.as_deref().and_then(observed_at) else {
                    return Vec::new();
                };
                let mut out = Vec::with_capacity(message.route.len() * 2);
                for hop in &message.route {
                    let (Some(name), Some(address)) = (hop.star_system.clone(), hop.system_address) else {
                        continue;
                    };
                    if hop.star_pos.is_some() {
                        out.push(Operation::System(SystemObservation {
                            system_name: name.clone(),
                            system_address: Some(address),
                            position: hop.star_pos,
                            observed_at: observed.clone(),
                            controlling_power: None,
                            powerplay_state: None,
                            powers: None,
                            population: None,
                            security: None,
                            allegiance: None,
                        }));
                    }
                    if let Some(class) = hop.star_class.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
                        out.push(Operation::Star(ed_domain::StarTeaching {
                            system_address: address,
                            system_name: Some(name),
                            position: hop.star_pos,
                            star_type: class.to_string(),
                            observed_at: observed.clone(),
                            source: "eddn:navroute".into(),
                        }));
                    }
                }
                out
            }
            Payload::Journal(message) => {
                let mut out: Vec<Operation> = self.journal_operation().into_iter().collect();
                out.extend(scan_operations(message));
                out
            }
            _ => self.journal_operation().into_iter().collect(),
        }
    }

    /// The pre-2026-09-09 single operation: boards, a jump's system row,
    /// a Docked identity.
    fn journal_operation(&self) -> Option<Operation> {
        match &self.payload {
            Payload::Commodity(message) => Some(Operation::Market(Snapshot {
                system_name: message.system_name.clone(),
                station_name: message.station_name.clone(),
                market_id: message.market_id,
                observed_at: observed_at(&message.timestamp)?,
                values: normalize_commodities(&message.commodities),
                prohibited: message
                    .prohibited
                    .iter()
                    .map(|p| p.trim().to_lowercase())
                    .filter(|p| !p.is_empty())
                    .collect(),
            })),
            Payload::Outfitting(message) => Some(Operation::Outfitting(Snapshot {
                system_name: message.system_name.clone(),
                station_name: message.station_name.clone(),
                market_id: message.market_id,
                observed_at: observed_at(&message.timestamp)?,
                values: normalize_symbols(message.modules.iter().map(ModuleRef::symbol)),
                prohibited: Vec::new(),
            })),
            Payload::Shipyard(message) => Some(Operation::Shipyard(Snapshot {
                system_name: message.system_name.clone(),
                station_name: message.station_name.clone(),
                market_id: message.market_id,
                observed_at: observed_at(&message.timestamp)?,
                values: normalize_symbols(message.ships.iter().map(String::as_str)),
                prohibited: Vec::new(),
            })),
            Payload::Journal(message) => {
                let system_name = message.star_system.clone()?;
                // Docked: the one wire source for pads, carrier-ness,
                // arrival distance, type and services. Identity wins the
                // envelope; Docked events rarely carry the system-level
                // fields the System arm wants anyway.
                if message.event.as_deref() == Some("Docked") {
                    let station_name = message.station_name.clone()?;
                    let market_id = message.market_id?;
                    let pads = message.landing_pads.as_ref();
                    return Some(Operation::StationIdentity(ed_domain::StationIdentity {
                        system_name,
                        system_address: message.system_address,
                        station_name,
                        market_id,
                        observed_at: observed_at(message.timestamp.as_deref()?)?,
                        station_type: message.station_type.clone(),
                        arrival_ls: message.dist_from_star_ls,
                        pad_small: pads.and_then(|p| p.small),
                        pad_medium: pads.and_then(|p| p.medium),
                        pad_large: pads.and_then(|p| p.large),
                        services: message.station_services.clone().unwrap_or_default(),
                    }));
                }
                if message.controlling_power.is_none()
                    && message.powerplay_state.is_none()
                    && message.star_pos.is_none()
                    && message.population.is_none()
                {
                    return None;
                }
                Some(Operation::System(SystemObservation {
                    system_name,
                    system_address: message.system_address,
                    position: message.star_pos,
                    observed_at: observed_at(message.timestamp.as_deref()?)?,
                    controlling_power: message.controlling_power.clone(),
                    powerplay_state: message.powerplay_state.clone(),
                    powers: message.powers.clone(),
                    population: message.population,
                    security: message
                        .system_security
                        .as_deref()
                        .map(ed_domain::system::security_name),
                    allegiance: message.system_allegiance.clone(),
                }))
            }
            Payload::NavRoute(_) | Payload::Other { .. } => None,
        }
    }
}

/// The star, body and signal teachings inside one journal event.
fn scan_operations(message: &JournalMessage) -> Vec<Operation> {
    let mut out = Vec::new();
    let (Some(address), Some(observed)) = (message.system_address, message.timestamp.as_deref().and_then(observed_at)) else {
        return out;
    };
    match message.event.as_deref() {
        Some("Scan") => {
            // The arrival star is the one the router cares about: the
            // journal puts it at 0 ls from arrival.
            if let Some(star_type) = message.star_type.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                if message.distance_from_arrival_ls.is_some_and(|d| d == 0.0) {
                    out.push(Operation::Star(ed_domain::StarTeaching {
                        system_address: address,
                        system_name: message.star_system.clone(),
                        position: message.star_pos,
                        star_type: star_type.to_string(),
                        observed_at: observed.clone(),
                        source: "eddn:scan".into(),
                    }));
                }
            }
            if let (Some(body_id), Some(name)) = (message.body_id, message.body_name.clone()) {
                if message.planet_class.is_some() {
                    let materials: Vec<(String, f64)> = message
                        .materials
                        .iter()
                        .flatten()
                        .filter(|m| !m.name.trim().is_empty())
                        .map(|m| (material_name(&m.name), m.percent))
                        .collect();
                    let rings: Vec<ed_domain::RingTeaching> = message
                        .rings
                        .iter()
                        .flatten()
                        .filter(|r| !r.name.trim().is_empty())
                        .map(|r| ed_domain::RingTeaching {
                            name: r.name.trim().to_string(),
                            kind: r.ring_class.as_deref().and_then(ring_kind),
                            mass: r.mass_mt,
                            inner_radius: r.inner_rad,
                            outer_radius: r.outer_rad,
                        })
                        .collect();
                    let landable = message.landable.unwrap_or(false);
                    if landable || !materials.is_empty() || !rings.is_empty() {
                        out.push(Operation::Body(ed_domain::BodyTeaching {
                            id64: ed_domain::body_id64(address, body_id),
                            system_address: address,
                            body_id: i32::try_from(body_id).ok(),
                            name: Some(name),
                            kind: Some("Planet".into()),
                            sub_type: message.planet_class.clone(),
                            is_landable: landable,
                            distance_to_arrival: message.distance_from_arrival_ls,
                            gravity: message.surface_gravity.map(|g| g / 9.80665),
                            atmosphere: message.atmosphere.clone().filter(|a| !a.is_empty()),
                            volcanism: message.volcanism.clone().filter(|v| !v.is_empty()),
                            bio_signals: None,
                            geo_signals: None,
                            observed_at: observed.clone(),
                            provenance: "eddn:scan".into(),
                            materials,
                            rings,
                            hotspots: Vec::new(),
                        }));
                    }
                }
            }
        }
        Some("SAASignalsFound") | Some("FSSBodySignals") => {
            let (Some(body_id), Some(name), Some(signals)) = (message.body_id, message.body_name.as_deref(), message.signals.as_ref()) else {
                return out;
            };
            let is_ring = ring_parent_name(name).is_some();
            let hotspots: Vec<(String, i32)> = signals
                .iter()
                .filter(|s| !s.kind.starts_with('$') && !s.kind.trim().is_empty())
                .map(|s| (s.kind.trim().to_string(), i32::try_from(s.count).unwrap_or(i32::MAX)))
                .collect();
            let signal = |key: &str| -> Option<i32> {
                signals
                    .iter()
                    .find(|s| s.kind.eq_ignore_ascii_case(key))
                    .and_then(|s| i32::try_from(s.count).ok())
            };
            let bio = signal("$SAA_SignalType_Biological;");
            let geo = signal("$SAA_SignalType_Geological;");
            if is_ring && !hotspots.is_empty() {
                out.push(Operation::RingHotspots(ed_domain::RingHotspots {
                    system_address: address,
                    ring_name: name.trim().to_string(),
                    signals: hotspots,
                    observed_at: observed.clone(),
                }));
            } else if !is_ring && (bio.is_some() || geo.is_some()) {
                out.push(Operation::BodySignals(ed_domain::BodySignals {
                    id64: ed_domain::body_id64(address, body_id),
                    system_address: address,
                    body_id: i32::try_from(body_id).ok(),
                    name: Some(name.trim().to_string()),
                    bio_signals: bio,
                    geo_signals: geo,
                    observed_at: observed,
                }));
            }
        }
        _ => {}
    }
    out
}

#[derive(Debug, Deserialize)]
struct RawEnvelope {
    #[serde(rename = "$schemaRef")]
    schema_ref: String,
    #[serde(default)]
    header: Option<Header>,
    message: serde_json::Value,
}

/// Decode one EDDN frame: zlib-compressed JSON on the wire.
///
/// Uncompressed input is also accepted -- the relay has historically sent
/// both, and being strict here would drop real data for no benefit.
pub fn decode(frame: &[u8]) -> Result<Envelope> {
    let json = inflate(frame)?;
    let raw: RawEnvelope =
        serde_json::from_slice(&json).context("EDDN envelope was not the expected JSON shape")?;

    let payload = match short_schema(&raw.schema_ref) {
        s if s.starts_with("commodity/") => {
            Payload::Commodity(serde_json::from_value(raw.message)?)
        }
        s if s.starts_with("outfitting/") => {
            Payload::Outfitting(serde_json::from_value(raw.message)?)
        }
        s if s.starts_with("shipyard/") => Payload::Shipyard(serde_json::from_value(raw.message)?),
        s if s.starts_with("journal/") => Payload::Journal(serde_json::from_value(raw.message)?),
        s if s.starts_with("navroute/") => Payload::NavRoute(serde_json::from_value(raw.message)?),
        // The journal-shaped side schemas (measured 2026-09-09: fssbodysignals
        // is 9.5 % of all frames): the same event fields, their own schema.
        s if s.starts_with("fssbodysignals/") => Payload::Journal(serde_json::from_value(raw.message)?),
        other => Payload::Other {
            schema: other.to_string(),
        },
    };

    Ok(Envelope {
        schema_ref: raw.schema_ref.clone(),
        header: raw.header.unwrap_or(Header {
            uploader_id: None,
            software_name: None,
            software_version: None,
            gateway_timestamp: None,
        }),
        payload,
    })
}

fn short_schema(schema_ref: &str) -> &str {
    schema_ref.rsplit("/schemas/").next().unwrap_or(schema_ref)
}

fn inflate(frame: &[u8]) -> Result<Vec<u8>> {
    // Cheap check: a JSON document starts with '{' or whitespace, never
    // with a zlib header byte.
    if frame.first().is_some_and(|b| *b == b'{') {
        return Ok(frame.to_vec());
    }
    let mut out = Vec::with_capacity(frame.len() * 8);
    flate2::read::ZlibDecoder::new(frame)
        .read_to_end(&mut out)
        .context("EDDN frame was neither zlib-compressed nor plain JSON")?;
    Ok(out)
}

/// Running totals, so a long-lived subscriber can report what it is doing.
#[derive(Debug, Default, Clone)]
pub struct FeedStats {
    pub received: u64,
    pub decoded: u64,
    pub decode_errors: u64,
    pub commodity: u64,
    /// Price rows inside the commodity messages — what the "Live prices"
    /// pill counts. Decode-side on purpose: the number keeps moving while
    /// application is parked behind a bulk write, without exposing the
    /// queue to the user (maintainer ruling 2026-09-04).
    pub commodity_rows: u64,
    pub outfitting: u64,
    pub shipyard: u64,
    pub journal: u64,
    pub other: u64,
    pub reconnects: u64,
    pub normalization_skipped: u64,
    /// Exact schemas observed, including schemas EDDA does not handle yet.
    pub schemas: BTreeMap<String, u64>,
    /// Event breakdown within the legacy journal schema.
    pub journal_events: BTreeMap<String, u64>,
    /// Decode failures grouped by recoverable schema and error stage.
    pub decode_failures: BTreeMap<String, u64>,
}

impl FeedStats {
    pub fn count(&mut self, env: &Envelope) {
        self.decoded += 1;
        *self.schemas.entry(env.schema().to_string()).or_default() += 1;
        match &env.payload {
            Payload::Commodity(message) => {
                self.commodity += 1;
                self.commodity_rows += message.commodities.len() as u64;
            }
            Payload::Outfitting(_) => self.outfitting += 1,
            Payload::Shipyard(_) => self.shipyard += 1,
            Payload::Journal(message) => {
                self.journal += 1;
                *self
                    .journal_events
                    .entry(message.event.as_deref().unwrap_or("unknown").to_string())
                    .or_default() += 1;
            }
            Payload::NavRoute(message) => {
                self.journal += 1;
                *self.journal_events.entry("NavRoute".to_string()).or_default() += 1;
                *self.journal_events.entry("NavRoute.hops".to_string()).or_default() += message.route.len() as u64;
            }
            Payload::Other { .. } => self.other += 1,
        }
    }
}

fn observed_at(source: &str) -> Option<ObservedAt> {
    Some(ObservedAt::new(source, epoch_secs(source)?))
}

/// Whole seconds since the Unix epoch for the EDDN RFC-3339 UTC shape.
pub fn epoch_secs(timestamp: &str) -> Option<i64> {
    let bytes = timestamp.as_bytes();
    if bytes.len() < 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || !timestamp.ends_with('Z')
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| timestamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = (month + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

fn normalize_symbols<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    values
        .into_iter()
        .map(str::to_lowercase)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_commodities(values: &[CommodityEntry]) -> Vec<Commodity> {
    let mut by_symbol = BTreeMap::new();
    for value in values {
        let mut name = value.name.clone();
        name.make_ascii_lowercase();
        by_symbol.insert(
            name.clone(),
            Commodity {
                name,
                buy_price: value.buy_price,
                sell_price: value.sell_price,
                demand: value.demand,
                stock: value.stock,
            },
        );
    }
    by_symbol.into_values().collect()
}

/// Safe diagnostic metadata for a malformed frame. Values are deliberately
/// omitted: schema, event and field names are enough to extend the decoder
/// without retaining uploader identifiers or arbitrary payload contents.
#[cfg(feature = "live")]
fn decode_diagnostic(frame: &[u8], error: &anyhow::Error) -> (String, String) {
    let stage = if error.to_string().contains("neither zlib-compressed") {
        "wire"
    } else {
        "json"
    };
    let Ok(json) = inflate(frame) else {
        return (format!("unknown/{stage}"), error.to_string());
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&json) else {
        return (format!("unknown/{stage}"), error.to_string());
    };
    let schema = value
        .get("$schemaRef")
        .and_then(|v| v.as_str())
        .map(short_schema)
        .unwrap_or("unknown");
    let message = value.get("message");
    let event = message
        .and_then(|m| m.get("event"))
        .and_then(|v| v.as_str());
    let mut keys: Vec<&str> = message
        .and_then(|m| m.as_object())
        .map(|m| m.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    let label = format!("{schema}/{stage}");
    let detail = format!(
        "{}; event={}; fields={}",
        error,
        event.unwrap_or("-"),
        keys.join(",")
    );
    (label, detail)
}

#[cfg(feature = "live")]
pub mod live {
    use super::*;
    use std::future::Future;
    use std::time::Duration;
    use zeromq::{Socket, SocketRecv, SubSocket};

    /// Whether the subscriber keeps going after a message.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Flow {
        Continue,
        Stop,
    }

    /// Where decoded envelopes go. One loop ([`subscribe`]) drives every
    /// sink; the sinks are the adapters: a closure, a bounded channel.
    pub trait Sink {
        fn deliver(&mut self, envelope: Envelope, stats: &FeedStats) -> impl Future<Output = Flow> + Send;
    }

    /// How long a live connection may stay silent before it is presumed
    /// dead and rebuilt. EDDN carries several messages a second around
    /// the clock; minutes of silence mean the relay dropped us without a
    /// FIN, or the socket machinery wedged -- both were observed as an
    /// ESTABLISHED socket with a full receive queue that `recv` never
    /// returned from (2026-08-31: the WSL server ingested for 20 minutes
    /// and then sat wedged for 1.3 days without a log line).
    pub const RECV_IDLE_RECONNECT: Duration = Duration::from_secs(120);

    /// The subscribe/decode/reconnect loop. Runs until the sink says stop.
    /// Reconnects on its own: the relay drops long-lived connections
    /// routinely, and a subscriber that dies on the first disconnect is
    /// useless as a background service.
    pub async fn subscribe<S: Sink + Send>(relay: &str, sink: &mut S) -> Result<FeedStats> {
        subscribe_with_idle(relay, sink, RECV_IDLE_RECONNECT).await
    }

    /// [`subscribe`] with its own idle limit -- the tests fake a silent
    /// relay and cannot wait two minutes for the real one.
    pub async fn subscribe_with_idle<S: Sink + Send>(relay: &str, sink: &mut S, idle: Duration) -> Result<FeedStats> {
        let mut stats = FeedStats::default();
        let mut backoff = Duration::from_secs(1);

        loop {
            let mut socket = SubSocket::new();
            match socket.connect(relay).await {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(error) => {
                    stats.reconnects += 1;
                    metrics::counter!("edda_eddn_reconnects_total", "reason" => "connect_failed").increment(1);
                    tracing::warn!(%error, ?backoff, "EDDN connection failed");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                    continue;
                }
            }
            socket.subscribe("").await.context("subscribing to EDDN")?;

            loop {
                let received = match tokio::time::timeout(idle, socket.recv()).await {
                    Ok(received) => received,
                    Err(_) => {
                        // The wedge this heals leaves the socket
                        // ESTABLISHED with a full receive queue and a
                        // `recv` that never wakes; dropping the socket
                        // and dialling fresh is the only exit.
                        stats.reconnects += 1;
                        metrics::counter!("edda_eddn_reconnects_total", "reason" => "idle").increment(1);
                        tracing::warn!(idle_secs = idle.as_secs_f64(), "EDDN silent past the idle limit; rebuilding the connection");
                        break;
                    }
                };
                match received {
                    Ok(message) => {
                        stats.received += 1;
                        metrics::counter!("edda_eddn_frames_received_total").increment(1);
                        let Some(frame) = message.get(0) else {
                            continue;
                        };
                        match decode(frame) {
                            Ok(envelope) => {
                                stats.count(&envelope);
                                if sink.deliver(envelope, &stats).await == Flow::Stop {
                                    return Ok(stats);
                                }
                            }
                            // One malformed frame must not kill the feed.
                            Err(error) => {
                                stats.decode_errors += 1;
                                metrics::counter!("edda_eddn_decode_errors_total").increment(1);
                                let (kind, detail) = decode_diagnostic(frame, &error);
                                let count = stats.decode_failures.entry(kind.clone()).or_default();
                                *count += 1;
                                // A few examples identify a new shape without flooding logs.
                                if *count <= 3 {
                                    tracing::warn!(kind, detail, "EDDN decode failed");
                                }
                            }
                        }
                    }
                    Err(error) => {
                        stats.reconnects += 1;
                        metrics::counter!("edda_eddn_reconnects_total", "reason" => "recv_failed").increment(1);
                        tracing::warn!(%error, "EDDN receive failed; reconnecting");
                        break;
                    }
                }
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(60));
        }
    }

    struct FnSink<F>(F);

    impl<F> Sink for FnSink<F>
    where
        F: FnMut(&Envelope, &FeedStats) -> bool + Send,
    {
        fn deliver(&mut self, envelope: Envelope, stats: &FeedStats) -> impl Future<Output = Flow> + Send {
            let flow = if (self.0)(&envelope, stats) { Flow::Continue } else { Flow::Stop };
            std::future::ready(flow)
        }
    }

    /// Subscribe to the relay and hand every decoded envelope to `on_message`.
    /// Runs until `on_message` returns `false`.
    pub async fn run(
        relay: &str,
        on_message: impl FnMut(&Envelope, &FeedStats) -> bool + Send,
    ) -> Result<FeedStats> {
        subscribe(relay, &mut FnSink(on_message)).await
    }

    struct ChannelSink<O> {
        sender: tokio::sync::mpsc::Sender<Operation>,
        observe: O,
        normalization_skipped: u64,
    }

    impl<O: FnMut(&FeedStats) + Send> Sink for ChannelSink<O> {
        fn deliver(&mut self, envelope: Envelope, stats: &FeedStats) -> impl Future<Output = Flow> + Send {
            let operations = envelope.operations();
            if operations.is_empty() {
                self.normalization_skipped += 1;
            }
            let mut observed = stats.clone();
            observed.normalization_skipped = self.normalization_skipped;
            (self.observe)(&observed);
            async move {
                // Backpressure: a full channel parks the subscriber
                // rather than dropping the message.
                for op in operations {
                    if self.sender.send(op).await.is_err() {
                        return Flow::Stop;
                    }
                }
                Flow::Continue
            }
        }
    }

    /// Subscribe continuously and apply backpressure through a bounded
    /// channel of normalized operations. Raw envelopes and uploader metadata
    /// never enter the storage queue.
    pub async fn run_to_channel(
        relay: &str,
        sender: tokio::sync::mpsc::Sender<Operation>,
    ) -> Result<FeedStats> {
        run_to_channel_with(relay, sender, |_| {}).await
    }

    /// [`run_to_channel`] that also reports the running totals after every
    /// message, for a subscriber that shows feed health while it runs.
    pub async fn run_to_channel_with(
        relay: &str,
        sender: tokio::sync::mpsc::Sender<Operation>,
        observe: impl FnMut(&FeedStats) + Send,
    ) -> Result<FeedStats> {
        let mut sink = ChannelSink { sender, observe, normalization_skipped: 0 };
        let mut stats = subscribe(relay, &mut sink).await?;
        stats.normalization_skipped = sink.normalization_skipped;
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zlib(s: &str) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(s.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    const COMMODITY: &str = r#"{
      "$schemaRef":"https://eddn.edcd.io/schemas/commodity/3",
      "header":{"uploaderID":"abc","softwareName":"E:D Market Connector","gatewayTimestamp":"2026-08-24T01:00:00Z"},
      "message":{"systemName":"Deciat","stationName":"Garay Terminal","marketId":3229332736,
        "timestamp":"2026-08-24T00:59:00Z",
        "commodities":[{"name":"gold","buyPrice":0,"sellPrice":48549,"demand":1200,"stock":0}]}
    }"#;

    const JOURNAL_JUMP: &str = r#"{
      "$schemaRef":"https://eddn.edcd.io/schemas/journal/1",
      "header":{"uploaderID":"xyz"},
      "message":{"event":"FSDJump","StarSystem":"Deciat","SystemAddress":6681123623626,
        "StarPos":[122.625,-0.625,-47.28125],"timestamp":"2026-08-24T00:58:00Z",
        "ControllingPower":"A. Lavigny-Duval","PowerplayState":"Stronghold",
        "PowerplayStateControlProgress":0.434213,"PowerplayStateReinforcement":62721,
        "PowerplayStateUndermining":46108,"Population":31778844,
        "SystemSecurity":"$SYSTEM_SECURITY_high;"}}"#;

    #[test]
    fn decodes_a_zlib_commodity_frame() {
        let env = decode(&zlib(COMMODITY)).unwrap();
        assert_eq!(env.schema(), "commodity/3");
        let Payload::Commodity(m) = env.payload else {
            panic!("expected commodity")
        };
        assert_eq!(m.system_name, "Deciat");
        assert_eq!(m.market_id, Some(3229332736));
        assert_eq!(m.commodities[0].name, "gold");
        assert_eq!(m.commodities[0].sell_price, 48549);
        assert_eq!(m.commodities[0].demand, 1200);
    }

    #[test]
    fn accepts_uncompressed_frames_too() {
        // The relay has historically sent both; being strict would silently
        // drop real data.
        let env = decode(COMMODITY.as_bytes()).unwrap();
        assert_eq!(env.schema(), "commodity/3");
    }

    #[test]
    fn journal_frames_carry_powerplay_state() {
        // This is the whole reason journal/1 is worth subscribing to: it
        // keeps control state current for systems never personally visited.
        let env = decode(&zlib(JOURNAL_JUMP)).unwrap();
        let Payload::Journal(m) = env.payload else {
            panic!("expected journal")
        };
        assert_eq!(m.star_system.as_deref(), Some("Deciat"));
        assert_eq!(m.controlling_power.as_deref(), Some("A. Lavigny-Duval"));
        assert_eq!(m.powerplay_state.as_deref(), Some("Stronghold"));
        assert_eq!(m.reinforcement, Some(62721));
        assert_eq!(m.star_pos.unwrap()[0], 122.625);
    }

    /// EDDN strips the `_Localised` fields, so a journal frame carries only
    /// the security symbol; the store gets the display name.
    #[test]
    fn journal_frames_store_security_as_a_display_name() {
        let env = decode(&zlib(JOURNAL_JUMP)).unwrap();
        let Some(Operation::System(obs)) = env.operation() else {
            panic!("expected a system observation")
        };
        assert_eq!(obs.security.as_deref(), Some("High"));
    }

    #[test]
    fn an_unknown_schema_is_reported_not_dropped() {
        let raw = r#"{"$schemaRef":"https://eddn.edcd.io/schemas/fssdiscoveryscan/1",
                      "header":{},"message":{"systemName":"X"}}"#;
        let env = decode(&zlib(raw)).unwrap();
        match env.payload {
            Payload::Other { schema } => assert_eq!(schema, "fssdiscoveryscan/1"),
            _ => panic!("expected Other"),
        }
    }

    #[test]
    fn a_malformed_frame_is_an_error_not_a_panic() {
        assert!(decode(b"not zlib and not json").is_err());
        assert!(decode(&zlib("{\"nope\":1}")).is_err());
    }

    #[test]
    fn stats_tally_by_schema() {
        let mut stats = FeedStats::default();
        stats.count(&decode(&zlib(COMMODITY)).unwrap());
        stats.count(&decode(&zlib(JOURNAL_JUMP)).unwrap());
        assert_eq!(stats.decoded, 2);
        assert_eq!(stats.commodity, 1);
        assert_eq!(stats.journal, 1);
    }

    #[test]
    fn normalization_rejects_undated_destructive_snapshots() {
        let raw = COMMODITY.replace("2026-08-24T00:59:00Z", "not-a-timestamp");
        let envelope = decode(raw.as_bytes()).unwrap();
        assert!(envelope.operation().is_none());
    }

    #[test]
    fn normalization_produces_backend_neutral_market_operation() {
        let envelope = decode(COMMODITY.as_bytes()).unwrap();
        let Operation::Market(snapshot) = envelope.operation().unwrap() else {
            panic!("expected market operation");
        };
        assert_eq!(snapshot.market_id, Some(3229332736));
        assert_eq!(snapshot.observed_at.epoch_seconds, 1_787_533_140);
        assert_eq!(snapshot.values[0].name, "gold");
    }

    #[test]
    fn normalization_deduplicates_catalog_symbols() {
        let raw = r#"{"$schemaRef":"https://eddn.edcd.io/schemas/outfitting/2","header":{},
          "message":{"systemName":"Sol","stationName":"Galileo","marketId":10,
          "timestamp":"2026-08-24T03:00:00Z",
          "modules":["Int_Hyperdrive_Size2_Class1","int_hyperdrive_size2_class1"]}}"#;
        let envelope = decode(raw.as_bytes()).unwrap();
        let Operation::Outfitting(snapshot) = envelope.operation().unwrap() else {
            panic!("expected outfitting operation");
        };
        assert_eq!(snapshot.values, vec!["int_hyperdrive_size2_class1"]);
    }

    /// Live uploaders ship outfitting modules both ways -- bare symbol
    /// strings per the schema, and `{ "name": ... }` objects (observed
    /// on the relay 2026-09-01, several a minute; rejecting the object
    /// form silently cost whole boards). One message may mix them.
    #[test]
    fn outfitting_modules_decode_as_strings_and_as_name_objects() {
        let raw = r#"{"$schemaRef":"https://eddn.edcd.io/schemas/outfitting/3","header":{},
          "message":{"systemName":"Sol","stationName":"Galileo","marketId":10,
          "timestamp":"2026-08-24T03:00:00Z","horizons":true,"odyssey":true,
          "modules":[{"name":"Int_Hyperdrive_Size2_Class1"},"Hpt_PulseLaser_Fixed_Small"]}}"#;
        let envelope = decode(raw.as_bytes()).unwrap();
        let Operation::Outfitting(snapshot) = envelope.operation().unwrap() else {
            panic!("expected outfitting operation");
        };
        assert_eq!(snapshot.values, vec!["hpt_pulselaser_fixed_small", "int_hyperdrive_size2_class1"]);
    }

    /// The third live shape (captured off the relay 2026-09-02 after
    /// edda_eddn_decode_errors_total flagged it in production): raw
    /// Outfitting.json-style entries with capital `Name` and price/id
    /// fields. One uploader ships these in docking bursts; the
    /// case-sensitive decode was dropping its whole boards.
    #[test]
    fn outfitting_modules_decode_capital_name_journal_entries() {
        let raw = r#"{"$schemaRef":"https://eddn.edcd.io/schemas/outfitting/3","header":{},
          "message":{"systemName":"Sol","stationName":"Galileo","marketId":10,
          "timestamp":"2026-09-02T05:11:08Z","horizons":true,"odyssey":true,
          "modules":[{"BuyMercCoinsPrice":0,"BuyPrice":1391227,"Name":"Hpt_BasicMissileRack_Fixed_Large","id":128049494},
                     {"BuyMercCoinsPrice":0,"BuyPrice":149391,"Name":"Hpt_AdvancedTorpPylon_Fixed_Large","id":128049511}]}}"#;
        let envelope = decode(raw.as_bytes()).unwrap();
        let Operation::Outfitting(snapshot) = envelope.operation().unwrap() else {
            panic!("expected outfitting operation");
        };
        assert_eq!(snapshot.values, vec!["hpt_advancedtorppylon_fixed_large", "hpt_basicmissilerack_fixed_large"]);
    }
}

#[cfg(all(test, feature = "live"))]
mod live_tests {
    use super::*;
    use std::io::Write as _;
    use std::time::Duration;

    const COMMODITY: &str = r#"{
      "$schemaRef":"https://eddn.edcd.io/schemas/commodity/3",
      "header":{"uploaderID":"abc","gatewayTimestamp":"2026-08-24T01:00:00Z"},
      "message":{"systemName":"Deciat","stationName":"Garay Terminal","marketId":3229332736,
        "timestamp":"2026-08-24T00:59:00Z",
        "commodities":[{"name":"gold","buyPrice":0,"sellPrice":48549,"demand":1200,"stock":0}]}
    }"#;

    fn zlib(s: &str) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(s.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    /// Stops the feed after the first delivered envelope.
    struct StopAfterFirst {
        received: u64,
    }

    impl live::Sink for StopAfterFirst {
        fn deliver(&mut self, _envelope: Envelope, _stats: &FeedStats) -> impl std::future::Future<Output = live::Flow> + Send {
            self.received += 1;
            std::future::ready(live::Flow::Stop)
        }
    }

    /// The wedge observed on the real relay (2026-08-31): an ESTABLISHED
    /// socket that `recv` never returns from. A subscriber left waiting
    /// on a silent connection must presume it dead, rebuild the socket,
    /// and hear the relay again on its own -- here the fake relay says
    /// nothing for four idle periods, then starts talking; only a
    /// subscriber that reconnected ever receives.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_silent_relay_connection_is_torn_down_and_reconnected() {
        use zeromq::{PubSocket, Socket, SocketSend};
        let mut publisher = PubSocket::new();
        let endpoint = publisher.bind("tcp://127.0.0.1:0").await.unwrap();
        let addr = endpoint.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            loop {
                let _ = publisher.send(zeromq::ZmqMessage::from(zlib(COMMODITY))).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        });
        let mut sink = StopAfterFirst { received: 0 };
        let stats = tokio::time::timeout(
            Duration::from_secs(15),
            live::subscribe_with_idle(&addr, &mut sink, Duration::from_millis(150)),
        )
        .await
        .expect("a silent connection must not hang the feed forever")
        .unwrap();
        assert_eq!(sink.received, 1);
        assert!(stats.received >= 1);
        assert!(stats.reconnects >= 1, "the silent stretch must have forced a reconnect: {stats:?}");
    }
}

/// The feed's teachings (2026-09-09, maintainer: "if it could have been gotten
/// from the eddn feed and we just aren't parsing it, we need to fix that
/// pronto"): a Scan of the arrival star teaches its class, a planet Scan
/// a prospecting body, a NavRoute a system and a star per hop, a ring's
/// SAASignalsFound its hotspots, a body's signals its bio/geo counts.
#[cfg(test)]
mod teaching_tests {
    use super::*;

    fn journal(message: &str) -> Envelope {
        let raw = format!(r#"{{"$schemaRef":"https://eddn.edcd.io/schemas/journal/1","header":{{"uploaderID":"x","softwareName":"t","softwareVersion":"1"}},"message":{message}}}"#);
        decode(raw.as_bytes()).unwrap()
    }

    #[test]
    fn a_star_scan_at_arrival_teaches_the_class_and_the_system() {
        let env = journal(r#"{"timestamp":"2026-09-09T10:00:00Z","event":"Scan","ScanType":"AutoScan","StarSystem":"Deciat","SystemAddress":6681123623626,"StarPos":[122.1875,-0.8125,-47.28125],"BodyName":"Deciat","BodyID":0,"DistanceFromArrivalLS":0.0,"StarType":"K","Subclass":3,"StellarMass":0.68}"#);
        let ops = env.operations();
        assert_eq!(ops.len(), 2, "{ops:?}");
        assert!(matches!(&ops[0], Operation::System(s) if s.system_address == Some(6681123623626) && s.position.is_some()));
        let Operation::Star(star) = &ops[1] else { panic!("star teaching") };
        assert_eq!((star.system_address, star.star_type.as_str(), star.source.as_str()), (6681123623626, "K", "eddn:scan"));
        assert_eq!(star.observed_at.epoch_seconds, epoch_secs("2026-09-09T10:00:00Z").unwrap());
        // A secondary star (not at 0 ls) is not the arrival star.
        let secondary = journal(r#"{"timestamp":"2026-09-09T10:00:00Z","event":"Scan","StarSystem":"Deciat","SystemAddress":6681123623626,"StarPos":[122.1875,-0.8125,-47.28125],"BodyName":"Deciat B","BodyID":1,"DistanceFromArrivalLS":9800.5,"StarType":"M"}"#);
        assert_eq!(secondary.operations().len(), 1, "system only");
    }

    #[test]
    fn a_planet_scan_becomes_a_prospecting_body_in_spansh_spelling() {
        let env = journal(r#"{"timestamp":"2026-09-09T10:01:00Z","event":"Scan","StarSystem":"Deciat","SystemAddress":6681123623626,"StarPos":[122.1875,-0.8125,-47.28125],"BodyName":"Deciat 6 a","BodyID":7,"DistanceFromArrivalLS":1510.2,"PlanetClass":"Rocky body","Landable":true,"SurfaceGravity":1.17679,"Atmosphere":"","Volcanism":"","Materials":[{"Name":"iron","Percent":21.3},{"Name":"nickel","Percent":16.1}],"Rings":[{"Name":"Deciat 6 a A Ring","RingClass":"eRingClass_Metalic","MassMT":1.5e12,"InnerRad":1.0e8,"OuterRad":2.0e8}]}"#);
        let ops = env.operations();
        let Some(Operation::Body(body)) = ops.iter().find(|o| matches!(o, Operation::Body(_))) else { panic!("body: {ops:?}") };
        assert_eq!(body.id64, ed_domain::body_id64(6681123623626, 7));
        assert_eq!(body.name.as_deref(), Some("Deciat 6 a"));
        assert_eq!(body.sub_type.as_deref(), Some("Rocky body"));
        assert!(body.is_landable);
        assert!((body.gravity.unwrap() - 0.12).abs() < 0.001, "m/s² → g");
        assert_eq!(body.materials, vec![("Iron".to_string(), 21.3), ("Nickel".to_string(), 16.1)]);
        assert_eq!(body.rings[0].kind.as_deref(), Some("Metallic"));
        assert_eq!(body.atmosphere, None, "empty strings are absent");
        // A bare gas giant with no rings, no materials, not landable: no row.
        let bare = journal(r#"{"timestamp":"2026-09-09T10:01:00Z","event":"Scan","StarSystem":"Deciat","SystemAddress":6681123623626,"StarPos":[1,2,3],"BodyName":"Deciat 5","BodyID":5,"PlanetClass":"Sudarsky class II gas giant","Landable":false}"#);
        assert!(!bare.operations().iter().any(|o| matches!(o, Operation::Body(_))));
    }

    #[test]
    fn a_navroute_teaches_a_system_and_a_star_per_hop() {
        let raw = r#"{"$schemaRef":"https://eddn.edcd.io/schemas/navroute/1","header":{"uploaderID":"x","softwareName":"t","softwareVersion":"1"},"message":{"timestamp":"2026-09-09T10:02:00Z","event":"NavRoute","Route":[{"StarSystem":"Deciat","SystemAddress":6681123623626,"StarPos":[122.1875,-0.8125,-47.28125],"StarClass":"K"},{"StarSystem":"Jackson's Lighthouse","SystemAddress":9999,"StarPos":[-8.6,52.7,4.0],"StarClass":"N"},{"StarSystem":"Nameless","SystemAddress":1,"StarPos":[0,0,0],"StarClass":""}]}}"#;
        let env = decode(raw.as_bytes()).unwrap();
        assert_eq!(env.schema(), "navroute/1");
        let ops = env.operations();
        assert_eq!(ops.len(), 5, "2 systems+stars, 1 system without a class: {ops:?}");
        let stars: Vec<&ed_domain::StarTeaching> = ops.iter().filter_map(|o| if let Operation::Star(s) = o { Some(s) } else { None }).collect();
        assert_eq!(stars.len(), 2);
        assert_eq!((stars[1].star_type.as_str(), stars[1].source.as_str()), ("N", "eddn:navroute"));
        let mut stats = FeedStats::default();
        stats.count(&env);
        assert_eq!(stats.journal_events.get("NavRoute.hops"), Some(&3));
    }

    #[test]
    fn ring_signals_are_hotspots_and_body_signals_are_counts() {
        let ring = journal(r#"{"timestamp":"2026-09-09T10:03:00Z","event":"SAASignalsFound","StarSystem":"Deciat","SystemAddress":6681123623626,"StarPos":[1,2,3],"BodyName":"Deciat 6 a A Ring","BodyID":8,"Signals":[{"Type":"Painite","Type_Localised":"Painite","Count":2},{"Type":"Platinum","Count":1}]}"#);
        let ops = ring.operations();
        let Some(Operation::RingHotspots(h)) = ops.iter().find(|o| matches!(o, Operation::RingHotspots(_))) else { panic!("{ops:?}") };
        assert_eq!(h.ring_name, "Deciat 6 a A Ring");
        assert_eq!(h.signals, vec![("Painite".to_string(), 2), ("Platinum".to_string(), 1)]);
        assert_eq!(ring_parent_name(&h.ring_name), Some("Deciat 6 a"));
        assert_eq!(ring_parent_name("Deciat 6 a"), None);

        let planet = journal(r#"{"timestamp":"2026-09-09T10:03:00Z","event":"SAASignalsFound","StarSystem":"Deciat","SystemAddress":6681123623626,"StarPos":[1,2,3],"BodyName":"Deciat 6 a","BodyID":7,"Signals":[{"Type":"$SAA_SignalType_Biological;","Count":3},{"Type":"$SAA_SignalType_Geological;","Count":5}]}"#);
        let ops = planet.operations();
        let Some(Operation::BodySignals(s)) = ops.iter().find(|o| matches!(o, Operation::BodySignals(_))) else { panic!("{ops:?}") };
        assert_eq!((s.id64, s.bio_signals, s.geo_signals), (ed_domain::body_id64(6681123623626, 7), Some(3), Some(5)));

        // fssbodysignals is its own schema with the journal's shape.
        let raw = r#"{"$schemaRef":"https://eddn.edcd.io/schemas/fssbodysignals/1","header":{"uploaderID":"x","softwareName":"t","softwareVersion":"1"},"message":{"timestamp":"2026-09-09T10:04:00Z","event":"FSSBodySignals","StarSystem":"Deciat","SystemAddress":6681123623626,"StarPos":[1,2,3],"BodyName":"Deciat 6 b","BodyID":9,"Signals":[{"Type":"$SAA_SignalType_Biological;","Count":1}]}}"#;
        let env = decode(raw.as_bytes()).unwrap();
        let ops = env.operations();
        assert!(ops.iter().any(|o| matches!(o, Operation::BodySignals(s) if s.bio_signals == Some(1) && s.geo_signals.is_none())), "{ops:?}");
    }
}
