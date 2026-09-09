//! Spansh bulk-dump decoding and streaming: the *source* side of the
//! [`GalaxySink`] seam. Nothing here knows about a database.
//!
//! The dumps are a JSON array with **one system object per line**, which is
//! what makes streaming them tractable: read a line, parse it, hand it to
//! the sink, drop it. Individual lines reach 1.3 MB (10 MB+ for populated
//! systems with full body lists), and the dumps together total many GB
//! compressed, so nothing here may hold more than one parse batch in memory.
//!
//! Parsing uses typed structs rather than `serde_json::Value`. Most of the
//! bytes in `galaxy_populated` are body data -- orbital elements, atmosphere
//! composition, ring geometry -- that no sink uses yet. Deserialising into a
//! struct lets serde walk past those fields without allocating them;
//! building a `Value` would materialise every one.
//!
//! ## Which file carries what (<https://spansh.co.uk/dumps>)
//!
//! | File | Systems | Carries |
//! |------|---------|---------|
//! | `galaxy_populated.json.gz` | inhabited only | population, factions, Powerplay, full bodies, every station with market/outfitting/shipyard boards |
//! | `galaxy_stations.json.gz` | any system with a station (fleet carriers included) | stations and their boards; no Powerplay, sparse bodies |
//! | `galaxy.json.gz` | every known system | coordinates, bodies (what `ed-galaxy` indexes); stations where known |
//! | `galaxy_1day.json.gz` / `galaxy_7days.json.gz` | recently updated | same shape as `galaxy.json.gz`, incremental |
//!
//! All of them share one record shape, so one decoder streams any of them;
//! [`dump_kind`] only names the file for provenance and for the
//! population/Powerplay merge rules the sinks apply (a null from the
//! stations dump must not erase a value the populated dump supplied).

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use anyhow::{Context, Result};
use ed_domain::freshness::parse_timestamp;
use rayon::prelude::*;
use serde::Deserialize;

use super::sink::{GalaxySink, SystemVisit};
use super::ImportStats;

/// Lines can be over a megabyte; give the reader room so a single system
/// rarely needs the buffer to grow.
const READ_BUFFER: usize = 4 * 1024 * 1024;

/// Systems per checkpoint. A crash loses at most this many systems' worth
/// of work, which the next run redoes in seconds. The sink commits here.
pub const CHECKPOINT_SYSTEMS: usize = 5_000;
/// Bound raw JSON queued for parallel parsing. Populated-system lines can be
/// over 10 MB, so bytes rather than line count is the important memory cap.
const PARSE_BATCH_BYTES: usize = 32 * 1024 * 1024;
const PARSE_BATCH_LINES: usize = 512;

/// Which Spansh dump a file name claims to be. Purely informational: every
/// kind decodes the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DumpKind {
    /// `galaxy_populated.json[.gz]`: inhabited systems with Powerplay.
    GalaxyPopulated,
    /// `galaxy_stations.json[.gz]`: every system that has a station.
    GalaxyStations,
    /// `galaxy.json[.gz]` and its `_1day`/`_7days` slices: the whole galaxy.
    Galaxy,
    /// Anything else with the same line-per-system shape.
    Other,
}

impl DumpKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            DumpKind::GalaxyPopulated => "galaxy_populated",
            DumpKind::GalaxyStations => "galaxy_stations",
            DumpKind::Galaxy => "galaxy",
            DumpKind::Other => "other",
        }
    }
}

pub fn dump_kind(path: &Path) -> DumpKind {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.starts_with("galaxy_populated") {
        DumpKind::GalaxyPopulated
    } else if name.starts_with("galaxy_stations") {
        DumpKind::GalaxyStations
    } else if name.starts_with("galaxy") {
        DumpKind::Galaxy
    } else {
        DumpKind::Other
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Coords {
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct LandingPads {
    #[serde(default)]
    pub large: Option<i64>,
    #[serde(default)]
    pub medium: Option<i64>,
    #[serde(default)]
    pub small: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Commodity {
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub category: Option<String>,
    #[serde(rename = "buyPrice")]
    pub buy_price: Option<i64>,
    #[serde(rename = "sellPrice")]
    pub sell_price: Option<i64>,
    pub demand: Option<i64>,
    pub supply: Option<i64>,
}

impl Commodity {
    /// The internal symbol, lowercased -- never the display name -- the
    /// same key `ed_journal::catalog` and EDDN use.
    pub fn key(&self) -> Option<String> {
        self.symbol
            .as_deref()
            .or(self.name.as_deref())
            .map(str::to_lowercase)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Market {
    #[serde(default)]
    pub commodities: Vec<Commodity>,
    #[serde(rename = "prohibitedCommodities", default)]
    pub prohibited: Vec<String>,
    #[serde(rename = "updateTime")]
    pub update_time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct NamedThing {
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

impl NamedThing {
    pub fn key(&self) -> Option<String> {
        self.symbol
            .as_deref()
            .or(self.name.as_deref())
            .map(str::to_lowercase)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Module {
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub class: Option<i64>,
    #[serde(default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    /// Set for ship-specific parts (armour); null for generic modules.
    #[serde(default)]
    pub ship: Option<String>,
}

impl Module {
    pub fn key(&self) -> Option<String> {
        self.symbol
            .as_deref()
            .or(self.name.as_deref())
            .map(str::to_lowercase)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct Outfitting {
    #[serde(default)]
    pub modules: Vec<Module>,
    #[serde(rename = "updateTime")]
    pub update_time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Faction {
    pub name: Option<String>,
    pub allegiance: Option<String>,
    pub government: Option<String>,
    pub influence: Option<f64>,
    pub state: Option<String>,
    #[serde(rename = "activeStates", default)]
    pub active_states: Vec<StateRef>,
}

#[derive(Debug, Default, Deserialize)]
pub struct StateRef {
    pub state: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Signals {
    #[serde(default)]
    pub signals: HashMap<String, i64>,
    #[serde(rename = "updateTime")]
    pub update_time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Ring {
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub mass: Option<f64>,
    #[serde(rename = "innerRadius")]
    pub inner_radius: Option<f64>,
    #[serde(rename = "outerRadius")]
    pub outer_radius: Option<f64>,
    pub signals: Option<Signals>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Shipyard {
    #[serde(default)]
    pub ships: Vec<NamedThing>,
    #[serde(rename = "updateTime")]
    pub update_time: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Station {
    pub id: Option<i64>,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(rename = "distanceToArrival")]
    pub distance_to_arrival: Option<f64>,
    #[serde(rename = "primaryEconomy")]
    pub primary_economy: Option<String>,
    pub government: Option<String>,
    #[serde(rename = "controllingFaction")]
    pub controlling_faction: Option<String>,
    #[serde(rename = "landingPads")]
    pub landing_pads: Option<LandingPads>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(rename = "updateTime")]
    pub update_time: Option<String>,
    pub market: Option<Market>,
    pub outfitting: Option<Outfitting>,
    pub shipyard: Option<Shipyard>,
    /// `{"Industrial": 60.0, "Refinery": 40.0}`.
    #[serde(default)]
    pub economies: Option<HashMap<String, f64>>,
    /// The controlling faction's state as seen at the station.
    #[serde(rename = "controllingFactionState", default)]
    pub controlling_faction_state: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
}

/// Every timestamp a station record carries, parsed once so each sink
/// applies the same freshness rules to the same numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationTimes {
    /// `updateTime` of the station itself.
    pub station: Option<i64>,
    /// When the market board was observed. A dump market is a complete
    /// board, so it goes through the same snapshot rule as EDDN: newer than
    /// the station's watermark replaces it whole. A board with no usable
    /// timestamp lands at epoch 0: kept, reported as infinitely old, and
    /// outranked by any dated snapshot.
    pub market: i64,
    /// Outfitting's own `updateTime`, else the station's.
    pub outfitting: Option<i64>,
    /// Shipyard's own `updateTime`, else the station's.
    pub shipyard: Option<i64>,
}

impl Station {
    /// A station type that moves. Carriers have markets worth knowing
    /// about, but a route planned through one can evaporate, so callers
    /// need to be able to exclude them rather than discovering it in flight.
    pub fn is_carrier(&self) -> bool {
        is_carrier(self.kind.as_deref())
    }

    pub fn has_service(&self, want: &str) -> bool {
        has(&self.services, want)
    }

    pub fn times(&self) -> StationTimes {
        let station = self.update_time.as_deref().and_then(parse_timestamp);
        let own = |t: Option<&str>| t.and_then(parse_timestamp).or(station);
        StationTimes {
            station,
            market: own(self.market.as_ref().and_then(|m| m.update_time.as_deref())).unwrap_or(0),
            outfitting: own(self
                .outfitting
                .as_ref()
                .and_then(|o| o.update_time.as_deref())),
            shipyard: own(self
                .shipyard
                .as_ref()
                .and_then(|s| s.update_time.as_deref())),
        }
    }
}

pub(crate) fn is_carrier(kind: Option<&str>) -> bool {
    kind.is_some_and(|k| k.contains("Carrier"))
}

pub(crate) fn has(services: &[String], want: &str) -> bool {
    services.iter().any(|s| s.eq_ignore_ascii_case(want))
}

/// Bodies: what sourcing and mining need. Orbital elements, composition and
/// the rest are skipped by serde without being allocated.
#[derive(Debug, Default, Deserialize)]
pub struct Body {
    pub id64: Option<i64>,
    #[serde(rename = "bodyId")]
    pub body_id: Option<i64>,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    #[serde(rename = "subType")]
    pub sub_type: Option<String>,
    /// The system's arrival star (item 47: the routing index's class
    /// knowledge comes from exactly these bodies).
    #[serde(rename = "mainStar", default)]
    pub main_star: bool,
    #[serde(rename = "isLandable", default)]
    pub is_landable: bool,
    #[serde(rename = "distanceToArrival")]
    pub distance_to_arrival: Option<f64>,
    pub gravity: Option<f64>,
    #[serde(rename = "atmosphereType")]
    pub atmosphere: Option<String>,
    #[serde(rename = "volcanismType")]
    pub volcanism: Option<String>,
    /// Surface raw materials as percentages, landable bodies only.
    #[serde(default)]
    pub materials: Option<HashMap<String, f64>>,
    pub signals: Option<Signals>,
    #[serde(default)]
    pub rings: Vec<Ring>,
    #[serde(rename = "updateTime")]
    pub update_time: Option<String>,
    #[serde(default)]
    pub stations: Vec<Station>,
}

impl Body {
    pub fn updated(&self) -> Option<i64> {
        self.update_time.as_deref().and_then(parse_timestamp)
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct System {
    pub id64: Option<i64>,
    pub name: Option<String>,
    pub coords: Option<Coords>,
    pub allegiance: Option<String>,
    pub government: Option<String>,
    #[serde(rename = "primaryEconomy")]
    pub primary_economy: Option<String>,
    #[serde(rename = "secondaryEconomy")]
    pub secondary_economy: Option<String>,
    pub security: Option<String>,
    pub population: Option<i64>,
    #[serde(rename = "controllingPower")]
    pub controlling_power: Option<String>,
    #[serde(rename = "powerState")]
    pub power_state: Option<String>,
    #[serde(default)]
    pub powers: Option<Vec<String>>,
    pub date: Option<String>,
    #[serde(default)]
    pub stations: Vec<Station>,
    #[serde(default)]
    pub bodies: Vec<Body>,
    #[serde(default)]
    pub factions: Vec<Faction>,
}

impl System {
    /// The system's `date`, as an epoch.
    pub fn updated(&self) -> Option<i64> {
        self.date.as_deref().and_then(parse_timestamp)
    }

    pub fn position(&self) -> Option<[f64; 3]> {
        let c = self.coords.as_ref()?;
        Some([c.x?, c.y?, c.z?])
    }
}

/// Parse one dump line into a system. Handles the array brackets and the
/// trailing comma the line-per-system layout leaves behind.
pub fn parse_line(line: &str) -> Option<std::result::Result<System, serde_json::Error>> {
    let trimmed = line.trim().trim_end_matches(',');
    trimmed
        .starts_with('{')
        .then(|| serde_json::from_str::<System>(trimmed))
}

/// Counts bytes passing through, so progress can be a fraction of the
/// compressed size even when the source is a network stream.
struct Counted<R: Read> {
    inner: R,
    bytes: std::rc::Rc<std::cell::Cell<u64>>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes.set(self.bytes.get() + n as u64);
        Ok(n)
    }
}

/// Stream one dump file -- gzip-compressed or plain `.json` -- into `sink`.
///
/// `progress` receives `(stats, total_bytes)` periodically, where
/// `stats.bytes_in` counts bytes of the *file* consumed (compressed bytes
/// for a `.gz`), so `bytes_in / total` is a usable fraction.
pub fn stream_dump<S: GalaxySink>(
    path: &Path,
    sink: &mut S,
    progress: impl FnMut(&ImportStats, u64),
) -> Result<ImportStats> {
    let file =
        std::fs::File::open(path).with_context(|| format!("opening dump {}", path.display()))?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    if path.extension().is_some_and(|e| e == "gz") {
        stream_gz(file, total, sink, progress)
    } else {
        stream_json(file, total, sink, progress)
    }
}

/// Stream a gzip-compressed dump from any byte source -- a file or an HTTP
/// body -- so the archive never has to touch the disk. `total_compressed`
/// is a hint for progress (0 = unknown).
pub fn stream_gz<S: GalaxySink>(
    source: impl Read,
    total_compressed: u64,
    sink: &mut S,
    progress: impl FnMut(&ImportStats, u64),
) -> Result<ImportStats> {
    let bytes = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let counted = Counted {
        inner: source,
        bytes: bytes.clone(),
    };
    let decoder = flate2::read::MultiGzDecoder::new(counted);
    stream_lines(decoder, total_compressed, bytes, sink, progress)
}

/// Stream an already-decompressed dump.
pub fn stream_json<S: GalaxySink>(
    source: impl Read,
    total: u64,
    sink: &mut S,
    progress: impl FnMut(&ImportStats, u64),
) -> Result<ImportStats> {
    let bytes = std::rc::Rc::new(std::cell::Cell::new(0u64));
    let counted = Counted {
        inner: source,
        bytes: bytes.clone(),
    };
    stream_lines(counted, total, bytes, sink, progress)
}

fn stream_lines<S: GalaxySink>(
    decoded: impl Read,
    total: u64,
    bytes_in: std::rc::Rc<std::cell::Cell<u64>>,
    sink: &mut S,
    mut progress: impl FnMut(&ImportStats, u64),
) -> Result<ImportStats> {
    let mut reader = BufReader::with_capacity(READ_BUFFER, decoded);
    let mut stats = ImportStats::default();
    let mut in_batch = 0usize;
    let mut last_tick = std::time::Instant::now();

    progress(&stats, total);
    sink.begin()?;

    loop {
        let mut raw = Vec::with_capacity(PARSE_BATCH_LINES);
        let mut raw_bytes = 0usize;
        while raw.len() < PARSE_BATCH_LINES && raw_bytes < PARSE_BATCH_BYTES {
            let mut line = String::new();
            let n = match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    // Keep what was streamed so far: a resumed run skips it
                    // as unchanged.
                    sink.checkpoint(&stats).ok();
                    return Err(e).context("reading dump");
                }
            };
            raw_bytes += n;
            if line.trim_start().starts_with('{') {
                raw.push(line);
            }
        }
        if raw.is_empty() {
            break;
        }
        // Parsing is CPU work and independent per system. Indexed parallel
        // iteration preserves dump order for deterministic progress/results;
        // the sink remains a single batched writer below.
        let parsed: Vec<_> = raw.par_iter().filter_map(|line| parse_line(line)).collect();

        for parsed_system in parsed {
            let sys = match parsed_system {
                Ok(s) => s,
                Err(_) => {
                    stats.parse_errors += 1;
                    continue;
                }
            };
            stats.bytes_in = bytes_in.get();
            deliver(&sys, sink, &mut stats)?;

            in_batch += 1;
            if in_batch >= CHECKPOINT_SYSTEMS || last_tick.elapsed().as_secs() >= 2 {
                sink.checkpoint(&stats)?;
                in_batch = 0;
                last_tick = std::time::Instant::now();
                progress(&stats, total);
            }
        }
    }

    stats.bytes_in = bytes_in.get();
    sink.finish(&stats)?;
    progress(&stats, total);
    Ok(stats)
}

/// Walk one decoded system through the sink in the fixed order every
/// adapter can rely on: system, then its stations (system-hosted first,
/// then body-hosted with the body name), then bodies, then factions.
/// Stations are always delivered -- they carry their own timestamps -- but
/// bodies and factions only when the sink asked for a full visit.
pub fn deliver<S: GalaxySink>(sys: &System, sink: &mut S, stats: &mut ImportStats) -> Result<()> {
    let Some(id64) = sys.id64 else {
        return Ok(());
    };
    let updated = sys.updated();
    let visit = sink.system(sys, updated, stats)?;
    if visit == SystemVisit::Skip {
        return Ok(());
    }
    for st in &sys.stations {
        if st.id.is_some() {
            sink.station(sys, st, None, &st.times(), stats)?;
        }
    }
    for body in &sys.bodies {
        if visit == SystemVisit::Full && body.id64.is_some() {
            sink.body(id64, body, body.updated(), stats)?;
        }
        for st in &body.stations {
            if st.id.is_some() {
                sink.station(sys, st, body.name.as_deref(), &st.times(), stats)?;
            }
        }
    }
    if visit == SystemVisit::Full && !sys.factions.is_empty() {
        sink.factions(id64, &sys.factions, updated, stats)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carrier_detection_matches_dump_type_names() {
        assert!(is_carrier(Some("Drake-Class Carrier")));
        assert!(is_carrier(Some("Fleet Carrier")));
        assert!(!is_carrier(Some("Coriolis Starport")));
        assert!(!is_carrier(Some("Outpost")));
        assert!(!is_carrier(None));
    }

    #[test]
    fn services_lookup_is_case_insensitive() {
        let s = vec!["Dock".to_string(), "market".to_string()];
        assert!(has(&s, "Market"));
        assert!(!has(&s, "Shipyard"));
    }

    #[test]
    fn dump_kind_is_read_from_the_file_name() {
        assert_eq!(
            dump_kind(Path::new("/x/galaxy_populated.json.gz")),
            DumpKind::GalaxyPopulated
        );
        assert_eq!(
            dump_kind(Path::new("galaxy_stations.json")),
            DumpKind::GalaxyStations
        );
        assert_eq!(dump_kind(Path::new("galaxy.json.gz")), DumpKind::Galaxy);
        assert_eq!(
            dump_kind(Path::new("galaxy_7days.json.gz")),
            DumpKind::Galaxy
        );
        assert_eq!(dump_kind(Path::new("wongi.json")), DumpKind::Other);
    }

    #[test]
    fn a_dump_line_parses_into_the_typed_shape() {
        let line = r#"{"id64":819519,"name":"Test Sys","coords":{"x":1.0,"y":2.0,"z":3.0},"allegiance":null,"government":"None","security":"Anarchy","population":0,"bodies":[{"id64":1,"stations":[{"id":99,"name":"Body Port","type":"Outpost","services":["Dock","Market"],"landingPads":{"large":0,"medium":2,"small":4},"market":{"commodities":[{"name":"Tritium","symbol":"Tritium","category":"Chemicals","demand":0,"supply":19966,"buyPrice":215079,"sellPrice":0}]}}]}],"stations":[{"id":100,"name":"Sys Port","type":"Coriolis Starport","services":["Dock","Market","Shipyard"]}]},"#;
        let sys = parse_line(line).expect("object line").expect("typed parse");
        assert_eq!(sys.id64, Some(819519));
        assert_eq!(sys.stations.len(), 1);
        assert_eq!(sys.bodies.len(), 1);
        // Stations hang off bodies as well as the system, and missing either
        // place silently loses markets.
        assert_eq!(sys.bodies[0].stations.len(), 1);
        let body_station = &sys.bodies[0].stations[0];
        assert_eq!(body_station.market.as_ref().unwrap().commodities.len(), 1);
        assert!(sys.stations[0].has_service("Shipyard"));
        assert!(parse_line("[").is_none());
        assert!(parse_line("]").is_none());
    }
}
